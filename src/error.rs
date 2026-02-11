//! Error types for rilua.
//!
//! Maps to PUC-Rio's error status codes:
//! - `LUA_ERRRUN` (2) — runtime error
//! - `LUA_ERRSYNTAX` (3) — syntax error during parsing
//! - `LUA_ERRMEM` (4) — memory allocation error
//! - `LUA_ERRERR` (5) — error in error handler

use std::fmt;

/// Result type alias used throughout rilua.
pub type LuaResult<T> = Result<T, LuaError>;

/// Top-level error type for all rilua errors.
///
/// Corresponds to PUC-Rio's `LUA_ERR*` status codes. Every fallible
/// operation in the library returns `LuaResult<T>`.
#[derive(Debug)]
pub enum LuaError {
    /// Syntax error during lexing or parsing (`LUA_ERRSYNTAX`).
    Syntax(SyntaxError),

    /// Runtime error during VM execution (`LUA_ERRRUN`).
    ///
    /// The error object may be any Lua value, not just a string.
    /// PUC-Rio's `error()` function can throw tables, numbers, etc.
    Runtime(RuntimeError),

    /// Memory allocation failure (`LUA_ERRMEM`).
    ///
    /// PUC-Rio pushes `"not enough memory"` as the error message.
    Memory,

    /// Error in error handler (`LUA_ERRERR`).
    ///
    /// Occurs when an error is raised while running the `xpcall`
    /// error handler, or when a C stack overflow persists during
    /// error recovery.
    ErrorHandler,

    /// I/O error from file operations.
    ///
    /// Wraps `std::io::Error` for file loading and the I/O library.
    Io(std::io::Error),

    /// Coroutine yield signal (`LUA_YIELD`).
    ///
    /// Not a real error -- used to propagate yield through the Rust call
    /// stack back to the resume handler. The `u32` is the number of
    /// yielded values on the coroutine's stack.
    ///
    /// Must NOT be caught by `pcall`/`xpcall`. The `n_ccalls > 0` check
    /// in `coroutine.yield()` prevents yield from inside C-call boundaries
    /// (metamethods, pcall, etc.), so this variant only appears in the
    /// resume path.
    Yield(u32),
}

/// Syntax error with source location.
///
/// Produced by the lexer and parser. Message format matches PUC-Rio:
/// `"source:line: message near 'token'"`.
#[derive(Debug)]
pub struct SyntaxError {
    /// Error description (e.g., `"')' expected near 'end'"`).
    pub message: String,
    /// Source name (e.g., `"stdin"`, `"@filename.lua"`, `"[string \"...\"]"`).
    pub source: String,
    /// Line number where the error was detected (1-based).
    pub line: u32,
}

/// Runtime error with error object and traceback.
///
/// In Lua 5.1.1, `error()` can throw any value. The error object
/// propagates through `pcall` and `xpcall`. Tracebacks are generated
/// on demand (by `debug.traceback` in `xpcall` handlers), but we
/// store trace entries for generating formatted messages.
#[derive(Debug)]
pub struct RuntimeError {
    /// Human-readable error message.
    ///
    /// For errors raised by the VM (type errors, arithmetic errors,
    /// etc.), this contains the formatted message matching PUC-Rio's
    /// wording. For errors raised by `error(obj)`, this is the
    /// string representation of `obj`.
    pub message: String,

    /// Stack level at which the error was raised.
    ///
    /// 0 = error position itself, 1 = caller, 2 = caller's caller.
    /// Corresponds to the `level` parameter of `error(msg, level)`.
    pub level: u32,

    /// Stack traceback entries, from innermost to outermost.
    pub traceback: Vec<TraceEntry>,
}

/// A single entry in a stack traceback.
#[derive(Debug, Clone)]
pub struct TraceEntry {
    /// Source name (e.g., `"stdin"`, `"@file.lua"`).
    pub source: String,
    /// Line number (1-based, 0 if unknown).
    pub line: u32,
    /// Function name, if known.
    pub name: Option<String>,
}

