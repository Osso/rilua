# Value Representation

## Decision

**Rust enum (`Val`) with GC references as generational arena indices.**

## The Val Enum

```rust
/// A Lua value.
///
/// All Lua values are represented by this enum. Value types (nil,
/// boolean, number) are stored inline. Reference types (string,
/// table, function, userdata, thread) store an index into the GC
/// arena.
#[derive(Clone, Copy)]
pub enum Val {
    Nil,
    Bool(bool),
    Num(f64),
    Str(GcRef<LuaString>),
    Table(GcRef<Table>),
    Function(GcRef<Closure>),
    RustFunction(RustFn),
    Userdata(GcRef<Userdata>),
    Thread(GcRef<LuaThread>),
    LightUserdata(*const ()),
}
```

## Design Rationale

### Why an Enum (Not NaN-boxing)

PUC-Rio uses a C tagged union (`TValue`: 16 bytes — 8-byte `Value`
union + 4-byte `int tt` + padding). NaN-boxing packs type tags into
the unused bits of `f64` NaN values, reducing size to 8 bytes.

We use a plain Rust enum because:

1. **Safety** — NaN-boxing requires reinterpreting bit patterns between
   `f64` and pointers, which is inherently unsafe. Rust enums provide
   exhaustive matching and no undefined behavior.
2. **Clarity** — `match val { Val::Num(n) => ... }` is readable.
   NaN-boxing requires bit extraction macros.
3. **Correctness** — NaN-boxing constrains pointer values to 48 bits.
   On 64-bit systems with 5-level page tables, this may not hold.
   Arena indices (u32) have no such constraint.
4. **Adequate performance** — The enum will be 16-24 bytes depending
   on alignment. This is comparable to PUC-Rio's 16-byte TValue.
   The extra bytes are unlikely to be a bottleneck.

### GcRef for Reference Types

Reference types (strings, tables, closures, userdata, threads) are
managed by the garbage collector. They are stored in typed arenas
and referenced by `GcRef` — a generational index:

```rust
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct GcRef<T> {
    index: u32,
    generation: u32,
    _marker: PhantomData<T>,
}
```

See [gc.md](gc.md) for details on the arena and GC design.

### RustFn for Native Functions

Rust functions callable from Lua have the signature:

```rust
pub type RustFn = fn(&mut Lua) -> Result<u32>;
```

Where the return value is the number of results pushed onto the
stack. This is stored inline in the `Val` enum (it is a function
pointer, not a closure — sized 8 bytes).

### LightUserdata

Light userdata is an unmanaged raw pointer. It is not garbage
collected, has no metatable, and is compared by pointer value.
Included for Lua 5.1.1 completeness.

## Equality Semantics

Lua 5.1.1 equality rules (Section 2.5.2 of the reference manual):

| Type | Comparison |
|------|------------|
| nil | Always equal to nil, nothing else |
| boolean | By value |
| number | By numeric value (with float semantics) |
| string | By content (pointer equality due to interning) |
| table | By reference (same GcRef) |
| function | By reference (same GcRef) |
| userdata | By reference (same GcRef) |
| thread | By reference (same GcRef) |

Metatables can override equality via `__eq` metamethod for tables
and userdata only. In Lua 5.1.1, `__eq` is NOT checked for
functions, strings, or threads — these always use raw reference
comparison.

Light userdata is compared by pointer value (not shown in the table
above because it is a raw pointer, not a GcRef).

## Hashing

`Val` implements `Hash` for use as table keys:

- `Nil` — not hashable (rejected as table key)
- `Bool` — hash the boolean value
- `Num` — hash the f64 bits, normalizing `-0.0` to `+0.0` and
  rejecting NaN (not a valid table key)
- `Str` — use the string's cached hash
- Reference types — hash the GcRef index

## Truthiness

Lua 5.1.1 truthiness: `nil` and `false` are falsy. Everything else
(including `0`, `0.0`, and `""`) is truthy.

```rust
impl Val {
    pub fn is_truthy(&self) -> bool {
        !matches!(self, Val::Nil | Val::Bool(false))
    }
}
```
