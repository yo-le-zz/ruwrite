//! End-to-end demo: rewrite a copy of `target_demo`, run it before and after.
//!
//! ```text
//! cargo build --workspace
//! cargo run --example patch_demo
//! ```
//!
//! Optional argument: path of the target executable
//! (default `target/debug/target_demo`).

use std::path::Path;
use std::process::Command;

use ruwrite::Executable;

fn run(path: &Path) -> String {
    let out = Command::new(path).output().expect("cannot run the target");
    String::from_utf8_lossy(&out.stdout).into_owned()
}

fn main() -> ruwrite::Result<()> {
    let original = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "target/debug/target_demo".into());
    let patched = format!("{original}.patched");

    println!("--- before ---\n{}", run(Path::new(&original)));

    // `exe` designates the target executable; nothing is modified yet.
    let mut exe = Executable::open(&original)?;

    exe.set_static("LIMIT", 100)?; // static LIMIT: i32 = 10
    exe.local("compute", "a")?.set(2u8)?; // let a: i32 = 3  ->  2
    exe.function("is_admin")?.returns(true)?; // always true
    exe.function("say_hi")?.disable()?; // does nothing any more

    println!("Pending edits:");
    for patch in exe.pending() {
        println!("  {patch}");
    }

    // Validate: write a patched copy and leave the original untouched.
    let report = exe.commit_to(&patched)?;
    println!(
        "\n{} edits, {} bytes written to {}\n",
        report.patches, report.bytes, patched
    );

    println!("--- after ---\n{}", run(Path::new(&patched)));
    Ok(())
}
