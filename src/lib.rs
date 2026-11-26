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
mod patch;
mod value;
mod x86;

pub use arch::{Arch, Endian, Format, Info};
pub use edit::{FunctionEditor, LocalEditor};
pub use error::{Error, Result};
pub use exe::{CommitReport, Executable, Function, Section, Variable, VariableKind};
pub use patch::Patch;
pub use value::{Class, ScalarType, Value};
