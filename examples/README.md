# Examples

## run_file

Runs a Lua file passed as a command-line argument.

```bash
cargo run --example run_file -- examples/hello.lua
```

This demonstrates the minimal embedding API:

1. `Lua::new()` creates a state with all standard libraries loaded
2. `Lua::exec_file(path)` reads, compiles, and executes a Lua file
3. Errors are printed to stderr with a non-zero exit code

## hello.lua

Sample Lua script used by `run_file`. Prints a greeting and a factorial
table to exercise `print`, `string.format`, local variables, and
recursive function calls.
