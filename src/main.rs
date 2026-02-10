use std::env;
use std::process;

fn main() {
    let args: Vec<String> = env::args().collect();

    if args.len() < 2 {
        eprintln!("Usage: rilua [options] [script [args]]");
        eprintln!("Options:");
        eprintln!("  -e stat  execute string 'stat'");
        eprintln!("  -v       show version information");
        process::exit(1);
    }

    match args[1].as_str() {
        "-e" => {
            if args.len() < 3 {
                eprintln!("rilua: '-e' needs argument");
                process::exit(1);
            }
            if let Err(e) = rilua::exec(&args[2]) {
                eprintln!("{e}");
                process::exit(1);
            }
        }
        "-v" => {
            println!("Lua 5.1.1  Copyright (C) 1994-2006 Lua.org, PUC-Rio");
        }
        other => {
            // Treat as a script file.
            let source = match std::fs::read_to_string(other) {
                Ok(s) => s,
                Err(e) => {
                    eprintln!("rilua: {other}: {e}");
                    process::exit(1);
                }
            };
            let name = format!("@{other}");
            if let Err(e) = rilua::exec_with_name(&source, &name) {
                eprintln!("{e}");
                process::exit(1);
            }
        }
    }
}
