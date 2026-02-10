# Architecture Documentation

Design documentation for rilua, a Lua 5.1.1 implementation in Rust.

## Reading Order

Start with the overview, then follow the pipeline from source code to
execution. GC and API design can be read independently after
understanding the pipeline.

### 1. Foundation

1. [architecture.md](architecture.md) -- Design principles, module
   structure, decision summary
2. [use-cases.md](use-cases.md) -- WoW ecosystem and general
   embedding use cases
3. [references.md](references.md) -- Studied implementations and what
   we learned from each

### 2. Compilation Pipeline

1. [pipeline.md](pipeline.md) -- Why AST-based, phase responsibilities,
   tradeoffs vs single-pass
2. [instructions.md](instructions.md) -- PUC-Rio's 38 opcodes, Rust enum
   representation, encoding formats

### 3. Runtime Data

1. [values.md](values.md) -- Val enum, GcRef indices, equality, hashing,
   truthiness
2. [strings.md](strings.md) -- Interning, cached hash, pointer equality
3. [tables.md](tables.md) -- Array + hash parts, resizing, length operator
4. [closures.md](closures.md) -- Open/closed upvalues, Proto ownership,
   compiler support

### 4. Execution and Memory

1. [callinfo.md](callinfo.md) -- Call stack frames, call/return protocol,
   tail calls, vararg handling, error recovery
2. [metatables.md](metatables.md) -- 17 metamethod events, dispatch
   algorithms, type coercion, fast-path caching
3. [gc.md](gc.md) -- Arena with generational indices, mark-sweep,
   weak tables, finalizers, collectgarbage() API
4. [errors.md](errors.md) -- Result-based propagation, protected calls,
   error objects
5. [coroutines.md](coroutines.md) -- Thread structure, resume/yield
   protocols, GC interaction, coroutine library

### 5. External Interface

1. [api.md](api.md) -- Trait-based public API, IntoLua/FromLua,
   UserData, embedding example
2. [stdlib.md](stdlib.md) -- All 9 standard libraries, function lists,
   implementation priority

### 6. Quality

1. [testing.md](testing.md) -- Unit tests, integration tests, PUC-Rio
   suite, behavioral equivalence

### 7. Planning

1. [roadmap.md](roadmap.md) -- Implementation phases, dependencies,
   milestones
