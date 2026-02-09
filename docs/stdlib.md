# Standard Library

## Decision

**Modular implementation, one file per library, matching PUC-Rio Lua
5.1.1 standard library behavior.**

## Overview

Lua 5.1.1 ships with 9 standard libraries (base, coroutine, string,
table, math, io, os, debug, package). The coroutine library is
registered as part of `luaopen_base()` but occupies its own namespace. Each library is a
collection of functions registered in a table (or as globals for the
base library). Libraries can be loaded selectively — an embedded Lua
environment may omit `io` and `os` for sandboxing.

## Libraries

### Base Library (`stdlib/base.rs`)

Global functions not in any table.

| Function | Status | Notes |
|----------|--------|-------|
| `assert` | Required | Error with optional message |
| `collectgarbage` | Required | 7 options, see [gc.md](gc.md) |
| `dofile` | Required | Load and execute file |
| `error` | Required | Throw error object at level |
| `getfenv` | Required | Get function environment |
| `getmetatable` | Required | Get metatable (respects `__metatable`) |
| `ipairs` | Required | Integer key iterator |
| `load` | Required | Load chunk from function |
| `loadfile` | Required | Load chunk from file |
| `loadstring` | Required | Load chunk from string |
| `gcinfo` | Required | Deprecated GC info (returns KB used) |
| `next` | Required | Table traversal |
| `pairs` | Required | Generic table iterator |
| `pcall` | Required | Protected call |
| `print` | Required | Print to stdout (uses `tostring`) |
| `rawequal` | Required | Equality without metamethods |
| `rawget` | Required | Table access without metamethods |
| `rawset` | Required | Table assignment without metamethods |
| `_G` | Required | Global table reference |
| `select` | Required | `select(n, ...)` or `select('#', ...)` |
| `setfenv` | Required | Set function environment |
| `setmetatable` | Required | Set metatable (respects `__metatable`) |
| `tonumber` | Required | Convert to number (with base) |
| `tostring` | Required | Convert to string (uses `__tostring`) |
| `type` | Required | Type name as string |
| `unpack` | Required | Table to multiple values |
| `xpcall` | Required | Protected call with error handler |
| `_VERSION` | Required | `"Lua 5.1"` |
| `newproxy` | Optional | Undocumented, creates proxy userdata |

### Coroutine Library (registered by `luaopen_base`)

The coroutine library is registered as the `coroutine` table by
`luaopen_base()`. It provides cooperative multithreading.

| Function | Notes |
|----------|-------|
| `coroutine.create` | Create coroutine from function |
| `coroutine.resume` | Resume suspended coroutine |
| `coroutine.running` | Return running coroutine (returns nothing if main thread) |
| `coroutine.status` | Return status string (running/suspended/normal/dead) |
| `coroutine.wrap` | Create coroutine as iterator function |
| `coroutine.yield` | Suspend execution, return values to resume |

### String Library (`stdlib/string.rs`)

Registered as the `string` table and as the string metatable's
`__index`.

| Function | Notes |
|----------|-------|
| `string.byte` | Character codes |
| `string.char` | Characters from codes |
| `string.dump` | Dump function bytecode |
| `string.find` | Pattern matching search |
| `string.format` | Formatted string output |
| `string.gmatch` | Global pattern match iterator |
| `string.gsub` | Global pattern substitution |
| `string.len` | String length |
| `string.lower` | Lowercase conversion |
| `string.match` | Pattern match extraction |
| `string.rep` | String repetition |
| `string.reverse` | String reversal |
| `string.sub` | Substring extraction |
| `string.upper` | Uppercase conversion |
| `string.gfind` | Deprecated alias for gmatch (works by default; raises error only if `LUA_COMPAT_GFIND` is undefined) |

Lua 5.1 patterns are NOT regular expressions. They support character
classes (`%a`, `%d`, `%w`, etc.), anchors (`^`, `$`), quantifiers
(`*`, `+`, `-`, `?`), captures, and backreferences (`%1` through
`%9` to match a previous capture). They do not support alternation.

### Table Library (`stdlib/table.rs`)

