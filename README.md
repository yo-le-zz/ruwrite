# ruwrite

**Inspect and rewrite compiled Rust executables — even when the program never planned for it.**

`ruwrite` (*rust + rewrite*) is a Rust library, [published on crates.io](https://crates.io/crates/ruwrite).
A program hands it another Rust executable; it can list its functions and
variables, then rewrite pieces of the binary: change a constant, force a
function result, disable a function, redirect it to another one. Edits are
queued, then validated with `commit()` — on the file, on a copy, or **on a
process that is running right now**.

```rust
use ruwrite::{Executable, Value};

fn main() -> ruwrite::Result<()> {
    let mut exe = Executable::open("target/debug/demo")?;

    exe.set_static("LIMIT", Value::i32(100))?;   // static LIMIT: i32 = 10
    exe.local("compute", "a")?.set(Value::u8(2))?; // let a: i32 = 3;  ->  2
    exe.function("is_admin")?.returns(true)?;    // always true
    exe.function("log")?.disable()?;             // does nothing any more

    exe.commit()?;                               // validate: the binary is rewritten
    Ok(())
}
```

## Features

- **Inspect**: format, architecture, sections, functions (demangled), statics and
  local variables with their types, x86 disassembly.
- **Rewrite** without recompiling: statics, locals' initial values, function
  bodies, raw bytes, string literals.
- **Works without debug info**, too: [`find_value`](#works-without-debug-info-release-builds)
  recognises a variable by its current content (and/or its type) and gives
  back an address you can `peek_*` / `poke_*` directly — the way to inspect
  and patch a release build that has no DWARF at all.
- **Cascade mode**: retyping a variable's signedness can automatically flip
  the `<` / `<=` / `>` / `>=` comparisons made against it, so the rest of the
  function keeps reading the new value correctly. See
  [below](#cascade-mode-keeping-the-rest-of-the-function-consistent).
- **Transactional**: edits are queued; `commit()` applies all of them or none.
  Every edit remembers the bytes it replaces and refuses to apply on a binary
  that does not match.
- **Safe by default**: atomic file replacement, `<file>.orig` backup, values
  range-checked against the variable type, overlapping edits refused,
  `mut`-correctness enforced (see below).
- **Hot patching** of a running process (Linux): `exe.commit_hot(pid)`.
- **Multi-architecture**: x86, x86_64 and aarch64 get full code generation; any
  other architecture (RISC-V, MIPS, PowerPC, ...) supports byte-level patching.
- **Protection built in**: `#[ruwrite::protect(..)]` lets a program declare
  parts of itself nobody may rewrite — see
  [Protecting your program](#protecting-your-program).
- **Target programs don't need to cooperate**: no hook, no special build flag
  other than debug info for variable access (and not even that for
  `find_value`/raw bytes).

## Installation

```toml
[dependencies]
ruwrite = "0.4"
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
(`compute`); an ambiguous name returns an error listing the candidates. A
function the compiler fully inlined (never marked `#[inline(never)]`, so no
standalone copy of its code exists) is reported with `Error::Inlined` instead
of a plain "not found", naming the problem.

### 3. Queue edits

Values are built with a named constructor (`Value::u8(2)`, `Value::i32(-5)`,
`Value::f64(1.5)`, `Value::bool(true)`) instead of a Rust literal suffix like
`2u8`: the number stays separate from its type, which stays readable when the
two don't match the variable's own type (see
[Same size only](#limitations-read-this)).

| Call | Effect |
|------|--------|
| `exe.set_static("NAME", Value::i32(5))` | new value of a scalar `static` (width and signedness taken from the variable) |
| `exe.set_static_bytes("NAME", b"...")` | new contents of a `static` of any type (arrays, structs) |
| `exe.local("fn", "var")?.set(Value::u8(2))` | initial value of a local variable |
| `exe.local("fn", "var")?.cascade().set(..)` | same, and also flips comparisons that no longer match the new signedness (see below) |
| `exe.local("fn", "var")?.assign_at(addr, Value::i32(7))` | new assignment inserted in freed space; only for `mut` variables (see below) |
| `exe.function("f")?.returns(true)` | the function immediately returns this value |
| `exe.function("f")?.disable()` | the function immediately returns (`()`) |
| `exe.function("f")?.redirect_to("g")` | calls to `f` run `g` |
| `exe.function("f")?.replace_code(&bytes)` | your own machine code |
| `exe.patch_bytes(addr, &bytes)` / `nop(addr, n)` | raw bytes at a virtual address |
| `exe.replace_bytes(b"old", b"new")` | same-length replacement of every occurrence (string literals) |
| `exe.peek_i32(addr)` / `exe.poke_i32(addr, Value::i32(5))` (and `_bool`, `_i8`..`_i64`, `_u8`..`_u64`, `_f32`, `_f64`) | read / queue-write one scalar at a known address, no DWARF needed |

`exe.pending()` lists the queued edits, `exe.discard()` drops them.

### 4. Validate

| Call | Effect |
|------|--------|
| `exe.commit()` | rewrites the file (atomic, `.orig` backup, permissions kept) |
| `exe.commit_to("copy")` | writes a patched copy, original untouched |
| `exe.commit_hot(pid)` | patches the memory of a running process |

Hot and file commits combine: `commit_hot(pid)` then `commit()` makes the
change permanent as well.

### Mutability guard

DWARF does not say whether a variable is `mut`, so ruwrite reads the
declaration line in the program's source (`let a` vs `let mut a`). Rewriting
the **initial value** (`set`) is always allowed: that is the initialiser.
Adding a **later assignment** (`assign_at`) to a variable declared with a
plain `let` is refused, because the result would not be valid Rust:

```rust
exe.set_source_root("path/to/program/src");     // if the recorded path moved
let local = exe.local("counter", "fixed")?;
assert_eq!(local.mutability(), Mutability::Immutable);
local.assign_at(addr, Value::i32(1))?;          // Err(NotMutable)

// Your call: explicitly treat it as `mut`.
exe.local("counter", "fixed")?.make_mut().assign_at(addr, Value::i32(1))?;
```

When the source cannot be found the status is `Unknown`, which is treated as
not mutable. `assign_at` needs padding at `addr`: free the bytes first with
`exe.nop(addr, n)` and `commit()`.

### Works without debug info (release builds)

`exe.variables()` and `exe.local(..)` need DWARF. Without it (a release build,
or a stripped binary), recognise a variable by its **current content** instead:

```rust
let mut exe = ruwrite::Executable::open_any("target/release/demo")?;

for m in exe.find_value(ruwrite::Value::i32(10)) {
    println!("{:#x}  name: {:?}", m.address, m.name); // name, if a symbol covers it
}

// Several places can coincidentally hold the same bytes: only touch the one
// whose name (or address) you actually recognise.
let limit = exe.find_value(ruwrite::Value::i32(10))
    .into_iter()
    .find(|m| m.name.as_deref() == Some("demo::LIMIT"))
    .expect("LIMIT not found");
exe.poke_i32(limit.address, ruwrite::Value::i32(999))?;
exe.commit()?;
```

`find_value` only scans data-like sections (`.data`, `.rodata`,
`.data.rel.ro`): never `.text` or relocation/unwind metadata, so a coincidental
match cannot corrupt unrelated parts of the binary — but *you* still decide
which match is the right one before writing to it.

### Cascade mode: keeping the rest of the function consistent

ruwrite never changes a variable's declared type or width (see
[Same size only](#limitations-read-this)): the slot stays the same size, only
its content changes. But when the new value's **signedness** differs from the
variable's declared type (writing an unsigned value into a variable declared
signed, or the reverse), the comparisons the rest of the function makes
against that variable may no longer mean what they used to. **Cascade mode**
keeps them consistent:

```rust
// `a: i32` compared with `a < 5` (a signed `jl`). Writing an unsigned value
// flips that comparison to its unsigned reading (`jb`) wherever it is made
// directly against `a`'s stack slot in the same function.
exe.local("check", "a")?.cascade().set(ruwrite::Value::u8(200))?;
```

It only touches a comparison compiled as `cmp` directly against the
variable's stack slot (the same limitation `set` itself has for finding the
store to rewrite — see [Locals need an unoptimised
build](#limitations-read-this)): one made through a register loaded from the
slot is left untouched, and nothing is touched when the signedness does not
change. Without `.cascade()`, `set` behaves exactly as before.

## Protecting your program

A program can declare parts of itself that nobody may rewrite with ruwrite —
no separate crate, it is built into `ruwrite` itself:

```rust
use ruwrite::protect;

#[protect(immutable)]
fn check_license() -> bool { true }

#[protect(protected)]
static SECRET_KEY: u32 = 0xDEAD_BEEF;

#[protect(hide)]
fn internal_formula() -> i32 { 7 }

// ruwrite::protect_all!();   // the whole executable, instead of per item
```

Four modes, from least to most restrictive:

| Mode | Effect |
|------|--------|
| `editable` | explicitly not protected (the same as writing nothing, spelled out) |
| `immutable` | every edit touching it is refused |
| `protected` | `immutable`, and included in `Executable::integrity_snapshot()` / `verify_integrity()` (see below) |
| `hide` | `immutable`, its name is not stored as readable text in the binary (a hash is, instead), and it is left out of `Executable::functions()` / `variables()` |

The attribute embeds a small record in the executable. When ruwrite opens it,
every edit that touches a protected part is refused, whatever API is used:

```text
function `app::check_license` is defined as sensitive and protected by the
program (rule: function `check_license`): ruwrite refuses to modify it
```

- Covers function editors, `set_static*`, `patch_bytes`, `patch_file_bytes`,
  `nop`, `replace_bytes`, and the three commit modes (checked again at commit).
- The records themselves and the file structure (headers, section table) are
  protected as soon as anything is declared, so they cannot be edited away.
- A stripped executable cannot resolve plain names: everything named by a
  plain name is then **locked entirely**; a `hide`d item keeps working since
  it is matched by hash, not by name.
- Mark a protected function `#[inline(never)]` so the compiler keeps one
  identifiable copy of it instead of folding it into every caller.
- There is deliberately no option to bypass a protection.

**What `hide` does and does not do.** It keeps the plain name out of the
protection record and `ruwrite`'s own listings. It does **not** rename the
function's debug or symbol-table entry, strip its machine code, or defend
against disassembly: a determined reader with `objdump` still sees it. Build
without debug info and strip symbols for that. It is meant for a program to
tuck away its own internal helpers from a casual `ruwrite` inspection, not to
defeat deliberate reverse engineering.

**Integrity checking.** For `protected` items, take a snapshot while you trust
the binary, and check it again later (for instance at startup, from a small
helper program):

```rust
let snapshot = exe.integrity_snapshot();      // hash of every `protected` part
// ... time passes, the file could have been tampered with ...
for violation in exe.verify_integrity(&snapshot) {
    println!("{}: {:?}", violation.what, violation.kind); // Changed / Missing / New
}
```

**What this is not.** The protection is enforced by ruwrite. Someone editing
the file with a hex editor or another tool is not stopped by it, and nothing
verifies the program at run time by itself — `verify_integrity` only checks
what you ask it to, when you ask it to. Treat it as a clear, enforced "do not
touch" for ruwrite users, not as tamper-proofing.

## Compiler-style checks

ruwrite refuses edits that Rust itself would not accept, using the debug info:

| Check | Refused with |
|-------|--------------|
| value of the wrong kind or out of range for the variable / return type | `TypeMismatch`, `ValueOutOfRange` |
| `returns(..)` on a function returning `()` | `Rejected` |
| `disable()` on a function that returns a value | `Rejected` |
| `redirect_to` a function with another signature | `Rejected` |
| new assignment to a variable not declared `mut` | `NotMutable` (see above) |
| looking up a function the compiler fully inlined | `Inlined` (see above) |

Function checks need DWARF; without it they are skipped.

## Try it

The `playground/` crate contains programs built to be rewritten
(`target_demo`, `hot_demo`, `locked_demo`) and the end-to-end tests that
rewrite and run them.

```bash
cargo build --workspace
cargo run --example inspect      -- target/debug/target_demo target_demo
cargo run --example disasm       -- target/debug/target_demo compute
cargo run --example patch_demo                         # before / after output
cargo run --example protection                         # protected parts refuse edits
cargo run --example release_probe -- target/release/target_demo  # no debug info at all

# hot patching
target/debug/hot_demo &                                # prints value=1
cargo run --example hot_patch -- target/debug/hot_demo $!   # now prints value=42
```

## Limitations (read this)

ruwrite rewrites **machine code and data in place**. That is powerful, and it
has honest limits:

- **Same size only.** A rewrite never moves or grows code or data. Changing
  `a: i32 = 3` to `2` therefore stores `2` in the *existing 4-byte slot*: the
  value changes, the width does not (the compiled code still treats it as an
  `i32`). Values that do not fit the slot are refused. Cascade mode (above)
  can keep *comparisons* against a retyped variable consistent, but it cannot
  change the variable's width either.
- **Locals need an unoptimised build** (`opt-level = 0`) and x86/x86_64: ruwrite
  rewrites the constant the compiler stored in the variable's stack slot. In an
  optimised build the value usually lives in a register or is folded into other
  instructions, and ruwrite reports that no store was found instead of guessing.
  Only the *initial* value can change, not a later assignment. `find_value` /
  `peek_*` / `poke_*` work in release builds too, by content instead of DWARF.
- **Variables need DWARF** (`debug = true`) for `variables()`/`local(..)`.
  Functions need the symbol table (not stripped). Raw byte patching and
  `find_value`/`peek_*`/`poke_*` need neither.
- **Zero-initialised statics** (`.bss`) have no bytes in the file and cannot be
  rewritten; they are reported with an explicit error. A plain `static X: i32 = 0`
  falls in this category.
- **No code re-optimisation.** ruwrite does not regenerate or optimise code
  around an edit: it writes small fixed-size sequences (a store, a return, a
  jump, a one-byte condition flip for cascade mode) where space exists.
  Growing a function or restructuring its body is done by supplying your own
  bytes with `replace_code`.
- **Function rewrites must fit** in the symbol's size (`returns(true)` needs 3
  bytes, an `f64` constant 16). `returns` on a 32-bit x86 float is unsupported.
- **Hot patching** is Linux only, needs ptrace rights on the target (same user,
  and `kernel.yama.ptrace_scope` permitting), and rewrites memory while the
  program runs: patch functions that are not currently executing. Programs that
  already copied a value elsewhere keep the old one.
- **Code signing / integrity checks.** A modified binary no longer matches its
  signature or checksum.
- **Windows: Controlled Folder Access.** `commit()` replaces the target file
  with a write-then-rename, the same thing ransomware protection is designed
  to catch: if Controlled Folder Access is on and the program is not on its
  allow list, the rename is refused outright (and, unlike antivirus scanning,
  does not clear up on its own — retrying does not help). `commit_to` a path
  outside a protected folder, or allow your program under Windows Security →
  Virus & threat protection → Ransomware protection, if you hit this.
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
