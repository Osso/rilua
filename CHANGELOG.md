# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to
[Conventional Commits](https://www.conventionalcommits.org/en/v1.0.0/).

## [Unreleased]

### Added

- Architecture documentation in `docs/`:
  - `architecture.md` -- overview, design principles, module structure
  - `pipeline.md` -- compilation pipeline (Lexer -> Parser -> AST -> Compiler)
  - `instructions.md` -- PUC-Rio's 38 opcodes as Rust enums
  - `values.md` -- Val enum with GC arena references
  - `gc.md` -- arena-based mark-sweep GC with generational indices
  - `tables.md` -- array + hash dual representation
  - `strings.md` -- interned strings with cached hash
  - `closures.md` -- open/closed upvalue model
  - `errors.md` -- Result-based error handling
  - `api.md` -- trait-based Rust-idiomatic public API
  - `stdlib.md` -- modular standard library
  - `testing.md` -- spec-driven multi-layer testing strategy
  - `references.md` -- classification of studied implementations

### Changed

- Complete rewrite from scratch. Previous implementation (based on
  lua-in-rust) replaced with new architecture:
  - AST-based pipeline (was: single-pass bytecode emission)
  - Register-based VM with PUC-Rio opcodes (was: stack-based custom opcodes)
  - Arena GC with generational indices (was: raw-pointer mark-sweep)
  - Trait-based API (was: stack-based C API mirror)
