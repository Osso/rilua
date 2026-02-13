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
| 4: Language Features | Done | 521 total (439 unit + 43 integration + 39 oracle) |
| 5: Standard Libraries | Done (5a-5h) | 1071 total (481 unit + 342 integration + 248 oracle) |
| 6: Coroutines | Done | 1071 total (481 unit + 342 integration + 248 oracle) |
| 7: GC Collector | Done (7a-7b) | 1080 total (490 unit + 342 integration + 248 oracle) |
| 8: Public API + CLI | Done (8a-8e) | 1189 total (560 unit + 376 integration + 253 oracle) |
| 9: Compatibility | In progress (9a-9d done; 11/20 applicable PUC-Rio tests pass) | 1289 total (586 unit + 426 integration + 277 oracle) |

Phase 3 audit found and fixed 9 bugs across the compiler and VM.
Phase 4 added metatables, metamethods, protected calls, and 15 stdlib
functions. Phase 5a added iterators, dynamic loading, environments, and
globals. Phase 5b added the string library with all 14 functions plus
pattern matching engine (find, match, gmatch, gsub with character
classes, quantifiers, captures, balanced match, frontier patterns).
String metatable enables method syntax (`("hello"):upper()`). Phase 5c
added the table library with all 9 functions (concat, insert, remove,
sort, maxn, getn, setn, foreach, foreachi). Sort implements PUC-Rio's
median-of-three quicksort (`auxsort`). Phase 5d added the math library
with all 28 functions (abs through tanh), 2 constants (math.pi,
math.huge), and the math.mod alias. frexp/ldexp use IEEE 754 bit
manipulation. RNG uses glibc-compatible LCG. Phase 5f added the OS library
with all 11 functions (clock, date, difftime, execute, exit, getenv,
remove, rename, setlocale, time, tmpname) via libc FFI. Userdata
infrastructure added: arena, alloc, __gc support, registry metatable
helpers. Phase 5e added the I/O library with 11 library functions
(close, flush, input, lines, open, output, popen, read, tmpfile, type,
write), 7 file methods (close, flush, lines, read, seek, setvbuf,
write), 2 metamethods (__gc, __tostring), 3 standard file handles
(stdin, stdout, stderr), and a lines iterator with auto-close. Uses
libc FFI for C stdio operations. Phase 5g added the package library
with require, module, 4 default loaders (preload, Lua file, C stub,
C root stub), path searching, package.loaded/preload/loaders/config/
path/cpath, package.seeall, and package.loadlib. Uses upvalue[0] on
RustClosures to pass the package table (replaces LUA_ENVIRONINDEX).
C loaders return "not supported" (incompatible ABI). Circular require
uses LightUserdata(0xDEAD_CAFE) sentinel. Phase 5h added the debug
library with all 14 functions (getregistry, getmetatable, setmetatable,
getfenv, setfenv, getinfo, getlocal, setlocal, getupvalue, setupvalue,
gethook, sethook, debug, traceback). Hooks (sethook/gethook) and
debug() interactive mode are stubs. Fixed compiler bugs: activate_locals
PC indexing and missing end_pc for local variable debug info. Fixed
level calculation in debug introspection (off-by-one from base CI).
Phase 6 added coroutines (create, resume, yield, wrap, status, running)
with cooperative threading via LuaThread. Phase 7a added mark-sweep GC
with tri-color marking, two-white-generation flipping, Proto constant
marking, weak table clearing, and wired `collectgarbage()` to the real
collector. Phase 7b replaced stop-the-world with PUC-Rio's 5-state
incremental collector (Pause/Propagate/SweepString/Sweep/Finalize),
added memory tracking on allocation, write barriers (backward for tables,
forward for upvalues), `__gc` finalizers with error propagation, and
automatic GC triggering at allocation points. PUC-Rio gc.lua test passes.
Phase 8a added the public Rust embedding API (`Lua` struct, `IntoLua`/
`FromLua` traits, handle types, selective library loading). Phase 8c moved
binaries to `src/bin/` (`rilua` + `riluac` stub). Phase 8d implemented the
full PUC-Rio `lua.c` CLI: all flags, `LUA_INIT`, `arg` table, REPL with
multiline detection, TTY detection, error reporting. Phase 9a switched
the compilation pipeline from `&str` to `&[u8]` for non-UTF-8 source
support. 1158 total tests pass (538 unit + 367 integration + 253 oracle).
The full quality gate passes.

### Known issues

Issues discovered during implementation, deferred to later phases.
All must be resolved before the project is considered complete.

**Codegen bugs (Phase 9d)**:
- ~~`(true or 1) == true` hangs~~ **FIXED**: or-shortcircuit result used
  directly as EQ operand produced a jump cycle. Fix: discharge pending
  jumps in exp2rk before attempting RK encoding.
- ~~`{...}` vararg table constructor captures only first argument~~
  **FIXED**: SETLIST was emitted before vararg expansion. Fix: reorder
  to expand varargs first.
- ~~Mixed named parameters + varargs register misassignment~~ **FIXED**:
  off-by-one in parameter count calculation.
