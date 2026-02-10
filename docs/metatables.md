# Metatables and Type Coercion

## Decision

**Metamethod dispatch follows PUC-Rio Lua 5.1.1 semantics exactly.
Type coercion is implicit for arithmetic (string to number) and
concatenation (number to string). All other conversions require
explicit calls.**

Reference: `lvm.c`, `ltm.c`, `ltm.h`, `lobject.c` in PUC-Rio Lua
5.1.1, and [Lua 5.1 Reference Manual section 2.8](https://lua.org/manual/5.1/manual.html#2.8).

## Metamethod Events

Lua 5.1.1 defines 17 metamethod events. The first 5 use fast-path
lookup with caching in the metatable's flags field.

| Index | Name | Category | Notes |
|-------|------|----------|-------|
| 0 | `__index` | Indexing | Fast-path. Table or function. |
| 1 | `__newindex` | Indexing | Fast-path. Table or function. |
| 2 | `__gc` | Lifecycle | Fast-path. Userdata only. |
| 3 | `__mode` | Lifecycle | Fast-path. Weak table mode string. |
| 4 | `__eq` | Comparison | Fast-path (last cached event). |
| 5 | `__add` | Arithmetic | |
| 6 | `__sub` | Arithmetic | |
| 7 | `__mul` | Arithmetic | |
| 8 | `__div` | Arithmetic | |
| 9 | `__mod` | Arithmetic | |
| 10 | `__pow` | Arithmetic | |
| 11 | `__unm` | Arithmetic | Unary negation. |
| 12 | `__len` | Length | Not called for tables in 5.1. |
| 13 | `__lt` | Comparison | |
| 14 | `__le` | Comparison | Falls back to `__lt`. |
| 15 | `__concat` | String | |
| 16 | `__call` | Call | |

Events 0-4 use `fasttm()` which checks a flags byte in the
metatable before performing a full table lookup. Events 5-16 always
use `luaT_gettmbyobj()` (full lookup by object type).

## Arithmetic Metamethods

Applies to: `__add`, `__sub`, `__mul`, `__div`, `__mod`, `__pow`.

### Dispatch algorithm

```
1. Try tonumber(left_operand)
2. Try tonumber(right_operand)
3. If BOTH succeed: perform native f64 arithmetic, return result
4. If EITHER fails:
   a. Look up metamethod on left operand's metatable
   b. If not found, look up on right operand's metatable
   c. If found and is a function: call(metamethod, left, right)
   d. If not found: raise "attempt to perform arithmetic on a T value"
```

### Unary negation (__unm)

```
1. If operand is already a number: return -operand
2. Try tonumber(operand)
3. If tonumber succeeds: return -result
4. Look up __unm on operand's metatable
5. If found: call(__unm, operand, operand)  -- both args are the same
6. If not found: raise arithmetic error
```

Both operands are passed to `__unm` for consistency with the binary
metamethod calling convention.

## Comparison Metamethods

### Equality (__eq)

Only consulted for tables and userdata. Never for nil, boolean,
number, string, or light userdata (these use direct comparison).

```
1. If types differ: not equal (no metamethod)
2. If same reference: equal (no metamethod)
3. Look up __eq on left operand's metatable
4. If not found: not equal
5. Look up __eq on right operand's metatable
6. If both metamethods are the same function (raw equality): call it
7. If metamethods differ: not equal (no call)
8. Result: interpret return value as boolean (false/nil = false)
```

The consistency check (step 6-7) prevents `a == b` and `b == a`
from giving different results.

### Less than (__lt)

```
1. If types differ: raise "attempt to compare T with T"
2. If both numbers: native f64 comparison
3. If both strings: lexicographic comparison (strcoll-based)
4. Look up __lt on left operand's metatable
5. If not found: look up __lt on right operand's metatable
6. If both have same metamethod: call(metamethod, left, right)
7. If metamethods differ or not found: raise order error
```

### Less than or equal (__le)

```
1. If types differ: raise order error
2. If both numbers: native comparison
3. If both strings: lexicographic comparison
4. Try __le metamethod (same consistency check as __lt)
5. If __le not found: try __lt with swapped operands
   - call(__lt, right, left) and negate result
   - Implements: a <= b  <==>  not (b < a)
6. If neither found: raise order error
```

The `__le` to `__lt` fallback means implementing only `__lt` gives
working `<=` behavior automatically.

## Index Metamethods

### Read access (__index)

Dispatched by `luaV_gettable`. Loop limit: `MAXTAGLOOP` = 100.

```
for iteration 0..100:
    if t is a table:
        result = raw_get(t, key)
        if result is not nil OR t has no __index: return result
        tm = t.__index
    else:
        tm = metatable(t).__index
        if tm is nil: raise "attempt to index a T value"

    if tm is a function: return call(tm, t, key)
    t = tm  -- chain: repeat with tm as new table
raise "loop in gettable"
```

When `__index` is a table, the lookup chains through it. When
`__index` is a function, it is called with the original table and
key.

### Write access (__newindex)

Dispatched by `luaV_settable`. Loop limit: `MAXTAGLOOP` = 100.

```
for iteration 0..100:
    if t is a table:
        slot = raw_get_or_create(t, key)
        if slot exists (non-nil) OR t has no __newindex:
            raw_set(t, key, value)
            return
        tm = t.__newindex
    else:
        tm = metatable(t).__newindex
        if tm is nil: raise "attempt to index a T value"

    if tm is a function: call(tm, t, key, value); return
    t = tm  -- chain: repeat with tm as new table
raise "loop in settable"
```

When `__newindex` is a function, the function is called **instead**
of writing to the table. The function is responsible for any side
effects. When `__newindex` is a table, the write chains to that
table.

## Concatenation (__concat)

Dispatched by `luaV_concat`. Handles multiple operands from
registers B through C.

```
while more than 1 operand:
    if tostring(left) succeeds AND tostring(right) succeeds:
        collect consecutive string-coercible operands into buffer
        concatenate all into one interned string
    else:
        look up __concat on left, then right
        if found: call(metamethod, left, right)
        if not found: raise concat error
    reduce operand count
```

The buffer-based strategy concatenates multiple consecutive strings
in one pass (avoids O(n^2) intermediate string creation).

## Length (__len)

```
if operand is a table:
    return luaH_getn(table)  -- direct, NO metamethod
if operand is a string:
    return string.len
else:
    look up __len on operand's metatable
    if found: call(__len, operand, nil)
    if not found: raise "attempt to get length of a T value"
```

In Lua 5.1, `#table` **never** invokes `__len`. The `__len`
metamethod only applies to userdata and other non-table,
non-string types. This changed in Lua 5.2.

## Call (__call)

Dispatched by `tryfuncTM` when a non-function value is called.

```
1. Look up __call on the value's metatable
2. If not found or not a function: raise "attempt to call a T value"
3. Shift all stack values above func up by 1
4. Place __call at the function position
5. Continue with normal call dispatch
```

Effect: `obj(a, b)` becomes `__call(obj, a, b)`. The original
object becomes the first argument.

## Type Coercion Rules

### String to number (tonumber)

Used automatically by arithmetic operators.

```rust
fn tonumber(val: &Val) -> Option<f64> {
    match val {
        Val::Number(n) => Some(*n),
        Val::String(s) => str_to_number(s),
        _ => None,
    }
}
```

`str_to_number` accepts (via `luaO_str2d`):

| Format | Example | Method |
|--------|---------|--------|
| Decimal integer | `123` | strtod |
| Decimal float | `123.456`, `.5`, `123.` | strtod |
| Scientific | `1e10`, `1.5e-5`, `123E+2` | strtod |
| Hexadecimal | `0xff`, `0xFF` | strtoul base 16 |
| With whitespace | `  42  ` | Leading/trailing spaces stripped |

Hex is a fallback: `strtod` is tried first. If it stops at `x`/`X`,
`strtoul` retries with base 16.

Any trailing non-whitespace characters cause failure.

### Number to string (tostring)

Used automatically by concatenation.

```rust
fn val_to_string(val: &Val) -> Option<String> {
    match val {
        Val::String(s) => Some(s.clone()),
        Val::Number(n) => Some(format!("{:.14g}", n)),
        _ => None,
    }
}
```

Format: `%.14g` (14 significant digits, general notation). This
matches PUC-Rio's `LUA_NUMBER_FMT`.

### When coercion happens automatically

| Operation | Coercion | Direction |
|-----------|----------|-----------|
| Arithmetic (`+ - * / % ^`) | string to number | Before metamethod check |
| Unary negation (`-`) | string to number | Before metamethod check |
| Concatenation (`..`) | number to string | Before metamethod check |
| Comparison (`< <= > >=`) | None | Requires same type |
| Equality (`== ~=`) | None | Different types are never equal |
| Length (`#`) | None | Only tables and strings |

### What __tostring does NOT do

The `__tostring` metamethod is **only** called by the `tostring()`
standard library function. It is **not** called by:

- Automatic concatenation coercion (uses `%.14g` for numbers)
- `print()` (calls `tostring()` which does check `__tostring`)
- String comparison
- Any implicit conversion

## Metatable Access

Only tables and userdata can have individual metatables. Other types
share a per-type metatable stored in the global state.

| Type | Metatable source |
|------|-----------------|
| Table | `table.metatable` field (per-instance) |
| Userdata | `userdata.metatable` field (per-instance) |
| String | `global_state.mt[LUA_TSTRING]` (shared) |
| Number | `global_state.mt[LUA_TNUMBER]` (shared) |
| Boolean | `global_state.mt[LUA_TBOOLEAN]` (shared) |
| Nil | `global_state.mt[LUA_TNIL]` (shared, usually nil) |
| Function | `global_state.mt[LUA_TFUNCTION]` (shared, usually nil) |
| Thread | `global_state.mt[LUA_TTHREAD]` (shared, usually nil) |

The string metatable is pre-set to `{__index = string_lib}` so that
methods like `s:upper()` work. Other type metatables are nil by
default and can only be set via the debug library.

## Fast-path Metamethod Caching

Each metatable has a `flags` byte where each bit indicates "this
metamethod is definitely absent". The `fasttm()` macro checks the
flag before performing a table lookup.

```
fasttm(metatable, event):
    if metatable is nil: return nil
    if flag bit for event is set: return nil  -- cached absence
    return full lookup via luaT_gettm(metatable, event)
```

The flags are invalidated (cleared) whenever the metatable is
modified. Only events 0-4 (`__index` through `__eq`) use this
cache.
