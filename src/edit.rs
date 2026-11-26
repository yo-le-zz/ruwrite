//! Editors returned by [`Executable::function`] and [`Executable::local`].
//!
//! Each editor borrows the executable mutably and queues edits on it. Methods
//! that finish an edit consume the editor and give the executable back, so
//! calls chain naturally:
//!
//! ```no_run
//! # fn main() -> ruwrite::Result<()> {
//! let mut exe = ruwrite::Executable::open("target/debug/demo")?;
//! exe.function("is_admin")?.returns(true)?
//!     .function("log")?.disable()?
//!     .commit()?;
//! # Ok(()) }
//! ```

use iced_x86::{Decoder, DecoderOptions, Formatter, IntelFormatter};

use crate::arch::Arch;
use crate::code;
use crate::dwarf::{DwFunction, DwLocal, FrameBase};
use crate::error::{Error, Result};
use crate::exe::{Executable, Function};
use crate::mutability::{self, Mutability};
use crate::value::Value;
use crate::x86::{self, ImmStore};

/// Editor for one function of the executable.
pub struct FunctionEditor<'a> {
    exe: &'a mut Executable,
    function: Function,
}

impl<'a> FunctionEditor<'a> {
    pub(crate) fn new(exe: &'a mut Executable, function: Function) -> Self {
        FunctionEditor { exe, function }
    }

    /// Facts about the function (name, address, size).
    pub fn info(&self) -> &Function {
        &self.function
    }

    /// Machine code of the function, as stored in the file.
    pub fn bytes(&self) -> Result<&[u8]> {
        self.exe.slice(self.file_offset()?, self.function.size)
    }

    /// Disassembly in Intel syntax (x86 and x86_64 only), as `(address, text)`.
    pub fn disassemble(&self) -> Result<Vec<(u64, String)>> {
        let bitness = match self.exe.arch() {
            Arch::X86 => 32,
            Arch::X86_64 => 64,
            other => return Err(Error::unsupported("disassembly", other)),
        };
        let mut decoder = Decoder::with_ip(
            bitness,
            self.bytes()?,
            self.function.address,
            DecoderOptions::NONE,
        );
        let mut formatter = IntelFormatter::new();
        let mut lines = Vec::new();
        let mut text = String::new();
        while decoder.can_decode() {
            let insn = decoder.decode();
            text.clear();
            formatter.format(&insn, &mut text);
            lines.push((insn.ip(), text.clone()));
        }
        Ok(lines)
    }

    /// Makes the function return `value` without running its body.
    ///
    /// The value must match the return type of the function (an `i32`
    /// function expects `Value::I32`, a `bool` function expects a `bool`).
    /// Supported on x86, x86_64 and aarch64.
    pub fn returns(self, value: impl Into<Value>) -> Result<&'a mut Executable> {
        let value = value.into();
        let code = code::return_value(self.exe.arch(), value)?;
        let description = format!("{} always returns {value}", self.function.name);
        self.write_prefix(code, description)
    }

    /// Makes the function return immediately (for functions returning `()`).
    pub fn disable(self) -> Result<&'a mut Executable> {
        let code = code::ret(self.exe.arch())?;
        let description = format!("{} disabled", self.function.name);
        self.write_prefix(code, description)
    }

    /// Makes every call to this function run `target` instead.
    ///
    /// `target` must have the same signature. Needs room for a jump at the
    /// start of the function (5 bytes on x86, 4 on aarch64).
    pub fn redirect_to(self, target: &str) -> Result<&'a mut Executable> {
        let destination = self.exe.find_function(target)?.clone();
        let code = code::jump(self.exe.arch(), self.function.address, destination.address)?;
        let description = format!("{} -> {}", self.function.name, destination.name);
        self.write_prefix(code, description)
    }

    /// Replaces the beginning of the function by `machine_code`, filling the
    /// rest with `nop`. The code must fit in the function.
    pub fn replace_code(self, machine_code: &[u8]) -> Result<&'a mut Executable> {
        let size = usize::try_from(self.function.size).unwrap_or(usize::MAX);
        if machine_code.len() > size {
            return Err(self.too_small(machine_code.len()));
        }
        let mut full = machine_code.to_vec();
        full.extend(code::nop_run(self.exe.arch(), size - machine_code.len())?);
        let description = format!("{} rewritten", self.function.name);
        self.write_prefix(full, description)
    }

    fn file_offset(&self) -> Result<u64> {
        self.function.file_offset.ok_or(Error::NotInFile {
            address: self.function.address,
            reason: format!("function `{}`", self.function.name),
        })
    }

    fn too_small(&self, needed: usize) -> Error {
        Error::FunctionTooSmall {
            name: self.function.name.clone(),
            needed,
            available: usize::try_from(self.function.size).unwrap_or(usize::MAX),
        }
    }

    fn write_prefix(self, code: Vec<u8>, description: String) -> Result<&'a mut Executable> {
        let available = usize::try_from(self.function.size).unwrap_or(usize::MAX);
        if code.len() > available {
            return Err(self.too_small(code.len()));
        }
        let offset = self.file_offset()?;
        self.exe
            .push_patch(offset, Some(self.function.address), code, description)?;
        Ok(self.exe)
    }
}

