//! Closures and upvalues.
//!
//! A closure pairs a function prototype (or Rust function pointer)
//! with captured upvalues. Lua closures reference a `Proto` via `Rc`
//! and an array of upvalues. Upvalues start "open" (pointing into the
//! stack) and are "closed" (copied to the heap) when the creating
//! scope exits.
//!
//! Full implementation in Phase 3b.

/// A Lua or Rust closure with captured upvalues.
///
/// Placeholder -- variants and fields will be added in Phase 3b.
pub struct Closure;
