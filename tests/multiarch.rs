//! Multi-architecture checks on synthetic ELF64 executables.
//!
//! No cross toolchain is needed: each test builds a tiny executable by hand
//! (one `.text` section, one symbol with a Rust-mangled name) for the
//! architecture under test, then rewrites it through the public API.

use std::fs;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use ruwrite::{Arch, Error, Executable};

const MACHINE_X86_64: u16 = 62;
const MACHINE_AARCH64: u16 = 183;
const MACHINE_RISCV: u16 = 243;
const TEXT_ADDR: u64 = 0x40_0040;
const SYMBOL: &str = "_ZN4demo4main17h0123456789abcdefE";

fn push_u16(v: &mut Vec<u8>, x: u16) {
    v.extend(x.to_le_bytes());
}
fn push_u32(v: &mut Vec<u8>, x: u32) {
    v.extend(x.to_le_bytes());
}
fn push_u64(v: &mut Vec<u8>, x: u64) {
    v.extend(x.to_le_bytes());
}

/// Executable ELF64 little-endian minimal : `.text` contient `code`, avec un
/// symbole de fonction couvrant tout le code.
fn build_elf(machine: u16, code: &[u8]) -> Vec<u8> {
    let shstrtab = b"\0.text\0.symtab\0.strtab\0.shstrtab\0";
    let mut strtab = vec![0u8];
    strtab.extend(SYMBOL.as_bytes());
    strtab.push(0);

    let mut symtab = vec![0u8; 24]; // symbole nul
    push_u32(&mut symtab, 1); // st_name
    symtab.push(0x12); // GLOBAL | FUNC
    symtab.push(0);
    push_u16(&mut symtab, 1); // section .text
    push_u64(&mut symtab, TEXT_ADDR);
    push_u64(&mut symtab, code.len() as u64);

    let text_off = 0x40usize;
    let symtab_off = text_off + code.len().next_multiple_of(8);
    let strtab_off = symtab_off + symtab.len();
    let shstr_off = strtab_off + strtab.len();
    let sh_off = (shstr_off + shstrtab.len()).next_multiple_of(8);

    let mut elf = Vec::new();
    elf.extend([0x7F, b'E', b'L', b'F', 2, 1, 1, 0]);
    elf.extend([0u8; 8]);
    push_u16(&mut elf, 2); // ET_EXEC
    push_u16(&mut elf, machine);
    push_u32(&mut elf, 1);
    push_u64(&mut elf, TEXT_ADDR); // entry
    push_u64(&mut elf, 0); // pas de table de segments
    push_u64(&mut elf, sh_off as u64);
    push_u32(&mut elf, 0);
    push_u16(&mut elf, 64);
    push_u16(&mut elf, 0);
    push_u16(&mut elf, 0);
    push_u16(&mut elf, 64);
    push_u16(&mut elf, 5);
    push_u16(&mut elf, 4); // index de .shstrtab

    elf.extend(code);
    elf.resize(symtab_off, 0);
    elf.extend(&symtab);
    elf.extend(&strtab);
    elf.extend(shstrtab);
    elf.resize(sh_off, 0);

    let mut section = |name: u32,
                       kind: u32,
                       flags: u64,
                       addr: u64,
                       off: usize,
                       size: usize,
                       link: u32,
                       info: u32,
                       align: u64,
                       entsize: u64| {
        push_u32(&mut elf, name);
        push_u32(&mut elf, kind);
        push_u64(&mut elf, flags);
        push_u64(&mut elf, addr);
        push_u64(&mut elf, off as u64);
        push_u64(&mut elf, size as u64);
        push_u32(&mut elf, link);
        push_u32(&mut elf, info);
        push_u64(&mut elf, align);
        push_u64(&mut elf, entsize);
    };
    section(0, 0, 0, 0, 0, 0, 0, 0, 0, 0);
    section(1, 1, 0x6, TEXT_ADDR, text_off, code.len(), 0, 0, 4, 0); // .text AX
    section(7, 2, 0, 0, symtab_off, symtab.len(), 3, 1, 8, 24); // .symtab
    section(15, 3, 0, 0, strtab_off, strtab.len(), 0, 0, 1, 0); // .strtab
    section(23, 3, 0, 0, shstr_off, shstrtab.len(), 0, 0, 1, 0); // .shstrtab
    elf
}

struct Sample(PathBuf);

/// Writes `data` to `path`, then waits until it can be read back.
///
/// On Windows CI runners, a file whose content starts with an executable
/// magic number (`\x7fELF` here) can be briefly locked by antivirus
/// scanning right after it is created, so an immediate read fails with
/// "Access is denied" even though the write itself succeeded. This is purely
/// about the sample files this test writes to a temp directory, not about
/// anything ruwrite does to the user's own files.
fn write_then_wait_readable(path: &std::path::Path, data: &[u8]) {
    fs::write(path, data).unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        match fs::read(path) {
            Ok(_) => return,
            Err(e) if Instant::now() < deadline => {
                let _ = e;
                std::thread::sleep(Duration::from_millis(50));
            }
            Err(e) => panic!("cannot read back {path:?} after writing it: {e}"),
        }
    }
}