impl fmt::Display for LuaError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Syntax(e) => write!(f, "{e}"),
            Self::Runtime(e) => write!(f, "{e}"),
            Self::Memory => write!(f, "not enough memory"),
            Self::ErrorHandler => write!(f, "error in error handling"),
            Self::Io(e) => write!(f, "{e}"),
            Self::Yield(_) => write!(f, "cannot resume dead coroutine"),
        }
    }
}

impl fmt::Display for SyntaxError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}:{}: {}", self.source, self.line, self.message)
    }
}

impl fmt::Display for RuntimeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.message)
    }
}

impl fmt::Display for TraceEntry {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}:{}", self.source, self.line)?;
        if let Some(name) = &self.name {
            write!(f, " in function '{name}'")?;
        }
        Ok(())
    }
}

impl std::error::Error for LuaError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(e) => Some(e),
            Self::Syntax(_)
            | Self::Runtime(_)
            | Self::Memory
            | Self::ErrorHandler
            | Self::Yield(_) => None,
        }
    }
}

impl From<std::io::Error> for LuaError {
    fn from(err: std::io::Error) -> Self {
        Self::Io(err)
    }
}

impl From<SyntaxError> for LuaError {
    fn from(err: SyntaxError) -> Self {
        Self::Syntax(err)
    }
}

impl From<RuntimeError> for LuaError {
    fn from(err: RuntimeError) -> Self {
        Self::Runtime(err)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn memory_error_display() {
        let err = LuaError::Memory;
        assert_eq!(err.to_string(), "not enough memory");
    }

    #[test]
    fn error_handler_display() {
        let err = LuaError::ErrorHandler;
        assert_eq!(err.to_string(), "error in error handling");
    }

    #[test]
    fn syntax_error_display() {
        let err = LuaError::Syntax(SyntaxError {
            message: "')' expected near 'end'".into(),
            source: "stdin".into(),
            line: 3,
        });
        assert_eq!(err.to_string(), "stdin:3: ')' expected near 'end'");
    }

    #[test]
    fn runtime_error_display() {
        let err = LuaError::Runtime(RuntimeError {
            message: "attempt to perform arithmetic on a string value".into(),
            level: 0,
            traceback: vec![],
        });
        assert_eq!(
            err.to_string(),
            "attempt to perform arithmetic on a string value"
        );
    }

    #[test]
    fn runtime_error_with_location() {
        let err = RuntimeError {
            message: "stdin:5: attempt to index a nil value".into(),
            level: 1,
            traceback: vec![TraceEntry {
                source: "stdin".into(),
                line: 5,
                name: Some("foo".into()),
            }],
        };
        assert_eq!(err.to_string(), "stdin:5: attempt to index a nil value");
        assert_eq!(err.traceback[0].to_string(), "stdin:5 in function 'foo'");
    }

    #[test]
    fn trace_entry_without_name() {
        let entry = TraceEntry {
            source: "@test.lua".into(),
            line: 10,
            name: None,
        };
        assert_eq!(entry.to_string(), "@test.lua:10");
    }

    #[test]
    fn io_error_conversion() {
        let io_err = std::io::Error::new(std::io::ErrorKind::NotFound, "file not found");
        let err: LuaError = io_err.into();
        assert!(matches!(err, LuaError::Io(_)));
        assert_eq!(err.to_string(), "file not found");
    }

    #[test]
    fn syntax_error_conversion() {
        let syn = SyntaxError {
            message: "unexpected symbol".into(),
            source: "test".into(),
            line: 1,
        };
        let err: LuaError = syn.into();
        assert!(matches!(err, LuaError::Syntax(_)));
    }

    #[test]
    fn runtime_error_conversion() {
        let rt = RuntimeError {
            message: "error".into(),
            level: 0,
            traceback: vec![],
        };
        let err: LuaError = rt.into();
        assert!(matches!(err, LuaError::Runtime(_)));
    }

    #[test]
    fn error_is_std_error() {
        let err = LuaError::Memory;
        let _: &dyn std::error::Error = &err;
    }
}
