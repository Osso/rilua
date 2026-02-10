# String Handling

## Decision

**Interned strings with cached hash, stored in the GC arena.
Equality by index comparison (O(1)).**

## Overview

In Lua 5.1.1, strings are immutable values. Equal strings are
interned -- only one copy exists in memory. This enables O(1)
equality comparison by comparing references rather than content.

PUC-Rio stores strings in a dedicated hash table (`strt`) separate
from the main GC object list. rilua adapts this: strings live in
the GC arena like all other objects, and a separate interning table
maps hashes to arena references.

## Structure

```rust
pub struct LuaString {
    hash: u32,
    data: Box<[u8]>,
}
```

Each `LuaString` stores:

- `hash`: precomputed hash value (32-bit unsigned, matching PUC-Rio's
  `unsigned int`)
- `data`: the string content as a byte slice (immutable after creation,
  may contain embedded nulls)

Strings are allocated in the GC arena and referenced via
`GcRef<LuaString>`.

### PUC-Rio TString Layout (Reference)

In the C implementation (`lobject.h`):

```c
typedef union TString {
    L_Umaxalign dummy;
    struct {
        CommonHeader;       // next, tt, marked
        lu_byte reserved;   // 0 for normal, 1-21 for keywords
        unsigned int hash;
        size_t len;
    } tsv;
} TString;
// String data stored immediately after the struct: (char *)(ts + 1)
```

rilua does not need the `reserved` field. Keyword detection is
handled by the lexer's keyword lookup table, not by marking strings.

## Hash Algorithm

PUC-Rio uses a step-based hash that samples characters at intervals.
rilua must use the same algorithm to match behavioral equivalence
for `next()` iteration order, table key distribution, and any code
that depends on hash-sensitive ordering.

### Algorithm (from `lstring.c`)

```text
function hash(str, len):
    h = len                          // seed with length
    step = (len >> 5) + 1            // step = (len / 32) + 1
    l1 = len
    while l1 >= step:
        h = h XOR ((h << 5) + (h >> 2) + str[l1 - 1])
        l1 = l1 - step
    return h
```

```rust
fn lua_hash(data: &[u8]) -> u32 {
    let len = data.len();
    let mut h = len as u32;
    let step = (len >> 5) + 1;
    let mut l1 = len;
    while l1 >= step {
        h = h ^ ((h << 5)
            .wrapping_add(h >> 2)
            .wrapping_add(u32::from(data[l1 - 1])));
        l1 -= step;
    }
    h
}
```

### Sampling Behavior

The step value controls how many characters are hashed:

| String length | Step | Characters sampled |
|--------------|------|-------------------|
| 1-31 | 1 | all |
| 32-63 | 2 | ~half |
| 64-95 | 3 | ~third |
| 96-127 | 4 | ~quarter |
| n | (n/32)+1 | ~32 |

For strings shorter than 32 bytes, every character is hashed
(step = 1). For longer strings, approximately 32 characters are
sampled, walking backward from the end. This bounds hash computation
time for very long strings.

The loop walks backward: it samples `str[len-1]`, `str[len-1-step]`,
`str[len-1-2*step]`, etc. This means the last characters of a string
have more influence on the hash than the first characters.

### Hash-to-Bucket Mapping

```text
bucket = h & (size - 1)     // size is always a power of 2
```

This is a bitwise AND, not modulo. It requires the table size to be
a power of 2.

## Interning Table

The GC heap maintains a string interning table:

```rust
pub struct StringTable {
    buckets: Vec<Vec<GcRef<LuaString>>>,
    count: usize,
}
```

Each bucket is a list of `GcRef<LuaString>` values whose hashes
map to that bucket index. The table size is always a power of 2.

### PUC-Rio stringtable (Reference)

```c
typedef struct stringtable {
    GCObject **hash;    // array of hash bucket chains
    lu_int32 nuse;      // number of strings in the table
    int size;           // number of buckets (power of 2)
} stringtable;
```

