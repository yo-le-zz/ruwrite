//! Architecture, byte order and container format of an executable.

use std::fmt;

use object::{Architecture, BinaryFormat};

/// Byte order of the target.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Endian {
    /// Least significant byte first.
    Little,
    /// Most significant byte first.
    Big,
}

/// Container format of the executable.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Format {
    /// Linux, BSD and most Unix systems.
    Elf,
    /// Windows.
    Pe,
    /// macOS and iOS.
    MachO,
    /// Any other format recognised by the parser.
    Other(String),
}

impl fmt::Display for Format {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Format::Elf => f.write_str("ELF"),
            Format::Pe => f.write_str("PE"),
            Format::MachO => f.write_str("Mach-O"),
            Format::Other(name) => f.write_str(name),
        }
    }
}

impl From<BinaryFormat> for Format {
    fn from(format: BinaryFormat) -> Self {
        match format {
            BinaryFormat::Elf => Format::Elf,
            BinaryFormat::Pe => Format::Pe,
            BinaryFormat::MachO => Format::MachO,
            other => Format::Other(format!("{other:?}")),
        }
    }
}

/// CPU architecture of the executable.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Arch {
    /// 32-bit x86.
    X86,
    /// 64-bit x86 (AMD64).
    X86_64,
    /// 64-bit ARM.
    Aarch64,
    /// 32-bit ARM.
    Arm,
    /// 32-bit RISC-V.
    Riscv32,
    /// 64-bit RISC-V.
    Riscv64,
    /// 32-bit MIPS.
    Mips,
    /// 64-bit MIPS.
    Mips64,
    /// 32-bit PowerPC.
    PowerPc,
    /// 64-bit PowerPC.
    PowerPc64,
    /// IBM z/Architecture.
    S390x,
    /// 64-bit LoongArch.
    LoongArch64,
    /// Any other architecture, by its name.
    Other(String),
}

impl Arch {
    /// Returns `true` when machine-code helpers (`returns`, `disable`,
    /// `redirect_to`, ...) can generate code for this architecture.
    ///
    /// Byte-level patching and static/variable edits work on every
    /// architecture; only code generation is limited.
    pub fn supports_code_generation(&self) -> bool {
        matches!(self, Arch::X86 | Arch::X86_64 | Arch::Aarch64)
    }

    /// Returns `true` when the initial value of local variables can be rewritten.
    pub fn supports_local_patching(&self) -> bool {
        matches!(self, Arch::X86 | Arch::X86_64)
    }
}

impl fmt::Display for Arch {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let name = match self {
            Arch::X86 => "x86",
            Arch::X86_64 => "x86_64",
            Arch::Aarch64 => "aarch64",
            Arch::Arm => "arm",
            Arch::Riscv32 => "riscv32",
            Arch::Riscv64 => "riscv64",
            Arch::Mips => "mips",
            Arch::Mips64 => "mips64",
            Arch::PowerPc => "powerpc",
            Arch::PowerPc64 => "powerpc64",
            Arch::S390x => "s390x",
            Arch::LoongArch64 => "loongarch64",
            Arch::Other(name) => name,
        };
        f.write_str(name)
    }
}

impl From<Architecture> for Arch {
    fn from(arch: Architecture) -> Self {
        match arch {
            Architecture::I386 => Arch::X86,
            Architecture::X86_64 | Architecture::X86_64_X32 => Arch::X86_64,
            Architecture::Aarch64 | Architecture::Aarch64_Ilp32 => Arch::Aarch64,
            Architecture::Arm => Arch::Arm,
            Architecture::Riscv32 => Arch::Riscv32,
            Architecture::Riscv64 => Arch::Riscv64,
            Architecture::Mips => Arch::Mips,
            Architecture::Mips64 => Arch::Mips64,
            Architecture::PowerPc => Arch::PowerPc,
            Architecture::PowerPc64 => Arch::PowerPc64,
            Architecture::S390x => Arch::S390x,
            Architecture::LoongArch64 => Arch::LoongArch64,
            other => Arch::Other(format!("{other:?}").to_lowercase()),
        }
    }
}

/// General facts about an executable.
#[derive(Debug, Clone)]
pub struct Info {
    /// Container format.
    pub format: Format,
    /// CPU architecture.
    pub arch: Arch,
    /// Pointer width in bits (32 or 64).
    pub bits: u8,
    /// Byte order.
    pub endian: Endian,
    /// Virtual address of the entry point.
    pub entry: u64,
    /// `true` when the file looks like it was produced by `rustc`.
    pub is_rust: bool,
    /// `true` when the file embeds DWARF debug information.
    pub has_debug_info: bool,
}
