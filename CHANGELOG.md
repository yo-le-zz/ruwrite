# Changelog

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
