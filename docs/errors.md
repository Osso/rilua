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
    /// Memory allocation error (equivalent to LUA_ERRMEM).
    Memory,
    /// Error in error handler (equivalent to LUA_ERRERR).
    /// Occurs when the xpcall error handler itself errors.
    ErrorHandler,
}

pub struct SyntaxError {
    pub message: String,
    pub source: String,
    pub line: u32,
    pub column: u32,
}

pub struct RuntimeError {
    pub object: Val,  // The error value (often a string)
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
- On error: returns `false` followed by the error object (which
  may be any Lua value, not just a string).

`xpcall(f, err)` additionally runs an error handler `err` before
unwinding. Note: in Lua 5.1.1, `xpcall` takes exactly two arguments
(the function and the error handler). Extra arguments passed to `f`
were added in Lua 5.2.

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

## Error Recovery

When a protected call catches an error, the VM must:

1. Restore the stack to the pre-call state.
2. Close any open upvalues above the restored stack level
   (call `close_upvalues()` — equivalent to PUC-Rio's
   `luaF_close`). This is critical for correct closure behavior.
3. Set the error object on the stack.

## Stack Overflow

PUC-Rio limits the C call depth to `LUAI_MAXCCALLS` (200). rilua
tracks call depth and raises a "stack overflow" runtime error when
the limit is exceeded. If the overflow persists during error
handling, an `Error::ErrorHandler` is returned.

## Coroutine Errors

A coroutine that errors becomes dead and cannot be resumed. The
error propagates to `coroutine.resume()` which returns `false`
plus the error object, similar to `pcall`.

## No Panics

Library code (`src/`) never panics. All fallible operations return
`Result`. The safety lints in `Cargo.toml` enforce this:

```toml
unwrap_used = { level = "warn", priority = 2 }
panic = { level = "warn", priority = 2 }
expect_used = { level = "warn", priority = 2 }
```

Test code may use `unwrap()` and `assert!()` freely.
