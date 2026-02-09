# String Handling

## Decision

**Interned strings with cached hash, stored in the GC arena.
Equality by index comparison (O(1)).**

## Overview

In Lua 5.1.1, strings are immutable values. Equal strings are
interned — only one copy exists in memory. This enables O(1)
equality comparison by comparing references rather than content.

## Structure

```rust
pub struct LuaString {
    data: Box<str>,
    hash: u64,
}
```

Each `LuaString` stores:

- The string content (owned, immutable after creation)
- A precomputed hash value (computed once at creation time)

Strings are allocated in the GC arena and referenced via
`GcRef<LuaString>`. Interning is managed by a separate lookup
table in the `GcHeap`.

## Interning

The `GcHeap` maintains an interning table:

```rust
pub struct GcHeap {
    string_intern: HashMap<u64, Vec<GcRef<LuaString>>>,
    // ... other fields
}
```

When creating a new string:

1. Compute the hash of the content.
2. Look up the hash in `string_intern`.
3. For each candidate with the same hash, compare content.
4. If found, return the existing `GcRef`.
5. If not found, allocate in the arena, insert into `string_intern`,
   return the new `GcRef`.

This ensures that equal strings always have the same `GcRef`,
enabling O(1) equality comparison.

## Hashing

PUC-Rio Lua 5.1.1 uses a sampling hash for long strings (>32
characters) to avoid hashing the entire content. For short strings,
all characters are included.

We use Rust's standard `HashMap` hasher initially. The hash is
computed once and cached in `LuaString.hash`. A future optimization
could use PUC-Rio's sampling hash for compatibility and performance.

## String Equality

Two strings are equal if and only if they have the same `GcRef`
(same arena index). This is guaranteed by interning — there is
exactly one `GcRef` for each distinct string value.

```rust
impl PartialEq for Val {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Val::Str(a), Val::Str(b)) => a == b,  // Index comparison
            // ...
        }
    }
}
```

## GC Interaction

Strings are GC-managed objects. They are swept during the
`SweepString` phase. When a string is collected:

1. Remove it from the interning table.
2. Free the arena slot.

Strings are never weak-cleared from weak tables (Lua 5.1.1
semantics — strings are treated as values for weak table purposes).

## String Metatable

All strings share a single metatable (the `string` library table).
This enables method-call syntax: `("hello"):upper()`. The string
metatable is stored in the VM state and applied to all string
values via `__index`.

## Concatenation

String concatenation (`..`) creates a new string. When concatenating
multiple values (`a .. b .. c`), the CONCAT instruction handles
the range `R(B)` through `R(C)`, building the result in a single
allocation.
