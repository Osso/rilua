# Implementation Roadmap

Step-by-step build order for rilua. Each chunk produces compilable,
tested code. Later chunks depend on earlier ones. Testing is
integrated at every step.

## Current Status

| Phase | Status | Tests |
|-------|--------|-------|
| 0: Skeleton + Test Infra | Done | Quality gate passes, oracle helpers work |
| 1: Core Data Structures | Done | 143 unit tests + 10 oracle |
| 2: Compilation Pipeline | Done | 354 unit tests + 10 oracle, bytecode matches `luac -l` |
| 3: Core VM | Done | 466 total (431 unit + 16 integration + 19 oracle) |
| 4: Language Features | Not started | -- |
| 5: Standard Libraries | Not started | -- |
| 6: Coroutines | Not started | -- |
| 7: GC Collector | Not started | -- |
| 8: Public API + CLI | Not started | -- |
| 9: Compatibility | Not started | -- |

Phase 3 audit found and fixed 9 bugs across the compiler and VM.
60/60 oracle test cases pass against PUC-Rio Lua 5.1.1. The full
quality gate (`cargo fmt -- --check && cargo clippy --all-targets &&
cargo test && cargo doc --no-deps`) passes clean.

## Reference Tools

PUC-Rio Lua 5.1.1 serves as the oracle for behavioral equivalence
testing throughout development.

- **lua** (interpreter): `~/Repos/github.com/lua/lua/lua` (git tag
  `v5.1.1`)
- **luac** (bytecode lister): built from the 5.1.1 source
  distribution. Use `luac -l` for bytecode listing, `luac -l -l` for
  listing with constants and locals.
- **Test suite**: `~/Repos/github.com/lua/tests/` (24 files, tag
  `v5_1_1`)

See `CLAUDE.md` for distribution archive URLs and SHA256 checksums.

## Testing Integration

Four testing categories activate at different stages:

| Category | Activates after | How |
|----------|----------------|-----|
| Unit tests | Phase 0 | `#[cfg(test)]` in each module, `cargo test --lib` |
| Bytecode comparison | Phase 2 | Compare rilua compiler output with `luac -l` |
| Oracle comparison | Phase 3 | Run same Lua code in rilua and PUC-Rio `lua -e`, compare stdout/stderr |
| PUC-Rio test suite | Phase 5a | Official test files pass as features are implemented |

Every chunk lists its specific tests. See `docs/testing.md` for the
full testing strategy including the oracle comparison framework and
progressive PUC-Rio test suite unlocking.

## Phase 0: Project Skeleton + Test Infrastructure [Done]

**Goal**: Directory structure, error types, test helpers. Everything
compiles and the quality gate passes.

### 0a. Module skeleton and error types

Create all module files from the architecture layout with minimal
content (empty modules or placeholder types). Implement `LuaError`
enum and `LuaResult<T>` type alias.

Files: `src/error.rs`, all `mod.rs` files per architecture layout.

**Tests**: `cargo build` succeeds, quality gate passes. Unit tests
for error type construction and Display impl.

### 0b. Test infrastructure

Set up the test helper framework for oracle comparison and integration
test running.

Files: `tests/helpers/mod.rs`, `tests/helpers/oracle.rs`.

**Tests**: Helper functions compile. Reference `lua` binary is
callable. `run_reference("print(1+1)")` returns `"2\n"`.

## Phase 1: Core Data Structures [Done]

**Goal**: Val enum, GC arena, string interning, basic table.

### 1a. GC arena and GcRef

Implement typed arenas with generational indices.

- `Arena<T>` storing `Vec<Option<(T, Generation)>>`
- `GcRef<T>` as `(usize, Generation)` index
- Allocate, get, get_mut, free operations
- Generation check prevents use-after-free
- Tri-color marking support (white/gray/black)

Files: `src/vm/gc/arena.rs`, `src/vm/gc/trace.rs`, `src/vm/gc/mod.rs`.

