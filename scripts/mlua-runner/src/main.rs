use mlua::prelude::*;
use std::env;
use std::fs;
use std::process;

fn main() {
    let args: Vec<String> = env::args().collect();
    if args.len() < 2 {
        eprintln!("Usage: mlua-runner <script.lua> [args...]");
        process::exit(1);
    }

    let path = &args[1];
    let code = match fs::read_to_string(path) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("Error reading {path}: {e}");
            process::exit(1);
        }
    };

    let lua = Lua::new();

    // Set up arg table (some tests check it)
    if let Ok(globals) = lua.globals().get::<LuaTable>("arg") {
        let _ = globals;
    } else {
        let arg_table = lua.create_table().expect("create arg table");
        let _ = arg_table.set(0, path.as_str());
        for (i, a) in args.iter().skip(2).enumerate() {
            let _ = arg_table.set((i + 1) as i64, a.as_str());
        }
        lua.globals().set("arg", arg_table).expect("set arg table");
    }

    if let Err(e) = lua.load(&code).set_name(path).exec() {
        eprintln!("Error: {e}");
        process::exit(1);
    }
}
