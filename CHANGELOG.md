# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to
[Conventional Commits](https://www.conventionalcommits.org/en/v1.0.0/).

## [Unreleased]

### Added

- Architecture documentation in `docs/`:
  - `architecture.md` -- overview, design principles, module structure
  - `pipeline.md` -- compilation pipeline with Pratt parsing algorithms,
    register allocation, RK optimization, jump backpatching, codegen
    per statement type
  - `instructions.md` -- PUC-Rio's 38 opcodes as Rust enums
  - `values.md` -- Val enum with GC arena references
  - `gc.md` -- arena-based mark-sweep GC with generational indices,
    atomic phase, barriers, finalizers, incremental step sizing
  - `tables.md` -- array + hash dual representation with Brent's
    collision resolution, 3-phase resize, length operator algorithms
  - `strings.md` -- interned strings with cached hash, PUC-Rio hash
    algorithm, interning table, GC sweep mechanics, string fixation
  - `closures.md` -- open/closed upvalue model, upvalue list structures,
    findupval/close algorithms, OP_CLOSURE processing, GC interaction
  - `callinfo.md` -- call stack frames, call/return protocol, tail
    calls, vararg handling, error recovery
  - `metatables.md` -- 17 metamethod events, dispatch algorithms,
    type coercion rules
  - `errors.md` -- Result-based error handling
  - `api.md` -- trait-based Rust-idiomatic public API with internal
    stack model, GC handle safety, reference system
  - `stdlib.md` -- modular standard library with per-function behavioral
    specs and pattern language specification
  - `coroutines.md` -- thread structure, resume/yield protocols, GC
    interaction
  - `testing.md` -- spec-driven multi-layer testing strategy with test
    files mirroring Lua 5.1 manual structure
  - `references.md` -- classification of studied implementations
  - `use-cases.md` -- WoW ecosystem and general embedding use cases
  - `roadmap.md` -- 9-phase implementation plan with dependencies
- Use case summary in project README

### Changed

- Complete rewrite from scratch. Previous implementation (based on
  lua-in-rust) replaced with new architecture:
  - AST-based pipeline (was: single-pass bytecode emission)
  - Register-based VM with PUC-Rio opcodes (was: stack-based custom opcodes)
  - Arena GC with generational indices (was: raw-pointer mark-sweep)
  - Trait-based API (was: stack-based C API mirror)
