#![no_main]

use libfuzzer_sys::fuzz_target;
use rilua::compiler::lexer::Lexer;
use rilua::compiler::token::Token;

fuzz_target!(|data: &[u8]| {
    let mut lexer = Lexer::new(data, "fuzz");
    loop {
        match lexer.next() {
            Ok((Token::Eos, _)) => break,
            Ok(_) => continue,
            Err(_) => break,
        }
    }
});
