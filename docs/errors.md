# Error Handling

## Decision

**Result-based error propagation. No longjmp, no panics in library
code.**

## Overview

PUC-Rio Lua 5.1.1 uses `longjmp`/`setjmp` for error handling.
When a runtime error occurs, execution jumps back to the nearest
protected call boundary (`lua_pcall`). This is efficient in C but
has no safe equivalent in Rust.

rilua uses Rust's `Result<T, E>` type for all fallible operations.
Errors propagate up the call stack via `?` operator until caught
by a protected call boundary (the Lua `pcall`/`xpcall` functions
or the Rust API's `pcall` method).

## Error Types

```rust
/// Top-level error type for all rilua errors.
pub enum Error {
    /// Lexer or parser error.
    Syntax(SyntaxError),
    /// Runtime error during VM execution.
    Runtime(RuntimeError),
    /// I/O error from file operations.
    Io(std::io::Error),
}

pub struct SyntaxError {
    pub message: String,
    pub source: String,
    pub line: u32,
    pub column: u32,
}

pub struct RuntimeError {
    pub message: String,
    pub level: u32,
    pub traceback: Vec<TraceEntry>,
}

pub struct TraceEntry {
    pub source: String,
    pub line: u32,
    pub name: Option<String>,
}
```

## Error Messages

Error messages must match PUC-Rio Lua 5.1.1 wording for behavioral
compatibility. Lua programs often match on error message strings
(via `pcall` + string comparison), so messages are part of the
observable behavior.

Format: `source:line: message`

Examples:

- `stdin:3: attempt to perform arithmetic on a string value`
- `stdin:5: bad argument #1 to 'assert' (string expected, got nil)`
- `[string "..."]:2: ')' expected near 'end'`

## Protected Calls

Lua's `pcall(f, ...)` calls function `f` and catches any error:

- On success: returns `true` followed by the function's return values.
- On error: returns `false` followed by the error message.

`xpcall(f, msgh, ...)` additionally runs an error handler `msgh`
before unwinding.

In rilua, protected calls are implemented by catching `Err` results:

```rust
fn lua_pcall(&mut self, func: Val, args: &[Val]) -> Result<Vec<Val>> {
    match self.call(func, args) {
        Ok(results) => {
            // Push true + results
        }
        Err(e) => {
            // Push false + error message
        }
    }
}
```

## Recoverable Errors in the REPL

Some syntax errors indicate incomplete input (the user needs to type
more). These are "recoverable" errors:

- Unexpected end of input
- Unfinished string
- Unfinished long comment

The REPL detects these and prompts for continuation instead of
reporting an error. This is implemented via an `is_recoverable()`
method on `SyntaxError`.

## Error Object

In Lua 5.1.1, `error()` can throw any value as an error object,
not just strings. The error object propagates through `pcall`:

```lua
local ok, err = pcall(function()
    error({code = 404, msg = "not found"})
end)
-- err is the table {code = 404, msg = "not found"}
```

rilua's `RuntimeError` must support arbitrary Lua values as error
objects:

```rust
pub struct RuntimeError {
    pub object: Val,  // The error value (often a string)
    pub level: u32,
    pub traceback: Vec<TraceEntry>,
}
```

## No Panics

Library code (`src/`) never panics. All fallible operations return
`Result`. The safety lints in `Cargo.toml` enforce this:

```toml
unwrap_used = { level = "warn", priority = 2 }
panic = { level = "warn", priority = 2 }
expect_used = { level = "warn", priority = 2 }
```

Test code may use `unwrap()` and `assert!()` freely.