- ~~`debug_assert_eq` in `free_register` trips on complex expressions~~
  **FIXED**: Relaxed to match PUC-Rio behavior (silently ignores frees
  below freereg).
- ~~Table constructor NameField constant index overflow~~ **FIXED**: When
  constant pool exceeds 255 entries, named field keys in table
  constructors (`{__index = ...}`) used `k | BITRK` directly, causing
  silent 9-bit truncation. Fix: use `exp2rk` (falls back to LOADK +
  register when constant index > MAXINDEXRK).

**VM bugs (Phase 9d)**:
- ~~`load(func_reader)` hangs when the reader function never returns nil~~
  **FIXED**: Added 10MB size limit for collected reader data. Reader errors
  now properly recover call stack state (save/restore ci, n_ccalls, top).

**Parser bugs (Phase 9d)**:
- ~~Bare name accepted as statement~~ **FIXED**: `repeat until 1; a`
  compiled successfully instead of producing syntax error. Fix: require
  expression statements to be calls or assignments.

**Compiler bugs (Phase 9d)**:
- ~~Closure + for-loop break upvalue capture~~ **FIXED**: breaking out
  of a for loop didn't close upvalues. Fix: emit OP_CLOSE before jump.
- ~~`lastlinedefined` always 0~~ **FIXED**: FuncBody.end_line was not
  tracked. Fix: parse and store end line from `end` keyword.
- ~~`debug.getinfo "L"` activelines not populated~~ **FIXED**: added
  activelines table from proto line info.

**Pattern matching bugs (Phase 9d)**:
- ~~Non-ASCII bytes in bracket classes~~ **FIXED**: `matchbracketclass`
  treated bytes as signed, causing incorrect range comparisons. Fix:
  use unsigned byte comparisons.
- ~~`%z` character class~~ **FIXED**: `\0` byte not matched by `%z`.
  Fix: add explicit `b'z'` arm in `match_class`.
- ~~gsub `%n` replacement with unfinished captures~~ **FIXED**: position
  captures returned wrong len type. Fix: handle `CaptureLen::Position`
  in replacement expansion.
- ~~`%b` balanced match off-by-one~~ **FIXED**: end pointer was one
  byte short. Fix: advance past closing delimiter.
- ~~Backref `%1` matching~~ **FIXED**: `matchbalance` returned wrong
  pointer offset. Fix: correct capture length calculation.
- ~~gsub table replacement with metamethods~~ **FIXED**: gsub table
  replacement didn't invoke `__index` metamethods. Fix: use
  `state.gettable()` (metamethod-aware) instead of raw table get.
