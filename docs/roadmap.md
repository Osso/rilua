# Implementation Roadmap

Step-by-step build order for rilua. Each phase produces testable
output. Later phases depend on earlier ones.

## Phase 1: Core Data Structures

**Goal**: Val enum, GC arena, string interning, basic table.

### 1a. Value Representation (`vm/value.rs`)

Implement the `Val` enum and `GcRef` index type.

- `Val::Nil`, `Val::Boolean(bool)`, `Val::Number(f64)`
- `Val::String(GcRef<LuaString>)`, `Val::Table(GcRef<Table>)`
- `Val::Function(GcRef<Closure>)`, `Val::UserData(GcRef<UserData>)`
- `Val::Thread(GcRef<Thread>)`
- Equality: NaN != NaN, -0.0 == +0.0, string pointer equality
- Hashing: must be consistent with equality (hash -0.0 same as +0.0)
- Truthiness: nil and false are falsy, everything else is truthy
- Display: `"%.14g"` for numbers

**Test**: Unit tests for equality, hashing, truthiness, display.

### 1b. GC Arena (`vm/gc/arena.rs`)

Implement typed arenas with generational indices.

- `Arena<T>` storing `Vec<Option<(T, Generation)>>`
- `GcRef<T>` as `(usize, Generation)` index
- Allocate, get, get_mut, free operations
- Generation check prevents use-after-free
- Tri-color marking support (white/gray/black)

**Test**: Unit tests for alloc/free/generation checks.

### 1c. String Interning (`vm/string.rs`)

Implement `LuaString` with interning and cached hash.

- String table (hash set of `GcRef<LuaString>`)
- Hash: PUC-Rio's string hash algorithm
- Intern on creation: return existing ref if found
- Pointer equality for interned strings
- Minimum string table size: 32

**Test**: Unit tests for interning, pointer equality, hash.

### 1d. Table (`vm/table.rs`)

Implement `Table` with array + hash dual representation.