PUC-Rio chains strings through the GC `next` pointer in the string
header. rilua uses `Vec<GcRef>` per bucket instead, since our arena
GC does not use linked lists for object management.

### Initial Size

`MINSTRTABSIZE` = 32 buckets. The table starts at this size and
grows/shrinks dynamically.

### Interning Algorithm

When creating a string:

```text
function intern(str, len):
    h = hash(str, len)
    bucket = h & (table.size - 1)

    // Search existing strings in this bucket
    for each ref in table.buckets[bucket]:
        s = arena.get(ref)
        if s.hash == h and s.data.len == len and s.data == str:
            if is_dead(ref):
                resurrect(ref)      // flip to current white
            return ref              // found existing string

    // Not found: create new string
    new_string = LuaString { hash: h, data: str.into() }
    ref = arena.alloc(new_string)
    table.buckets[bucket].push(ref)
    table.count += 1

    // Check if resize needed
    if table.count > table.size:
        resize(table.size * 2)

    return ref
```

Key details:

1. **Hash first, then compare**: the hash is compared before the
   content to avoid expensive `memcmp` on hash collisions.
2. **Length check before content**: `len` comparison is cheaper than
   byte comparison.
3. **Resurrection**: during GC, a string may be marked dead (wrong
   white) but not yet freed. If interning finds it, flip its color
   to the current white to keep it alive. This happens when a string
   is created, collected, then the same content is interned again
   before the sweep reaches that bucket.
4. **Load factor**: resize when `count > size` (100% load factor).
   PUC-Rio uses this threshold.

### Resize Algorithm

Resizing redistributes all strings into a new bucket array:

```text
function resize(new_size):
    // Cannot resize during string sweep phase
    if gc_state == SweepString:
        return

    new_buckets = allocate new_size empty buckets

    // Rehash all strings
    for each old_bucket in table.buckets:
        for each ref in old_bucket:
            s = arena.get(ref)
            new_index = s.hash & (new_size - 1)
            new_buckets[new_index].push(ref)

    table.buckets = new_buckets
    table.size = new_size
```

**When resize is triggered:**

- **Growth**: after inserting a new string, if `count > size` and
  `size <= MAX_INT / 2`, double to `size * 2`.
- **Shrink**: during GC `checkSizes` (after sweep completes), if
  `count < size / 4` and `size > MINSTRTABSIZE * 2`, halve to
  `size / 2`.
- **Never during string sweep**: resizing would corrupt the
  bucket-by-bucket sweep state. The resize function checks the GC
  phase and returns early if sweeping strings.

## String Equality

Two strings are equal if and only if they have the same `GcRef`
(same arena index). Interning guarantees exactly one `GcRef` per
distinct string value.

```rust
// In Val equality
(Val::Str(a), Val::Str(b)) => a == b,  // GcRef index comparison
```

This is O(1) regardless of string length.

## GC Interaction

### Sweep Phase

PUC-Rio sweeps the string table in a dedicated phase
(`GCSsweepstring`) before sweeping other objects. It processes one
bucket per GC step for bounded pause times.

In rilua, the string table sweep:

