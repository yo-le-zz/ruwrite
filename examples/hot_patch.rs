//! Hot patching: change a function of a program while it is running.
//!
//! ```text
//! cargo build --workspace
//! target/debug/hot_demo &          # prints `value=1` every 50 ms
//! cargo run --example hot_patch -- target/debug/hot_demo $!
//! ```
//!
//! The running program starts printing `value=42`; the file on disk is not
//! modified (call `exe.commit()` as well to make the change permanent).

use ruwrite::Executable;

fn main() -> ruwrite::Result<()> {
    let mut args = std::env::args().skip(1);
    let (Some(path), Some(pid)) = (args.next(), args.next().and_then(|p| p.parse::<u32>().ok()))
    else {
        eprintln!("usage: hot_patch <executable> <pid>");
        std::process::exit(2);
    };

    let mut exe = Executable::open(path)?;
    exe.function("hot_demo::value")?.returns(42)?;

    let report = exe.commit_hot(pid)?;
    println!("patched process {pid}: {} edit(s) applied", report.patches);
    Ok(())
}
