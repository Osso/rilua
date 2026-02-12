use std::env;
use std::process;

fn main() {
    let args: Vec<String> = env::args().collect();

    if args.len() > 1 && args[1] == "-v" {
        println!("riluac 0.1.0 (Lua 5.1.1 bytecode compiler)");
        return;
    }

    eprintln!("Usage: riluac [options] [filenames]");
    eprintln!("Options:");
    eprintln!("  -v       show version information");
    eprintln!();
    eprintln!("riluac is not yet implemented.");
    process::exit(1);
}