- ~~`string.gfind` alias~~ **FIXED**: `string.gfind` now shares the same
  closure object as `string.gmatch` (copies value from table, matching
  PUC-Rio's `lua_getfield`/`lua_setfield` pattern).

**Parser strictness (Phase 9d)**:
- ~~Standalone semicolons accepted~~ **FIXED**: Lua 5.1 grammar is
  `{stat [';']}` -- semicolons are optional separators after statements,
  not empty statements. Removed leading-semicolon loop from parse_block.
- ~~Return consumes extra semicolon~~ **FIXED**: parse_return had its own
  `test_next_char(';')` plus parse_block's. Removed from parse_return.
- ~~`<eof>` token not quoted in error messages~~ **FIXED**: Added
  `token2str()` method (unquoted) and wrapped all `near` context with
  `'...'` (PUC-Rio's LUA_QS pattern). Error messages now match PUC-Rio.
- ~~Ambiguous syntax not detected~~ **FIXED**: Added `lastline` tracking
  to parser. Function call `(` on different line than expression emits
  "ambiguous syntax (function call x new statement)".

**Remaining bugs (discovered during 9d)**:
- ~~Bug #24: `not` applied to `and`/`or` expressions silently drops the
  negation~~ **FIXED**: Added `remove_values()` to `code_not()` to convert
  TESTSET -> TEST instructions after swapping true/false jump lists
  (matching PUC-Rio's `removevalues()` in `codenot()`). Unlocked
  constructs.lua.
- **Bug #25**: `load()` with reader function reads entire input before
  parsing. PUC-Rio streams reader output incrementally into the lexer.
  Reader call count differs. Affects calls.lua.
- **Bug #26**: `setfenv` on coroutine threads not supported. rilua has
  a single global table; PUC-Rio has per-thread globals (`gt(L)`).
  Affects closure.lua.
- **Bug #27**: Locale-aware number parsing missing. Rust `f64::parse()`
  ignores C locale. PUC-Rio uses `strtod()` which respects locale.
  Fix requires libc FFI. Affects literals.lua.
- **Bug #28**: Error message format mismatch in I/O __gc: "got nil"
  vs PUC-Rio "got no value". Affects errors.lua.

**Debug library (Phase 5h, stubs)**:
- `debug.sethook` / `debug.gethook` are stubs (no hook execution).
- `debug.debug()` interactive mode is a stub.

### PUC-Rio test suite status (11 / 20 applicable pass)

Re-baselined after Phase 9d bug fixes (Feb 2026). Three test files (api,
checktable, code) require the `testC` C library and are not applicable
to rilua. Of the 20 applicable tests, 11 pass and 9 fail.

Bugs #15-#24 fixed: timeouts resolved (parser/compiler infinite-loop
patterns), TAILCALL stale values, VARARG register targeting, select
boundary, coroutine cross-thread upvalue corruption, debug.getinfo name
field, semicolon parsing, error message quoting, ambiguous syntax
detection, and `not`+`and`/`or` negation (removevalues in codenot).

| Result | Count | Files |
|--------|-------|-------|
| Pass | 11 | constructs, events, files, gc, locals, math, nextvar, pm, sort, strings, vararg |
| N/A | 3 | api, checktable, code (require testC C library) |
| Fail | 9 | attrib, big, calls, closure, db, errors, literals, main, verybig |

**Fail details**:
- `attrib`: requires file creation test infrastructure (writes
  `libs/B.lua` etc. to disk). Fails at line 23.
- `big`: requires `checktable` module (C library). Also: PUC-Rio itself
  fails this test. Not a rilua issue.
- `calls`: assertion at line 250 -- `load()` with reader function reads
  entire input before parsing; PUC-Rio streams incrementally. Reader
  call count differs (`i=9` vs expected `i=2`).
- `closure`: `setfenv` on coroutine threads not supported. Fails at
  line 416 (`debug.setfenv(co, a)`). Per-thread global tables not
  implemented.
- `db`: fails at line 20 -- requires `debug.sethook` line hook execution
  (currently a stub). Also: debug.getinfo name nil fix applied.
- `errors`: assertion at line 111 -- `checkmessage` for `__gc` error
  message format mismatch (`"got nil"` vs PUC-Rio `"got no value"`).
  Also: several "ambiguous syntax" and semicolon errors now fixed.
- `literals`: assertion at line 162 -- locale-aware `tonumber("3,4")`
  returns nil because Rust `f64::parse` ignores C locale. Fix requires
  libc `strtod` FFI for number parsing.
- `main`: requires CLI subprocess execution with `LUA_PATH` manipulation.
  `require` path search fails for `/tmp/lua_*` temp files.
- `verybig`: assertion at line 120033 of generated code. Bug #24 fix
  resolved the `not`+`and`/`or` issue but verybig.lua has additional
  failures in the generated code (different expression combinations).

### Execution order corrections

The phase numbering (5e-5h) describes logical grouping, not execution
order. Several phases have infrastructure dependencies that require a
different execution sequence:

- **5e (I/O)** needs userdata with `__gc` finalizers (file handles).
  Userdata is currently a placeholder (Phase 8b). A minimal userdata
  arena must be built before 5e can start.
- **5g (Package)** needs file I/O for `require()` and userdata for
  dynamic library handles. Depends on 5e infrastructure.
- **5h (Debug)** needs per-thread hook state (`hook_fn`, `hook_mask`,
  `hook_count`) and debug stack introspection (`getstack`, `getinfo`,
  `getlocal`). These require `LuaThread` to be fully defined, which
  happens in Phase 6.
- **5f (OS)** has no special infrastructure needs. It wraps libc/std
  functions and can proceed immediately.

Corrected execution order:

```text
5d (math)  [done]
  |
5f (os)    [done]
  |
Userdata infrastructure  [done]
  |
5e (I/O)   [done]
  |
5g (Package)  [done]
  |
6 (Coroutines)  <- defines LuaThread with hook fields
  |
5h (Debug)  <- needs hooks + thread introspection from Phase 6
```

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

**Unlocks**: `constructs.lua` (partial) from PUC-Rio suite.
(`literals.lua` also requires 9a for byte-based source loading.)
Integration `.lua` tests now work (they use `assert()`). Most oracle
comparison tests become meaningful.

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

**Unlocks**: `strings.lua`, `pm.lua` from PUC-Rio suite (both also
require 9a for byte-based source loading).

### 5c. Table library

Priority 3.

Implement: `concat`, `insert`, `remove`, `sort` (quicksort with
median-of-three, error on invalid comparison), `maxn`.

Deprecated: `foreach`, `foreachi`, `getn`, `setn` (raises error).

Files: `src/stdlib/table.rs`.

**Tests**: Sort stability edge cases, NaN in sort, insert/remove
shifting, concat with separator. Oracle comparison.

**Unlocks**: `sort.lua` (also requires 9a), `nextvar.lua` from
PUC-Rio suite.

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

**Unlocks**: `files.lua` from PUC-Rio suite (also requires 9a for
byte-based source loading).

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

**Unlocks**: `db.lua` from PUC-Rio suite (also requires 9a for
byte-based source loading).

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

### 7a. Mark and sweep [Done]

Implemented stop-the-world mark-sweep with tri-color marking and
two-white-generation flipping (PUC-Rio algorithm).

- Root marking: main thread stack, call stack, global table, registry,
  type metatables, tm_names, open upvalues, error object
- Propagation: gray -> black, traverse tables (metatable + array +
  hash), closures (env + upvalues + Proto constants), threads (stack +
  open upvalues)
- Proto constant marking: recursively marks all Val constants in the
  Proto tree (Protos are Rc, not GC-managed, so constants must be
  explicitly marked during closure traversal)
- Two-white flip in atomic phase (before sweep), matching PUC-Rio's
  `lgc.c` ordering
- Sweep: arena-based sweep with generation bump for freed slots
- String table sweep: remove stale intern entries after arena sweep
- Weak table clearing: dead key/value detection, strings exempt
- Thread special case: threads go on grayagain list, re-traversed in
  atomic phase
- `collectgarbage()` wired to real GC: collect, stop, restart, count,
  step, setpause, setstepmul all functional
- `gcinfo()` uses real memory estimate
- GcState with pacing: gc_pause (200%), gc_stepmul (200%), initial
  threshold 64KB
- Memory estimation from arena occupancy

**Key bugs found and fixed during implementation**:
- White flip must happen in atomic phase (before sweep), not after
  sweep. Otherwise the first cycle never collects anything because
  all objects have current_white and sweep looks for other_white.
- Must NOT mark all threads unconditionally in mark_roots. Only
  threads reachable from roots (global table, registry, stack) should
  survive. Marking all threads prevents any thread from being collected.
- Proto constants (string refs in the constant pool) must be marked
  during closure traversal. Without this, strings used only in compiled
  code get collected and cause "string: ???" output.

Files: `src/vm/gc/collector.rs`, `src/vm/gc/arena.rs` (sweep method),
`src/vm/table.rs` (GC accessors), `src/vm/string.rs` (sweep_dead,
retain), `src/vm/state.rs` (GcState field), `src/stdlib/base.rs`
(collectgarbage wiring).

**Tests**: 8 unit tests (cycle collection, reachability, string
sweep, closure collection, stack preservation, threshold update).

### 7b. Incremental collection, write barriers, finalizers [Done]

Replaced stop-the-world with PUC-Rio's 5-state incremental GC.

- GC state machine: Pause -> Propagate -> SweepString -> Sweep ->
  Finalize -> Pause, with gc_singlestep/gc_step/full_gc drivers
- Memory tracking on allocation (total_bytes, gc_debt, gc_threshold)
- Automatic GC triggering via gc_check() at NEWTABLE, CONCAT, CLOSURE
- Write barriers: backward barrier for tables (demote black -> gray,
  re-traverse in atomic via grayagain); forward barrier for upvalues
  (mark white child when black parent writes)
- `__gc` finalizers: separate_userdata in atomic phase identifies dead
  userdata with __gc, marks them for resurrection; call_gc_finalizer
  runs __gc during Finalize phase; errors propagate to caller (PUC-Rio
  5.1.1 behavior, uses luaD_call not pcall for GCTM)
- sweep_partial for incremental arena sweeping with cursor tracking
- Weak table clearing handles finalized userdata (PUC-Rio iscleared:
  finalized userdata cleared from weak values but not weak keys)
- collectgarbage("step") with incremental step semantics matching
  PUC-Rio's luaC_step debt-based model

**Key bugs found and fixed during implementation**:
- Compiler CLOSE instruction never emitted: mark_upval was missing from
  resolve_var_aux (BlockContext.has_upval never set true)
- mark_tmudata must directly mark metatable/env of tmudata entries, not
  delegate to mark_userdata (which bails on non-white colors)
- iscleared must treat finalized userdata values as dead in weak tables
  (PUC-Rio lgc.c line 343-344 special case)
- __gc errors must propagate, not be silently discarded (call_gc_finalizer
  returns LuaResult, cascades through gc_singlestep/gc_step/full_gc)

Files: `src/vm/gc/collector.rs`, `src/vm/gc/arena.rs` (sweep_partial),
`src/vm/table.rs` (is_key parameter), `src/vm/value.rs` (finalized
flag), `src/vm/execute.rs` (gc_check calls, write barriers),
`src/compiler/codegen.rs` (mark_upval fix), `src/stdlib/base.rs`
(collectgarbage dispatch).

**Tests**: 8 unit tests (collector), integration tests for weak tables
and finalizers, PUC-Rio gc.lua passes. 1080 total tests pass
(490 unit + 342 integration + 248 oracle).

## Phase 8: Public API + CLI [Done]

**Goal**: Rust-idiomatic embedding API and PUC-Rio-compatible
command-line interpreter.

Phase 8a implemented the `Lua` struct with `new`/`exec`/`load` methods,
`IntoLua`/`FromLua` type conversion traits, `Table`/`Function`/`Thread`
handle types, selective library loading via `StdLib` bitflags, and GC
control methods. Phase 8b added the `AnyUserData` handle type with
`create_userdata`, `create_typed_userdata`, and `create_userdata_metatable`
methods on the `Lua` struct, plus `IntoLua`/`FromLua` conversions for
`AnyUserData`. Phase 8c moved binaries to `src/bin/` with `[[bin]]`
entries. Phase 8d implemented the full PUC-Rio `lua.c` CLI: all flags
(`-e`, `-l`, `-i`, `-v`, `--`, `-`), `LUA_INIT` env var, `arg` table
construction, REPL with multiline detection and `=expr` shorthand,
TTY detection, and error reporting. Phase 8e implemented the `riluac`
bytecode compiler/lister matching PUC-Rio `luac`: `-l` listing, `-l -l`
full listing (constants, locals, upvalues), `-p` parse-only, `-v` version,
stdin input, multiple file combining. Added `OpMode`/`OpArgMask` opcode
metadata and `listing` module for bytecode formatting. Binary output (`-o`)
and debug stripping (`-s`) added in Phase 9b.

### 8a. Public API: core [Done]

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

### 8b. Public API: UserData [Done]

- `AnyUserData` handle type wrapping `GcRef<Userdata>` with `borrow<T>`
  and `borrow_mut<T>` for type-safe access to the inner `Box<dyn Any>`
- `Lua::create_userdata<T>()`: create bare userdata with no metatable
- `Lua::create_typed_userdata<T>()`: create userdata with a named,
  registry-cached metatable
- `Lua::create_userdata_metatable()`: create or retrieve a metatable
  for a userdata type name
- `IntoLua`/`FromLua` conversions for `AnyUserData`
- `set_metatable()` / `metatable()` on the handle

Files: `src/handles.rs`, `src/conversion.rs`, `src/lib.rs`.

**Tests**: 11 unit tests (create, borrow, type mismatch, metatable
caching, set/get metatable, conversion round-trip).

### 8c. Binary restructuring [Done]

Move binaries from `src/main.rs` to `src/bin/` with explicit
`[[bin]]` entries in `Cargo.toml`. Two binaries, both thin wrappers
around the library crate (mirrors PUC-Rio's `lua.c` + `luac.c`
linking against the same `liblua`):

- `rilua` -- standalone interpreter (equivalent to PUC-Rio `lua`)
- `riluac` -- bytecode compiler/lister (equivalent to PUC-Rio `luac`)

Files: `src/bin/rilua.rs`, `src/bin/riluac.rs`, `Cargo.toml`.

### 8d. CLI: standalone interpreter [Done]

Implement the `rilua` binary matching PUC-Rio `lua.c` behavior.

- `-e stat`: execute string
- `-l name`: require library
- `-i`: interactive mode after script
- `-v`: version information
- `--`: stop handling options
- `-`: execute stdin (read source from standard input)
- `LUA_INIT` environment variable
- `arg` table with script arguments
- Shebang line handling: skip leading `#` line in source files
  (PUC-Rio's `luaL_loadfile` strips `#!` lines; `lua.c` also handles
  this for stdin input)
- REPL: multiline input detection (incomplete chunk), `=expr`
  shorthand, `_PROMPT`/`_PROMPT2` globals
- SIGINT handling via debug hook
- Error reporting to stderr with program name prefix

Files: `src/bin/rilua.rs`.

**Tests**: CLI flag parsing, `-e` execution, `-v` output,
`LUA_INIT` handling, `arg` table construction, error output format.
Oracle comparison: same CLI invocations produce same output in both
`rilua` and PUC-Rio `lua`.

**Unlocks**: `main.lua` from PUC-Rio suite.

### 8e. CLI: bytecode compiler/lister [Done]

Implemented the `riluac` binary matching PUC-Rio `luac` behavior.

- Compile Lua source files to bytecode
- `-l`: list bytecode (instruction disassembly)
- `-l -l`: detailed listing with constants, locals, and upvalue info
- `-p`: parse only (syntax check)
- `-v`: version information
- `-`: compile stdin
- `--`: stop handling options
- Multiple input files combined via CLOSURE+CALL wrapper Proto
- `-o file` and `-s`: implemented in Phase 9b
- Added `OpMode`, `OpArgMask` enums and opcode metadata methods to
  `instructions.rs` (matches PUC-Rio `luaP_opmodes`)
- Added `src/vm/listing.rs` module: bytecode listing output matching
  PUC-Rio's `print.c` format (header, code with comments, constants,
  locals, upvalues tables; recursive for nested protos)

Files: `src/bin/riluac.rs`, `src/vm/instructions.rs`,
`src/vm/listing.rs` (new), `src/vm/mod.rs`.

**Tests**: 10 unit tests (opcode metadata, listing format) + 9
integration tests (CLI flags, stdin, multiple files, error cases).

## Phase 9: Compatibility

**Goal**: Pass PUC-Rio's official test suite (`~/Repos/github.com/lua/tests`,
tag `v5_1_1`). All 24 test files run and pass when executed through
`rilua`.

