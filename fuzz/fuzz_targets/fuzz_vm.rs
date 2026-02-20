#![no_main]

use libfuzzer_sys::fuzz_target;
use rilua::{Lua, StdLib};

fuzz_target!(|data: &[u8]| {
    if let Ok(mut lua) = Lua::new_with(StdLib::ALL) {
        let _ = lua.exec_bytes(data, "fuzz");
    }
});
