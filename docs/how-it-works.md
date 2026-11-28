# How ruwrite works

## Pipeline

```
Executable::open ──► parse (object)  ── sections, symbols, arch, endianness
                 └─► DWARF (gimli)   ── statics, locals, types, line table (lazy)

edits (set_static, local().set, function().returns ...)
        │  each edit = Patch { file_offset, old bytes, new bytes }
        ▼
   pending queue ── overlap check, size check
        │
        ├─ commit()      all patches verified, then temp file + atomic rename
        ├─ commit_to(p)  same, to another path
        └─ commit_hot(pid)  /proc/<pid>/mem, each patch checked against live memory
```

Nothing touches the target until a `commit*` call. Every `Patch` records the
bytes it expects to replace, so applying a patch prepared for another build
fails with `BytesMismatch` instead of corrupting a binary.

## Modules

| Module | Role |
|--------|------|
| `exe.rs` | `Executable`: loading, inspection, queue, `commit*`, `find_value`/`peek_*`/`poke_*`, integrity snapshots |
| `edit.rs` | `FunctionEditor` and `LocalEditor` (including cascade mode) |
| `dwarf.rs` | DWARF reader: functions, statics, locals, type sizes, line table, fully-inlined functions |
| `code.rs` | machine-code generation (x86, x86_64, aarch64) |
| `x86.rs` | finds/encodes `mov [mem], imm` stores and `cmp`-driven conditional jumps in x86 code |
| `protect.rs` | reads the protection records (v2 format), name and hash matching |
| `mutability.rs` | `let` / `let mut` detection from the source |
| `patch.rs` | `Patch` and the all-or-nothing `PatchSet` |
| `live.rs` | hot patching through `/proc/<pid>/mem`, ELF load-segment bias |
| `value.rs` | `Value`: scalar construction, encoding/decoding and range checks |
| `arch.rs` | architecture, format, byte order |
| `macros/` (`ruwrite-macros` crate) | implementation of `#[ruwrite::protect(..)]`, re-exported by `ruwrite` |

## How each edit works

**Statics** (`set_static`). DWARF gives the address and type of the variable;
the address is mapped to a file offset through the section table and the new
value is encoded with the variable's width and signedness.

**Locals** (`local(f, v).set(x)`). A local has no address. In an unoptimised
build, `let a: i32 = 3;` is compiled to `mov dword ptr [rsp+0x10], 3`. ruwrite
computes the stack slot from DWARF (`DW_OP_fbreg` + frame base), scans the
function's code for a store of an immediate into exactly that slot with the
same width, and rewrites the immediate. If several stores match, the line
table and the declaration line pick the right one.

**Mutability** (`mutability`, `make_mut`, `assign_at`). `DW_AT_decl_file` and
`DW_AT_decl_line` locate the `let` of a variable; `mutability.rs` classifies the
pattern (`let mut a`, `let (mut x, y)`). `assign_at` refuses non-`mut`
variables unless `make_mut` was called, then writes a `mov [slot], imm`
encoded like the compiler's (disp8 when it fits) into existing padding.

**Functions** (`returns`, `disable`, `redirect_to`, `replace_code`). The new
code is written at the start of the function; it must fit in the symbol's
size. `returns` uses the shortest encoding (`mov al, 1; ret` for a `bool`).

**Fully inlined functions.** A `DW_TAG_subprogram` with `DW_AT_inline` set but
no address range is the compiler's record of a function it inlined at every
call site (no standalone copy exists). `dwarf.rs` collects these names
separately (`DebugInfo::inlined_only`); `find_function` reports `Error::Inlined`
for one of them instead of a plain "not found".

**Without debug info** (`find_value`, `peek_*`/`poke_*`). No DWARF, no
address: `find_value` scans the data-like sections only (`.data`, `.rodata`,
`.data.rel.ro` — never `.text` or relocation/unwind metadata, which earlier
versions scanned and which made a coincidental byte match corrupt the
binary) for the exact byte pattern of the given value, and reports every
address it occurs at, cross-referenced against the symbol table and any DWARF
statics that happen to be present. `peek_*`/`poke_*` then read or queue a
write at a chosen address directly; `poke_*` queues like every other edit and
is only written at `commit()`.

**Cascade mode** (`LocalEditor::cascade`). Only engages when the value given
to `set`/`set_all` has a different signedness (`Class::Signed` vs
`Class::Unsigned`) than the variable's declared type. `x86.rs` then scans the
whole function for a `cmp` directly against the same stack slot (same base
register and displacement `set` itself resolves the store from) immediately
followed by a conditional jump whose condition reads that comparison as
signed (`jl`/`jge`/`jle`/`jg`) or unsigned (`jb`/`jae`/`jbe`/`ja`). Every one
that does not already match the new signedness gets a single opcode byte
flipped to its counterpart (`flip_condition_byte`): the short `0x7_` form and
the near `0F 8_` form both keep their length, so this is a same-size,
same-form byte edit like any other, queued and verified the same way.
Comparisons made through a register loaded from the slot (not the slot
itself) are not tracked back to it and are left untouched.

**Hot patch** (`commit_hot`). The ELF's `PT_LOAD` segments are read from the
file (`object` crate); the load bias (ASLR offset) is computed from the one
segment with the smallest file offset, matched by *exact* equality against
`/proc/<pid>/maps`'s own `offset` field — not range containment, which can
ambiguously match more than one mapping when segments are page-aligned and
puts the patch at the wrong address. The current bytes are compared with the
expected ones, then written through `/proc/<pid>/mem`, which ignores page
protections for a process with ptrace rights.

## Adding an architecture

Byte-level edits already work everywhere. To support code generation on a new
architecture, add its encodings to `code.rs` (`ret`, `nop_run`, `return_value`,
`jump`) with tests, and declare it in `Arch::supports_code_generation`.

## Protection

`#[ruwrite::protect(editable | immutable | protected | hide)]` and
`ruwrite::protect_all!()` (in the `ruwrite-macros` proc-macro crate,
re-exported by `ruwrite` so nothing else needs to be added as a dependency)
place a small `RUWPROT2` record — magic, target kind, mode, name encoding,
name length, name or a `fnv1a64` hash of it — in the `.ruwprot` section
(`__ruwprot` on Mach-O). `protect.rs` reads them at open; `exe.rs` resolves
each rule to byte ranges (function symbols, data symbols, or a hash matched
against the last segment of a qualified name for `hide`) and stores them as
guards, now carrying a `ProtectionMode` instead of a single immutable/hidden
flag. `push_patch` is the single entry point of every edit, so the check
there covers all APIs; it is repeated before each commit. With any rule
present, bytes outside every section are refused as well. If rules exist but
the symbol table is empty, one guard covers the whole file. `hide`-mode items
are additionally filtered out of `Executable::functions()`/`variables()`, and
`protected`-mode items are the ones `integrity_snapshot()`/`verify_integrity()`
hash (`fnv1a64_bytes`) for later tamper detection.
