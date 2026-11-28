//! # ruwrite
//!
//! Inspect and rewrite **compiled Rust executables** — even when the program
//! was never designed for it.
//!
//! ```no_run
//! use ruwrite::Executable;
//!
//! fn main() -> ruwrite::Result<()> {
//!     // `exe` represents the target executable.
//!     let mut exe = Executable::open("target/debug/demo")?;
//!
//!     // Inspect: architecture, functions, variables...
//!     println!("{} / {}", exe.info().arch, exe.info().format);
//!     for f in exe.functions().iter().take(5) {
//!         println!("{:#x} {}", f.address, f.name);
//!     }
//!
//!     // Write: nothing is modified yet, edits are only queued.
//!     exe.set_static("LIMIT", 100)?;              // a `static`
//!     exe.local("compute", "a")?.set(2u8)?;       // `let a: i32 = 3;` becomes 2
//!     exe.function("is_admin")?.returns(true)?;   // always `true`
//!
//!     // Validate: all edits applied together (or none).
//!     exe.commit()?;
//!     Ok(())
//! }
//! ```
//!
//! ## Concepts
//!
//! * [`Executable`] — the entry point. `open`, inspect, queue edits, `commit`.
//! * [`FunctionEditor`] — returned by [`Executable::function`]: `returns`,
//!   `disable`, `redirect_to`, `replace_code`, `disassemble`.
//! * [`LocalEditor`] — returned by [`Executable::local`]: rewrite the initial
//!   value of a local variable.
//! * [`Value`] — a scalar (`bool`, integers, floats) used for every write.
//!
//! Three ways to validate the queued edits:
//!
//! | Method | Effect |
//! |--------|--------|
//! | [`Executable::commit`] | rewrites the file (atomic, `.orig` backup) |
//! | [`Executable::commit_to`] | writes a patched copy |
//! | [`Executable::commit_hot`] | patches the memory of a running process |
//!
//! ## Requirements on the target
//!
//! The target must be a Rust executable. Reading variables and rewriting
//! locals needs DWARF information (`debug = true` in the Cargo profile);
//! functions and raw bytes only need the symbol table. See the README for the
//! full list of limitations.

#![forbid(unsafe_code)]
#![warn(missing_docs)]

mod arch;
mod code;
mod dwarf;
mod edit;
mod error;
mod exe;
mod live;
mod mutability;
mod patch;
mod protect;
mod value;
mod x86;

pub use arch::{Arch, Endian, Format, Info};
pub use edit::{FunctionEditor, LocalEditor};
pub use error::{Error, Result};
pub use exe::{
    CommitReport, Executable, Function, IntegrityIssue, IntegritySnapshot, IntegrityViolation,
    Section, ValueMatch, Variable, VariableKind,
};
pub use mutability::Mutability;
pub use patch::Patch;
pub use protect::{Protection, ProtectionMode, ProtectionTarget};
pub use ruwrite_macros::protect;
pub use value::{Class, ScalarType, Value};

/// Declares the **whole executable** as protected: `ruwrite` refuses every
/// edit, whatever it targets. For one function or `static`, use
/// [`macro@protect`] on it instead.
///
/// ```
/// ruwrite::protect_all!();
/// ```
#[macro_export]
macro_rules! protect_all {
    () => {
        #[doc(hidden)]
        const _: () = {
            #[used]
            #[cfg_attr(
                any(target_os = "macos", target_os = "ios"),
                link_section = "__DATA,__ruwprot"
            )]
            #[cfg_attr(
                not(any(target_os = "macos", target_os = "ios")),
                link_section = ".ruwprot"
            )]
            static RUWRITE_RECORD: [u8; 13] = *b"RUWPROT2\x03\x01\x00\x00\x00";
        };
    };
}