| Function | Notes |
|----------|-------|
| `table.concat` | Concatenate array elements |
| `table.insert` | Insert element at position |
| `table.maxn` | Maximum positive numeric key |
| `table.remove` | Remove element at position |
| `table.sort` | In-place sort |
| `table.foreach` | Deprecated: iterate table (use pairs) |
| `table.foreachi` | Deprecated: iterate array (use ipairs) |
| `table.getn` | Deprecated: table length (use # operator) |
| `table.setn` | Deprecated: raises error in 5.1.1 |

### Math Library (`stdlib/math.rs`)

| Function | Notes |
|----------|-------|
| `math.abs` | Absolute value |
| `math.acos` | Arc cosine |
| `math.asin` | Arc sine |
| `math.atan` | Arc tangent |
| `math.atan2` | Two-argument arc tangent |
| `math.ceil` | Ceiling |
| `math.cos` | Cosine |
| `math.cosh` | Hyperbolic cosine |
| `math.deg` | Radians to degrees |
| `math.exp` | Exponential |
| `math.floor` | Floor |
| `math.fmod` | Float modulo |
| `math.frexp` | Decompose float |
| `math.huge` | Infinity constant |
| `math.ldexp` | Scale by power of 2 |
| `math.log` | Natural logarithm |
| `math.log10` | Base-10 logarithm |
| `math.max` | Maximum |
| `math.min` | Minimum |
| `math.mod` | Deprecated alias for fmod (enabled by default via `LUA_COMPAT_MOD`) |
| `math.modf` | Integer and fractional parts |
| `math.pi` | Pi constant |
| `math.pow` | Power |
| `math.rad` | Degrees to radians |
| `math.random` | Random number |
| `math.randomseed` | Set random seed |
| `math.sin` | Sine |
| `math.sinh` | Hyperbolic sine |
| `math.sqrt` | Square root |
| `math.tan` | Tangent |
| `math.tanh` | Hyperbolic tangent |

### I/O Library (`stdlib/io.rs`)

| Function | Notes |
|----------|-------|
| `io.close` | Close file |
| `io.flush` | Flush output |
| `io.input` | Set/get default input |
| `io.lines` | Line iterator |
| `io.open` | Open file |
| `io.output` | Set/get default output |
| `io.popen` | Open process (platform-dependent) |
| `io.read` | Read from default input |
| `io.tmpfile` | Create temporary file |
| `io.type` | Check file handle type |
| `io.write` | Write to default output |
| File methods | `:close`, `:flush`, `:lines`, `:read`, `:seek`, `:setvbuf`, `:write` |
| `io.stdin` | Standard input file handle |
| `io.stdout` | Standard output file handle |
| `io.stderr` | Standard error file handle |

### OS Library (`stdlib/os.rs`)

| Function | Notes |
|----------|-------|
| `os.clock` | CPU time |
| `os.date` | Date formatting |
| `os.difftime` | Time difference |
| `os.execute` | Run shell command |
| `os.exit` | Exit process |
| `os.getenv` | Environment variable |
| `os.remove` | Delete file |
| `os.rename` | Rename file |
| `os.setlocale` | Set locale |
| `os.time` | Current time |
| `os.tmpname` | Temporary file name |

### Debug Library (`stdlib/debug.rs`)

| Function | Notes |
|----------|-------|
| `debug.debug` | Interactive debug prompt |
| `debug.getfenv` | Get environment |
| `debug.gethook` | Get hook function |
| `debug.getinfo` | Function information |
| `debug.getlocal` | Local variable value |
| `debug.getmetatable` | Raw metatable |
| `debug.getregistry` | Registry table |
| `debug.getupvalue` | Upvalue value |
| `debug.setfenv` | Set environment |
| `debug.sethook` | Set hook function |
| `debug.setlocal` | Set local variable |
| `debug.setmetatable` | Set metatable |
| `debug.setupvalue` | Set upvalue |
| `debug.traceback` | Stack traceback |

### Package Library (`stdlib/package.rs`)

| Function/Field | Notes |
|----------------|-------|
| `require` | Module loader (registered as global) |
| `module` | Create module (registered as global) |
| `package.config` | Directory/path separator configuration string |
| `package.cpath` | C module search path |
| `package.loaded` | Cache of loaded modules |
| `package.loaders` | Ordered list of module searchers |
| `package.loadlib` | Load C module |
| `package.path` | Lua module search path |
| `package.preload` | Pre-registered module loaders |
| `package.seeall` | Set module environment to globals |

## Loading

Libraries are loaded via `Lua::new()` (all standard libraries) or
selectively:

```rust
let mut lua = Lua::new_empty();
lua.open_base()?;
lua.open_string()?;
lua.open_table()?;
lua.open_math()?;
// io, os, debug, package omitted (sandboxed)
```

## Implementation Priority

1. **Base library** (with coroutine) — required for any Lua program
2. **String library** — heavily used, pattern matching is complex
3. **Table library** — common operations
4. **Math library** — straightforward wrappers around `f64` methods
5. **I/O library** — file operations
6. **OS library** — system operations
7. **Package library** — module system
8. **Debug library** — introspection, lowest priority
