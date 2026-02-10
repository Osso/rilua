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
    nodes: Vec<Node>,
    last_free: usize,
    metatable: Option<GcRef<Table>>,
}

struct Node {
    key: Val,
    value: Val,
    next: Option<usize>,  // index into nodes[] for collision chain
}
```

### Array Part

The array part stores values for integer keys `1..=n` where `n` is
chosen such that more than half the slots in `[1, n]` are non-nil.
This is the same heuristic PUC-Rio uses in `ltable.c`.

Array access for integer keys in range is O(1) with no hashing.
Integer-valued floats use the array part: `t[1]` and `t[1.0]` access
the same slot. A number qualifies as integer if truncating it to an
integer and converting back produces the same value.

### Hash Part

The hash part uses open-addressing with chained scatter and Brent's
collision resolution. All nodes reside in a single flat `Vec<Node>`
whose size is always a power of 2.

**Empty table sentinel**: when the hash part is empty, `nodes` is
represented by a shared dummy node (a static empty node with nil key
and nil value). This avoids special-casing empty hash parts in get
operations -- they walk the chain, find nil, and return nothing.

**Main position** (home bucket) is computed per key type:

| Key type | Hash method |
|----------|-------------|
| String | Cached hash from interning, `hash & (size-1)` |
| Number | Copy double bytes to ints, sum, apply `& (size-1)`. Add 1.0 before hashing to normalize `-0.0` |
| Boolean | `hash(b as u32) & (size-1)` |
| Other GC objects | Hash the arena index / pointer |

**Free position scanning**: `last_free` starts at the end of the
nodes array and scans backward. A slot is free if its key is nil.
`last_free` is never reset during normal operation (only during
rehash). When it reaches 0, the next insertion triggers a rehash.

### Metatable

Each table has an optional metatable reference. The metatable is
itself a GC-managed table.

### Insertion Algorithm (newkey)

When a key is not found in the table, `newkey` inserts it:

1. Compute `mp = mainposition(key)` — the home bucket.
2. If `mp` is free (value is nil and not the dummy node), place the
   key directly.
3. If `mp` is occupied, get a free slot via `last_free`. If no free
   slots remain, trigger a rehash then retry.
4. Check whether the node occupying `mp` is in its own main position:
   - **Case A** (interloper — not in its main position): Move the
     interloper to the free slot. Walk the interloper's home chain to
     find its predecessor, repoint predecessor's `next` to the free
     slot. Clear `mp`. Place the new key at `mp`.
   - **Case B** (owner — in its own main position): Place the new key
     at the free slot. Insert the free slot into `mp`'s chain.
5. Write the key into the chosen node. Return a mutable reference to
   the (currently nil) value slot.

This is Brent's variation: when the home bucket is occupied by a
non-owner, the non-owner is relocated so the new key can occupy its
home position. This keeps chains short even at high load factors.

**Main invariant**: if a node is not in its main position, then the
node at its main position IS in that node's own main position.

### Get Operations

**Integer key lookup** (`getnum`): unsigned comparison
`(key-1) < sizearray` simultaneously tests `key >= 1` and
`key <= sizearray` in one check (negative values wrap to large
unsigned values). If in range, returns `array[key-1]` directly.
Otherwise converts to float, hashes, and walks the collision chain.

**String key lookup** (`getstr`): hashes using the cached string
hash, walks the collision chain comparing by pointer identity
(all strings are interned).

**Generic key lookup** (`get`): dispatches by type. For numbers,
truncates to integer; if the number equals its truncation (e.g.,
`5.0`), delegates to `getnum`. For non-integer numbers and all other
types, hashes via `mainposition` and walks the chain.

**Set operations**: first try `get`. If the key exists, overwrite
the value in place. If not found, validate the key (nil and NaN
are errors), then call `newkey`.

## Key Constraints

From the Lua 5.1.1 specification:

- **Nil keys are invalid** — `table[nil]` is a runtime error
  (`"table index is nil"`).
- **NaN keys are invalid** — `table[0/0]` is a runtime error
  (`"table index is NaN"`). NaN is detected via `!(a == a)`.
- **Nil values mean absent** — Setting `table[k] = nil` removes the
  entry. Reading a missing key returns nil.
- **Integer/float equivalence** — `table[1]` and `table[1.0]` refer
  to the same entry. A number that is exactly an integer uses the
  array part if in range.
- **Negative zero** — `-0.0` and `+0.0` are the same key. They hash
  identically (the `+1.0` normalization in `hashnum` ensures this)
  and compare equal under IEEE 754.

## Resizing

Rehash is triggered inside `newkey` when `last_free` finds no free
slots. The resize recomputes the optimal array/hash split for the
current key distribution plus the new key being inserted.

### Phase 1: Count integer keys by power-of-2 range

Build a histogram `nums[i]` counting integer keys where
`2^(i-1) < k <= 2^i`. Scan both the array part and hash part.
Non-integer keys, zero, and negative keys are excluded.

### Phase 2: Compute optimal array size

Iterate through power-of-2 sizes `1, 2, 4, 8, ...`. Accumulate
total integer keys seen. At each size `2^i`, if more than half the
slots would be occupied (`count > 2^i / 2`), record `2^i` as the
best array size so far. The threshold is **strictly greater than
50% occupancy**.

### Phase 3: Resize

1. Save the old hash array pointer.
2. Grow the array part if needed (realloc + nil-fill new slots).
3. Allocate a new hash part (fresh nodes array). The old hash is
   still accessible.
4. If the array is shrinking, move displaced array elements into
   the new hash part.
5. Re-insert all old hash entries into the new hash (in reverse
   order, which is an optimization for the backward `last_free`
   scanning).
6. Free the old hash array.

The resize does not work in-place. Both the old and new hash parts
exist simultaneously during re-insertion.

## Length Operator (#)

The `#` operator finds a boundary in a table: an integer index `n`
such that `t[n] ~= nil` and `t[n+1] == nil` (or 0 if `t[1] == nil`).
For tables with holes, the result is undefined (any boundary is
valid). For contiguous arrays starting at 1, it returns the count.