Phase 9 is not a single pass. It contains architectural changes that
must happen in order, followed by iterative bug fixing. The sub-phases
below are ordered by dependency and impact.

### 9a. Byte-based source loading [Done]

The lexer and file loaders currently operate on `&str` (UTF-8). Lua
source files can contain arbitrary bytes in string literals (e.g.
`"\255"`, `"\0"`). Six PUC-Rio test files fail before executing a
single instruction because `std::fs::read_to_string` rejects non-UTF-8
content.

This is an architectural change that ripples through the compilation
pipeline:

- Change file loading from `std::fs::read_to_string` to
  `std::fs::read` (returns `Vec<u8>`)
- Change `Lexer` to operate on `&[u8]` instead of `&str`
- Change `Parser` input accordingly
- Change `compile()` entry point to accept `&[u8]`
- Preserve the string pool as `Vec<u8>` (already byte-based internally)
- Update `loadfile`, `dofile`, `loadstring`, `-e` flag, and the REPL
  to work with byte-based loading
- Ensure non-ASCII bytes in string literals are preserved exactly

Reference: PUC-Rio's `llex.c` operates on `unsigned char`. The lexer
never assumes UTF-8.

Files: `src/compiler/lexer.rs`, `src/compiler/parser.rs`,
`src/compiler/codegen.rs`, `src/stdlib/base.rs`, `src/stdlib/package.rs`,
`src/main.rs`.

