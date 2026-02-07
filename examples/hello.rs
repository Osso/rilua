//! Run a Lua script from Rust.
//!
//! ```sh
//! cargo run --example hello
//! ```

fn main() {
    let mut state = rilua::State::new();
    state.open_libs();

    if let Err(e) = state.do_string("print('hello from rilua')") {
        eprintln!("error: {e}");
        std::process::exit(1);
    }
}
