//! Works without debug info: finds a static by its content, then reads/writes
//! it directly by address.
//!
//! ```text
//! cargo build --workspace --release
//! cargo run --example release_probe -- target/release/target_demo
//! ```

use ruwrite::{Executable, Value};

fn main() -> ruwrite::Result<()> {
    let path = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "target/release/target_demo".into());
    let mut exe = Executable::open_any(&path)?;
    println!("debug info: {}", exe.info().has_debug_info);

    let matches = exe.find_value(Value::i32(10));
    println!("{} place(s) hold the value 10:", matches.len());
    for m in &matches {
        println!("  {:#x}  name: {:?}", m.address, m.name);
    }

    // Several places can coincidentally hold the same bytes: only touch the
    // one whose name you actually recognise.
    let target = matches
        .iter()
        .find(|m| m.name.as_deref() == Some("target_demo::LIMIT"))
        .expect("LIMIT not found");
    println!("\nLIMIT is currently {}", exe.peek_i32(target.address)?);
    exe.poke_i32(target.address, 999)?;
    exe.commit_to("/tmp/release_probe_patched")?;
    Ok(())
}