1. Iterate through each bucket.
2. For each entry, check if the referenced string is dead (marked
   with the previous cycle's white).
3. If dead: remove from the bucket, decrement `count`, free the
   arena slot.
4. If alive: flip to the current white for the next cycle.

```text
function sweep_string_bucket(bucket_index):
    bucket = table.buckets[bucket_index]
    retain only entries where is_alive(ref)
    for removed entries:
        table.count -= 1
        arena.free(ref)
```

### Incremental Sweep

To keep GC pauses bounded, sweep one bucket per step. Track the
current bucket index in the GC state (`sweep_string_index`). When
all buckets are swept, advance to the next GC phase.

### Size Check After Sweep

After all string buckets are swept, check if the table should shrink:

```text
if table.count < table.size / 4 and table.size > MINSTRTABSIZE * 2:
    resize(table.size / 2)
```

### Strings Are Never Gray

Strings contain no references to other GC objects. They are marked
directly (white -> black) with no traversal. They never appear on
the gray list.

### Strings and Weak Tables

Strings are never cleared from weak tables (Lua 5.1.1 semantics).
Even in a table with `__mode = "k"` or `__mode = "v"`, string keys
and values are treated as non-collectable values, like numbers and
booleans.

## String Fixation

Certain strings are "fixed" -- immune to garbage collection:

- **Reserved words** (21 keywords): "and", "break", "do", "else",
  "elseif", "end", "false", "for", "function", "if", "in", "local",
  "nil", "not", "or", "repeat", "return", "then", "true", "until",
  "while"
- **Metamethod names** (17 events): "__index", "__newindex", "__gc",
  "__mode", "__eq", "__add", "__sub", "__mul", "__div", "__mod",
  "__pow", "__unm", "__len", "__lt", "__le", "__concat", "__call"
- **The out-of-memory error message**: "not enough memory"

In PUC-Rio, fixation sets bit 5 (`FIXEDBIT`) in the GC mark byte.
In rilua, fixed strings can be tracked by a flag in the arena slot
or by holding persistent `GcRef` values that prevent collection.

Fixed strings are interned during VM initialization and never freed.

## String Metatable

All strings share a single metatable stored in the VM state. This
metatable has `__index` pointing to the `string` library table,
enabling method-call syntax:

```lua
local upper = ("hello"):upper()   -- calls string.upper("hello")
local len = ("hello"):len()       -- calls string.len("hello")
```

The string metatable is one of the per-type metatables stored in the
global state (`mt[LUA_TSTRING]`). It is set during string library
initialization.

## Concatenation

### Single Concatenation

`a .. b` creates a new interned string from the concatenated content.
Both operands must be strings or numbers (numbers are coerced to
strings via `"%.14g"` format). If neither is a string/number, the
`__concat` metamethod is invoked.

### Batch Concatenation (OP_CONCAT)

The CONCAT instruction handles `R(A) := R(B) .. R(B+1) .. ... .. R(C)`
in a single operation. The algorithm:

1. Start from the rightmost pair: concatenate `R(C-1)` and `R(C)`.
2. Place the result in `R(C-1)`.
3. Repeat, working leftward, until `R(B)` is reached.
4. The final result goes into `R(A)`.

Each intermediate concatenation may trigger a `__concat` metamethod
if an operand is not a string or number.

The compiler optimizes chained concatenation (`a .. b .. c .. d`)
into a single CONCAT instruction covering the full register range,
avoiding intermediate string allocations for the common case where
all operands are strings/numbers.

## Number-to-String Conversion

When a number is used where a string is expected (concatenation,
`tostring`, `print`), it is converted using the format `"%.14g"`.
This is the `LUA_NUMBER_FMT` constant from `luaconf.h`.

Examples:

| Number | String |
|--------|--------|
| 1.0 | `"1"` |
| 1.5 | `"1.5"` |
| 100000.0 | `"100000"` |
| 1e15 | `"1e+15"` |
| 0.0/0.0 | `"-nan"` or `"nan"` (platform-dependent) |
| 1.0/0.0 | `"inf"` |

The `%g` format strips trailing zeros and uses exponential notation
for very large or very small numbers.

## String-to-Number Conversion

When a string is used where a number is expected (arithmetic,
comparison with a number), Lua attempts conversion using C's
`strtod`. The conversion:

1. Skips leading whitespace.
2. Accepts optional sign (`+` or `-`).
3. Accepts decimal notation (`123.456`) or hex notation (`0xff`).
4. Accepts optional exponent (`e10`, `E-3`, `p2` for hex).
5. Skips trailing whitespace.
6. Fails if any non-whitespace characters remain after the number.

If conversion fails, a runtime error is raised (for arithmetic) or
the comparison falls through to a metamethod.
