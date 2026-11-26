//! The [`Executable`] type: loading, inspection, edits and `commit`.

use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use object::{Object, ObjectSection, ObjectSymbol, SymbolKind};

use crate::arch::{Arch, Endian, Info};
use crate::code;
use crate::dwarf::{self, DebugInfo, DwFunction, DwStatic};
use crate::edit::{FunctionEditor, LocalEditor};
use crate::error::{Error, Result};
use crate::live;
use crate::patch::{checked_range, Patch, PatchSet};
use crate::value::Value;

/// A section of the executable.
#[derive(Debug, Clone)]
pub struct Section {
    /// Section name (`.text`, `.rodata`, ...).
    pub name: String,
    /// Virtual address once loaded.
    pub address: u64,
    /// Size in bytes.
    pub size: u64,
    /// Offset in the file, `None` for sections that only exist in memory (`.bss`).
    pub file_offset: Option<u64>,
    /// `true` for code sections.
    pub executable: bool,
    /// `true` for sections that are writable at run time.
    pub writable: bool,
}

/// A function found in the symbol table.
#[derive(Debug, Clone)]
pub struct Function {
    /// Readable name, without the trailing hash (`demo::compute`).
    pub name: String,
    /// Raw symbol name as stored in the file.
    pub mangled: String,
    /// Virtual address of the first instruction.
    pub address: u64,
    /// Size of the machine code in bytes.
    pub size: u64,
    /// Offset of the first instruction in the file.
    pub file_offset: Option<u64>,
}

/// A variable discovered through the debug information.
#[derive(Debug, Clone)]
pub struct Variable {
    /// Fully qualified name for statics, plain name for locals.
    pub name: String,
    /// Type as written in the source (`i32`, `[u8; 4]`, ...).
    pub ty: String,
    /// Size in bytes.
    pub size: u64,
    /// Where the variable lives.
    pub kind: VariableKind,
}

/// Storage of a [`Variable`].
#[derive(Debug, Clone)]
pub enum VariableKind {
    /// A `static`: fixed address in the loaded image.
    Static {
        /// Virtual address.
        address: u64,
        /// Offset in the file, `None` when the bytes only exist at run time.
        file_offset: Option<u64>,
    },
    /// A local variable living in the stack frame of a function.
    Local {
        /// Fully qualified name of the owning function.
        function: String,
        /// Line of the declaration, when known.
        decl_line: Option<u32>,
    },
}

/// Summary returned by the `commit*` methods.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CommitReport {
    /// Number of edits applied.
    pub patches: usize,
    /// Number of bytes written.
    pub bytes: usize,
    /// File written, for `commit` and `commit_to`.
    pub path: Option<PathBuf>,
    /// Backup of the previous file, if one was created by this commit.
    pub backup: Option<PathBuf>,
    /// Process that was patched, for `commit_hot`.
    pub pid: Option<u32>,
}

/// A compiled Rust executable opened for inspection and rewriting.
///
/// Edits are queued, then applied all at once by [`commit`](Self::commit) (the
/// file), [`commit_to`](Self::commit_to) (a copy) or
/// [`commit_hot`](Self::commit_hot) (a running process). Until then nothing
/// is modified, and [`discard`](Self::discard) throws the queue away.
pub struct Executable {
    path: PathBuf,
    image: Vec<u8>,
    info: Info,
    sections: Vec<Section>,
    functions: Vec<Function>,
    debug: OnceLock<std::result::Result<DebugInfo, String>>,
    pending: PatchSet,
    backup: bool,
}

impl fmt::Debug for Executable {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Executable")
            .field("path", &self.path)
            .field("info", &self.info)
            .field("pending", &self.pending.as_slice().len())
            .finish_non_exhaustive()
    }
}

/// Résultat de l'analyse initiale du fichier.
struct Parsed {
    info: Info,
    sections: Vec<Section>,
    functions: Vec<Function>,
}

