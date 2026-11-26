# ruwrite

**Inspect and rewrite compiled Rust executables — even when the program never planned for it.**

`ruwrite` (*rust + rewrite*) is a Rust library. A program hands it another Rust
executable; it can list its functions and variables, then rewrite pieces of the
binary: change a constant, force a function result, disable a function,
redirect it to another one. Edits are queued, then validated with `commit()` —
on the file, on a copy, or **on a process that is running right now**.

```rust
use ruwrite::Executable;

fn main() -> ruwrite::Result<()> {
    let mut exe = Executable::open("target/debug/demo")?;

    exe.set_static("LIMIT", 100)?;              // static LIMIT: i32 = 10
    exe.local("compute", "a")?.set(2u8)?;       // let a: i32 = 3;  ->  2
    exe.function("is_admin")?.returns(true)?;   // always true
    exe.function("log")?.disable()?;            // does nothing any more

    exe.commit()?;                              // validate: the binary is rewritten
    Ok(())
}
```

## Features

- **Inspect**: format, architecture, sections, functions (demangled), statics and
  local variables with their types, x86 disassembly.
- **Rewrite** without recompiling: statics, locals' initial values, function
  bodies, raw bytes, string literals.
- **Transactional**: edits are queued; `commit()` applies all of them or none.
  Every edit remembers the bytes it replaces and refuses to apply on a binary
  that does not match.
- **Safe by default**: atomic file replacement, `<file>.orig` backup, values
  range-checked against the variable type, overlapping edits refused.
- **Hot patching** of a running process (Linux): `exe.commit_hot(pid)`.
- **Multi-architecture**: x86, x86_64 and aarch64 get full code generation; any
  other architecture (RISC-V, MIPS, PowerPC, ...) supports byte-level patching.
- **Target programs don't need to cooperate**: no hook, no special build flag
  other than debug info for variable access.

## Installation

```toml
[dependencies]
ruwrite = { git = "https://github.com/yo-le-zz/ruwrite" }
```

Requires Rust 1.88 or newer.

## Usage

### 1. Open the target

```rust
let mut exe = ruwrite::Executable::open("path/to/program")?;
println!("{} {}", exe.info().format, exe.info().arch);
```

`open` only accepts executables that look like they were built by `rustc`;
`open_any` skips that check.

### 2. Inspect

```rust
for f in exe.functions() {
    println!("{:#x} {} ({} bytes)", f.address, f.name, f.size);
}
for v in exe.variables()? {          // needs DWARF debug info
    println!("{} : {}", v.name, v.ty);
}
let limit = exe.static_value("LIMIT")?;   // Value::I32(10)
```

Names can be given in full (`demo::compute`) or by their last segments
(`compute`); an ambiguous name returns an error listing the candidates.

### 3. Queue edits

| Call | Effect |
|------|--------|
| `exe.set_static("NAME", 5)` | new value of a scalar `static` (width and signedness taken from the variable) |
| `exe.set_static_bytes("NAME", b"...")` | new contents of a `static` of any type (arrays, structs) |
| `exe.local("fn", "var")?.set(2u8)` | initial value of a local variable |
| `exe.function("f")?.returns(true)` | the function immediately returns this value |
| `exe.function("f")?.disable()` | the function immediately returns (`()`) |
| `exe.function("f")?.redirect_to("g")` | calls to `f` run `g` |
| `exe.function("f")?.replace_code(&bytes)` | your own machine code |
| `exe.patch_bytes(addr, &bytes)` / `nop(addr, n)` | raw bytes at a virtual address |
| `exe.replace_bytes(b"old", b"new")` | same-length replacement of every occurrence (string literals) |

`exe.pending()` lists the queued edits, `exe.discard()` drops them.

### 4. Validate

| Call | Effect |
|------|--------|
| `exe.commit()` | rewrites the file (atomic, `.orig` backup, permissions kept) |
| `exe.commit_to("copy")` | writes a patched copy, original untouched |
| `exe.commit_hot(pid)` | patches the memory of a running process |

Hot and file commits combine: `commit_hot(pid)` then `commit()` makes the
change permanent as well.

## Try it

The `playground/` crate contains programs built to be rewritten
(`target_demo`, `hot_demo`) and the end-to-end tests that rewrite and run them.

```bash
cargo build --workspace
cargo run --example inspect   -- target/debug/target_demo target_demo
cargo run --example disasm    -- target/debug/target_demo compute
cargo run --example patch_demo                        # before / after output

# hot patching
target/debug/hot_demo &                               # prints value=1
cargo run --example hot_patch -- target/debug/hot_demo $!   # now prints value=42
```

## Limitations (read this)

ruwrite rewrites **machine code and data in place**. That is powerful, and it
has honest limits:

- **Same size only.** A rewrite never moves or grows code or data. Changing
  `a: i32 = 3` to `a: u8 = 2` therefore stores `2` in the *existing 4-byte slot*:
  the value changes, the width does not (the compiled code still treats it as an
  `i32`). Values that do not fit the slot are refused.
- **Locals need an unoptimised build** (`opt-level = 0`) and x86/x86_64: ruwrite
  rewrites the constant the compiler stored in the variable's stack slot. In an
  optimised build the value usually lives in a register or is folded into other
  instructions, and ruwrite reports that no store was found instead of guessing.
  Only the *initial* value can change, not a later assignment.
- **Variables need DWARF** (`debug = true`). Functions need the symbol table
  (not stripped). Raw byte patching needs neither.
- **Zero-initialised statics** (`.bss`) have no bytes in the file and cannot be
  rewritten; they are reported with an explicit error. A plain `static X: i32 = 0`
  falls in this category.
- **Function rewrites must fit** in the symbol's size (`returns(true)` needs 3
  bytes, an `f64` constant 16). `returns` on a 32-bit x86 float is unsupported.
- **Hot patching** is Linux only, needs ptrace rights on the target (same user,
  and `kernel.yama.ptrace_scope` permitting), and rewrites memory while the
  program runs: patch functions that are not currently executing. Programs that
  already copied a value elsewhere keep the old one.
- **Code signing / integrity checks.** A modified binary no longer matches its
  signature or checksum.
- Code generation (`returns`, `disable`, `redirect_to`) covers x86, x86_64 and
  aarch64. Elsewhere use `replace_code` with your own bytes.

Use it on programs you own or are allowed to modify.

## Development

```bash
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace         # unit, multi-arch and end-to-end tests
```

- `tests/multiarch.rs` builds synthetic ELF executables for x86_64, aarch64 and
  RISC-V and rewrites them, so every architecture is checked without a cross
  toolchain.
- `playground/tests/e2e.rs` rewrites real compiled programs and runs them.
- Architecture notes: [`docs/how-it-works.md`](docs/how-it-works.md).

## License

MIT — see [LICENSE](LICENSE).

## Author

yolezz
