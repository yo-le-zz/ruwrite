//! Tests de bout en bout : on réécrit un vrai exécutable, on le lance, et on
//! vérifie ce qu'il affiche. Les binaires cibles sont ceux de ce paquet.

use std::collections::HashMap;
use std::fs;
use std::io::{BufRead, BufReader};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::{Duration, Instant};

use ruwrite::{Error, Executable, Value};

const TARGET: &str = env!("CARGO_BIN_EXE_target_demo");
const HOT: &str = env!("CARGO_BIN_EXE_hot_demo");

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
    let err = exe.function("is_admin").unwrap().returns(u64::MAX);
    assert!(matches!(err, Err(Error::FunctionTooSmall { .. })));
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