/// Editor for a local variable of a function.
///
/// Only the **initial value** can be rewritten: the compiler stores it with a
/// constant (`mov [slot], 3`), and that constant is what gets replaced. This
/// works for unoptimised builds (`opt-level = 0`), where every local keeps its
/// own stack slot; x86 and x86_64 only.
///
/// The width of the slot never changes: setting `2u8` on an `i32` stores
/// `02 00 00 00`, the variable keeps being a 4-byte integer.
pub struct LocalEditor<'a> {
    exe: &'a mut Executable,
    function: DwFunction,
    locals: Vec<DwLocal>,
}

/// Une instruction candidate et la variable à laquelle elle correspond.
struct Candidate {
    store: ImmStore,
    local: usize,
    address: u64,
    file_offset: u64,
}

impl<'a> LocalEditor<'a> {
    pub(crate) fn new(exe: &'a mut Executable, function: DwFunction, locals: Vec<DwLocal>) -> Self {
        LocalEditor {
            exe,
            function,
            locals,
        }
    }

    /// Rewrites the initial value of the variable.
    ///
    /// Fails when no instruction stores a constant in the variable (optimised
    /// build, value computed at run time) or when several do and the source
    /// line cannot tell them apart; use [`set_all`](Self::set_all) then.
    pub fn set(self, value: impl Into<Value>) -> Result<&'a mut Executable> {
        self.apply(value.into(), false)
    }

    /// Rewrites every constant store found for the variable.
    pub fn set_all(self, value: impl Into<Value>) -> Result<&'a mut Executable> {
        self.apply(value.into(), true)
    }

    /// Whether the variable is declared `mut`, read from its declaration line.
    ///
    /// `Unknown` when the source file cannot be found (see
    /// [`Executable::set_source_root`]); it is then treated as not mutable.
    pub fn mutability(&self) -> Mutability {
        self.declared_mutability().0
    }

    /// Mutability as declared in the source, with an explanation.
    fn declared_mutability(&self) -> (Mutability, String) {
        let local = &self.locals[0];
        let (Some(file), Some(line)) = (&local.decl_file, local.decl_line) else {
            return (
                Mutability::Unknown,
                "no declaration in the debug info".into(),
            );
        };
        match mutability::read_source(file, self.exe.source_root()) {
            Some(text) => {
                let found = mutability::classify(&text, line, &local.name);
                let why = match found {
                    Mutability::Mutable => format!("declared with `let mut` at {file}:{line}"),
                    Mutability::Immutable => {
                        format!("declared with a plain `let` at {file}:{line}")
                    }
                    Mutability::Unknown => format!("declaration at {file}:{line} not recognised"),
                };
                (found, why)
            }
            None => (
                Mutability::Unknown,
                format!("source file `{file}` not found; use `set_source_root`"),
            ),
        }
    }

    /// Explicitly allows later modifications of this variable, as if it were
    /// declared `mut`. The decision is yours: ruwrite only enforces that it
    /// is taken on purpose.
    pub fn make_mut(self) -> Self {
        self.exe
            .force_mut(&self.function.name, &self.locals[0].name);
        self
    }

    /// Inserts `variable = value;` at `address`, a place where the original
    /// code has only padding (`nop` / `int3`).
    ///
    /// Unlike [`set`](Self::set), which rewrites the initialiser, this adds a
    /// **new assignment**, so it is only accepted for a variable declared
    /// `mut` in the source or made mutable with [`make_mut`](Self::make_mut);
    /// otherwise [`Error::NotMutable`] is returned. To free a spot, queue and
    /// commit [`Executable::nop`] on instructions you decide to drop.
    pub fn assign_at(self, address: u64, value: impl Into<Value>) -> Result<&'a mut Executable> {
        let value = value.into();
        let arch = self.exe.arch().clone();
        if !arch.supports_local_patching() {
            return Err(Error::unsupported("assigning local variables", arch));
        }
        let name = self.locals[0].name.clone();
        if !self.exe.is_forced_mut(&self.function.name, &name) {
            let (found, reason) = self.declared_mutability();
            if found != Mutability::Mutable {
                return Err(Error::NotMutable {
                    function: self.function.name.clone(),
                    variable: name,
                    reason,
                });
            }
        }

        let bitness = if arch == Arch::X86 { 32 } else { 64 };
        let local = &self.locals[0];
        let (register, disp) =
            slot_of(local, self.function.frame_base, bitness).ok_or_else(|| {
                Error::LocalPatch(format!("cannot locate the stack slot of `{name}`"))
            })?;
        let disp = i32::try_from(disp)
            .map_err(|_| Error::LocalPatch(format!("stack offset of `{name}` is too large")))?;
        let bytes = value.encode(&local.ty, self.exe.endian())?;
        let code = x86::encode_store(register, disp, &bytes).ok_or_else(|| {
            Error::LocalPatch(format!("cannot encode a store of {value} into `{name}`"))
        })?;

        let inside = self
            .function
            .ranges
            .iter()
            .any(|&(begin, end)| address >= begin && address + code.len() as u64 <= end);
        if !inside {
            return Err(Error::LocalPatch(format!(
                "{address:#x} is not inside `{}`",
                self.function.name
            )));
        }
        let current = self.exe.code_bytes(address, code.len())?;
        if !current.iter().all(|b| matches!(b, 0x90 | 0xCC)) {
            return Err(Error::LocalPatch(format!(
                "the {} bytes at {address:#x} are not padding: free them first with \
                 `Executable::nop` (and commit), then call `assign_at`",
                code.len()
            )));
        }
        let description = format!("{}::{name} = {value}", self.function.name);
        let offset = self.exe.vaddr_to_offset(address).ok_or(Error::NotInFile {
            address,
            reason: String::new(),
        })?;
        self.exe
            .push_patch(offset, Some(address), code, description)?;
        Ok(self.exe)
    }

    fn apply(self, value: Value, all: bool) -> Result<&'a mut Executable> {
        let arch = self.exe.arch().clone();
        if !arch.supports_local_patching() {
            return Err(Error::unsupported("patching local variables", arch));
        }
        let bitness = if arch == Arch::X86 { 32 } else { 64 };
        let endian = self.exe.endian();
        let name = self.locals[0].name.clone();

        let mut chosen = self.find_candidates(bitness)?;
        if chosen.is_empty() {
            return Err(Error::LocalPatch(format!(
                "no instruction stores a constant in `{name}` ({}): the build is probably \
                 optimised, or the value is computed at run time",
                self.function.name
            )));
        }

        if !all && chosen.len() > 1 {
            // Départage par la ligne de déclaration de la variable.
            let by_line: Vec<usize> = (0..chosen.len())
                .filter(|&i| {
                    let line = self
                        .exe
                        .debug()
                        .ok()
                        .and_then(|d| d.line_at(chosen[i].address));
                    line.is_some() && line == self.locals[chosen[i].local].decl_line
                })
                .collect();
            if by_line.len() == 1 {
                chosen = vec![chosen.swap_remove(by_line[0])];
            } else {
                let places: Vec<String> =
                    chosen.iter().map(|c| format!("{:#x}", c.address)).collect();
                return Err(Error::LocalPatch(format!(
                    "`{name}` is assigned a constant at {} places ({}); use `set_all` to change them all",
                    chosen.len(),
                    places.join(", ")
                )));
            }
        }

        // Tout est encodé avant la première mise en file d'attente.
        let mut edits = Vec::with_capacity(chosen.len());
        for c in &chosen {
            let local = &self.locals[c.local];
            let imm = encode_immediate(value, local, &c.store, endian)?;
            let description = format!(
                "{}::{}: {} = {} -> {value}",
                self.function.name, local.name, local.ty.name, c.store.value
            );
            edits.push((c.file_offset, c.address, imm, description));
        }
        for (file_offset, address, imm, description) in edits {
            self.exe
                .push_patch(file_offset, Some(address), imm, description)?;
        }
        Ok(self.exe)
    }

    fn find_candidates(&self, bitness: u32) -> Result<Vec<Candidate>> {
        let mut found = Vec::new();
        for &(begin, end) in &self.function.ranges {
            let offset = self.exe.vaddr_to_offset(begin).ok_or(Error::NotInFile {
                address: begin,
                reason: format!("function `{}`", self.function.name),
            })?;
            let code = self.exe.slice(offset, end - begin)?;
            for store in x86::scan_imm_stores(code, begin, bitness) {
                for (index, local) in self.locals.iter().enumerate() {
                    if matches_slot(&store, local, self.function.frame_base, bitness) {
                        found.push(Candidate {
                            address: begin + store.offset as u64,
                            file_offset: offset + (store.offset + store.imm_offset) as u64,
                            local: index,
                            store: store.clone(),
                        });
                    }
                }
            }
        }
        Ok(found)
    }
}