### Algorithm

1. Let `j = sizearray`. If `j > 0` and `array[j-1]` is nil, there
   is a boundary within the array part. Binary search between `i=0`
   and `j=sizearray`:
   - Invariant: `array[i]` is non-nil (or `i=0`), `array[j-1]` is nil.
   - `m = (i+j)/2`. If `array[m-1]` is nil, set `j=m`. Else set `i=m`.
   - Return `i`.

2. If the last array slot is non-nil (or array is empty):
   - Hash part is empty: return `j` (the array size).
   - Hash part exists: call `unbound_search(j)`.

### Unbound search

For integer keys beyond the array part (stored in the hash part):

1. Start with `i = j` (array size, known present) and `j = j + 1`.
2. Exponential probe: double `j` until `t[j]` is nil (checking both
   array and hash parts).
3. Binary search between `i` (present) and `j` (absent) to find the
   exact boundary.
4. Overflow guard: if `j` exceeds `MAX_INT`, fall back to linear
   scan from 1.

## Table Traversal

`next(t, key)` returns the next key-value pair after `key` in the
table. Order is:

1. Array part entries (index 1, 2, 3, ...) in order.
2. Hash part entries in hash-internal order (not insertion order,
   not sorted).

The `pairs()` and `next()` functions expose this traversal.
`ipairs()` iterates only integer keys starting at 1 until a nil
value is found.

### findindex Algorithm

`findindex(key)` converts a key into a unified index for iteration:

- Nil key returns -1 (signals start of iteration; incrementing
  gives index 0).
- Integer key within array range returns the 0-based array index.
- All other keys: hash the key, walk the collision chain, compute
  the node index via pointer arithmetic, then offset by `sizearray`.

**Dead key support**: the GC may mark a key as dead
(`LUA_TDEADKEY`) when its value is nil'd during sweep. `findindex`
still recognizes dead keys by comparing GC object pointers. This
allows `next()` to work correctly even if GC runs between iterations.

If the key is not found at all, `next()` raises `"invalid key to
'next'"` — this happens when the table was structurally modified
during iteration (rehashed).

## Trace Implementation

Tables participate in garbage collection. The `Trace` implementation
marks:

1. The metatable (if present)
2. All keys in both array and hash parts
3. All values in both array and hash parts

For weak tables (`__mode`), the trace behavior changes — weak keys
or values are not marked, allowing them to be collected.
