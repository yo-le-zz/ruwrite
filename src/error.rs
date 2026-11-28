//! Error type shared by the whole crate.

use std::io;
use std::path::PathBuf;

/// Convenient alias used by every fallible function of the crate.
pub type Result<T, E = Error> = std::result::Result<T, E>;

/// Everything that can go wrong while inspecting or rewriting an executable.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
    /// A file-system operation failed.
    #[error("I/O error on `{}`: {source}", path.display())]
    Io {
        /// File involved in the failed operation.
        path: PathBuf,
        /// Underlying error.
        #[source]
        source: io::Error,
    },

    /// The file is not a valid executable image.
    #[error("cannot parse the executable: {0}")]
    Parse(String),

    /// The executable does not look like it was produced by `rustc`.
    #[error(
        "`{}` does not look like a Rust executable (use `Executable::open_any` to skip this check)",
        .0.display()
    )]
    NotRust(PathBuf),

    /// The DWARF debug information is missing or unreadable.
    #[error("no usable debug information: {0}")]
    NoDebugInfo(String),

    /// A function, variable or pattern could not be found.
    #[error("{kind} `{name}` not found")]
    NotFound {
        /// What was searched (`"function"`, `"static"`, ...).
        kind: &'static str,
        /// Name that was searched.
        name: String,
    },

    /// Several items match the requested name.
    #[error("{kind} `{name}` is ambiguous, candidates: {}", candidates.join(", "))]
    Ambiguous {
        /// What was searched.
        kind: &'static str,
        /// Name that was searched.
        name: String,
        /// Fully qualified names of the matches.
        candidates: Vec<String>,
    },

    /// The feature is not implemented for this architecture or format.
    #[error("{what} is not supported on {arch}")]
    Unsupported {
        /// Description of the unsupported operation.
        what: String,
        /// Architecture (or platform) concerned.
        arch: String,
    },

    /// A virtual address does not map to bytes stored in the file
    /// (for instance `.bss`, which only exists at run time).
    #[error("address {address:#x} is not backed by file contents{}", reason_suffix(.reason))]
    NotInFile {
        /// Virtual address requested.
        address: u64,
        /// Extra explanation, if any.
        reason: String,
    },

    /// A byte range is outside the image.
    #[error(
        "range at {offset:#x} (+{length:#x} bytes) is outside the image ({image_len:#x} bytes)"
    )]
    OutOfBounds {
        /// Start offset of the range.
        offset: u64,
        /// Length of the range.
        length: u64,
        /// Size of the image.
        image_len: u64,
    },

    /// The value has the wrong kind for the destination (e.g. a float for an integer).
    #[error("type mismatch: `{target}` expects {expected}, got `{got}`")]
    TypeMismatch {
        /// Destination (variable) type name.
        target: String,
        /// Kind of value expected.
        expected: &'static str,
        /// Type of the provided value.
        got: &'static str,
    },

    /// The value does not fit in the destination.
    #[error("value {value} does not fit in `{target}`")]
    ValueOutOfRange {
        /// Offending value, as text.
        value: String,
        /// Destination type description.
        target: String,
    },

    /// A raw byte replacement has the wrong length.
    #[error("size mismatch: expected {expected} bytes, got {got}")]
    SizeMismatch {
        /// Expected number of bytes.
        expected: usize,
        /// Provided number of bytes.
        got: usize,
    },

    /// The new code does not fit in the function.
    #[error("function `{name}` is too small: {needed} bytes needed, {available} available")]
    FunctionTooSmall {
        /// Function name.
        name: String,
        /// Bytes required by the patch.
        needed: usize,
        /// Bytes available in the function.
        available: usize,
    },

    /// A relative jump cannot reach its target.
    #[error("jump from {from:#x} to {to:#x} is out of range for this architecture")]
    JumpOutOfRange {
        /// Source address.
        from: u64,
        /// Target address.
        to: u64,
    },

    /// Two pending edits touch the same bytes.
    #[error("pending edit overlaps an earlier one at file offset {offset:#x} ({existing})")]
    Overlap {
        /// File offset of the conflict.
        offset: u64,
        /// Description of the earlier edit.
        existing: String,
    },

    /// The bytes found in the target are not the ones the edit was prepared for.
    #[error("unexpected bytes at {location}: expected {expected}, found {found}")]
    BytesMismatch {
        /// Where the mismatch happened.
        location: String,
        /// Expected bytes (hex).
        expected: String,
        /// Actual bytes (hex).
        found: String,
    },

    /// Several candidate instructions were found for a local variable.
    #[error("{0}")]
    LocalPatch(String),

    /// The edit would modify a variable that is not declared `mut`.
    #[error(
        "`{variable}` in `{function}` is not declared `mut` ({reason}): \
         changing it after its initialisation would not be valid Rust. \
         Call `make_mut()` on the editor to allow it explicitly"
    )]
    NotMutable {
        /// Function owning the variable.
        function: String,
        /// Variable name.
        variable: String,
        /// How the status was determined.
        reason: String,
    },

    /// The part of the program is declared sensitive and protected.
    #[error(
        "{what} is defined as sensitive and protected by the program \
         (rule: {rule}): ruwrite refuses to modify it"
    )]
    Protected {
        /// What the edit tried to touch.
        what: String,
        /// The protection rule declared by the program.
        rule: String,
    },

    /// The edit would produce code that the Rust compiler would not accept.
    #[error("rejected, the compiler would not accept this: {0}")]
    Rejected(String),

    /// Hot patching a running process failed.
    #[error("hot patch failed: {0}")]
    Live(String),

    /// The function was not compiled as a standalone piece of code: every
    /// call site got its own inlined copy instead.
    #[error(
        "function `{name}` has no standalone machine code: the compiler inlined every call to it \
         instead of keeping one copy. Mark it `#[inline(never)]` and rebuild so ruwrite has a \
         single place to rewrite"
    )]
    Inlined {
        /// Function name.
        name: String,
    },
}

fn reason_suffix(reason: &str) -> String {
    if reason.is_empty() {
        String::new()
    } else {
        format!(" ({reason})")
    }
}

impl Error {
    pub(crate) fn io(path: impl Into<PathBuf>, source: io::Error) -> Self {
        Error::Io {
            path: path.into(),
            source,
        }
    }

    pub(crate) fn not_found(kind: &'static str, name: impl Into<String>) -> Self {
        Error::NotFound {
            kind,
            name: name.into(),
        }
    }

    pub(crate) fn unsupported(what: impl Into<String>, arch: impl std::fmt::Display) -> Self {
        Error::Unsupported {
            what: what.into(),
            arch: arch.to_string(),
        }
    }
}

/// Formats bytes as lowercase hex separated by spaces (used in error messages).
pub(crate) fn hex(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 3);
    for (i, b) in bytes.iter().enumerate() {
        if i > 0 {
            out.push(' ');
        }
        out.push_str(&format!("{b:02x}"));
    }
    out
}
