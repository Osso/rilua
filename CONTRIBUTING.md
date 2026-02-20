# Contributing to rilua

Thank you for considering contributing to rilua. We welcome community
contributions. This document provides guidelines and instructions to make the
contribution process smooth and effective for everyone.

## Table of Contents

- [Code of Conduct](#code-of-conduct)
- [Getting Started](#getting-started)
  - [Set Up Your Environment](#set-up-your-environment)
  - [Find Issues to Work On](#find-issues-to-work-on)
- [Making Contributions](#making-contributions)
  - [Create a Branch](#create-a-branch)
  - [Make Your Changes](#make-your-changes)
  - [Test Your Changes](#test-your-changes)
  - [Submit a Pull Request](#submit-a-pull-request)
- [Coding Guidelines](#coding-guidelines)
- [Documentation](#documentation)
- [Community](#community)
- [Recognition](#recognition)

## Code of Conduct

This project and everyone participating in it follows our community guidelines.
By participating, you are expected to be respectful and constructive.
Please report unacceptable behavior to [the maintainer](mailto:daniel@kogito.network).

## Getting Started

### Set Up Your Environment

1. **Fork the repository**: Click the "Fork" button at the top right of the
   [repository page](https://github.com/wowemulation-dev/rilua).

2. **Clone your fork**:

   ```bash
   git clone https://github.com/your-username/rilua.git
   cd rilua
   ```

3. **Set up the upstream remote**:

   ```bash
   git remote add upstream https://github.com/wowemulation-dev/rilua.git
   ```

4. **Install Rust and tools**:
   - Install [Rust](https://www.rust-lang.org/tools/install) (MSRV: 1.92.0)
   - Install [Mise](https://mise.jdx.dev/) for development tools (optional):
     `mise install` will set up the correct Rust toolchain and tools
   - Run `cargo build` to verify the project builds

### Find Issues to Work On

- Check the [Issues](https://github.com/wowemulation-dev/rilua/issues) tab
  for tasks labeled "good first issue" or "help wanted"
- Review the documentation in the `docs/` directory for planned features

## Making Contributions

### Create a Branch

```bash
# Make sure you're up to date
git checkout main
git pull upstream main

# Create a new branch
git checkout -b my-feature-branch
```

Name your branch descriptively, e.g., `feat/add-userdata-api` or
`fix/gc-sweep-crash`.

### Make Your Changes

1. **Code**: Implement your changes following our
   [Coding Guidelines](#coding-guidelines)
2. **Tests**: Add or update tests for your changes
3. **Documentation**: Update documentation as needed

### Test Your Changes

Before submitting your changes, run the quality gate:

```bash
# Format your code
cargo fmt

# Check for common issues (strict clippy)
cargo clippy --all-targets

# Run tests
cargo test

# Check documentation builds without warnings
cargo doc --no-deps
```

Or run all checks at once with Mise:

```bash
mise run quality-gate
```

If you've added a new feature, consider adding a benchmark:

```bash
cargo bench
```

If your changes affect the `dynmod` feature, also test with:

```bash
cargo clippy --all-targets --features dynmod
cargo test --features dynmod
```

#### Continuous Integration

The CI pipeline runs on all pull requests and pushes to main. It has 5
functional jobs:

1. **Changed Files Detection**: Identifies what changed to scope checks.

2. **Quick Checks** (runs first, fails fast):
   - Code formatting (`cargo fmt -- --check`)
   - Compilation check (`cargo check --all-targets`)
   - Linting (`cargo clippy --all-targets`)

3. **Tests** (MSRV 1.92.0 + stable):
   - `cargo nextest run --profile ci` on both Rust versions

4. **Documentation**:
   - `cargo doc --no-deps` with `RUSTDOCFLAGS=-D warnings`

5. **PUC-Rio Compatibility** (runs after CI passes):
   - Runs the official Lua 5.1.1 test suite (23 tests) against rilua

### Submit a Pull Request

1. **Push your changes**:

   ```bash
   git push origin my-feature-branch
   ```

2. **Create a Pull Request**: Go to the
   [repository page](https://github.com/wowemulation-dev/rilua) and click
   "New Pull Request"

3. **Describe your changes**:
   - Provide a clear title following
     [Conventional Commits](https://www.conventionalcommits.org/) format
   - Explain what you've changed and why
   - Reference any related issues (e.g., "Fixes #42")
   - Include any special instructions for testing

4. **Respond to feedback**: Maintainers may suggest changes to your PR.
   Discuss and make any necessary updates.

## Coding Guidelines

- Follow Rust's official
  [style guide](https://doc.rust-lang.org/1.0.0/style/README.html)
- Use meaningful variable and function names
- Write comments for non-obvious logic (explain "why", not "what")
- Keep functions focused on a single responsibility
- Use `Result` and `Option` for error handling -- no `unwrap()` or `expect()`
  in library code (these are enforced by clippy lints)
- All `unsafe` code must be feature-gated or in `src/platform.rs` with
  `#[allow(unsafe_code)]` and SAFETY comments
- Zero external runtime dependencies -- do not add crate dependencies
  to `[dependencies]` without discussion

### Commit Messages

Follow the [Conventional Commits](https://www.conventionalcommits.org/)
specification:

- `feat:` for new features
- `fix:` for bug fixes
- `perf:` for performance improvements
- `refactor:` for code changes that neither fix bugs nor add features
- `docs:` for documentation changes
- `test:` for test changes
- `chore:` for maintenance tasks

## Documentation

- **Code Comments**: Document functions and non-obvious logic
- **Examples**: Add examples for new features in the `examples/` directory
- **README**: Update the README if your changes add new features or change
  existing functionality
- **Rustdoc**: Add documentation comments (`///`) to public API elements
  with runnable examples where practical

## Community

- **Ask questions**: If you're unsure about something, open an issue
- **Be respectful**: Always be kind and constructive in communications

## Recognition

All contributions are valued. Contributors will be mentioned in release notes
when their contributions are included.

---

## First-Time Contributors

New to open source or Rust? Here are tips to get started:

### Understanding the Codebase

rilua is a Lua 5.1.1 interpreter. The main components are:

- `src/compiler/` -- Lexer, parser, AST, bytecode compiler
- `src/vm/` -- Virtual machine, instruction dispatch, GC, tables, strings
- `src/stdlib/` -- Standard library (base, string, table, math, io, os, etc.)
- `src/platform.rs` -- Platform-specific FFI (centralized)
- `src/lib.rs` -- Public Rust embedding API

See `docs/architecture.md` for a detailed overview.

### Tips

1. **Start small**: Fix a typo, improve documentation, or tackle a "good first
   issue"
2. **Read the tests**: The `tests/` directory and PUC-Rio test suite
   (`lua-5.1-tests/`) show expected behavior
3. **Reference PUC-Rio**: The original C source is in `lua-5.1.1/` for
   cross-referencing

### Learning Resources

- [Rust Book](https://doc.rust-lang.org/book/)
- [Lua 5.1 Reference Manual](https://www.lua.org/manual/5.1/manual.html)
- [How to Contribute to Open Source](https://opensource.guide/how-to-contribute/)

---

Thank you for contributing to rilua.
