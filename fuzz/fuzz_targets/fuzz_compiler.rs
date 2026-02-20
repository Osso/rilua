#![no_main]

use libfuzzer_sys::fuzz_target;
use rilua::compiler::compile;

fuzz_target!(|data: &[u8]| {
    let _ = compile(data, "fuzz");
});
