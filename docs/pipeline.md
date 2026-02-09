# Compilation Pipeline

## Decision

Lexer -> Parser -> AST -> Compiler -> Proto (bytecode)

A multi-phase compilation pipeline with an explicit AST intermediate
representation, following the approach proven by Luau.

## Context

PUC-Rio Lua 5.1.1 uses a single-pass recursive descent compiler that
emits bytecode directly during parsing. There is no AST — the parser
and code generator are interleaved in `lparser.c` and `lcode.c`.

This is efficient but tightly couples parsing and code generation.
Bugs in one phase are hard to isolate. Testing requires running the
full pipeline. Adding optimizations means modifying the parser.

Luau (Roblox's production Lua 5.1 fork) introduced an explicit AST
phase. The parser produces `AstStatBlock` trees. A separate compiler
walks the AST and emits bytecode. This design has been proven at
scale in a production environment serving millions of users.

## Phases

### Phase 1: Lexer (source -> tokens)

The lexer converts source text into a stream of tokens. Each token
has a type, optional value (for literals), and source location
(line, column).

Responsibilities:

- Character-level scanning
- Keyword recognition
- Number literal parsing (decimal, hex, float)
- String literal parsing (short strings with escapes, long strings
  with bracket notation)
- Comment handling (single-line, long comments)
- Whitespace skipping
- Source position tracking

The lexer provides one-token lookahead via a `peek()` method.

### Phase 2: Parser (tokens -> AST)

The parser consumes tokens and produces an abstract syntax tree.
It is a hand-written recursive descent parser following Lua 5.1.1
grammar rules.

Responsibilities:

- Statement parsing (if, while, for, repeat, do, return, break,
  assignment, function call, local declaration)
- Expression parsing with operator precedence
- Block/scope tracking
- Syntax error reporting with source locations

The parser does NOT:

- Resolve variable scopes (that is the compiler's job)
- Emit bytecode
- Perform optimizations

### Phase 3: Compiler (AST -> Proto)

The compiler walks the AST and emits register-based bytecode into
Proto structures.

Responsibilities:

- Variable resolution (locals, upvalues, globals)
- Register allocation
- Instruction emission
- Constant pool management
- Jump backpatching
- Nested function compilation (recursive Proto creation)
- Debug information (line numbers, local names)

The compiler maintains a `FuncState` for each function being
compiled, tracking:

- `freereg` — next available register
- `nactvar` — number of active local variables
- Local variable declarations and scopes
- Upvalue resolution chain
- Constant table

### Output: Proto

The compilation output is a `Proto` (function prototype) containing:

- Bytecode instruction array
- Constant pool (numbers, strings)
- Nested Proto array (for inner function definitions)
- Upvalue descriptors
- Debug information (line map, local variable names)
- Function metadata (parameter count, vararg flag, max stack size)

Proto is immutable after compilation and shared between closures
via `Rc<Proto>`.

## Tradeoffs

### vs. Single-Pass (PUC-Rio)

| Aspect | Single-pass | AST-based |
|--------|------------|-----------|
| Memory | Lower (no AST allocation) | Higher (AST in memory) |
| Speed | Faster compilation | Slower (two passes) |
| Testability | Hard (phases coupled) | Each phase testable alone |
| Debuggability | Hard (interleaved logic) | Clear phase boundaries |
| Future optimization | Requires parser changes | Operates on AST |

For a Rust implementation prioritizing correctness and maintainability,
the AST approach is the right tradeoff. Compilation speed is not the
bottleneck — Lua programs are typically small.

### vs. Full Lossless AST (full-moon)

We do NOT need a lossless AST that preserves whitespace and comments.
That is a requirement for formatters and linters, not for a VM. Our
AST only needs to preserve semantics and source locations for error
reporting.
