# Changelog

## v0.4.0

### Added
- **Protection is now built into `ruwrite` itself**, through the new internal
  `ruwrite-macros` crate: `#[ruwrite::protect(editable | immutable | protected
  | hide)]` on a function or `static`, and `ruwrite::protect_all!()` for the
  whole executable. The separate `ruwrite-guard` crate is gone.
  - `protected` additionally marks the item for `Executable::integrity_snapshot()`
    / `Executable::verify_integrity()`, a hash-based tamper check the program
    can run against itself.
  - `hide` keeps the item's plain name out of the binary (a hash is stored
    instead) and out of `Executable::functions()` / `variables()`; it is
    honestly documented as not a defence against disassembly.
- **Works without debug info**: `Executable::find_value(value)` recognises a
  variable by its current content in a release build (scanning `.data`,
  `.rodata`, `.data.rel.ro` only, to avoid corrupting unrelated bytes), and
  the new `peek_bool`/`poke_bool` .. `peek_f64`/`poke_f64` pairs read or queue
  a write at a known address without needing a name at all.
- **`Value` named constructors** (`Value::u8(2)`, `Value::i32(-5)`, ...)
  replace Rust literal suffixes (`2u8`) for readability; `Value::size()` and
  `Value::own_type()` describe a value's own width and category.
- **Cascade mode**: `exe.local("f", "x")?.cascade().set(..)` additionally
  flips `<`/`<=`/`>`/`>=` comparisons made directly against the variable's
  stack slot when the new value's signedness differs from the declared type,
  so the rest of the function keeps reading it correctly.
- **Friendlier error for fully inlined functions**: looking up a function the
  compiler folded into every caller (never marked `#[inline(never)]`) now
  reports `Error::Inlined` naming the problem, instead of a plain "not found".

### Fixed
- **Hot-patch addressing**: the load bias (ASLR offset) is now computed from
  an unambiguous anchor (the `PT_LOAD` segment with the smallest file offset,
  matched by exact equality against `/proc/<pid>/maps`) instead of a
  range-containment check that could match more than one mapping and apply a
  patch 0x1000 bytes off its intended address.
- Windows CI flakiness (`PermissionDenied` on freshly written sample
  executables, caused by antivirus scanning): retried with backoff, and the
  runner's temp directory is excluded from Windows Defender.

## v0.3.0

### Added
- **Protection system.** A program declares sensitive parts with the new
  `ruwrite-guard` crate (`protect_function!`, `protect_static!`,
  `protect_all!`). ruwrite reads the declaration from the executable and
  refuses every edit touching them (`Error::Protected`: "defined as sensitive
  and protected"), whatever the API used: function editors, statics, raw
  patches, `nop`, pattern replacement, `commit`, `commit_to`, `commit_hot`.
  The records and the file structure are protected too, so they cannot be
  removed with a raw patch. A stripped executable that declares protections is
  locked completely, since the names cannot be located.
- `Executable::protections()` and `Executable::is_protected()`.
- Compiler-style checks on functions: `returns` checks the value against the
  declared return type (and converts it), refuses to make a `()` function
  return a value; `disable` refuses functions that return a value;
  `redirect_to` refuses targets with a different signature
  (`Error::Rejected`).
- Example `protection`, demo programs `locked_demo` and a protected `target_demo`.

## v0.2.0

### Added
- Mutability guard: ruwrite reads the declaration line of a local variable
  (`let` / `let mut`) from the program's source and refuses to add a new
  assignment to a variable that is not `mut` (`Error::NotMutable`).
- `LocalEditor::mutability()`, `LocalEditor::make_mut()` (explicit opt-in to
  modify a non-`mut` variable) and `Executable::set_source_root()`.
- `LocalEditor::assign_at(address, value)`: inserts `variable = value;` in
  padding left by `Executable::nop`, with the same encoding size as the compiler's.
- `Mutability` public type.

## v0.1.0

### Added
- `Executable`: open a Rust executable (ELF, PE, Mach-O parsing; x86, x86_64,
  aarch64 and other architectures for byte-level edits).
- Inspection: general info, sections, functions (demangled), statics and
  local variables with their types (DWARF).
- Edits: `set_static`, `set_static_bytes`, `local(..).set(..)`,
  `function(..).returns/disable/redirect_to/replace_code`, `patch_bytes`,
  `nop`, `replace_bytes`, `disassemble` (x86).
- Transactional validation: `commit` (atomic, `.orig` backup), `commit_to`,
  `commit_hot` (running process, Linux), `discard`.
- Safety checks: expected bytes verified before writing, overlapping edits
  refused, values range-checked against the variable type.
- `playground/` crate with target programs and end-to-end tests; examples in
  `examples/`; GitHub Actions for tests and releases.
