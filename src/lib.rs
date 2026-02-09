//! rilua — Lua 5.1.1 implemented in Rust.
//!
//! A from-scratch implementation targeting behavioral equivalence with
//! the PUC-Rio reference interpreter. Designed for embedding in Rust
//! applications, with a focus on the World of Warcraft addon variant.
//!
//! # Architecture
//!
//! Pipeline: Source → Lexer → Parser → AST → Compiler → Proto → VM
//!
//! See `docs/architecture.md` for design documentation.