**Tests**: Load and execute Lua files containing `\xff` byte literals,
`\0` in strings, binary escape sequences. Oracle comparison.

**Unlocks**: `literals.lua`, `strings.lua`, `pm.lua`, `sort.lua`,
`files.lua`, `db.lua` from PUC-Rio suite (currently blocked by UTF-8
rejection).

### 9b. Bytecode serialization (string.dump + binary chunk loading) -- DONE

Implemented PUC-Rio Lua 5.1.1 binary chunk format (12-byte header +
recursive function blocks). Byte-identical output with PUC-Rio on
64-bit little-endian Linux.

- `src/vm/dump.rs`: DumpState, dump Proto to binary chunk bytes.
  Handles both patched Protos (live closures with `Val::Str`) and
  unpatched Protos (compiler output with `string_pool`).
- `src/vm/undump.rs`: LoadState, load binary chunk bytes to unpatched
  Proto (strings in `string_pool`, `Val::Nil` placeholders).
- `src/stdlib/string.rs`: `string.dump(func)` serializes Lua closures,
  rejects Rust closures with `"unable to dump given function"`.
- `src/stdlib/base.rs`: `loadstring`/`load`/`loadfile` detect `\27Lua`
  header and dispatch to undump instead of compiler.
- `src/lib.rs`: `compile_or_undump()` helper, `exec_bytes`/`load_bytes`
  updated.