impl Sample {
    fn new(tag: &str, machine: u16, code: &[u8]) -> Sample {
        let dir = std::env::temp_dir().join(format!("ruwrite-arch-{}-{tag}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("sample");
        write_then_wait_readable(&path, &build_elf(machine, code));
        Sample(path)
    }
    fn code(&self) -> Vec<u8> {
        // Même précaution : on vient peut-être de réécrire ce fichier.
        let deadline = Instant::now() + Duration::from_secs(5);
        let image = loop {
            match fs::read(&self.0) {
                Ok(data) => break data,
                Err(e) if Instant::now() < deadline => {
                    let _ = e;
                    std::thread::sleep(Duration::from_millis(50));
                }
                Err(e) => panic!("cannot read back {:?}: {e}", self.0),
            }
        };
        image[0x40..0x40 + 16].to_vec()
    }
    fn open(&self) -> Executable {
        // Même précaution qu'à l'écriture : réessaie si l'antivirus tient
        // encore le fichier verrouillé.
        retry_io(|| Executable::open(&self.0))
    }
}

/// Retries a fallible `ruwrite` operation for a few seconds before giving up.
///
/// On Windows CI runners, a sample file this test just wrote or is about to
/// rewrite can be briefly locked by antivirus scanning, so an otherwise
/// correct call (`Executable::open`, `commit`, ...) can fail with
/// `PermissionDenied` even though nothing is genuinely wrong. Pending edits
/// are only cleared by `commit` on success, so retrying it is safe: a failed
/// attempt leaves them queued for the next one.
fn retry_io<T>(mut f: impl FnMut() -> ruwrite::Result<T>) -> T {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        match f() {
            Ok(v) => return v,
            Err(Error::Io { .. }) if Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(50));
            }
            Err(e) => panic!("{e}"),
        }
    }
}

impl Drop for Sample {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(self.0.parent().unwrap());
    }
}

/// 16 octets de code factice (nop) pour que les réécritures aient de la place.
const X86_BODY: [u8; 16] = [0x90; 16];
const A64_BODY: [u8; 16] = [
    0x1F, 0x20, 0x03, 0xD5, 0x1F, 0x20, 0x03, 0xD5, 0x1F, 0x20, 0x03, 0xD5, 0xC0, 0x03, 0x5F, 0xD6,
];

#[test]
fn x86_64_function_is_found_and_rewritten() {
    let s = Sample::new("x86", MACHINE_X86_64, &X86_BODY);
    let mut exe = s.open();
    assert_eq!(exe.info().arch, Arch::X86_64);
    assert_eq!(exe.functions()[0].name, "demo::main");
    exe.function("main").unwrap().returns(7).unwrap();
    retry_io(|| exe.commit());
    assert_eq!(&s.code()[..6], [0xB8, 7, 0, 0, 0, 0xC3]);
}

#[test]
fn aarch64_function_is_found_and_rewritten() {
    let s = Sample::new("a64", MACHINE_AARCH64, &A64_BODY);
    let mut exe = s.open();
    assert_eq!(exe.info().arch, Arch::Aarch64);
    exe.function("demo::main").unwrap().returns(42).unwrap();
    retry_io(|| exe.commit());
    // mov w0, #42 ; ret
    assert_eq!(
        &s.code()[..8],
        [0x40, 0x05, 0x80, 0x52, 0xC0, 0x03, 0x5F, 0xD6]
    );

    let mut exe = s.open();
    exe.function("main").unwrap().disable().unwrap();
    retry_io(|| exe.commit());
    assert_eq!(&s.code()[..4], [0xC0, 0x03, 0x5F, 0xD6]);

    // Désassemblage et patch de locaux : réservés à x86.
    let mut exe = s.open();
    assert!(matches!(
        exe.function("main").unwrap().disassemble(),
        Err(Error::Unsupported { .. })
    ));
}

#[test]
fn unsupported_arch_still_allows_byte_patches() {
    let s = Sample::new("rv", MACHINE_RISCV, &X86_BODY);
    let mut exe = s.open();
    assert_eq!(exe.info().arch, Arch::Riscv64);
    assert!(matches!(
        exe.function("main").unwrap().returns(1),
        Err(Error::Unsupported { .. })
    ));
    // Le patch d'octets bruts, lui, marche partout.
    exe.patch_bytes(TEXT_ADDR, &[0xAA, 0xBB]).unwrap();
    retry_io(|| exe.commit());
    assert_eq!(&s.code()[..3], [0xAA, 0xBB, 0x90]);
}

#[test]
fn raw_patching_needs_no_debug_info() {
    // Lecture et écriture d'octets par adresse : aucune information de debug
    // n'est nécessaire (ici l'exemple n'a pas de DWARF).
    let s = Sample::new("raw", MACHINE_X86_64, &X86_BODY);
    let mut exe = s.open();
    assert_eq!(exe.read_bytes(TEXT_ADDR, 2).unwrap(), [0x90, 0x90]);
    exe.nop(TEXT_ADDR + 4, 4).unwrap();
    assert!(exe.variables().is_err(), "no DWARF in this sample");
    assert_eq!(retry_io(|| exe.commit()).bytes, 4);
}