/// Registre et déplacement de l'emplacement de pile d'une variable.
fn slot_of(local: &DwLocal, base: FrameBase, bitness: u32) -> Option<(iced_x86::Register, i64)> {
    match base {
        FrameBase::Register(number) => {
            Some((x86::frame_register(number, bitness)?, local.frame_offset))
        }
        // CFA = valeur de rbp/ebp + 2 mots, quand un pointeur de frame existe.
        FrameBase::Cfa if bitness == 64 => Some((iced_x86::Register::RBP, local.frame_offset - 16)),
        FrameBase::Cfa => Some((iced_x86::Register::EBP, local.frame_offset - 8)),
        FrameBase::Unknown => None,
    }
}

/// L'instruction écrit-elle exactement dans l'emplacement de cette variable ?
fn matches_slot(store: &ImmStore, local: &DwLocal, base: FrameBase, bitness: u32) -> bool {
    if store.has_index || store.mem_size != local.ty.size {
        return false;
    }
    slot_of(local, base, bitness).is_some_and(|(register, displacement)| {
        store.base == register && store.disp == displacement
    })
}

/// Encode `value` pour le champ immédiat de l'instruction.
///
/// L'immédiat peut être plus étroit que l'écriture (imm32 étendu à 64 bits) :
/// la valeur doit alors survivre à l'extension de signe.
fn encode_immediate(
    value: Value,
    local: &DwLocal,
    store: &ImmStore,
    endian: crate::arch::Endian,
) -> Result<Vec<u8>> {
    let full = value.encode(&local.ty, endian)?;
    if store.imm_size >= full.len() {
        return Ok(full);
    }
    let bits = full
        .iter()
        .rev()
        .fold(0u64, |acc, byte| (acc << 8) | u64::from(*byte));
    let narrow_bits = (store.imm_size * 8) as u32;
    let shift = 64 - narrow_bits;
    let extended = (((bits << shift) as i64) >> shift) as u64;
    if extended != x86::mask(bits, full.len()) {
        return Err(Error::ValueOutOfRange {
            value: value.to_string(),
            target: format!(
                "`{}` ({}): the compiler encodes this store with a {}-byte immediate",
                local.name, local.ty.name, store.imm_size
            ),
        });
    }
    Ok(bits.to_le_bytes()[..store.imm_size].to_vec())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::arch::Endian;
    use crate::value::{Class, ScalarType};
    use iced_x86::Register;

    fn local(ty: &str, class: Class, size: usize) -> DwLocal {
        DwLocal {
            name: "a".into(),
            ty: ScalarType {
                name: ty.into(),
                class,
                size,
            },
            frame_offset: 0x14,
            decl_line: Some(3),
            decl_file: None,
        }
    }

    fn store(mem_size: usize, imm_size: usize) -> ImmStore {
        ImmStore {
            offset: 0,
            imm_offset: 4,
            imm_size,
            mem_size,
            base: Register::RSP,
            has_index: false,
            disp: 0x14,
            value: 3,
        }
    }

    #[test]
    fn immediate_is_replaced_at_slot_width() {
        let bytes = encode_immediate(
            Value::U8(2),
            &local("i32", Class::Signed, 4),
            &store(4, 4),
            Endian::Little,
        )
        .unwrap();
        assert_eq!(bytes, [2, 0, 0, 0]);
    }

    #[test]
    fn qword_store_needs_a_value_that_survives_sign_extension() {
        let l = local("i64", Class::Signed, 8);
        let ok = encode_immediate(Value::I64(-2), &l, &store(8, 4), Endian::Little).unwrap();
        assert_eq!(ok, [0xFE, 0xFF, 0xFF, 0xFF]);
        let too_big = encode_immediate(Value::I64(1 << 40), &l, &store(8, 4), Endian::Little);
        assert!(matches!(too_big, Err(Error::ValueOutOfRange { .. })));
    }

    #[test]
    fn slot_matching_uses_register_displacement_and_width() {
        let l = local("i32", Class::Signed, 4);
        let s = store(4, 4);
        assert!(matches_slot(&s, &l, FrameBase::Register(7), 64));
        assert!(!matches_slot(&s, &l, FrameBase::Register(6), 64));
        assert!(!matches_slot(&store(8, 4), &l, FrameBase::Register(7), 64));
        let mut other = store(4, 4);
        other.disp = 0x18;
        assert!(!matches_slot(&other, &l, FrameBase::Register(7), 64));
    }
}