**Tests**: Unit tests for alloc/free/generation checks, stale ref
detection, color transitions.

### 1b. Value representation

Implement the `Val` enum and core value operations.

- `Val::Nil`, `Val::Boolean(bool)`, `Val::Number(f64)`
- `Val::String(GcRef<LuaString>)`, `Val::Table(GcRef<Table>)`
- `Val::Function(GcRef<Closure>)`, `Val::UserData(GcRef<UserData>)`
- `Val::Thread(GcRef<Thread>)`
- Equality: NaN != NaN, -0.0 == +0.0, string pointer equality
- Hashing: consistent with equality (hash -0.0 same as +0.0)
- Truthiness: nil and false are falsy, everything else is truthy
- Display: `"%.14g"` for numbers

Files: `src/vm/value.rs`.

**Tests**: Unit tests for equality edge cases (NaN, -0.0, mixed
types), hashing consistency, truthiness, display formatting.

### 1c. String interning

Implement `LuaString` with interning and cached hash.

- `LuaString` struct: hash (u32) + data (Box<[u8]>)
- `StringTable`: bucket array with power-of-2 sizing
- Hash: PUC-Rio's step-based hash algorithm
- Intern on creation: return existing ref if found
- Pointer equality for interned strings
- Minimum table size: 32 buckets
- Resize at 100% load factor

Files: `src/vm/string.rs`.

**Tests**: Unit tests for hash algorithm (verify against known PUC-Rio
hash values), interning (same content returns same ref), pointer
equality, resize trigger, empty string handling.

### 1d. Table: basic operations

Implement `Table` with array + hash dual representation, basic
get/set.

- Array part: `Vec<Val>` for integer keys 1..=n
- Hash part: open-addressing with Brent's collision resolution
- Power-of-2 hash part size with `lastfree` backward scan
- `get` and `set` operations
- NaN key error, nil key error
- Integer-float key equivalence (1.0 maps to array[0])
- `dummynode` sentinel for empty hash part

Files: `src/vm/table.rs`.

**Tests**: Unit tests for get/set with integer and string keys,
array/hash split, NaN key rejection, nil key rejection, integer-float
equivalence.

### 1e. Table: resize and Brent's algorithm

Implement full hash collision resolution and resize.

- Brent's main position displacement
- Resize with >50% array occupancy heuristic
- 3-phase resize: count keys, compute optimal split, rehash
- `lastfree` backward scan for free positions

Files: `src/vm/table.rs` (extends 1d).

**Tests**: Unit tests for collision resolution, resize trigger,
array/hash split after resize, large table stress test.

### 1f. Table: iteration and length

Implement `next()` traversal and the length operator.

- `next(table, key)`: array-first, then hash-part traversal
- Length operator (`#`): binary search for boundary in array part,
  fallback to hash part boundary detection
- Traversal order must match PUC-Rio for behavioral equivalence

Files: `src/vm/table.rs` (extends 1e).

**Tests**: Unit tests for next() traversal order, length operator
edge cases (sparse arrays, mixed keys, empty table), boundary
detection matching PUC-Rio.

## Phase 2: Compilation Pipeline [Done]

**Goal**: Lex, parse, and compile Lua source to Proto bytecode.
Bytecode comparison tests verify output matches `luac -l`.

### 2a. Token types

Define the token enum.

- 21 keywords, 6 multi-char operators, 3 literal types
- Single-char tokens as byte values
- End-of-stream sentinel
- Source position (line number)

Files: `src/compiler/token.rs`, `src/compiler/mod.rs`.

**Tests**: Unit tests for token construction, Display impl.

### 2b. Lexer: core scanning

Implement the tokenizer core: character scanning, whitespace,
keywords, single and multi-character operators.

- `peek()`/`advance()` character scanning
- Keyword recognition via string matching
- All operator tokens (==, ~=, <=, >=, .., ...)
- Source position tracking (line, last_line)
- One-token lookahead

Files: `src/compiler/lexer.rs`.