- `src/bin/riluac.rs`: `-o file` writes binary output, `-s` strips
  debug info, binary chunk re-dumping supported.

Cross-compatible: rilua dumps load in PUC-Rio and vice versa. 1229
total tests (576 unit + 390 integration + 263 oracle).

**Unlocks**: `all.lua` test runner (can now execute test files through
dump/undump cycle).

### 9c. Error message formatting [Done]

Ported PUC-Rio's `ldebug.c` error message infrastructure. Runtime errors
now include variable names and context, matching PUC-Rio output exactly.

- `src/vm/debug_info.rs` (new): `get_local_name`, `symbexec`, `kname`,
  `getobjname`, `getfuncname` -- ported from PUC-Rio's `ldebug.c`.
  `symbexec` walks bytecode forward tracking which instruction last
  wrote to a register. `getobjname` resolves register to variable
  kind+name ("local"/"global"/"field"/"upvalue"/"method").
- `src/vm/execute.rs`: Replaced 5 error functions with `type_error()`
  using `getobjname`. Updated `arith_error` with RK-aware operand
  resolution. Updated all call sites for `vm_gettable`, `vm_settable`,
  `call_bin_tm`, and precall.
- `src/stdlib/debug.rs`: Extracted `generate_traceback()` from
  `db_traceback` as `pub(crate)` shared function. Updated `db_getinfo`
  to use `getfuncname` for name/namewhat fields. Made `chunkid` and
  `current_line` `pub(crate)`.
- `src/lib.rs`: Added `call_function_traced()` that generates a stack
  traceback on unhandled errors (Result-based equivalent of PUC-Rio's
  `docall` errfunc pattern).
- `src/bin/rilua.rs`: Updated `handle_script`, `dotty`, stdin execution,
  and `-e` option to use `call_function_traced`.
- TAILCALL fix: Restructured to match PUC-Rio's approach (call `precall`
  first, then optimize only for Lua-to-Lua calls). C/Rust tail calls
  no longer incorrectly pop the caller's frame.

Error messages verified identical to PUC-Rio: call (local/global/upvalue/
field), index, arithmetic, concatenation, length, comparison. Traceback
format matches exactly (including `[C]: ?` base frame).

1262 total tests (583 unit + 402 integration + 277 oracle).

**Unlocks**: `errors.lua` from PUC-Rio suite.

### 9d. Compiler and VM bug fixes [In Progress]

Fix known codegen and VM bugs that cause test failures. These are
discovered iteratively by running PUC-Rio test files after 9a-9c.

**Fixed bugs** (13 total):

1. **Or-shortcircuit in EQ operand**: `(true or 1) == true` produced an
   infinite loop. Fix: discharge pending jumps in `exp2rk` before RK
   encoding. Unblocked `constructs.lua`, `calls.lua`.
