# Table Implementation

## Decision

**Dual representation: array part + hash part, following PUC-Rio's
design.**

## Overview

Lua tables are the sole data structuring mechanism in Lua 5.1.1.
They serve as arrays, dictionaries, objects, modules, and namespaces.
Efficient table implementation is critical for performance.

PUC-Rio uses a dual representation: an array part for consecutive
integer keys starting at 1, and a hash part for all other keys. This
optimization gives O(1) access for array-like usage patterns while
supporting arbitrary keys.

## Structure

```rust
pub struct Table {
    array: Vec<Val>,
    map: HashMap<Val, Val>,
    metatable: Option<GcRef<Table>>,
}
```

### Array Part

The array part stores values for integer keys `1..=n` where `n` is
chosen such that at least half the slots in `[1, n]` are non-nil.
This is the same heuristic PUC-Rio uses in `ltable.c`.

Array access for integer keys in range is O(1) with no hashing.

### Hash Part

The hash part stores all non-integer keys and integer keys outside
the array range. Uses Rust's standard `HashMap<Val, Val>`.

The initial implementation uses `HashMap` for simplicity. A future
optimization could switch to PUC-Rio's open-addressing hash with
Brent's variation, which avoids per-entry heap allocation.

### Metatable

Each table has an optional metatable reference. The metatable is
itself a GC-managed table.

## Key Constraints

From the Lua 5.1.1 specification:

- **Nil keys are invalid** — `table[nil]` is a runtime error.
- **NaN keys are invalid** — `table[0/0]` is a runtime error.
- **Nil values mean absent** — Setting `table[k] = nil` removes the
  entry. Reading a missing key returns nil.
- **Integer/float equivalence** — `table[1]` and `table[1.0]` refer
  to the same entry. A number that is exactly an integer uses the
  array part if in range.

## Resizing

When a table grows (via new key insertion), the array and hash parts
are resized together:

1. Count how many integer keys exist in ranges `[1,1]`, `[1,2]`,
   `[1,4]`, `[1,8]`, ... up to the maximum integer key.
2. Choose the largest power of 2 `n` such that more than `n/2` of
   the slots `[1, n]` would be occupied.
3. Resize the array part to `n`.
4. All other keys (including integers outside `[1, n]`) go to the
   hash part.

This heuristic ensures the array part is always at least 50% full,
avoiding waste on sparse tables.

## Length Operator (#)

The `#` operator finds a boundary in a table: an integer index `n`
such that `t[n] ~= nil` and `t[n+1] == nil` (or 0 if `t[1] == nil`).
For tables with holes, the result is undefined (any boundary is
valid). For contiguous arrays starting at 1, it returns the count.

PUC-Rio implements this as a binary search on the array part first.
If the array part is fully occupied (no nil at the end), it extends
the search into the hash part via an exponential+binary search
(`unbound_search` in `ltable.c`).

## Table Traversal

`next(t, key)` returns the next key-value pair after `key` in the
table. Order is:

1. Array part entries (index 1, 2, 3, ...) in order.
2. Hash part entries in hash-internal order (not insertion order,
   not sorted).

The `pairs()` and `next()` functions expose this traversal.
`ipairs()` iterates only integer keys starting at 1 until a nil
value is found.

## Trace Implementation

Tables participate in garbage collection. The `Trace` implementation
marks:

1. The metatable (if present)
2. All keys in both array and hash parts
3. All values in both array and hash parts

For weak tables (`__mode`), the trace behavior changes — weak keys
or values are not marked, allowing them to be collected.