**Tests**: Unit tests for keyword recognition, all operator tokens,
whitespace handling, position tracking.

### 2c. Lexer: strings, numbers, comments

Complete the lexer: string parsing, number parsing, comment handling.

- Short strings with 11 escape sequences (\a \b \f \n \r \t \v
  \\ \" \' \ddd)
- Long strings with bracket notation `[==[...]==]` and newline
  normalization
- Number parsing: decimal, hex (0x), float, scientific notation
- Short comments (`--`) and long comments (`--[==[]==]`)

Files: `src/compiler/lexer.rs` (extends 2b).

**Tests**: Unit tests for every escape sequence, long strings at
multiple bracket levels, all number formats (decimal, hex, float,
exponent), unterminated string errors, invalid escape errors, nested
long comments.

### 2d. AST types

Define AST node enums.

- 13 statement variants (Assign, LocalAssign, Do, While, Repeat,
  If, NumericFor, GenericFor, Return, Break, FunctionDef, MethodDef,
  Call)
- 14 expression variants (Nil, True, False, Number, String, Vararg,
  BinOp, UnOp, Ident, Index, Field, MethodCall, FunctionCall,
  FunctionDef)
- Supporting types: Block, BinOp, UnOp, Field, FuncBody, FuncName,
  Span

Files: `src/compiler/ast.rs`.

**Tests**: Construction tests (AST types are data, not logic).

### 2e. Parser: statements

Implement statement parsing in the recursive descent parser.

- Statement dispatch by leading token
- Assignment, local declaration, do/end, while, repeat/until
- if/elseif/else, numeric for, generic for
- return, break
- Function call as statement
- Assignment vs function call disambiguation

Files: `src/compiler/parser.rs`.

**Tests**: Parse every statement type, verify AST structure,
error messages for malformed input, assignment disambiguation.

### 2f. Parser: expressions

Implement expression parsing with Pratt precedence.

- Pratt parsing with left/right priority table
- Right-associativity for `^` and `..`
- Precedence: or=1, and=2, cmp=3, concat=4, add=5, mul=6,
  unary=7, pow=8
- Table constructors: `{1, 2, a=3, [expr]=4}`
- Function calls and method calls
- Function definitions (named, anonymous, method syntax)
- Parenthesized expressions, varargs

Files: `src/compiler/parser.rs` (extends 2e).

**Tests**: Operator precedence, associativity, table constructors,
function calls with string/table shorthand, method syntax, varargs
in correct positions, error cases.

### 2g. Instruction types and Proto

Define opcodes and the function prototype container.

- `Instruction` as u32 with PUC-Rio's encoding
- Opcode enum: 38 opcodes (MOVE through VARARG)
- Three formats: iABC (A:8, B:9, C:9), iABx (A:8, Bx:18),
  iAsBx (signed)
- RK encoding: bit 8 set = constant index, clear = register
- Field extraction/construction helpers
- `Proto` struct: code, constants, nested protos, debug info,
  metadata

Files: `src/vm/instructions.rs`, `src/vm/proto.rs`.

**Tests**: Round-trip encode/decode for each format. Every opcode
encodes and decodes correctly. Proto construction.

### 2h. Compiler: variable resolution and constants

Implement the first half of code generation: FuncState, variable
resolution, register allocation, constant pool.

- `FuncState` per function: freereg, nactvar, locals, upvalues,
  constants
- Variable resolution: locals (register), upvalues (chain),
  globals (name)
- Register allocation via `freereg` counter
- Constant pool with RK optimization (constants <= 255 inline)
- Expression discharge to register, anyreg, or RK

Files: `src/compiler/codegen.rs`.

**Tests**: Unit tests for variable resolution, register allocation,
constant pool deduplication, RK encoding decisions.

### 2i. Compiler: full code generation

Implement code generation for all statement and expression types.

- Code generation for each statement type
- Jump backpatching via linked lists through sBx fields
- Upvalue resolution with `markupval` for OP_CLOSE
- OP_CLOSURE pseudo-instructions (MOVE for locals, GETUPVAL for
  chained upvalues)
- Debug info: line numbers, local variable scopes

Files: `src/compiler/codegen.rs` (extends 2h).

**Tests**: Compile Lua programs and verify bytecode output.
**Bytecode comparison** tests activate here: compile snippets with
both rilua and `luac -l`, compare instruction sequences.

Test cases:
- Simple arithmetic: `local a = 1 + 2`
- Variable access: locals, globals, upvalues
- Control flow: if/while/for/repeat
- Function definitions and calls
- Upvalue capture chains
- Table constructors

## Phase 3: Core VM [Done]

**Goal**: Execute bytecode. Run simple Lua programs end-to-end.
Oracle comparison tests activate at the end of this phase.

Phase 3 audit fixed 9 bugs: invertjump target, GT/GE expression
order, VM comparison condition, boolean materialization, closure
upvalue pseudo-instructions, push_ci/pop_ci desync, TESTSET vs TEST
in jumponcond, set_multret calling set_one_ret, and multi-target
assignment register allocation.

### 3a. CallInfo and VM state

Implement the call stack and VM state container.

- `CallInfo` struct: func, base, top, saved_pc, num_results,
  tail_calls
- Push/pop CallInfo on call/return
- Stack limits: MAXCALLS=20000, MAXCCALLS=200
- `LuaState`: value stack (`Vec<Val>`), call stack
  (`Vec<CallInfo>`), global table, registry, type metatables
- GC integration points (placeholder)

Files: `src/vm/callinfo.rs`, `src/vm/state.rs`, `src/vm/mod.rs`.

**Tests**: Unit tests for CallInfo push/pop, stack limit
enforcement, state construction.

### 3b. Closures and upvalues

Implement Closure types and the upvalue lifecycle.

- Lua closure: Proto reference + upvalue array
- Rust closure: function pointer + upvalue array
- Open upvalues: point into stack, sorted linked list per thread
- Close upvalues: copy value, detach from stack
- OP_CLOSE triggers close for registers >= target

Files: `src/vm/closure.rs`.

**Tests**: Unit tests for open/close upvalue lifecycle, upvalue
sharing between closures, close operation.

### 3c. Execution loop: arithmetic, tables, control flow

Implement instruction dispatch for data movement, arithmetic, table
access, and control flow.

- Register-based dispatch: decode instruction, execute, advance PC
- MOVE, LOADK, LOADBOOL, LOADNIL
- ADD, SUB, MUL, DIV, MOD, POW, UNM (f64, no metamethods yet)
- LEN, CONCAT (no metamethods yet)
- EQ, LT, LE with TEST, TESTSET (no metamethods yet)
- JMP, FORLOOP, FORPREP, TFORLOOP
- NEWTABLE, GETTABLE, SETTABLE (raw access, no metamethods yet)
- GETGLOBAL, SETGLOBAL
- SELF
- SETLIST

Files: `src/vm/execute.rs`.

**Tests**: Unit tests for each opcode group. Execute small bytecode
sequences directly (without going through the compiler).

### 3d. Execution loop: calls, returns, closures

Implement function call machinery.

- CALL: push CallInfo, transfer arguments, enter callee
- TAILCALL: reuse CallInfo, shift arguments
- RETURN: pop CallInfo, transfer results
- CLOSURE: create closure, read pseudo-instructions (MOVE/GETUPVAL)
- GETUPVAL, SETUPVAL, CLOSE
- VARARG: copy vararg values to registers

Files: `src/vm/execute.rs` (extends 3c).

**Tests**: Unit tests for call/return protocol, tail call stack
behavior, closure creation, upvalue read/write.

### 3e. End-to-end wiring + `print`

Connect the full pipeline: lex -> parse -> compile -> execute. Wire
up a minimal `print` built-in so output can be tested.

- `Lua::exec(source)` that runs the full pipeline
- Minimal `print` function registered as a Rust closure in globals
- Wire `src/main.rs` to accept a filename argument and run it

Files: `src/lib.rs`, `src/main.rs`, `src/stdlib/mod.rs` (minimal).

**Tests**: End-to-end Lua programs:
- `print("hello world")` -- verifies full pipeline
- `print(1 + 2)` -- arithmetic
- `print("a" .. "b")` -- concatenation
- `local t = {1,2,3}; print(t[2])` -- tables
- `local function f(x) return x+1 end; print(f(1))` -- functions
- `local x=1; local f=function() return x end; print(f())` -- closures
- while, for, if/else, repeat/until, break -- control flow

**Oracle comparison tests activate here**: every test above runs in
both rilua and PUC-Rio `lua -e`, output must match.

## Phase 4: Language Features

**Goal**: Metatables, error handling, varargs, environments. After
this phase, all core Lua 5.1.1 language semantics work.

### 4a. Metatable dispatch: arithmetic and comparison

Implement metamethods for arithmetic and comparison operators.

- `__add`, `__sub`, `__mul`, `__div`, `__mod`, `__pow`, `__unm`
- `__eq`, `__lt`, `__le` (with `__lt` fallback for `__le`)
- `__concat`, `__len`
- Metamethod lookup: check left operand first, then right
- String-to-number coercion for arithmetic
- Fast-path flags byte in metatable for events 0-4
- `setmetatable` / `getmetatable` built-in support

Files: `src/vm/execute.rs` (modifies arithmetic/comparison opcodes).

**Tests**: Unit tests for each metamethod. Oracle comparison for
edge cases (mixed types, missing metamethods, coercion).

### 4b. Metatable dispatch: indexing and call

Implement indexing metamethods and `__call`.

- `__index`: table or function, chain up to MAXTAGLOOP=100
- `__newindex`: table or function, chain up to MAXTAGLOOP=100
- `__call`: invoke non-function values
- Type metatables: per-type shared metatables in global state
- String metatable: `{__index = string_lib}` (wired in Phase 5b)

Files: `src/vm/execute.rs` (modifies table access opcodes).

**Tests**: Unit tests for index chaining, __newindex with rawset
bypass, __call on tables, MAXTAGLOOP limit. Oracle comparison for
complex metatable chains.

### 4c. Error handling

Implement Result-based error propagation, pcall, xpcall.

- `LuaError` variants: runtime, syntax, memory, error-in-handler
- Protected calls: save/restore VM state on error
- `error(msg, level)`: create error with stack info
- `pcall(f, ...)`: returns true+results or false+error
- `xpcall(f, handler)`: error handler receives error object
- Stack traceback generation
- Error message format matching PUC-Rio

Files: `src/error.rs` (extends 0a), `src/vm/execute.rs`.

**Tests**: pcall catching errors, xpcall with custom handler,
nested pcall, error level parameter, stack traceback format.
Oracle comparison for error message wording.

### 4d. Varargs and environments

Implement `...` and function environments.

- `OP_VARARG`: copy vararg values to registers
- Stack layout: fixed params above vararg area
- `VARARG_HASARG`, `VARARG_ISVARARG`, `VARARG_NEEDSARG` flags
- Legacy `arg` table when `VARARG_NEEDSARG` is set
- `setfenv(f, t)` / `getfenv(f)`: change global lookup table
- Level 0 = thread environment, level N = Nth stack frame
- `OP_GETGLOBAL`/`OP_SETGLOBAL` use the closure's environment

Files: `src/vm/execute.rs`, `src/compiler/codegen.rs`.

**Tests**: Vararg passing, select with varargs, `...` in nested
functions, setfenv/getfenv, environment inheritance, level parameter.
Oracle comparison for all vararg edge cases.

## Phase 5: Standard Libraries

**Goal**: Implement all 9 standard libraries. PUC-Rio test suite
files progressively unlock as each library is completed.

### 5a. Base library

Priority 1. Required by all integration tests and PUC-Rio tests
(they use `assert`, `print`, `type`, etc.).

Key functions: `print`, `type`, `tostring`, `tonumber`, `assert`,
`error`, `pcall`, `xpcall`, `select`, `unpack`, `pairs`, `ipairs`,
`next`, `rawget`, `rawset`, `rawequal`, `getmetatable`,
`setmetatable`, `getfenv`, `setfenv`, `collectgarbage`, `load`,
`loadstring`, `loadfile`, `dofile`, `newproxy`.

Globals: `_G`, `_VERSION` ("Lua 5.1").

Files: `src/stdlib/base.rs`, `src/stdlib/mod.rs`.

**Tests**: One oracle comparison test per function. Integration
test file `stdlib-base.lua`.

**Unlocks**: `literals.lua`, `constructs.lua` (partial) from
PUC-Rio suite. Integration `.lua` tests now work (they use
`assert()`). Most oracle comparison tests become meaningful.

### 5b. String library

Priority 2. Pattern matching is the most complex part.

Implement pattern engine first: character classes (`%a` through
`%z`), bracket classes, quantifiers (`*`, `+`, `-`, `?`), anchors
(`^`, `$`), captures (up to 32), position captures `()`,
back-references (`%1`-`%9`), balanced match `%b`, frontier
`%f[set]`.

Then: `find`, `match`, `gmatch`, `gsub`, `format` (with all
specifiers), `byte`, `char`, `len`, `sub`, `rep`, `reverse`,
`lower`, `upper`, `dump`.

Set up string metatable: `{__index = string_lib}`.

Files: `src/stdlib/string.rs`.

**Tests**: Pattern matching edge cases, format specifiers, method
syntax (`s:upper()`). Oracle comparison for pattern matching.

**Unlocks**: `strings.lua`, `pm.lua` from PUC-Rio suite.

### 5c. Table library

Priority 3.

Implement: `concat`, `insert`, `remove`, `sort` (quicksort with
median-of-three, error on invalid comparison), `maxn`.

Deprecated: `foreach`, `foreachi`, `getn`, `setn` (raises error).

Files: `src/stdlib/table.rs`.

**Tests**: Sort stability edge cases, NaN in sort, insert/remove
shifting, concat with separator. Oracle comparison.

**Unlocks**: `sort.lua`, `nextvar.lua` from PUC-Rio suite.

### 5d. Math library

Priority 4. Wraps Rust f64 methods.

All functions: abs, acos, asin, atan, atan2, ceil, cos, cosh, deg,
exp, floor, fmod, frexp, ldexp, log, log10, max, min, modf, pow,
rad, random, randomseed, sin, sinh, sqrt, tan, tanh.

Constants: `math.pi`, `math.huge`. Alias: `math.mod` = `math.fmod`.

Files: `src/stdlib/math.rs`.

**Tests**: Edge cases (NaN, infinity, -0.0), random determinism
with seed. Oracle comparison.

**Unlocks**: `math.lua` from PUC-Rio suite.

### 5e. I/O library

Priority 5. File handles as userdata with metatables.

Implement: `open`, `close`, `read`, `write`, `lines`, `flush`,
`input`, `output`, `type`, `tmpfile`, `popen`.

File methods: `:read`, `:write`, `:lines`, `:seek`, `:setvbuf`,
`:close`, `:flush`.

Handles: `io.stdin`, `io.stdout`, `io.stderr`.

Files: `src/stdlib/io.rs`.

**Tests**: File read/write, line iteration, seek, standard handles.
Oracle comparison.

**Unlocks**: `files.lua` from PUC-Rio suite.

### 5f. OS library

Priority 6.

Implement: `clock`, `date`, `difftime`, `execute`, `exit`, `getenv`,
`remove`, `rename`, `setlocale`, `time`, `tmpname`.

Files: `src/stdlib/os.rs`.

**Tests**: clock monotonicity, date formatting, execute return
values. Oracle comparison.

### 5g. Package library

Priority 7. Module system.

Implement: `require`, `module`, `package.loaded`, `package.path`,
`package.cpath`, `package.preload`, `package.loaders`,
`package.loadlib`, `package.seeall`, `package.config`.

Four default loaders: preload, Lua file, C library, all-in-one C.

Files: `src/stdlib/package.rs`.

**Tests**: require with preload, path searching, loaded cache,
circular require detection. Oracle comparison.

**Unlocks**: `attrib.lua` from PUC-Rio suite.

### 5h. Debug library

Priority 8. Introspection.

Implement: `getinfo`, `getlocal`, `setlocal`, `getupvalue`,
`setupvalue`, `gethook`, `sethook`, `traceback`, `getfenv`,
`setfenv`, `getmetatable`, `setmetatable`, `getregistry`, `debug`.

Hook events: call, return, line, count.

Files: `src/stdlib/debug.rs`.

**Tests**: getinfo fields, local variable access, hook callbacks,
traceback format. Oracle comparison.

**Unlocks**: `db.lua` from PUC-Rio suite.

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

Files: `src/vm/state.rs`, `src/stdlib/coroutine.rs`.

**Tests**: Basic resume/yield, argument passing, error propagation,
nested coroutines, wrap iterator, status transitions, GC of suspended
coroutines. Oracle comparison.

**Unlocks**: `closure.lua` from PUC-Rio suite (contains coroutine
tests).

## Phase 7: GC Collector

**Goal**: Mark-sweep garbage collection with incremental steps.

### 7a. Mark and sweep

- Root marking: main thread, global table, registry, type metatables
- Propagation: gray -> black, mark referenced objects
- Tri-color invariant with write barriers (forward/back)
- Sweep: walk rootgc list, free white (dead) objects
- Flip white bit for next cycle
- String table sweep (separate from rootgc)
- Open upvalue sweep per thread
- Thread special case: always re-gray (grayagain list)

Files: `src/vm/gc/collector.rs`.

**Tests**: Cycle collection, object reachability, write barrier
correctness, string table sweep. Oracle comparison for
`collectgarbage("count")`.

### 7b. Atomic phase, incremental collection, weak tables

- Atomic phase: remark upvalues, propagate remaining grays
- Weak table processing: clear dead keys/values per `__mode`
- Finalizer processing (`__gc`): separate finalization list
- Incremental collection: step/pause/stepmul tuning
- GC debt tracking
- `collectgarbage` API: all 7 options

Files: `src/vm/gc/collector.rs` (extends 7a).

**Tests**: Weak table clearing, finalizer execution order,
incremental step correctness, collectgarbage API options. Oracle
comparison for GC-observable behavior.

**Unlocks**: `gc.lua` from PUC-Rio suite.

## Phase 8: Public API + CLI

**Goal**: Rust-idiomatic embedding API and PUC-Rio-compatible
command-line interpreter.

### 8a. Public API: core

- `Lua` struct: owns all state
- `Lua::new()`, `Lua::exec()`, `Lua::load()`
- `IntoLua`/`FromLua` traits for type conversion
- `IntoLuaMulti`/`FromLuaMulti` for multiple values
- `Table`, `Function`, `Thread` handle types
- Error types with source locations
- Selective library loading for sandboxing

Files: `src/lib.rs`.

**Tests**: Embedding examples from api.md, type conversion
round-trips, error propagation across Rust/Lua boundary.

### 8b. Public API: UserData

- `UserData` trait for custom Rust types
- Metatable generation from trait methods
- GC integration for UserData instances
- Method and field registration

Files: `src/lib.rs` (extends 8a).

**Tests**: Custom UserData types, method calls from Lua, GC of
UserData, metatable operations.

### 8c. CLI: standalone interpreter

Implement the `rilua` binary matching PUC-Rio `lua.c` behavior.

- `-e stat`: execute string
- `-l name`: require library
- `-i`: interactive mode after script
- `-v`: version information
- `--`: stop handling options
- `-`: execute stdin
- `LUA_INIT` environment variable
- `arg` table with script arguments
- REPL: multiline input detection (incomplete chunk), `=expr`
  shorthand, `_PROMPT`/`_PROMPT2` globals
- SIGINT handling via debug hook
- Error reporting to stderr with program name prefix

Files: `src/main.rs`.

**Tests**: CLI flag parsing, `-e` execution, `-v` output,
`LUA_INIT` handling, `arg` table construction, error output format.
Oracle comparison: same CLI invocations produce same output in both
`rilua` and PUC-Rio `lua`.

**Unlocks**: `main.lua` from PUC-Rio suite.

## Phase 9: Compatibility

**Goal**: Pass PUC-Rio's official test suite.

- Run `~/Repos/github.com/lua/tests` (tag `v5_1_1`) verbatim
- Fix behavioral differences found by test failures
- Match error message formats
- Match number formatting (`"%.14g"`)
- Match table traversal order
- Match GC observable behavior (finalizer order, weak table clearing)
- Build testC equivalent for `api.lua`, `code.lua`, `checktable.lua`

**Tests**: All 24 PUC-Rio test files pass. `big.lua` and
`verybig.lua` pass (stress tests requiring all features).

## Dependencies

```text
Phase 0 (skeleton + test infra)
  |
  +-------> Phase 1 (data structures)
  |                   |
  |                   v
  +-------> Phase 2 (compilation) -----> Phase 3 (core VM)
                                            |
                                            v
                                Phase 4 (language features)
                                            |
                                            v
                                Phase 5 (standard libraries)
                                        /       \
                              Phase 6          Phase 7
                            (coroutines)     (GC collector)
                                        \       /
                                            v
                                Phase 8 (API + CLI)
                                            |
                                            v
                                Phase 9 (compatibility)
```

Phases 1 and 2 can be developed in parallel (no shared code).
Phases 5-7 can be developed in parallel once Phase 4 is complete.
Phase 8 can start once Phase 5 is functional. Phase 9 is ongoing
throughout but becomes the focus after Phase 8.

## Milestones

| Milestone | Criteria | Chunks | Status |
|-----------|----------|--------|--------|
| Skeleton builds | Phase 0 complete, quality gate passes | 0a-0b | Done |
| Data structures | Arena, Val, strings, tables work in isolation | 1a-1f | Done |
| First bytecode | Compile Lua to Proto, compare with `luac -l` | 2a-2i | Done |
| First execution | `print("hello world")` runs end-to-end | 3a-3e | Done |
| Language complete | All Lua 5.1.1 language semantics work | 4a-4d | -- |
| Stdlib complete | All 9 standard libraries implemented | 5a-5h | -- |
| Coroutines | resume/yield work, `closure.lua` passes | 6 | -- |
| GC functional | Incremental collection, `gc.lua` passes | 7a-7b | -- |
| Embeddable | Rust API functional, CLI matches PUC-Rio | 8a-8c | -- |
| Compatible | PUC-Rio test suite passing | 9 | -- |

## Chunk Summary

Total: 40 chunks across 10 phases.

| Phase | Chunks | Description |
|-------|--------|-------------|
| 0 | 0a-0b | Skeleton, errors, test infrastructure |
| 1 | 1a-1f | Arena, Val, strings, tables |
| 2 | 2a-2i | Tokens, lexer, AST, parser, instructions, compiler |
| 3 | 3a-3e | CallInfo, closures, execution loop, end-to-end |
| 4 | 4a-4d | Metatables, errors, varargs, environments |
| 5 | 5a-5h | 9 standard libraries |
| 6 | 6 | Coroutines |
| 7 | 7a-7b | GC mark/sweep, incremental collection |
| 8 | 8a-8c | Public API, UserData, CLI |
| 9 | 9 | PUC-Rio compatibility |