2. **Parser: bare name as statement**: `repeat until 1; a` compiled
   successfully. Fix: require expression statements to be calls or
   assignments in `exprstat`. Unblocked `errors.lua`.
3. **Vararg table constructor**: `{...}` captured only first argument.
   Fix: reorder SETLIST after vararg expansion.
4. **Mixed named params + varargs**: Register misassignment. Fix:
   off-by-one in parameter count.
5. **Closure + for-loop break**: Upvalue capture on break didn't close.
   Fix: emit OP_CLOSE before jump. Unblocked `closure.lua`.
6. **debug.getinfo "L" activelines**: Table not populated. Fix: build
   activelines from proto line info. Unblocked `db.lua`.
7. **lastlinedefined always 0**: FuncBody.end_line not tracked. Fix:
   parse and store end line from `end` keyword.
8. **Non-ASCII bytes in bracket classes**: `matchbracketclass` treated
   bytes as signed. Fix: unsigned byte comparisons.
9. **`%z` character class**: `\0` byte not matched. Fix: add `b'z'`
   arm in `match_class`.
10. **gsub `%n` replacement**: Position captures returned wrong len
    type. Fix: handle `CaptureLen::Position` in replacement expansion.
11. **`%b` balanced match off-by-one**: End pointer one byte short. Fix:
    advance past closing delimiter.
12. **Backref `%1` matching**: Wrong pointer offset. Fix: correct
    capture length calculation.
13. **Table constructor NameField constant overflow**: When constant pool
    exceeds 255 entries, `{__index = func}` referenced wrong constant
    (9-bit truncation of `k | BITRK`). Fix: use `exp2rk` for named
    field keys, falling back to LOADK + register. Root cause: rilua's
    constant pool lacks some deduplication PUC-Rio does (264 vs 252
    constants for pm.lua), pushing indices past MAXINDEXRK.

**Additional fixes** (stdlib):
- gsub table replacement with metamethods: used `state.gettable()`
  (metamethod-aware) instead of raw table get.
- String coercion in stdlib functions (various).
- Shebang line handling.

14. **load(func_reader) hang**: Reader-based loading collected data
    without limit. Fix: 10MB size limit, reader errors now recover call
    stack state (save/restore ci, n_ccalls, top).
15. **string.gfind alias**: `string.gfind ~= string.gmatch`. Fix: copy
    gmatch closure value from table (matching PUC-Rio's
    `lua_getfield`/`lua_setfield`). Unblocked `pm.lua`.
16. **LUA_COMPAT_VARARG**: Missing `arg` table for old-style vararg
    functions. Fix: compiler adds implicit `arg` local with
    HASARG|ISVARARG|NEEDSARG flags; VM creates arg table when NEEDSARG
    is set; NEEDSARG cleared when `...` is used in body.

**Remaining known bugs** (discovered during evaluation, Feb 2026):

17. ~~**Parenthesized multi-return truncation**~~: **FIXED**. Added
    `Expr::Paren` AST variant. Parser wraps `(expr)` in `Paren` node;
    codegen calls `discharge_vars` which triggers `set_one_ret` for
    Call/VarArg expressions, matching PUC-Rio's `prefixexp` behavior.
    Also makes `(f())` as a statement a syntax error (matching PUC-Rio).
    Files: `ast.rs`, `parser.rs`, `codegen.rs`.
18. ~~**While-true-if-break compiler bug**~~: **FIXED**. `compile_while`
    used `patch_jump` (single instruction) instead of `patch_list`
    (walks linked list) for the loop-back JMP. `emit_jump` concatenates
    pending `jpc` jumps into the new JMP, but `patch_jump` only patched
    the single loop-back JMP, leaving false-branch JMPs from conditions
    inside the body with their initial NO_JUMP (-1) sBx, which encodes
    as a self-loop. Fix: `codegen.rs` line 1787.
19. **Repeat-until upvalue scoping**: Closures in `repeat ... until`
    loops all share the same upvalue for locals declared in the loop
    body instead of each iteration creating its own copy. PUC-Rio
    creates fresh upvalues per iteration. Affects: closure.
20. **Coroutine register restoration**: After `coroutine.yield` and
    subsequent `resume`, upvalue registers in the coroutine's stack
    frame are corrupted. An upvalue pointing at a table gets resolved
    as the first element of that table (off-by-one or wrong base in
    stack restoration). Affects: literals (coroutine + pairs/ipairs
    pattern).
21. **Return-from-C stale value**: When a Rust/C function returns 0
    values and is called via `return f(...)` (tail position in a Lua
    wrapper), a stale register value leaks as the return value instead
    of nothing. Direct calls work correctly; only the `return f(...)`
    wrapper pattern is affected. Affects: vararg.
22. **Loadstring error message format**: `loadstring("break label")`
    produces `break label:1: ...` instead of `[string "break label"]:1:
    ...`. The source name uses the raw input string instead of the
    `[string "..."]` format. Additionally, the error text differs:
    rilua says `<eof> expected near <name>` instead of `no loop to
    break near 'label'`. Affects: errors.
