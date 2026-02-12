# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to
[Conventional Commits](https://www.conventionalcommits.org/en/v1.0.0/).

## [Unreleased]

### Added

- Full Lua 5.1.1 compilation pipeline: lexer, parser, AST, code generator
  producing PUC-Rio-compatible bytecode (38 register-based opcodes)
- Register-based VM with instruction dispatch, call stack (CallInfo),
  closures with open/closed upvalue model, tail call optimization
- Arena-based mark-sweep GC with generational indices, incremental
  collection (5-state machine), write barriers, `__gc` finalizers
- Value representation: 9-variant enum (Nil, Boolean, Number, String,
  Table, Function, UserData, Thread, LightUserData)
- String interning with PUC-Rio hash algorithm and cached hash values
- Table implementation: array + hash dual representation, Brent's
  collision resolution, 3-phase resize with optimal array/hash split
- Metatables and metamethods: arithmetic, comparison, concat, len,
  `__index`/`__newindex` (table and function), `__call`, `__eq`,
  `__lt`, `__le`, `__gc`, `__tostring`
- Protected calls: `pcall`, `xpcall` with Result-based error propagation
- Coroutines: create, resume, yield, wrap, status, running
- Standard library (all Lua 5.1.1 functions):
  - Base: 28 functions including print, assert, type, tostring, tonumber,
    pcall, xpcall, error, setmetatable, getmetatable, select, unpack,
    loadstring, loadfile, dofile, load, pairs, ipairs, next, rawget,
    rawset, rawequal, setfenv, getfenv, collectgarbage, newproxy
  - String: 14 functions (len, byte, char, sub, rep, reverse, lower,
    upper, format, find, match, gmatch, gsub, dump) with pattern
    matching engine
  - Table: 9 functions (concat, insert, remove, sort, maxn, getn, setn,
    foreach, foreachi) with PUC-Rio's median-of-three quicksort
  - Math: 28 functions (abs through tanh), math.pi, math.huge constants,
    glibc-compatible LCG random number generator
  - I/O: 11 library functions, 7 file methods, 3 standard handles,
    lines iterator with auto-close, libc FFI for C stdio
  - OS: 11 functions (clock, date, difftime, execute, exit, getenv,
    remove, rename, setlocale, time, tmpname) via libc FFI
  - Debug: 14 functions (getinfo, getlocal, setlocal, getupvalue,
    setupvalue, traceback, getregistry, getmetatable, setmetatable,
    getfenv, setfenv, gethook, sethook, debug)
  - Package: require, module, 4 loaders (preload, Lua file, 2 C stubs),
    path searching, package.loaded/preload/loaders/config/path/cpath
- Userdata infrastructure: typed arena, per-instance metatables,
  `__gc` support, registry metatable helpers
- Public Rust embedding API: `Lua` struct, `IntoLua`/`FromLua` traits,
  `Table`/`Function`/`Thread`/`AnyUserData` handle types, `StdLib`
  bitflags for selective library loading
- CLI interpreter (`rilua`): PUC-Rio `lua.c`-compatible command line
  interface with `-e`, `-l`, `-i`, `-v`, `--` flags, `LUA_INIT` env
  var, `arg` table, REPL with multiline detection
- Bytecode compiler/lister (`riluac`): `-l`/`-l -l`/`-p`/`-v`/`-o`/`-s`
  flags, binary chunk output, multiple file combining
- Bytecode serialization: `string.dump` and binary chunk loading,
  cross-compatible with PUC-Rio (byte-identical output)
- Byte-based source loading (`&[u8]` pipeline) for non-UTF-8 support
- Error message formatting with variable name resolution: `getobjname`,
  `symbexec`, `getfuncname` ported from PUC-Rio's `ldebug.c`. Messages
  include variable kind and name (local, global, field, upvalue, method)
- CLI stack tracebacks on unhandled errors via `call_function_traced`
- `LUA_COMPAT_VARARG`: implicit `arg` table for old-style vararg functions
- `LUA_COMPAT_GFIND`: `string.gfind` as alias for `string.gmatch`
- Architecture documentation in `docs/` (14 documents covering pipeline,
  instructions, values, GC, tables, strings, closures, call stack,
  metatables, errors, API, stdlib, coroutines, testing)
- 1262 tests: 583 unit + 402 integration + 277 oracle comparison
- PUC-Rio test suite: 8/23 files pass (api, checktable, code, files,
  gc, locals, pm, sort)

### Changed

- Complete rewrite from scratch. Previous implementation (based on
  lua-in-rust) replaced with new architecture:
  - AST-based pipeline (was: single-pass bytecode emission)
  - Register-based VM with PUC-Rio opcodes (was: stack-based custom opcodes)
  - Arena GC with generational indices (was: raw-pointer mark-sweep)
  - Trait-based API (was: stack-based C API mirror)

### Fixed

- Or-shortcircuit in EQ operand causing infinite loop
- Parser accepting bare names as statements
- Vararg table constructor capturing only first argument
- Mixed named params + varargs register misassignment
- Closure + for-loop break not closing upvalues
- Table constructor named field constant index overflow (9-bit truncation)
- `load(func_reader)` hang with infinite readers (added size limit)
- Non-ASCII bytes in pattern matching bracket classes
- `%z` character class not matching null bytes
- `gsub` `%n` replacement with position captures
- `%b` balanced match off-by-one
- Backref `%1` matching pointer offset
- `gsub` table replacement not invoking `__index` metamethods
- `debug.getinfo` activelines not populated
- `lastlinedefined` always 0 in function prototypes
- Shebang line handling in source files
- String coercion in stdlib functions
