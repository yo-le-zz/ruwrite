//! Tests de bout en bout : on réécrit un vrai exécutable, on le lance, et on
//! vérifie ce qu'il affiche. Les binaires cibles sont ceux de ce paquet.

use std::collections::HashMap;
use std::fs;
use std::io::{BufRead, BufReader};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::{Duration, Instant};

use ruwrite::{Error, Executable, Mutability, ProtectionMode, ProtectionTarget, Value};

const TARGET: &str = env!("CARGO_BIN_EXE_target_demo");
const HOT: &str = env!("CARGO_BIN_EXE_hot_demo");
const LOCKED: &str = env!("CARGO_BIN_EXE_locked_demo");

/// Copie privée d'un exécutable, supprimée en fin de test.
struct Copy {
    path: PathBuf,
}

impl Copy {
    fn of(source: &str) -> Copy {
        static NEXT: AtomicU32 = AtomicU32::new(0);
        let dir = std::env::temp_dir().join(format!(
            "ruwrite-e2e-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("target");
        fs::copy(source, &path).unwrap();
        Copy { path }
    }

    fn open(&self) -> Executable {
        Executable::open(&self.path).unwrap()
    }

    /// Lance le programme et renvoie ses lignes `clé=valeur`.
    fn run(&self) -> HashMap<String, String> {
        let output = Command::new(&self.path).output().unwrap();
        assert!(
            output.status.success(),
            "the patched program crashed: {output:?}"
        );
        String::from_utf8(output.stdout)
            .unwrap()
            .lines()
            .filter_map(|l| l.split_once('='))
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }
}

impl Drop for Copy {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(self.path.parent().unwrap());
    }
}

#[test]
fn baseline_output() {
    let t = Copy::of(TARGET);
    let out = t.run();
    assert_eq!(out["limit"], "10");
    assert_eq!(out["compute"], "7");
    assert_eq!(out["admin"], "false");
    assert_eq!(out["scale"], "1");
}

#[test]
fn inspects_functions_and_variables() {
    let t = Copy::of(TARGET);
    let exe = t.open();
    assert!(exe.info().is_rust && exe.info().has_debug_info);
    assert!(exe
        .functions()
        .iter()
        .any(|f| f.name == "target_demo::compute"));
    let vars = exe.variables().unwrap();
    assert!(vars
        .iter()
        .any(|v| v.name == "target_demo::LIMIT" && v.ty == "i32"));
    assert!(vars.iter().any(|v| v.name == "a" && v.ty == "i32"));
    assert_eq!(exe.static_value("LIMIT").unwrap(), Value::I32(10));
    assert_eq!(exe.static_value("RATIO").unwrap(), Value::F64(1.5));
    assert_eq!(exe.static_bytes("BANNER").unwrap(), b"hello");
}

#[test]
fn nothing_changes_before_commit() {
    let t = Copy::of(TARGET);
    let mut exe = t.open();
    exe.set_static("LIMIT", 99).unwrap();
    assert_eq!(exe.pending().len(), 1);
    assert_eq!(t.run()["limit"], "10");
    exe.discard();
    assert!(exe.pending().is_empty());
    assert_eq!(exe.commit().unwrap().patches, 0);
    assert_eq!(t.run()["limit"], "10");
}

#[test]
fn rewrites_statics_of_every_kind() {
    let t = Copy::of(TARGET);
    let mut exe = t.open();
    exe.set_static("LIMIT", 123).unwrap();
    exe.set_static("RATIO", 2.25f64).unwrap();
    exe.set_static("SMALL", 200u8).unwrap();
    exe.set_static_bytes("BANNER", b"world").unwrap();
    let report = exe.commit().unwrap();
    assert_eq!(report.patches, 4);
    assert!(report.backup.is_some());

    let out = t.run();
    assert_eq!(out["limit"], "123");
    assert_eq!(out["ratio"], "2.25");
    assert_eq!(out["small"], "200");
    assert_eq!(out["banner"], "world");
}

#[test]
fn rejects_values_that_do_not_fit() {
    let t = Copy::of(TARGET);
    let mut exe = t.open();
    assert!(matches!(
        exe.set_static("SMALL", 300),
        Err(Error::ValueOutOfRange { .. })
    ));
    assert!(matches!(
        exe.set_static("LIMIT", 1.5f32),
        Err(Error::TypeMismatch { .. })
    ));
    assert!(matches!(
        exe.set_static("NOPE", 1),
        Err(Error::NotFound { .. })
    ));
    assert!(exe.pending().is_empty());
}

#[test]
fn rewrites_a_local_with_a_narrower_literal() {
    // `let a: i32 = 3;` devient 2, écrit sous forme de u8.
    let t = Copy::of(TARGET);
    let mut exe = t.open();
    exe.local("compute", "a").unwrap().set(2u8).unwrap();
    exe.commit().unwrap();
    assert_eq!(t.run()["compute"], "6"); // 2 + 4
}

#[test]
fn rewrites_wide_and_bool_locals() {
    let t = Copy::of(TARGET);
    let mut exe = t.open();
    exe.local("small_local", "big").unwrap().set(-5i64).unwrap();
    exe.commit().unwrap();
    assert_eq!(t.run()["small_local"], "-5");

    let mut exe = t.open();
    exe.local("small_local", "flag").unwrap().set(true).unwrap();
    exe.commit().unwrap();
    assert_eq!(t.run()["small_local"], "-1");
}

#[test]
fn local_errors_are_explicit() {
    let t = Copy::of(TARGET);
    let mut exe = t.open();
    assert!(matches!(
        exe.local("compute", "zzz").map(|_| ()),
        Err(Error::NotFound { .. })
    ));
    let err = exe.local("small_local", "big").unwrap().set(1.0f32);
    assert!(matches!(err, Err(Error::TypeMismatch { .. })));
}

#[test]
fn rewrites_functions() {
    let t = Copy::of(TARGET);
    let mut exe = t.open();
    exe.function("is_admin").unwrap().returns(true).unwrap();
    exe.function("compute").unwrap().returns(100).unwrap();
    exe.function("scale").unwrap().returns(2.5f64).unwrap();
    exe.function("say_hi").unwrap().disable().unwrap();
    exe.function("redirected")
        .unwrap()
        .redirect_to("replacement")
        .unwrap();
    exe.commit().unwrap();

    let out = t.run();
    assert_eq!(out["admin"], "true");
    assert_eq!(out["compute"], "100");
    assert_eq!(out["scale"], "2.5");
    assert!(!out.contains_key("hi"), "say_hi should print nothing");
    assert_eq!(out["redirected"], "2");
}

#[test]
fn too_large_code_is_refused() {
    let t = Copy::of(TARGET);
    let mut exe = t.open();
    let err = exe.function("is_admin").unwrap().replace_code(&[0x90; 64]);
    assert!(matches!(err, Err(Error::FunctionTooSmall { .. })));
}

#[test]
fn disassembles_a_function() {
    let t = Copy::of(TARGET);
    let mut exe = t.open();
    let lines = exe.function("compute").unwrap().disassemble().unwrap();
    assert!(lines.iter().any(|(_, text)| text.starts_with("ret")));
    assert!(lines.iter().any(|(_, text)| text.contains("mov")));
}

#[test]
fn replaces_byte_patterns() {
    let t = Copy::of(TARGET);
    let mut exe = t.open();
    assert!(!exe.find_bytes(b"limit=").is_empty());
    let n = exe.replace_bytes(b"limit=", b"LIMIT=").unwrap();
    assert!(n >= 1);
    exe.commit().unwrap();
    assert_eq!(t.run()["LIMIT"], "10");
}

#[test]
fn commit_to_leaves_the_original_untouched() {
    let t = Copy::of(TARGET);
    let mut exe = t.open();
    exe.set_static("LIMIT", 5).unwrap();
    let copy = t.path.with_file_name("patched");
    let report = exe.commit_to(&copy).unwrap();
    assert_eq!(report.path.as_deref(), Some(copy.as_path()));

    assert_eq!(t.run()["limit"], "10");
    let out = Command::new(&copy).output().unwrap();
    assert!(String::from_utf8_lossy(&out.stdout).contains("limit=5"));
    let exe_bit = fs::metadata(&copy).unwrap();
    use std::os::unix::fs::PermissionsExt;
    assert!(
        exe_bit.permissions().mode() & 0o111 != 0,
        "must stay executable"
    );
}

#[test]
fn commit_is_refused_when_the_file_changed_underneath() {
    let t = Copy::of(TARGET);
    let mut exe = t.open();
    exe.set_static("LIMIT", 5).unwrap();

    // Quelqu'un d'autre modifie le même octet entre l'ouverture et le commit.
    let mut other = t.open();
    other.set_static("LIMIT", 6).unwrap();
    other.commit().unwrap();

    assert!(matches!(exe.commit(), Err(Error::BytesMismatch { .. })));
    // Le fichier garde l'état de l'autre modification, intact et exécutable.
    assert_eq!(t.run()["limit"], "6");
}

#[test]
fn backup_is_created_once_and_keeps_the_original() {
    let t = Copy::of(TARGET);
    let mut exe = t.open();
    exe.set_static("LIMIT", 1).unwrap();
    let first = exe.commit().unwrap();
    let backup = first.backup.expect("backup");
    exe.set_static("LIMIT", 2).unwrap();
    let second = exe.commit().unwrap();
    assert!(second.backup.is_none());
    let out = Command::new(&backup).output().unwrap();
    assert!(String::from_utf8_lossy(&out.stdout).contains("limit=10"));
}

#[test]
fn non_rust_files_are_rejected() {
    let dir = std::env::temp_dir().join(format!("ruwrite-nonrust-{}", std::process::id()));
    fs::create_dir_all(&dir).unwrap();
    let file = dir.join("garbage");
    fs::write(&file, b"this is not an executable").unwrap();
    assert!(matches!(Executable::open(&file), Err(Error::Parse(_))));
    assert!(matches!(
        Executable::open("/nonexistent/ruwrite"),
        Err(Error::Io { .. })
    ));
    // /bin/true est un ELF valide mais pas un programme Rust.
    if let Ok(true_bin) = fs::canonicalize("/bin/true") {
        let is_rust = Executable::open_any(&true_bin).map(|e| e.info().is_rust);
        if let Ok(false) = is_rust {
            assert!(matches!(
                Executable::open(&true_bin),
                Err(Error::NotRust(_))
            ));
        }
    }
    let _ = fs::remove_dir_all(dir);
}

/// Lit la prochaine valeur affichée par `hot_demo`.
fn next_value(lines: &mut impl Iterator<Item = String>) -> i32 {
    lines
        .find_map(|l| l.strip_prefix("value=").and_then(|v| v.parse().ok()))
        .expect("hot_demo stopped printing")
}

#[cfg(target_os = "linux")]
#[test]
fn patches_a_running_process() {
    let t = Copy::of(HOT);
    let mut child = Command::new(&t.path)
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let mut lines = BufReader::new(child.stdout.take().unwrap())
        .lines()
        .map_while(Result::ok);
    assert_eq!(next_value(&mut lines), 1);

    let mut exe = t.open();
    exe.function("hot_demo::value")
        .unwrap()
        .returns(42)
        .unwrap();
    let report = exe.commit_hot(child.id());
    let report = match report {
        Ok(r) => r,
        Err(e) => {
            let _ = child.kill();
            panic!("hot patch failed: {e}");
        }
    };
    assert_eq!(report.patches, 1);
    assert_eq!(report.pid, Some(child.id()));

    // Les lignes déjà en tampon peuvent encore afficher l'ancienne valeur.
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut seen = 0;
    while Instant::now() < deadline {
        seen = next_value(&mut lines);
        if seen == 42 {
            break;
        }
    }
    // Rejouer le patch ne doit rien réécrire.
    let again = exe.commit_hot(child.id()).unwrap();
    let _ = child.kill();
    let _ = child.wait();
    assert_eq!(seen, 42);
    assert_eq!(again.patches, 0);
    // Le fichier sur disque n'a pas été touché.
    assert_eq!(fs::read(&t.path).unwrap(), fs::read(HOT).unwrap());
}

#[cfg(target_os = "linux")]
#[test]
fn hot_patch_of_the_wrong_binary_is_refused() {
    let t = Copy::of(HOT);
    let mut child = Command::new(&t.path).stdout(Stdio::null()).spawn().unwrap();
    // On prépare un patch sur un autre binaire : aucune de ses pages n'est mappée.
    let other = Copy::of(TARGET);
    let mut exe = other.open();
    exe.function("is_admin").unwrap().returns(true).unwrap();
    let err = exe.commit_hot(child.id());
    let _ = child.kill();
    let _ = child.wait();
    // Soit la page n'est pas mappée, soit les octets attendus diffèrent :
    // dans les deux cas rien ne doit être écrit.
    assert!(matches!(
        err,
        Err(Error::Live(_) | Error::BytesMismatch { .. })
    ));
}

#[test]
fn mutability_is_read_from_the_source() {
    let t = Copy::of(TARGET);
    let mut exe = t.open();
    assert_eq!(
        exe.local("counter", "c").unwrap().mutability(),
        Mutability::Mutable
    );
    assert_eq!(
        exe.local("counter", "fixed").unwrap().mutability(),
        Mutability::Immutable
    );
}

#[test]
fn assigning_an_immutable_variable_is_refused_until_made_mut() {
    let t = Copy::of(TARGET);
    let mut exe = t.open();
    let err = exe.local("counter", "fixed").unwrap().assign_at(0, 1);
    assert!(matches!(err, Err(Error::NotMutable { .. })), "{err:?}");
    assert!(exe.pending().is_empty());

    // Une fois autorisé explicitement, c'est l'emplacement qui est vérifié :
    // l'adresse 0 n'est pas dans la fonction.
    let err = exe
        .local("counter", "fixed")
        .unwrap()
        .make_mut()
        .assign_at(0, 1);
    assert!(matches!(err, Err(Error::LocalPatch(_))), "{err:?}");
}

#[test]
fn inserts_an_assignment_into_freed_space() {
    let t = Copy::of(TARGET);

    // 1. On libère la place de `c = 1` (le store initial) et on valide.
    let mut exe = t.open();
    let lines = exe.function("counter").unwrap().disassemble().unwrap();
    let at = lines
        .iter()
        .position(|(_, text)| text.contains("dword ptr") && text.ends_with(",1"))
        .expect("store of c");
    let (address, next) = (lines[at].0, lines[at + 1].0);
    exe.nop(address, (next - address) as usize).unwrap();
    exe.commit().unwrap();
    assert_ne!(t.run()["counter"], "6"); // `c` n'est plus initialisée à 1

    // 2. `c` est `mut` : l'assignation est acceptée et réinsérée à cet endroit.
    let mut exe = t.open();
    exe.local("counter", "c")
        .unwrap()
        .assign_at(address, 10)
        .unwrap();
    exe.commit().unwrap();
    assert_eq!(t.run()["counter"], "15"); // 10 + 5

    // 3. `fixed` n'est pas `mut` : refus, sauf décision explicite.
    let mut exe = t.open();
    let err = exe.local("counter", "fixed").unwrap().assign_at(address, 1);
    assert!(matches!(err, Err(Error::NotMutable { .. })));
}

fn assert_protected<T: std::fmt::Debug>(result: Result<T, Error>) {
    match result {
        Err(Error::Protected { .. }) => {}
        other => panic!("expected a protection error, got {other:?}"),
    }
}

#[test]
fn protection_rules_are_read_from_the_binary() {
    let t = Copy::of(TARGET);
    let exe = t.open();
    assert!(exe.is_protected());
    let rules = exe.protections();
    assert_eq!(rules.len(), 4);
    assert!(rules.iter().any(
        |p| p.target == ProtectionTarget::Function("license_check".into())
            && p.mode == ProtectionMode::Immutable
            && p.matches == 1
    ));
    assert!(rules
        .iter()
        .any(|p| p.target == ProtectionTarget::Function("token".into())
            && p.mode == ProtectionMode::Immutable
            && p.matches == 1));
    assert!(rules
        .iter()
        .any(|p| p.target == ProtectionTarget::Static("SECRET".into())
            && p.mode == ProtectionMode::Protected
            && p.matches == 1));
    assert!(rules
        .iter()
        .any(|p| matches!(p.target, ProtectionTarget::HiddenName(_))
            && p.mode == ProtectionMode::Hidden
            && p.matches == 1));
}

#[test]
fn hidden_function_is_left_out_of_listings_but_still_runs() {
    let t = Copy::of(TARGET);
    let exe = t.open();
    assert!(!exe
        .functions()
        .iter()
        .any(|f| f.name.contains("hidden_formula")));
    assert_eq!(t.run()["hidden"], "7");
}

#[test]
fn integrity_snapshot_catches_a_change_to_a_protected_part() {
    let t = Copy::of(TARGET);
    let exe = t.open();
    let snapshot = exe.integrity_snapshot();
    assert!(exe.verify_integrity(&snapshot).is_empty());

    // La seule façon de faire bouger la valeur protégée ici est de copier un
    // fichier déjà différent : on simule une modification externe à ruwrite
    // en comparant plus tard l'instantané pris sur l'original avec celui
    // d'une version patchée ailleurs (LIMIT n'est pas protégé, donc le patch
    // passe par commit_to normalement puis on falsifie `SECRET` à la main).
    let patched = t.path.with_file_name("tampered");
    std::fs::copy(&t.path, &patched).unwrap();
    let mut raw = std::fs::read(&patched).unwrap();
    let secret_addr = exe
        .find_value(Value::i32(1234))
        .into_iter()
        .find(|m| m.name.as_deref() == Some("target_demo::SECRET"))
        .unwrap()
        .address;
    let offset = exe.vaddr_to_offset(secret_addr).unwrap() as usize;
    raw[offset] = 0xFF; // modification faite hors de ruwrite
    std::fs::write(&patched, &raw).unwrap();

    let tampered_exe = Executable::open(&patched).unwrap();
    let violations = tampered_exe.verify_integrity(&snapshot);
    assert!(
        violations
            .iter()
            .any(|v| v.what.contains("SECRET") && v.kind == ruwrite::IntegrityIssue::Changed),
        "{violations:?}"
    );
}

#[test]
fn protected_function_and_static_cannot_be_edited() {
    let t = Copy::of(TARGET);
    let mut exe = t.open();
    assert_protected(exe.function("license_check").unwrap().returns(true));
    assert_protected(exe.function("license_check").unwrap().disable());
    assert_protected(exe.function("secure::token").unwrap().returns(1));
    assert_protected(exe.set_static("SECRET", 1));
    assert_protected(exe.set_static_bytes("SECRET", &[0; 4]));
    assert!(exe.pending().is_empty());

    let message = exe.set_static("SECRET", 1).unwrap_err().to_string();
    assert!(message.contains("sensitive and protected"), "{message}");
    assert!(message.contains("SECRET"), "{message}");
}

#[test]
fn raw_patches_cannot_bypass_the_protection() {
    let t = Copy::of(TARGET);
    let mut exe = t.open();
    let function = exe
        .functions()
        .iter()
        .find(|f| f.name == "target_demo::license_check")
        .unwrap()
        .clone();
    // Par adresse virtuelle, par décalage dans le fichier, en rognant un octet.
    assert_protected(exe.patch_bytes(function.address, &[0x90]));
    assert_protected(exe.patch_bytes(function.address + 1, &[0x90]));
    assert_protected(exe.nop(function.address, 2));
    assert_protected(exe.patch_file_bytes(function.file_offset.unwrap(), &[0x90]));
    // Les sections qui portent les règles et l'en-tête du fichier aussi.
    let records = exe
        .sections()
        .iter()
        .find(|s| s.name == ".ruwprot")
        .unwrap()
        .clone();
    assert_protected(exe.patch_file_bytes(records.file_offset.unwrap(), &[0]));
    assert_protected(exe.patch_file_bytes(0, &[0]));
    // Un remplacement de motif qui toucherait une partie protégée échoue aussi.
    assert_protected(exe.replace_bytes(b"RUWPROT2", b"XXXXXXXX"));
    assert!(exe.pending().is_empty());
}

#[test]
fn unprotected_parts_of_a_protected_program_stay_editable() {
    let t = Copy::of(TARGET);
    let mut exe = t.open();
    exe.set_static("LIMIT", 77).unwrap();
    exe.function("is_admin").unwrap().returns(true).unwrap();
    exe.commit().unwrap();
    let out = t.run();
    assert_eq!(out["limit"], "77");
    assert_eq!(out["admin"], "true");
    // Les parties protégées n'ont pas bougé.
    assert_eq!(out["license"], "false");
    assert_eq!(out["secret"], "1234");
    assert_eq!(out["token"], "99");
}

#[test]
fn protection_applies_to_every_commit_mode() {
    let t = Copy::of(TARGET);
    let mut exe = t.open();
    assert_protected(exe.function("license_check").unwrap().returns(true));
    // Rien n'est en file d'attente : commit_to et commit_hot n'ont rien à écrire.
    let copy = t.path.with_file_name("copy");
    assert_eq!(exe.commit_to(&copy).unwrap().patches, 0);
    assert!(!copy.exists());
}

#[test]
fn protect_all_refuses_everything() {
    let t = Copy::of(LOCKED);
    let mut exe = t.open();
    assert!(exe
        .protections()
        .iter()
        .any(|p| p.target == ProtectionTarget::Everything));
    assert_protected(exe.function("answer").unwrap().returns(1));
    assert_protected(exe.set_static("VALUE", 1));
    assert_protected(exe.patch_file_bytes(0x1000, &[0]));
    assert_protected(exe.replace_bytes(b"value=", b"VALUE="));
    assert_eq!(t.run()["answer"], "42");
}

#[test]
fn unprotected_programs_report_no_rules() {
    let t = Copy::of(HOT);
    let exe = t.open();
    assert!(!exe.is_protected());
    assert!(exe.protections().is_empty());
}

#[test]
fn compiler_style_checks_on_functions() {
    let t = Copy::of(TARGET);
    let mut exe = t.open();
    // Mauvais type de valeur pour le type de retour.
    assert!(matches!(
        exe.function("compute").unwrap().returns(true),
        Err(Error::TypeMismatch { .. })
    ));
    assert!(matches!(
        exe.function("compute").unwrap().returns(1.5f64),
        Err(Error::TypeMismatch { .. })
    ));
    assert!(matches!(
        exe.function("is_admin").unwrap().returns(1),
        Err(Error::TypeMismatch { .. })
    ));
    // Valeur hors limites du type de retour (i32).
    assert!(matches!(
        exe.function("compute").unwrap().returns(1i64 << 40),
        Err(Error::ValueOutOfRange { .. })
    ));
    // `()` ne peut rien renvoyer ; une fonction typée ne peut pas être désactivée.
    assert!(matches!(
        exe.function("say_hi").unwrap().returns(1),
        Err(Error::Rejected(_))
    ));
    assert!(matches!(
        exe.function("compute").unwrap().disable(),
        Err(Error::Rejected(_))
    ));
    // Redirection entre signatures différentes.
    assert!(matches!(
        exe.function("redirected").unwrap().redirect_to("say_hi"),
        Err(Error::Rejected(_))
    ));
    assert!(exe.pending().is_empty());

    // Une valeur de type voisin est convertie vers le type déclaré.
    exe.function("compute")
        .unwrap()
        .returns(-7i64 as i32)
        .unwrap();
    exe.commit().unwrap();
    assert_eq!(t.run()["compute"], "-7");
}

#[test]
fn stripped_protected_executables_are_fully_locked() {
    // Retire la table de symboles : les noms protégés ne peuvent plus être
    // localisés, donc rien ne doit pouvoir être modifié.
    let t = Copy::of(TARGET);
    let status = Command::new("strip").arg(&t.path).status();
    let Ok(status) = status else { return }; // `strip` absent : test sans objet
    assert!(status.success());
    let mut exe = t.open();
    assert!(exe.is_protected());
    assert!(exe.functions().is_empty());
    assert_protected(exe.patch_file_bytes(0x1000, &[0]));
    let sections = exe.sections().to_vec();
    let text = sections.iter().find(|s| s.name == ".text").unwrap();
    assert_protected(exe.patch_bytes(text.address, &[0x90]));
    let message = exe
        .patch_bytes(text.address, &[0x90])
        .unwrap_err()
        .to_string();
    assert!(message.contains("stripped"), "{message}");
}

#[test]
fn finds_and_edits_a_static_without_debug_info() {
    // Simule un build release : aucune information DWARF n'est utilisée ici.
    let t = Copy::of(TARGET);
    let mut exe = t.open();
    let matches = exe.find_value(Value::i32(10));
    let target = matches
        .iter()
        .find(|m| m.name.as_deref() == Some("target_demo::LIMIT"))
        .expect("LIMIT should be found by its content");
    assert_eq!(exe.peek_i32(target.address).unwrap(), 10);
    exe.poke_i32(target.address, 55).unwrap();
    exe.commit().unwrap();
    assert_eq!(t.run()["limit"], "55");
}

#[test]
fn peek_and_poke_round_trip_every_scalar_kind() {
    let t = Copy::of(TARGET);
    let mut exe = t.open();
    let addr = exe.static_bytes("LIMIT").ok().and(None::<u64>); // garde le type inféré
    let _ = addr;
    let a = exe
        .find_value(Value::i32(10))
        .into_iter()
        .find(|m| m.name.as_deref() == Some("target_demo::LIMIT"))
        .unwrap()
        .address;
    exe.poke_i32(a, -7).unwrap();
    assert_eq!(
        exe.peek_i32(a).unwrap(),
        10,
        "poke is queued, not applied until commit"
    );

    let b = exe
        .find_value(Value::u8(7))
        .into_iter()
        .find(|m| m.name.as_deref() == Some("target_demo::SMALL"))
        .unwrap()
        .address;
    exe.poke_u8(b, 250).unwrap();

    let c = exe
        .find_value(Value::f64(1.5))
        .into_iter()
        .find(|m| m.name.as_deref() == Some("target_demo::RATIO"))
        .unwrap()
        .address;
    exe.poke_f64(c, 9.5).unwrap();

    exe.commit().unwrap();
    assert_eq!(exe.peek_i32(a).unwrap(), -7);
    assert_eq!(exe.peek_u8(b).unwrap(), 250);
    assert_eq!(exe.peek_f64(c).unwrap(), 9.5);
}

/// Compiles a tiny, optimised, standalone program so `helper` gets fully
/// inlined into `main` (no `#[inline(never)]`), then checks that looking it
/// up names the real problem instead of a plain "not found".
#[test]
fn inlined_function_reports_a_friendlier_error() {
    let dir = std::env::temp_dir().join(format!(
        "ruwrite-inline-probe-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::create_dir_all(&dir).unwrap();
    let src = dir.join("inline_probe.rs");
    fs::write(
        &src,
        r#"
        #[inline]
        fn helper(x: i32) -> i32 {
            x.wrapping_add(1)
        }

        fn main() {
            println!("{}", std::hint::black_box(helper(std::hint::black_box(41))));
        }
        "#,
    )
    .unwrap();
    let bin = dir.join(if cfg!(windows) {
        "inline_probe.exe"
    } else {
        "inline_probe"
    });
    let status = Command::new("rustc")
        .args(["-O", "-C", "debuginfo=2", "-o"])
        .arg(&bin)
        .arg(&src)
        .status()
        .expect("rustc should be on PATH");
    assert!(status.success(), "rustc failed to build the probe");

    let mut exe = Executable::open(&bin).expect("the probe is a debug-info-bearing Rust binary");
    match exe.function("helper") {
        Err(Error::Inlined { name }) => assert!(name.ends_with("helper"), "{name}"),
        Ok(_) => panic!("expected Error::Inlined, got Ok"),
        Err(other) => panic!("expected Error::Inlined, got {other}"),
    }

    let _ = fs::remove_dir_all(&dir);
}

/// `cascade_check(x)` compares `a` (set to `x`) against `5` with a signed
/// `jl`, so `cascade_check(-100)` returns `1` (`-100 < 5` is true for a
/// signed reading). Cascade mode, triggered by writing an unsigned value to
/// the (declared signed) `a`, flips that `jl` to the unsigned `jb`: the same
/// bit pattern read as unsigned is a huge number, no longer "below" `5`, so
/// the patched binary returns `2` instead for the same input.
#[test]
fn cascade_mode_flips_a_comparison_to_match_a_retyped_variable() {
    let t = Copy::of(TARGET);
    assert_eq!(t.run()["cascade"], "1");

    let mut exe = t.open();
    exe.local("cascade_check", "a")
        .unwrap()
        .cascade()
        .set(Value::u8(200))
        .unwrap();
    exe.commit().unwrap();

    assert_eq!(t.run()["cascade"], "2");
}

#[test]
fn value_matches_stay_within_data_like_sections() {
    let t = Copy::of(TARGET);
    let exe = t.open();
    let text = exe
        .sections()
        .iter()
        .find(|s| s.name == ".text")
        .unwrap()
        .clone();
    for m in exe.find_value(Value::i32(10)) {
        assert!(
            !(text.address..text.address + text.size).contains(&m.address),
            "a value match landed inside .text at {:#x}",
            m.address
        );
    }
}
