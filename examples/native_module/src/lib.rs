//! Example rilua-native module.
//!
//! Exports a `rilua_open_hello` function that creates a module table
//! with a `greet(name)` function and a `VERSION` string.
//!
//! Build with:
//! ```sh
//! cargo build --manifest-path examples/native_module/Cargo.toml
//! ```
//!
//! Then load from Lua:
//! ```lua
//! local hello = require("hello")
//! print(hello.greet("world"))  --> Hello, world!
//! print(hello.VERSION)         --> 0.1.0
//! ```

use rilua::vm::state::LuaState;
use rilua::vm::value::Val;

// Export the module info symbol for ABI validation.
rilua::export_module_info!();

/// Module entry point: `rilua_open_hello`.
///
/// Creates a module table with:
/// - `hello.greet(name)` — returns `"Hello, <name>!"`
/// - `hello.VERSION` — the string `"0.1.0"`
///
/// # Safety
///
/// Called by the host after ABI validation. The `state` pointer must be
/// valid and point to the host's `LuaState`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rilua_open_hello(state: *mut LuaState) -> i32 {
    let state = unsafe { &mut *state };

    // Create the module table.
    let mod_table = state.gc.alloc_table(rilua::vm::table::Table::new());

    // Register greet function.
    let greet_fn: rilua::RustFn = |s: &mut LuaState| {
        let name = if s.top > s.base {
            let val = s.stack_get(s.base);
            match val {
                Val::Str(r) => s
                    .gc
                    .string_arena
                    .get(r)
                    .map(|ls| String::from_utf8_lossy(ls.data()).to_string())
                    .unwrap_or_else(|| "world".to_string()),
                _ => "world".to_string(),
            }
        } else {
            "world".to_string()
        };
        let greeting = format!("Hello, {name}!");
        let str_ref = s.gc.intern_string(greeting.as_bytes());
        s.push(Val::Str(str_ref));
        Ok(1)
    };

    let greet_closure = rilua::vm::closure::Closure::Rust(rilua::vm::closure::RustClosure::new(
        greet_fn, "greet",
    ));
    let greet_ref = state.gc.alloc_closure(greet_closure);

    // Set mod_table.greet = greet_fn
    let key = state.gc.intern_string(b"greet");
    if let Some(t) = state.gc.tables.get_mut(mod_table) {
        if t.raw_set(Val::Str(key), Val::Function(greet_ref), &state.gc.string_arena)
            .is_err()
        {
            return -1;
        }
    }

    // Set mod_table.VERSION = "0.1.0"
    let ver_key = state.gc.intern_string(b"VERSION");
    let ver_val = state.gc.intern_string(b"0.1.0");
    if let Some(t) = state.gc.tables.get_mut(mod_table) {
        if t.raw_set(
            Val::Str(ver_key),
            Val::Str(ver_val),
            &state.gc.string_arena,
        )
        .is_err()
        {
            return -1;
        }
    }

    // Push the module table as the return value.
    state.push(Val::Table(mod_table));
    1
}