- Array part: `Vec<Val>` for integer keys 1..=n
- Hash part: open-addressing with chained scatter (Brent's variation)
- Power-of-2 hash part size with `lastfree` backward scan
- `get`, `set`, `next`, `len` operations
- NaN key error, nil key error
- Integer-float key equivalence (1.0 maps to array[0])
- Resize with >50% occupancy heuristic
- `dummynode` sentinel for empty hash part

**Test**: Unit tests for get/set, array/hash split, resize, len,
iteration order, edge cases (NaN, -0.0, integer floats).

## Phase 2: Compilation Pipeline

**Goal**: Lex, parse, and compile Lua source to Proto bytecode.

### 2a. Token Types (`compiler/token.rs`)

Define token enum: 21 keywords, 6 multi-char operators, 3 literals,
single-char tokens as byte values, end-of-stream.

**Test**: Unit tests for token construction.

### 2b. Lexer (`compiler/lexer.rs`)

Implement tokenizer with one-token lookahead.

- Character scanning with `peek()`/`advance()`
- Keyword recognition via string matching
- Number parsing: decimal, hex, float, scientific
- String parsing: short strings with 11 escape sequences, long strings
  with bracket notation and newline normalization
- Comment handling: short (`--`) and long (`--[==[]==]`)
- Source position tracking (line, last_line)

**Test**: Unit tests for each token type, all escape sequences,
all number formats, long strings at multiple bracket levels,
error cases (unterminated string, invalid escape).

### 2c. AST Types (`compiler/ast.rs`)

Define AST node enums: 13 statement variants, 14 expression variants,
supporting types (Block, BinOp, UnOp, Field, FuncBody, FuncName, Span).

**Test**: Construction tests (AST types are data, not logic).

### 2d. Parser (`compiler/parser.rs`)

Implement recursive descent parser producing AST.

- Statement dispatch by leading token
- Pratt expression parsing with left/right priority table
- Right-associativity for `^` and `..` (left priority > right priority)
- Operator precedence: or=1, and=2, cmp=3, concat=4, add=5, mul=6,
  unary=7, pow=8
- Assignment vs function call disambiguation
- For loop disambiguation (numeric vs generic, decided at `=` vs `in`)
- Error reporting with source locations

**Test**: Parse every statement type, every expression type, operator
precedence and associativity, error messages for malformed input.

### 2e. Instruction Types (`vm/instructions.rs`)

Define `Instruction` as u32 with PUC-Rio's encoding.

- Opcode enum: 38 opcodes (MOVE through VARARG)
- Three formats: iABC (A:8, B:9, C:9), iABx (A:8, Bx:18), iAsBx (signed)
- RK encoding: bit 8 set = constant index, clear = register
- Field extraction: `get_opcode`, `get_a`, `get_b`, `get_c`, `get_bx`,
  `get_sbx`

**Test**: Round-trip encode/decode for each format and opcode.

### 2f. Proto (`vm/proto.rs`)

Define `Proto` struct: code, constants, nested protos, debug info,
metadata.

**Test**: Construction tests.

### 2g. Compiler (`compiler/codegen.rs`)

Implement AST-to-Proto code generation.

- `FuncState` per function: freereg, nactvar, locals, upvalues, constants
- Variable resolution: locals (register), upvalues (chain), globals (name)
- Register allocation via `freereg` counter
- Constant pool with RK optimization (constants <= 255 encode inline)
- Jump backpatching via linked lists through JMP sBx fields
- Expression discharge to register, anyreg, or RK
- Code generation for each statement type
- Upvalue resolution with `markupval` for OP_CLOSE
- OP_CLOSURE pseudo-instructions (MOVE for locals, GETUPVAL for chained)
- Debug info: line numbers, local variable scopes

**Test**: Compile simple programs and verify bytecode output.
Test each statement type. Test upvalue capture chains.

## Phase 3: Core VM

**Goal**: Execute bytecode. Run simple Lua programs end-to-end.

### 3a. Call Stack (`vm/callinfo.rs`)

Implement `CallInfo` struct and call stack management.

- Fields: func, base, top, saved_pc, num_results, tail_calls
- Push/pop CallInfo on call/return
- Stack growth with limits (MAXCALLS=20000, MAXCCALLS=200)

**Test**: Unit tests for push/pop, limit enforcement.

### 3b. Closures and Upvalues (`vm/closure.rs`)

Implement `Closure` (Lua and Rust variants) and `UpVal`.

- Lua closure: Proto reference + upvalue array
- Rust closure: function pointer + upvalue array
- Open upvalues: point into stack, linked list per thread
- Close upvalues: copy value, detach from stack
- OP_CLOSE triggers close for registers >= target

**Test**: Unit tests for open/close upvalue lifecycle.

### 3c. VM State (`vm/state.rs`)

Implement `LuaState` / VM state.

- Value stack: `Vec<Val>` with base/top tracking
- Call stack: `Vec<CallInfo>` with ci index
- Global table, registry, type metatables
- GC integration points

**Test**: State construction, stack operations.

### 3d. Execution Loop (`vm/execute.rs`)

Implement instruction dispatch for all 38 opcodes.

- Register-based dispatch: decode instruction, execute, advance PC
- Arithmetic: f64 operations with string-to-number coercion
- Comparison: type-aware with string lexicographic ordering
- Table access: OP_GETTABLE, OP_SETTABLE with metamethod dispatch
- Function calls: OP_CALL, OP_TAILCALL, OP_RETURN
- Control flow: OP_JMP, OP_FORLOOP, OP_FORPREP, OP_TFORLOOP
- Upvalues: OP_GETUPVAL, OP_SETUPVAL, OP_CLOSE
- Closures: OP_CLOSURE with pseudo-instruction reading

**Test**: End-to-end tests. Compile and run Lua programs:
- Arithmetic: `assert(1 + 2 == 3)`
- String concat: `assert("a" .. "b" == "ab")`
- Tables: `local t = {1,2,3}; assert(t[2] == 2)`
- Functions: `local function f(x) return x+1 end; assert(f(1)==2)`
- Closures: `local x=1; local f=function() return x end; assert(f()==1)`
- Control flow: while, for, if/elseif/else, repeat/until, break
- Multiple return: `local a,b = (function() return 1,2 end)()`

## Phase 4: Language Features

**Goal**: Metatables, error handling, varargs, environments.

### 4a. Metatable Dispatch

Implement all 17 metamethods in the execution loop.

- Arithmetic: `__add`, `__sub`, `__mul`, `__div`, `__mod`, `__pow`, `__unm`
- Comparison: `__eq`, `__lt`, `__le` (with `__lt` fallback for `__le`)
- Indexing: `__index` (table or function chain, MAXTAGLOOP=100),
  `__newindex` (table or function chain)
- Other: `__concat`, `__len`, `__call`, `__gc`, `__mode`
- Fast-path caching: flags byte in metatable for events 0-4
- Type metatables: per-type shared metatables in global state
- String metatable: `{__index = string_lib}`

**Test**: Test each metamethod. Test chaining. Test fast-path cache
invalidation. Test type-specific metatables.

### 4b. Error Handling

Implement Result-based error propagation, pcall, xpcall.

- `LuaError` type: runtime, syntax, memory, error-in-error-handler
- Protected calls: save/restore VM state on error
- `error(msg, level)`: create error with stack info
- `pcall(f, ...)`: returns true+results or false+error
- `xpcall(f, handler)`: error handler receives error object
- Stack traceback generation

**Test**: pcall catching errors, xpcall with custom handler, nested
pcall, error level parameter, stack traceback format.

### 4c. Vararg Functions

Implement `...` and the `arg` table.

- `OP_VARARG`: copy vararg values to registers
- Stack layout: fixed params moved above vararg area
- `VARARG_HASARG`, `VARARG_ISVARARG`, `VARARG_NEEDSARG` flags
- Legacy `arg` table creation when `VARARG_NEEDSARG` is set

**Test**: Vararg passing, select with varargs, `...` in nested
functions.

### 4d. Environments

Implement function environments (`setfenv`/`getfenv`).

- Each closure has an environment table
- `setfenv(f, t)`: change function's global lookup table
- `getfenv(f)`: retrieve function's environment
- Level 0 = thread environment, level N = Nth stack frame
- OP_GETGLOBAL/OP_SETGLOBAL use the closure's environment

**Test**: setfenv/getfenv, environment inheritance, level parameter.

## Phase 5: Standard Libraries

**Goal**: Implement all 9 standard libraries.

### 5a. Base Library (`stdlib/base.rs`)

Priority 1. Implement all base library functions.

Key functions: `print`, `type`, `tostring`, `tonumber`, `assert`,
`error`, `pcall`, `xpcall`, `select`, `unpack`, `pairs`, `ipairs`,
`next`, `rawget`, `rawset`, `rawequal`, `getmetatable`, `setmetatable`,
`getfenv`, `setfenv`, `collectgarbage`, `load`, `loadstring`,
`loadfile`, `dofile`, `newproxy`.

Globals: `_G`, `_VERSION` ("Lua 5.1").

**Test**: One integration test per function.

### 5b. String Library (`stdlib/string.rs`)

Priority 2. Pattern matching is the most complex part.

Implement pattern engine first: character classes (`%a` through `%z`),
bracket classes, quantifiers (`*`, `+`, `-`, `?`), anchors (`^`, `$`),
captures (up to 32), position captures `()`, back-references
(`%1`-`%9`), balanced match `%b`, frontier `%f[set]`.

Then implement: `find`, `match`, `gmatch`, `gsub`, `format` (with
`%d`, `%i`, `%o`, `%u`, `%x`, `%X`, `%e`, `%E`, `%f`, `%g`, `%G`,
`%s`, `%q`, `%c`, `%%`), `byte`, `char`, `len`, `sub`, `rep`,
`reverse`, `lower`, `upper`, `dump`.

Set up string metatable: `{__index = string_lib}`.

**Test**: Pattern matching edge cases, format specifiers, string
methods via method syntax (`s:upper()`).

### 5c. Table Library (`stdlib/table.rs`)

Priority 3.

Implement: `concat`, `insert`, `remove`, `sort` (quicksort with
median-of-three, error on invalid comparison), `maxn`.

Deprecated: `foreach`, `foreachi`, `getn`, `setn` (raises error).

**Test**: Sort stability edge cases, NaN in sort, insert/remove
shifting, concat with separator.

### 5d. Math Library (`stdlib/math.rs`)

Priority 4. Wraps Rust f64 methods.

Implement all functions (abs, acos, asin, atan, atan2, ceil, cos,
cosh, deg, exp, floor, fmod, frexp, ldexp, log, log10, max, min,
modf, pow, rad, random, randomseed, sin, sinh, sqrt, tan, tanh).

Constants: `math.pi`, `math.huge`. Alias: `math.mod` = `math.fmod`.

Random: `rand()` equivalent, `randomseed` for deterministic sequences.

**Test**: Edge cases (NaN arguments, infinity, -0.0), random
determinism with seed.

### 5e. I/O Library (`stdlib/io.rs`)

Priority 5. File handles as userdata with metatables.

Implement: `open`, `close`, `read`, `write`, `lines`, `flush`,
`input`, `output`, `type`, `tmpfile`, `popen`.

File methods: `:read`, `:write`, `:lines`, `:seek`, `:setvbuf`,
`:close`, `:flush`.

Handles: `io.stdin`, `io.stdout`, `io.stderr`.

**Test**: File read/write, line iteration, seek, standard handles.

### 5f. OS Library (`stdlib/os.rs`)

Priority 6.

Implement: `clock`, `date`, `difftime`, `execute`, `exit`, `getenv`,
`remove`, `rename`, `setlocale`, `time`, `tmpname`.

**Test**: clock monotonicity, date formatting, execute return values.

### 5g. Package Library (`stdlib/package.rs`)

Priority 7. Module system.

Implement: `require`, `module`, `package.loaded`, `package.path`,
`package.cpath`, `package.preload`, `package.loaders`,
`package.loadlib`, `package.seeall`, `package.config`.

Four default loaders: preload, Lua file, C library, all-in-one C.

**Test**: require with preload, path searching, loaded cache,
circular require detection.

### 5h. Debug Library (`stdlib/debug.rs`)

Priority 8. Introspection.

Implement: `getinfo`, `getlocal`, `setlocal`, `getupvalue`,
`setupvalue`, `gethook`, `sethook`, `traceback`, `getfenv`,
`setfenv`, `getmetatable`, `setmetatable`, `getregistry`, `debug`.

Hook events: call, return, line, count.

**Test**: getinfo fields, local variable access, hook callbacks,
traceback format.

## Phase 6: Coroutines

**Goal**: Cooperative multithreading via coroutines.

Implement `lua_State` as a thread with its own stack and call stack
but sharing the GC heap with all other threads.

- `coroutine.create`: new thread from Lua function
- `coroutine.resume`: first resume calls body, subsequent resumes
  continue from yield point
- `coroutine.yield`: suspend with values, return to resume caller
- `coroutine.wrap`: iterator-style wrapper
- `coroutine.status`: running/suspended/normal/dead
- `coroutine.running`: current thread or nil for main
- Yield restriction: nCcalls > 0 blocks yield
- GC: threads on grayagain list, re-traversed atomically
- `lua_xmove`: transfer values between threads

**Test**: Basic resume/yield, argument passing, error propagation,
nested coroutines, wrap iterator, status transitions, GC of
suspended coroutines.

## Phase 7: GC Collector

**Goal**: Mark-sweep garbage collection.

### 7a. Mark Phase

- Root marking: main thread, global table, registry, type metatables
- Propagation: gray -> black, mark referenced objects
- Tri-color invariant with write barriers
- Thread special case: always re-gray (grayagain list)

### 7b. Sweep Phase

- Walk rootgc list, free white (dead) objects
- Flip white bit for next cycle
- String table sweep (separate from rootgc)
- Open upvalue sweep per thread

### 7c. Atomic Phase

- Remark upvalues, propagate remaining grays
- Process weak tables (clear dead keys/values)
- Process finalizers (__gc)

### 7d. Incremental Collection

- `collectgarbage("step", n)`: single step
- `collectgarbage("collect")`: full cycle
- Pause and step multiplier tuning
- GC debt tracking

**Test**: Cycle collection, weak table clearing, finalizer execution
order, incremental step correctness, collectgarbage API.

## Phase 8: Public API

**Goal**: Rust-idiomatic embedding API.

- `Lua` struct: owns all state
- `IntoLua`/`FromLua` traits for type conversion
- `IntoLuaMulti`/`FromLuaMulti` for multiple values
- `Table`, `Function`, `Thread` handle types
- `UserData` trait for custom Rust types
- `Lua::new()`, `Lua::exec()`, `Lua::load()`, `Lua::global()`
- Error types with source locations
- Selective library loading for sandboxing

**Test**: Embedding examples from api.md, type conversion round-trips,
error propagation across Rust/Lua boundary.

## Phase 9: Compatibility

**Goal**: Pass PUC-Rio's official test suite.

- Run `~/Repos/github.com/lua/tests` (tag v5_1_1) verbatim
- Fix behavioral differences found by test failures
- Match error message formats
- Match number formatting (`"%.14g"`)
- Match table traversal order
- Match GC observable behavior (finalizer order, weak table clearing)

## Dependencies

```text
Phase 1 (data structures)
  |
  v
Phase 2 (compilation) -----> Phase 3 (core VM)
                                |
                                v
                    Phase 4 (language features)
                                |
                                v
                    Phase 5 (standard libraries)
                                |
                    Phase 6 (coroutines)
                                |
                    Phase 7 (GC collector)
                                |
                    Phase 8 (public API)
                                |
                    Phase 9 (compatibility)
```

Phases 5-7 can be developed in parallel once Phase 4 is complete.
Phase 8 can start once Phase 5 is functional. Phase 9 is ongoing
throughout but becomes the focus after Phase 8.

## Milestones

| Milestone | Criteria |
|-----------|----------|
| First bytecode | Phase 2 complete: compile Lua to Proto |
| First execution | Phase 3 complete: run `print("hello")` |
| Language complete | Phase 4 complete: all Lua 5.1.1 semantics |
| Stdlib complete | Phase 5 complete: all 9 libraries |
| Embeddable | Phase 8 complete: Rust API functional |
| Compatible | Phase 9: PUC-Rio test suite passing |