impl Executable {
    /// Opens a Rust executable.
    ///
    /// Fails with [`Error::NotRust`] when the file does not look like it was
    /// built by `rustc`; use [`open_any`](Self::open_any) to skip that check.
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        Self::load(path.as_ref(), true)
    }

    /// Opens any ELF, PE or Mach-O executable without checking that it is Rust.
    pub fn open_any(path: impl AsRef<Path>) -> Result<Self> {
        Self::load(path.as_ref(), false)
    }

    fn load(path: &Path, require_rust: bool) -> Result<Self> {
        let image = fs::read(path).map_err(|e| Error::io(path, e))?;
        let parsed = parse(&image)?;
        if require_rust && !parsed.info.is_rust {
            return Err(Error::NotRust(path.to_path_buf()));
        }
        Ok(Executable {
            path: path.to_path_buf(),
            image,
            info: parsed.info,
            sections: parsed.sections,
            functions: parsed.functions,
            debug: OnceLock::new(),
            pending: PatchSet::default(),
            backup: true,
        })
    }

    // ------------------------------------------------------------------
    // Informations
    // ------------------------------------------------------------------

    /// Path of the file this executable is bound to.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Format, architecture, byte order and other general facts.
    pub fn info(&self) -> &Info {
        &self.info
    }

    /// Sections of the executable.
    pub fn sections(&self) -> &[Section] {
        &self.sections
    }

    /// Functions found in the symbol table, sorted by address.
    ///
    /// The list is empty for a stripped executable; [`function`](Self::function)
    /// then cannot be used, but byte-level patching still works.
    pub fn functions(&self) -> &[Function] {
        &self.functions
    }

    /// Statics and locals described by the DWARF debug information.
    ///
    /// Fails with [`Error::NoDebugInfo`] when the executable has none
    /// (release builds usually do not; set `debug = true` in the profile).
    pub fn variables(&self) -> Result<Vec<Variable>> {
        let debug = self.debug()?;
        let mut vars = Vec::new();
        for st in &debug.statics {
            vars.push(Variable {
                name: st.name.clone(),
                ty: st.ty.name.clone(),
                size: st.ty.size as u64,
                kind: VariableKind::Static {
                    address: st.address,
                    file_offset: self.vaddr_to_offset(st.address),
                },
            });
        }
        for func in &debug.functions {
            for local in &func.locals {
                vars.push(Variable {
                    name: local.name.clone(),
                    ty: local.ty.name.clone(),
                    size: local.ty.size as u64,
                    kind: VariableKind::Local {
                        function: func.name.clone(),
                        decl_line: local.decl_line,
                    },
                });
            }
        }
        Ok(vars)
    }

    pub(crate) fn debug(&self) -> Result<&DebugInfo> {
        let parsed = self.debug.get_or_init(|| {
            let file = object::File::parse(&*self.image).map_err(|e| e.to_string())?;
            dwarf::load(&file).map_err(|e| match e {
                Error::NoDebugInfo(msg) => msg,
                other => other.to_string(),
            })
        });
        parsed
            .as_ref()
            .map_err(|msg| Error::NoDebugInfo(msg.clone()))
    }

    pub(crate) fn endian(&self) -> Endian {
        self.info.endian
    }

    pub(crate) fn arch(&self) -> &Arch {
        &self.info.arch
    }

    // ------------------------------------------------------------------
    // Accès aux octets
    // ------------------------------------------------------------------

    /// Converts a virtual address to an offset in the file.
    ///
    /// Returns `None` for addresses outside the file-backed sections.
    pub fn vaddr_to_offset(&self, address: u64) -> Option<u64> {
        self.sections.iter().find_map(|s| {
            let base = s.file_offset?;
            (s.address != 0 && (s.address..s.address + s.size).contains(&address))
                .then(|| base + (address - s.address))
        })
    }

    pub(crate) fn slice(&self, offset: u64, len: u64) -> Result<&[u8]> {
        let len = usize::try_from(len).map_err(|_| Error::OutOfBounds {
            offset,
            length: len,
            image_len: self.image.len() as u64,
        })?;
        let range = checked_range(self.image.len(), offset, len)?;
        Ok(&self.image[range])
    }

    fn offset_of(&self, address: u64) -> Result<u64> {
        self.vaddr_to_offset(address).ok_or(Error::NotInFile {
            address,
            reason: "no file-backed section contains it; `.bss` only exists at run time".into(),
        })
    }

    /// Reads `len` bytes at a virtual address (committed contents, not pending edits).
    pub fn read_bytes(&self, address: u64, len: usize) -> Result<&[u8]> {
        self.slice(self.offset_of(address)?, len as u64)
    }

    /// Queues the replacement of the bytes at a virtual address.
    pub fn patch_bytes(&mut self, address: u64, bytes: &[u8]) -> Result<&mut Self> {
        let offset = self.offset_of(address)?;
        self.push_patch(offset, Some(address), bytes.to_vec(), "raw bytes".into())?;
        Ok(self)
    }

    /// Queues the replacement of the bytes at a file offset.
    pub fn patch_file_bytes(&mut self, offset: u64, bytes: &[u8]) -> Result<&mut Self> {
        self.push_patch(offset, None, bytes.to_vec(), "raw bytes".into())?;
        Ok(self)
    }

    /// Queues the replacement of `len` bytes by `nop` instructions.
    pub fn nop(&mut self, address: u64, len: usize) -> Result<&mut Self> {
        let nops = code::nop_run(&self.info.arch, len)?;
        let offset = self.offset_of(address)?;
        self.push_patch(offset, Some(address), nops, format!("{len} bytes -> nop"))?;
        Ok(self)
    }

    /// Virtual addresses of every occurrence of `pattern` in the loaded sections.
    pub fn find_bytes(&self, pattern: &[u8]) -> Vec<u64> {
        if pattern.is_empty() {
            return Vec::new();
        }
        let mut hits = Vec::new();
        for section in self.loaded_sections() {
            let (base, data) = section;
            for pos in memchr::memmem::find_iter(data, pattern) {
                hits.push(base + pos as u64);
            }
        }
        hits
    }

    /// Queues the replacement of every occurrence of `old` by `new`
    /// (same length), in the loaded sections only. Returns the number of
    /// occurrences found.
    ///
    /// Typical use: changing a string literal. Choose a pattern long enough
    /// to be unique, since machine code is searched as well.
    pub fn replace_bytes(&mut self, old: &[u8], new: &[u8]) -> Result<usize> {
        if old.len() != new.len() {
            return Err(Error::SizeMismatch {
                expected: old.len(),
                got: new.len(),
            });
        }
        let hits = self.find_bytes(old);
        if hits.is_empty() {
            return Err(Error::not_found(
                "byte pattern",
                String::from_utf8_lossy(old).into_owned(),
            ));
        }
        for address in &hits {
            let offset = self.offset_of(*address)?;
            self.push_patch(
                offset,
                Some(*address),
                new.to_vec(),
                "replaced pattern".into(),
            )?;
        }
        Ok(hits.len())
    }

    /// Sections chargées en mémoire et présentes dans le fichier, avec leur
    /// adresse virtuelle de départ.
    fn loaded_sections(&self) -> impl Iterator<Item = (u64, &[u8])> {
        self.sections.iter().filter_map(move |s| {
            let offset = s.file_offset?;
            if s.address == 0 {
                return None;
            }
            self.slice(offset, s.size)
                .ok()
                .map(|data| (s.address, data))
        })
    }

    // ------------------------------------------------------------------
    // Fonctions
    // ------------------------------------------------------------------

    /// Looks a function up by name and returns an editor for it.
    ///
    /// The name can be the full path (`demo::compute`), its last segments
    /// (`compute`) or the raw symbol. An error lists the candidates when the
    /// name is ambiguous.
    pub fn function(&mut self, name: &str) -> Result<FunctionEditor<'_>> {
        let function = self.find_function(name)?.clone();
        Ok(FunctionEditor::new(self, function))
    }

    pub(crate) fn find_function(&self, query: &str) -> Result<&Function> {
        let exact: Vec<&Function> = self
            .functions
            .iter()
            .filter(|f| f.name == query || f.mangled == query)
            .collect();
        if let Some(found) = single("function", query, exact, |f| f.name.clone())? {
            return Ok(found);
        }
        let suffix = format!("::{query}");
        let partial: Vec<&Function> = self
            .functions
            .iter()
            .filter(|f| f.name.ends_with(&suffix))
            .collect();
        single("function", query, partial, |f| f.name.clone())?
            .ok_or_else(|| Error::not_found("function", query))
    }

    // ------------------------------------------------------------------
    // Variables
    // ------------------------------------------------------------------

    fn find_static(&self, query: &str) -> Result<&DwStatic> {
        let debug = self.debug()?;
        let exact: Vec<&DwStatic> = debug.statics.iter().filter(|s| s.name == query).collect();
        if let Some(found) = single("static", query, exact, |s| s.name.clone())? {
            return Ok(found);
        }
        let suffix = format!("::{query}");
        let partial: Vec<&DwStatic> = debug
            .statics
            .iter()
            .filter(|s| s.name.ends_with(&suffix))
            .collect();
        single("static", query, partial, |s| s.name.clone())?
            .ok_or_else(|| Error::not_found("static", query))
    }

    fn static_offset(&self, st: &DwStatic) -> Result<u64> {
        self.vaddr_to_offset(st.address).ok_or_else(|| Error::NotInFile {
            address: st.address,
            reason: format!(
                "`{}` is not stored in the file (zero-initialised `.bss` data only exists at run time)",
                st.name
            ),
        })
    }

    /// Raw bytes of a `static`, as stored in the file.
    pub fn static_bytes(&self, name: &str) -> Result<&[u8]> {
        let st = self.find_static(name)?;
        self.slice(self.static_offset(st)?, st.ty.size as u64)
    }

    /// Current value of a scalar `static`, as stored in the file.
    pub fn static_value(&self, name: &str) -> Result<Value> {
        let st = self.find_static(name)?;
        let bytes = self.slice(self.static_offset(st)?, st.ty.size as u64)?;
        Value::decode(bytes, &st.ty, self.endian())
    }

    /// Queues a new value for a scalar `static`.
    ///
    /// The value is encoded with the width and signedness of the variable, so
    /// `set_static("LIMIT", 2u8)` on an `i32` stores `02 00 00 00`. Values
    /// that do not fit are refused.
    pub fn set_static(&mut self, name: &str, value: impl Into<Value>) -> Result<&mut Self> {
        let value = value.into();
        let st = self.find_static(name)?.clone();
        let bytes = value.encode(&st.ty, self.endian())?;
        let offset = self.static_offset(&st)?;
        let before = self
            .static_value(name)
            .map_or_else(|_| "?".to_string(), |v| v.to_string());
        self.push_patch(
            offset,
            Some(st.address),
            bytes,
            format!("static {}: {} = {before} -> {value}", st.name, st.ty.name),
        )?;
        Ok(self)
    }

    /// Queues new raw contents for a `static` of any type (arrays, structs...).
    pub fn set_static_bytes(&mut self, name: &str, bytes: &[u8]) -> Result<&mut Self> {
        let st = self.find_static(name)?.clone();
        if bytes.len() != st.ty.size {
            return Err(Error::SizeMismatch {
                expected: st.ty.size,
                got: bytes.len(),
            });
        }
        let offset = self.static_offset(&st)?;
        self.push_patch(
            offset,
            Some(st.address),
            bytes.to_vec(),
            format!("static {}: {}", st.name, st.ty.name),
        )?;
        Ok(self)
    }

    /// Looks up a local variable of a function and returns an editor for it.
    ///
    /// Only the initial value of the variable can be rewritten, and only in
    /// builds where the compiler stores it with a constant (`opt-level = 0`).
    pub fn local(&mut self, function: &str, name: &str) -> Result<LocalEditor<'_>> {
        let (func, locals) = self.find_local(function, name)?;
        Ok(LocalEditor::new(self, func, locals))
    }

    fn find_local(
        &self,
        function: &str,
        name: &str,
    ) -> Result<(DwFunction, Vec<crate::dwarf::DwLocal>)> {
        let debug = self.debug()?;
        let suffix = format!("::{function}");
        let having_local = |f: &&DwFunction| f.locals.iter().any(|l| l.name == name);

        let exact: Vec<&DwFunction> = debug
            .functions
            .iter()
            .filter(|f| f.name == function)
            .filter(having_local)
            .collect();
        let candidates = if exact.is_empty() {
            debug
                .functions
                .iter()
                .filter(|f| f.name.ends_with(&suffix))
                .filter(having_local)
                .collect()
        } else {
            exact
        };

        let func = single("function with that local", function, candidates, |f| {
            f.name.clone()
        })?
        .ok_or_else(|| Error::not_found("local variable", format!("{function}::{name}")))?;
        let locals = func
            .locals
            .iter()
            .filter(|l| l.name == name)
            .cloned()
            .collect();
        Ok((func.clone(), locals))
    }

    // ------------------------------------------------------------------
    // File d'attente et validation
    // ------------------------------------------------------------------

    /// Queues an edit, recording the bytes it replaces.
    pub(crate) fn push_patch(
        &mut self,
        file_offset: u64,
        address: Option<u64>,
        new: Vec<u8>,
        description: String,
    ) -> Result<()> {
        let old = self.slice(file_offset, new.len() as u64)?.to_vec();
        self.pending.push(Patch {
            file_offset,
            address,
            old,
            new,
            description,
        })
    }

    /// Edits waiting for a `commit`.
    pub fn pending(&self) -> &[Patch] {
        self.pending.as_slice()
    }

    /// Drops every queued edit.
    pub fn discard(&mut self) {
        self.pending.clear();
    }

    /// Enables or disables the `<file>.orig` backup made by [`commit`](Self::commit)
    /// (enabled by default; an existing backup is never overwritten).
    pub fn set_backup(&mut self, enabled: bool) -> &mut Self {
        self.backup = enabled;
        self
    }

    // ------------------------------------------------------------------
    // Validation
    // ------------------------------------------------------------------

    /// Applies every queued edit to the executable file.
    ///
    /// All edits are checked first, against the file as it is on disk right
    /// now (a file modified since `open` is refused with
    /// [`Error::BytesMismatch`]); if one fails, nothing is written. The
    /// file is replaced atomically (temporary file + rename), so a process
    /// already running the old binary keeps working.
    pub fn commit(&mut self) -> Result<CommitReport> {
        let target = self.path.clone();
        self.commit_file(&target, true)
    }

    /// Like [`commit`](Self::commit) but writes the patched executable to
    /// `path`, leaving the original file untouched. The `Executable` is then
    /// bound to the new file.
    pub fn commit_to(&mut self, path: impl AsRef<Path>) -> Result<CommitReport> {
        let path = path.as_ref().to_path_buf();
        let report = self.commit_file(&path, false)?;
        if !report.path.is_none() {
            self.path = path;
        }
        Ok(report)
    }

    /// `from_disk` : repartir du contenu actuel du fichier plutôt que de
    /// l'image chargée à l'ouverture, pour refuser le commit si quelqu'un
    /// d'autre a modifié l'exécutable entre-temps.
    fn commit_file(&mut self, target: &Path, from_disk: bool) -> Result<CommitReport> {
        if self.pending.is_empty() {
            return Ok(CommitReport::default());
        }
        let mut image = if from_disk {
            fs::read(target).map_err(|e| Error::io(target, e))?
        } else {
            self.image.clone()
        };
        let patches = self.pending.as_slice().len();
        let bytes = self.pending.apply(&mut image)?;

        let target = resolve_symlink(target);
        let backup = if self.backup {
            make_backup(&target)?
        } else {
            None
        };
        write_atomic(&target, &image, &self.path)?;

        self.image = image;
        self.pending.clear();
        Ok(CommitReport {
            patches,
            bytes,
            path: Some(target),
            backup,
            pid: None,
        })
    }

    /// Applies every queued edit to the memory of a running process.
    ///
    /// The process must be running this executable and be owned by the same
    /// user (ptrace permissions apply). Edits stay queued, so a following
    /// [`commit`](Self::commit) makes them permanent. Calling it twice is
    /// harmless. Linux only.
    ///
    /// Memory is rewritten while the program keeps running: patch code that
    /// is not executing at that very moment (functions that are called, not
    /// the one currently on the stack).
    pub fn commit_hot(&mut self, pid: u32) -> Result<CommitReport> {
        let written = live::apply(pid, self.pending.as_slice())?;
        let bytes = self
            .pending
            .as_slice()
            .iter()
            .map(Patch::len)
            .sum::<usize>();
        Ok(CommitReport {
            patches: written,
            bytes,
            path: None,
            backup: None,
            pid: Some(pid),
        })
    }
}

