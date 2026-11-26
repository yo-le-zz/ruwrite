# Changelog

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
