# WebAssembly Support

rilua compiles to `wasm32-unknown-unknown` for running Lua 5.1.1 code
in the browser or other WebAssembly runtimes.

## How It Works

All C FFI in rilua is centralized in `src/platform.rs`. When targeting
`wasm32`, a parallel set of pure-Rust stubs replaces every `extern "C"`
function. The VM, compiler, and core standard libraries run unchanged --
only the platform layer is swapped.

```text
platform.rs
  |
  +-- #[cfg(not(target_arch = "wasm32"))]  -> extern "C" { ... }
  |
  +-- #[cfg(target_arch = "wasm32")]       -> wasm_stubs module
```

### Stubs and Replacements

| C Function | WASM Replacement |
|------------|------------------|
| `strtod` | `f64::from_str()` (ASCII decimal only) |
| `localeconv` | Static `LConv` with `decimal_point = '.'` |
| `strcoll` | Byte-wise comparison (no locale) |
| `setlocale` | No-op (returns `"C"`) |
| `strftime` | Returns 0 (no formatting) |
| `localtime_r` / `gmtime_r` | Returns `false` (no time conversion) |
| `mktime` | Returns -1 |
| `clock` | Returns -1 |
| `time(NULL)` | Returns 0 |
| `isalpha`, `tolower`, etc. | ASCII-only Rust equivalents |
| FILE* operations | Return null/error values |
| `popen` / `pclose` | Return null/-1 |
| `signal` | No-op (SIGINT handling disabled) |

### Locale Differences

On native platforms, rilua uses `strtod` and `localeconv` for
locale-aware number parsing (matching PUC-Rio behavior where `3,14`
parses as a number in locales using comma as decimal separator).

On WASM, number parsing is ASCII-only with `.` as the decimal point.
This matches the `"C"` locale and is correct for all standard Lua
programs.

## Standard Library Availability

Libraries that need filesystem or process access are still loadable
on WASM but their functions return errors. Libraries that are pure
computation work without restrictions.

| Library | WASM Status | Notes |
|---------|-------------|-------|
| `base` | Full | All 29 functions work |
| `string` | Full | All 14 functions work, pattern matching included |
| `table` | Full | All 9 functions work |
| `math` | Full | All 28 functions work |
| `coroutine` | Full | All 6 functions work |
| `debug` | Full | All 14 functions work (no filesystem dependency) |
| `io` | Errors | File operations return `nil, "not supported"` |
| `os` | Partial | `os.clock`, `os.date`, `os.time` return defaults; `os.execute`, `os.remove`, `os.rename`, `os.tmpname` error |
| `package` | Limited | `require` works for preloaded modules; file-based loading fails |

### Sandboxed Loading

For WASM builds, load only the libraries that work:

```rust
use rilua::{Lua, StdLib};

let libs = StdLib::BASE | StdLib::STRING | StdLib::TABLE
         | StdLib::MATH | StdLib::COROUTINE;
let mut lua = Lua::new_with(libs)?;
```

## Building for WASM

### Prerequisites

- Rust 1.92+ with the `wasm32-unknown-unknown` target
- [wasm-pack](https://rustwasm.github.io/wasm-pack/installer/) (for
  browser builds)

```bash
rustup target add wasm32-unknown-unknown
```

### As a Library Dependency

Add rilua to your WASM crate's `Cargo.toml`:

```toml
[dependencies]
rilua = "0.1"
wasm-bindgen = "0.2"

[lib]
crate-type = ["cdylib"]
```

Example `lib.rs`:

```rust
use wasm_bindgen::prelude::*;
use rilua::{Lua, StdLib};

#[wasm_bindgen]
pub fn eval_lua(code: &str) -> String {
    let libs = StdLib::BASE | StdLib::STRING | StdLib::TABLE
             | StdLib::MATH | StdLib::COROUTINE;

    let mut lua = match Lua::new_with(libs) {
        Ok(l) => l,
        Err(e) => return format!("init error: {e}"),
    };

    match lua.exec(code) {
        Ok(()) => String::new(),
        Err(e) => format!("{e}"),
    }
}
```

Build with wasm-pack:

```bash
wasm-pack build --target web
```

### Browser Demo

A working browser demo is in `examples/wasm-demo/`. It provides a
textarea for Lua code and renders output in the page. See
`examples/wasm-demo/README.md` for build and serve instructions.

The demo replaces `print` with a version that writes to a thread-local
`String` buffer (stdout does not exist in WASM), then returns the
captured output after execution.

## Feature Interactions

| Feature | WASM Behavior |
|---------|---------------|
| `dynmod` | Disabled (no shared library loading on WASM) |
| `send` | Works (GcRef indices are just u32 values) |
| SIGINT | No-op (no signal handling on WASM) |

## Limitations

- **No filesystem**: `dofile`, `loadfile`, `io.*`, `os.remove`,
  `os.rename`, `os.tmpname` return errors
- **No process control**: `os.execute`, `io.popen` return errors
- **No locale support**: `os.setlocale` is a no-op, `os.date`
  formatting via `strftime` returns empty strings
- **No clock**: `os.clock` returns -1
- **ASCII-only number parsing**: locale-dependent decimal separators
  (e.g., comma) are not supported
- **No stdin/stdout/stderr**: `io.stdin`, `io.stdout`, `io.stderr`
  are null pointers; `print` writes to stdout which goes nowhere
  unless overridden (as the demo does)