/// Renvoie l'unique élément, `None` s'il n'y en a aucun, ou une erreur
/// `Ambiguous` listant les candidats (dédoublonnés).
fn single<'a, T>(
    kind: &'static str,
    query: &str,
    mut found: Vec<&'a T>,
    name: impl Fn(&T) -> String,
) -> Result<Option<&'a T>> {
    match found.len() {
        0 => Ok(None),
        1 => Ok(found.pop()),
        _ => {
            let mut candidates: Vec<String> = found.iter().map(|item| name(item)).collect();
            candidates.sort();
            candidates.dedup();
            Err(Error::Ambiguous {
                kind,
                name: query.to_string(),
                candidates,
            })
        }
    }
}

fn resolve_symlink(path: &Path) -> PathBuf {
    fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

/// Copie `<fichier>` vers `<fichier>.orig` si ce dernier n'existe pas encore.
fn make_backup(target: &Path) -> Result<Option<PathBuf>> {
    if !target.exists() {
        return Ok(None);
    }
    let mut name = target.as_os_str().to_owned();
    name.push(".orig");
    let backup = PathBuf::from(name);
    if backup.exists() {
        return Ok(None);
    }
    fs::copy(target, &backup).map_err(|e| Error::io(&backup, e))?;
    Ok(Some(backup))
}

/// Écrit dans un fichier temporaire voisin puis renomme : le remplacement est
/// atomique et un processus qui exécute l'ancien binaire n'est pas perturbé.
fn write_atomic(target: &Path, image: &[u8], permissions_from: &Path) -> Result<()> {
    let dir = target
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let file_name = target
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "executable".into());
    let tmp = dir.join(format!(".{file_name}.ruwrite-{}.tmp", std::process::id()));

    let write = || -> std::io::Result<()> {
        fs::write(&tmp, image)?;
        // On garde les droits (bit exécutable compris) du fichier d'origine.
        let source = if target.exists() {
            target
        } else {
            permissions_from
        };
        if let Ok(meta) = fs::metadata(source) {
            fs::set_permissions(&tmp, meta.permissions())?;
        }
        fs::File::open(&tmp)?.sync_all()?;
        fs::rename(&tmp, target)
    };
    write().map_err(|e| {
        let _ = fs::remove_file(&tmp);
        Error::io(target, e)
    })
}

