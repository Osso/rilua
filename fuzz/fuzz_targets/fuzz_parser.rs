#![no_main]

use libfuzzer_sys::fuzz_target;
use rilua::compiler::parser;

fuzz_target!(|data: &[u8]| {
    let _ = parser::parse(data, "fuzz");
});
