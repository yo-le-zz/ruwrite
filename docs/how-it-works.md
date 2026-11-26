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
| `exe.rs` | `Executable`: loading, inspection, queue, `commit*` |
| `edit.rs` | `FunctionEditor` and `LocalEditor` |
| `dwarf.rs` | DWARF reader: functions, statics, locals, type sizes, line table |
| `code.rs` | machine-code generation (x86, x86_64, aarch64) |
| `x86.rs` | finds `mov [mem], imm` stores in x86 code |
| `patch.rs` | `Patch` and the all-or-nothing `PatchSet` |
| `live.rs` | hot patching through `/proc/<pid>/mem` |
| `value.rs` | `Value`: scalar encoding/decoding and range checks |
| `arch.rs` | architecture, format, byte order |

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

**Functions** (`returns`, `disable`, `redirect_to`, `replace_code`). The new
code is written at the start of the function; it must fit in the symbol's
size. `returns` uses the shortest encoding (`mov al, 1; ret` for a `bool`).

**Hot patch** (`commit_hot`). `/proc/<pid>/maps` is used to translate file
offsets to process addresses; the current bytes are compared with the expected
ones, then written through `/proc/<pid>/mem`, which ignores page protections
for a process with ptrace rights.

## Adding an architecture

Byte-level edits already work everywhere. To support code generation on a new
architecture, add its encodings to `code.rs` (`ret`, `nop_run`, `return_value`,
`jump`) with tests, and declare it in `Arch::supports_code_generation`.
