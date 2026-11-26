//! Disassembles one function (x86 / x86_64).
//!
//! ```text
//! cargo run --example disasm -- target/debug/target_demo compute
//! ```

use ruwrite::Executable;

fn main() -> ruwrite::Result<()> {
    let mut args = std::env::args().skip(1);
    let (Some(path), Some(name)) = (args.next(), args.next()) else {
        eprintln!("usage: disasm <executable> <function>");
        std::process::exit(2);
    };
    let mut exe = Executable::open(path)?;
    for (address, text) in exe.function(&name)?.disassemble()? {
        println!("{address:#012x}  {text}");
    }
    Ok(())
}
