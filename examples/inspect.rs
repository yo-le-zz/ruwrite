//! Prints what ruwrite can learn about an executable.
//!
//! ```text
//! cargo run --example inspect -- target/debug/target_demo [filter]
//! ```

use ruwrite::{Executable, VariableKind};

fn main() -> ruwrite::Result<()> {
    let mut args = std::env::args().skip(1);
    let path = args.next().unwrap_or_else(|| {
        eprintln!("usage: inspect <executable> [name filter]");
        std::process::exit(2);
    });
    let filter = args.next().unwrap_or_default();
    let exe = Executable::open(&path)?;

    let info = exe.info();
    println!(
        "{path}: {} {} {}-bit, entry {:#x}, debug info: {}",
        info.format, info.arch, info.bits, info.entry, info.has_debug_info
    );

    println!("\nFunctions matching `{filter}`:");
    for f in exe.functions().iter().filter(|f| f.name.contains(&filter)) {
        println!("  {:#012x} {:>6} bytes  {}", f.address, f.size, f.name);
    }

    match exe.variables() {
        Ok(vars) => {
            println!("\nVariables matching `{filter}`:");
            for v in vars.iter().filter(|v| v.name.contains(&filter)) {
                match &v.kind {
                    VariableKind::Static { address, .. } => {
                        println!("  static {} : {} @ {address:#x}", v.name, v.ty)
                    }
                    VariableKind::Local { function, .. } => {
                        println!("  local  {} : {} in {function}", v.name, v.ty)
                    }
                }
            }
        }
        Err(e) => println!("\nNo variables: {e}"),
    }
    Ok(())
}
