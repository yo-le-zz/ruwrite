//! Shows the protection rules declared by a program and tries to break them.
//!
//! ```text
//! cargo build --workspace
//! cargo run --example protection -- target/debug/target_demo
//! ```

use ruwrite::{Executable, Value};

fn main() -> ruwrite::Result<()> {
    let path = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "target/debug/target_demo".into());
    let mut exe = Executable::open(&path)?;

    println!("Rules declared by the program:");
    for rule in exe.protections() {
        println!(
            "  {} — {} ({} match(es))",
            rule.target, rule.mode, rule.matches
        );
    }

    // `hidden_formula` is declared `#[protect(hide)]`: its name is not in
    // this listing at all, hidden from a casual look just like `ruwrite`'s
    // own inspection.
    println!(
        "\nfunctions() lists {} functions (the hidden one is not among them)",
        exe.functions().len()
    );

    println!("\nAttempts:");
    for result in [
        exe.function("license_check")?.returns(true).map(|_| ()),
        exe.set_static("SECRET", Value::i32(0)).map(|_| ()),
    ] {
        match result {
            Ok(()) => println!("  accepted"),
            Err(e) => println!("  refused: {e}"),
        }
    }

    // `SECRET` is `#[protect(protected)]`: take a snapshot now, and anything
    // that changes it later (even outside `ruwrite`) shows up here.
    let snapshot = exe.integrity_snapshot();
    let violations = exe.verify_integrity(&snapshot);
    println!(
        "\nintegrity check right after the snapshot: {} violation(s)",
        violations.len()
    );

    // A part that is not protected can still be edited.
    exe.set_static("LIMIT", Value::i32(1))?;
    println!(
        "\n`LIMIT` is not protected: {} edit queued",
        exe.pending().len()
    );
    Ok(())
}