23. **Debug.getinfo namewhat**: `debug.getinfo(1).namewhat` returns
    `"global"` instead of `"local"` for a `function f()` defined via
    `local function f()` in certain call contexts. The function name
    resolution in `getobjname`/`getfuncname` doesn't distinguish
    local function definitions. Affects: db.
24. **Constant pool deduplication gap**: rilua creates ~5% more
    constants than PUC-Rio for large scripts (264 vs 252 for pm.lua).
    Works correctly with exp2rk fix but wastes constant pool space.
25. **Missing constant folding**: rilua's compiler doesn't fold
    constant arithmetic expressions at compile time. PUC-Rio folds
    `1 + 2 * 3` to `LOADK 7`; rilua emits `MUL` + `ADD`. Unary `-1`
    also not folded. Correct behavior, just less optimal bytecode.

Files: `src/compiler/codegen.rs`, `src/compiler/parser.rs`,
`src/compiler/ast.rs`, `src/vm/execute.rs`, `src/stdlib/debug.rs`,
`src/stdlib/string.rs`, `src/vm/state.rs`.

**Tests**: Regression tests for each fixed bug. Oracle comparison for
the specific patterns.

### 9e. Behavioral equivalence pass

Final pass through all 24 PUC-Rio test files. Fix remaining
discrepancies:

- Table traversal order (must match PUC-Rio's hash-based ordering)
- GC observable behavior (finalizer execution order, weak table
  clearing timing, `collectgarbage("count")` accuracy)
- Number formatting edge cases (`"%.14g"` for all special values)
- `tostring` output format for functions, tables, userdata (address
  format)
- `pcall` / `xpcall` error object propagation
- Tail call behavior (`__call` chains, tail position detection)
- String comparison (unsigned byte ordering, locale independence)
- `setfenv` / `getfenv` with numeric level arguments on nested
  closures
- `newproxy(true)` with GC finalizers
- Build `testC` equivalent (Rust module providing the `T` debug table
  that `api.lua`, `code.lua`, and `checktable.lua` use for internal
  API testing; currently these pass because they skip `T`-dependent
  tests, but full coverage requires the test library)

Files: Various, depending on failures found.

**Tests**: All 24 PUC-Rio test files pass. `all.lua` runner completes
(requires 9b for dump/undump cycle and Phase 6 for coroutines).

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
                                9a (byte-based source)
                                            |
                                            v
                                9b (bytecode serialization)
                                            |
                                            v
                                9c (error messages)
                                            |
                                            v
                                9d (codegen/VM bug fixes)
                                            |
                                            v
                                9e (behavioral equivalence)
```

Phases 1 and 2 can be developed in parallel (no shared code).
Within Phase 5, sub-phases have infrastructure constraints (see
"Execution order corrections" above): 5f is independent, 5e/5g need
userdata, 5h needs Phase 6 thread structure. Phase 7 (GC) can run in
parallel with later stdlib phases. Phase 8 can start once Phase 5 is
functional.

Phase 9 sub-phases are ordered by dependency: 9a (byte sources) is
needed before most PUC-Rio tests can even load; 9b (bytecode
serialization) is needed for the `all.lua` test runner; 9c (error
messages) is independent but best done before 9d/9e so that error
format assertions pass; 9d (bug fixes) and 9e (equivalence) are
iterative and overlap.

## Milestones

| Milestone | Criteria | Chunks | Status |
|-----------|----------|--------|--------|
| Skeleton builds | Phase 0 complete, quality gate passes | 0a-0b | Done |
| Data structures | Arena, Val, strings, tables work in isolation | 1a-1f | Done |
| First bytecode | Compile Lua to Proto, compare with `luac -l` | 2a-2i | Done |
| First execution | `print("hello world")` runs end-to-end | 3a-3e | Done |
| Language complete | All Lua 5.1.1 language semantics work | 4a-4d | Done |
| Stdlib complete | All 9 standard libraries implemented | 5a-5h | Done |
| Coroutines | resume/yield work, `closure.lua` passes | 6 | Done |
| GC mark-sweep | Stop-the-world collection, `collectgarbage()` works | 7a | Done |
| GC incremental | Incremental collection, `gc.lua` passes | 7b | Done |
| Embeddable | Rust API functional, CLI matches PUC-Rio | 8a-8e | Done |
| Byte sources | Lua files with non-UTF-8 bytes load and run | 9a | Done |
| Bytecode I/O | `string.dump` + binary chunk loading work | 9b | Done |
| Error parity | Error messages match PUC-Rio format | 9c | Done |
| Bug-free codegen | Known compiler/VM bugs fixed | 9d | -- |
| Compatible | All 24 PUC-Rio test files pass | 9e | -- |

## Chunk Summary

Total: 46 chunks across 10 phases.

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
| 8 | 8a-8e | Public API, UserData, binaries, interpreter CLI, compiler CLI |
| 9 | 9a-9e | Byte sources, bytecode I/O, error messages, bug fixes, equivalence |