fn parse(image: &[u8]) -> Result<Parsed> {
    let file = object::File::parse(image).map_err(|e| Error::Parse(e.to_string()))?;
    let arch = Arch::from(file.architecture());

    let sections: Vec<Section> = file
        .sections()
        .map(|s| Section {
            name: s.name().unwrap_or("").to_string(),
            address: s.address(),
            size: s.size(),
            file_offset: s.file_range().map(|(offset, _)| offset),
            executable: s.kind() == object::SectionKind::Text,
            writable: matches!(
                s.kind(),
                object::SectionKind::Data
                    | object::SectionKind::UninitializedData
                    | object::SectionKind::Tls
                    | object::SectionKind::UninitializedTls
            ),
        })
        .collect();

    let mut functions = Vec::new();
    let mut rust_symbols = false;
    for symbol in file.symbols().chain(file.dynamic_symbols()) {
        let Ok(mangled) = symbol.name() else { continue };
        if rustc_demangle::try_demangle(mangled).is_ok() {
            rust_symbols = true;
        }
        if symbol.kind() != SymbolKind::Text || symbol.size() == 0 || symbol.address() == 0 {
            continue;
        }
        // Sur ARM 32 bits, le bit de poids faible signale le mode Thumb.
        let address = if arch == Arch::Arm {
            symbol.address() & !1
        } else {
            symbol.address()
        };
        functions.push(Function {
            name: format!("{:#}", rustc_demangle::demangle(mangled)),
            mangled: mangled.to_string(),
            address,
            size: symbol.size(),
            file_offset: None,
        });
    }
    functions.sort_by(|a, b| (a.address, &a.mangled).cmp(&(b.address, &b.mangled)));
    functions.dedup_by(|a, b| a.address == b.address && a.mangled == b.mangled);

    let info = Info {
        format: file.format().into(),
        arch,
        bits: if file.is_64() { 64 } else { 32 },
        endian: match file.endianness() {
            object::Endianness::Little => Endian::Little,
            object::Endianness::Big => Endian::Big,
        },
        entry: file.entry(),
        is_rust: rust_symbols || has_rust_markers(image),
        has_debug_info: file.has_debug_symbols(),
    };

    let mut parsed = Parsed {
        info,
        sections,
        functions,
    };
    // Les décalages dans le fichier se déduisent des sections.
    let offsets: Vec<Option<u64>> = parsed
        .functions
        .iter()
        .map(|f| section_offset(&parsed.sections, f.address))
        .collect();
    for (function, offset) in parsed.functions.iter_mut().zip(offsets) {
        function.file_offset = offset;
    }
    Ok(parsed)
}

fn section_offset(sections: &[Section], address: u64) -> Option<u64> {
    sections.iter().find_map(|s| {
        let base = s.file_offset?;
        (s.address != 0 && (s.address..s.address + s.size).contains(&address))
            .then(|| base + (address - s.address))
    })
}

/// Marques laissées par `rustc` même dans un binaire sans symboles.
fn has_rust_markers(image: &[u8]) -> bool {
    const MARKERS: [&[u8]; 3] = [b"/rustc/", b"rust_begin_unwind", b"RUST_BACKTRACE"];
    MARKERS
        .iter()
        .any(|m| memchr::memmem::find(image, m).is_some())
}
