# rilua WASM Demo

Browser-based Lua 5.1 interpreter using rilua compiled to WebAssembly.

Type Lua code in a textarea, press **Run** (or **Ctrl+Enter**), and see
output rendered in the page. All execution happens client-side in the
browser -- no server-side processing.

## Prerequisites

- Rust toolchain (1.92+) with the `wasm32-unknown-unknown` target
- [wasm-pack](https://rustwasm.github.io/wasm-pack/installer/)

Install both if you haven't already:

```bash
rustup target add wasm32-unknown-unknown
cargo install wasm-pack
```

If the project uses [mise](https://mise.jdx.dev/), `wasm-pack` is already
declared as a tool dependency. Run `mise install` to set it up.

## Build

From this directory (`examples/wasm-demo/`):

```bash
./build.sh
```

This runs `wasm-pack build --target web` and produces output in `pkg/`.
The build compiles rilua in release mode and runs `wasm-opt` on the
resulting `.wasm` file.

## Serve

The demo requires an HTTP server. Opening `index.html` directly as a
`file://` URL will not work because browsers block ES module imports
from `file://` origins.

Using Python's built-in server:

```bash
python3 -m http.server 8080
```

Then open <http://localhost:8080> in a browser.

Any static file server works (`npx serve`, `caddy file-server`, etc.)
as long as `.wasm` files are served with the `application/wasm` MIME type.

## Available Libraries

The demo loads these standard libraries:

| Library     | Examples                                      |
|-------------|-----------------------------------------------|
| `base`      | `print`, `type`, `tostring`, `pcall`, `error` |
| `string`    | `string.format`, `string.find`, `string.rep`  |
| `table`     | `table.insert`, `table.sort`, `table.concat`  |
| `math`      | `math.sin`, `math.random`, `math.pi`          |
| `coroutine` | `coroutine.create`, `coroutine.resume`         |

The following are excluded because they require filesystem or process
access not available in `wasm32-unknown-unknown`:

- `io` -- file and stream operations
- `os` -- time, environment, process control
- `package` -- `require` and module loading
- `debug` -- excluded to reduce surface area

## How It Works

The crate exports a single function via `wasm-bindgen`:

```rust
#[wasm_bindgen]
pub fn eval_lua(code: &str) -> String
```

1. A new `Lua` state is created for each call with the libraries above.
2. The built-in `print` is replaced with a version that writes to a
   thread-local `String` buffer instead of stdout (which does not exist
   in WASM).
3. After execution, the captured output is returned. If a Lua error
   occurs, it is appended to whatever output was produced before the
   error.

## Project Structure

```
examples/wasm-demo/
├── Cargo.toml    # cdylib crate, depends on rilua + wasm-bindgen
├── src/
│   └── lib.rs    # wasm-bindgen export + custom print
├── index.html    # browser UI (textarea, run button, output area)
├── build.sh      # wasm-pack build wrapper
└── README.md     # this file
```
