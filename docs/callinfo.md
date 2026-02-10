# Call Stack

## Decision

**Dynamic CallInfo array separate from the value stack. Each active
function call occupies one CallInfo entry. The value stack and call
stack grow independently.**

PUC-Rio Lua 5.1.1 maintains two parallel data structures: a value
stack (array of TValue) and a call stack (array of CallInfo). rilua
follows the same design, translating the C structs to Rust.

Reference: `lstate.h`, `ldo.c`, `lvm.c` in PUC-Rio Lua 5.1.1.

## CallInfo Struct

Each active function call (Lua or Rust) occupies one CallInfo entry.

```rust
struct CallInfo {
    /// Stack index of the function value itself.
    func: usize,

    /// Base of this function's stack frame. Points to the first
    /// local variable slot (register 0). For vararg functions,
    /// base is after the fixed-to-vararg adjustment.
    base: usize,

    /// Stack top limit for this function. Set to
    /// `base + proto.max_stack_size` for Lua functions, or
    /// `base + LUA_MINSTACK` for Rust functions.
    top: usize,

    /// Saved program counter. Index into the Proto's code array.
    /// Only meaningful for Lua functions. Saved before calling a
    /// child function, restored on return.
    saved_pc: usize,

    /// Number of results expected by the caller. The special value
    /// `LUA_MULTRET` (-1 as i32, or a sentinel) means "all results".
    num_results: i32,

    /// Count of tail calls optimized under this frame. Used by
    /// debug hooks to report elided frames. Only incremented for
    /// Lua functions.
    tail_calls: i32,
}
```

The struct is identical for Lua and Rust functions. The distinction
is made by examining the function value at `stack[ci.func]` -- check
whether it is a Lua closure or a Rust closure.

## VM State Fields

The VM state (`LuaState` / `State`) holds the following call-stack
related fields:

| Field | Type | Purpose |
|-------|------|---------|
| `stack` | `Vec<Val>` | Value stack. Grows independently from call stack. |
| `base` | `usize` | Base of current function's frame. Always equals `ci().base`. |
| `top` | `usize` | First free slot in value stack. |
| `call_stack` | `Vec<CallInfo>` | Dynamic array of CallInfo entries. |
| `ci` | `usize` | Index into `call_stack` for the current frame. |
| `saved_pc` | `usize` | Cached PC of current Lua function. Synced with `ci().saved_pc` on call/return. |
| `n_ccalls` | `u16` | Count of nested Rust function calls. Prevents Rust stack overflow. |

### Synchronization invariant

`base` and `saved_pc` in the VM state always mirror the current
CallInfo entry. They are explicitly synchronized on every call and
return.

### Limits

| Constant | Value | Purpose |
|----------|-------|---------|
| `LUAI_MAXCALLS` | 20000 | Max total call depth (Lua + Rust). |
| `LUAI_MAXCCALLS` | 200 | Max nested Rust function calls. |
| `LUA_MINSTACK` | 20 | Minimum stack slots guaranteed for Rust functions. |
| `BASIC_CI_SIZE` | 8 | Initial CallInfo array capacity. |

## Call Protocol

### Lua function call (precall)

When calling a Lua closure:

1. **Save caller's PC**: `call_stack[ci].saved_pc = saved_pc`.
2. **Validate function**: If the value at the call position is not a
   function, try the `__call` metamethod via `tryfuncTM`.
3. **Check stack space**: Ensure `stack.len() >= base + proto.max_stack_size`.
4. **Handle varargs**: If the function is not vararg, set
   `new_base = func + 1` and trim excess arguments. If vararg, call
   `adjust_varargs` which moves fixed parameters above the vararg
   area and returns the new base.
5. **Push CallInfo**: Append a new entry to `call_stack`:
   - `func` = stack position of the function value
   - `base` = computed base (first local)
   - `top` = `base + proto.max_stack_size`
   - `saved_pc` = 0 (start of bytecode)
   - `num_results` = caller-specified result count
   - `tail_calls` = 0
6. **Initialize locals**: Fill `stack[top..ci.top]` with nil.
7. **Update VM state**: Set `base = ci.base`, `saved_pc = 0`,
   `top = ci.top`.
8. **Call hook**: If `LUA_MASKCALL` is set, invoke the debug hook.
9. **Enter execution loop**: The caller (execute loop or `luaD_call`)
   enters/re-enters `luaV_execute`.

### Rust function call (precall)

When calling a Rust closure:

1. **Save caller's PC**: Same as Lua case.
2. **Check stack space**: Ensure at least `LUA_MINSTACK` (20) slots.
3. **Push CallInfo**: Similar to Lua, but `top = top + LUA_MINSTACK`.
4. **Call hook**: If enabled.
5. **Execute directly**: Call the Rust function pointer. It returns
   the number of return values pushed onto the stack (or a negative
   value for coroutine yield).
6. **Post-call**: Immediately invoke `poscall` to move results and
   pop the frame.

### Key difference

Lua calls return to the execution loop which eventually hits
`OP_RETURN`. Rust calls execute and complete within `precall`
itself.

## Return Protocol (poscall)

When a function returns (via `OP_RETURN` or after Rust function
completes):

1. **Call return hook**: If `LUA_MASKRET` is set.
2. **Pop CallInfo**: Decrement `ci` index. Save the popped entry.
3. **Determine result destination**: Results go to `stack[ci.func]`
   (where the function value was).
4. **Restore previous frame**: Set `base = call_stack[ci].base` and
   `saved_pc = call_stack[ci].saved_pc`.
5. **Move return values**: Copy from `first_result` position to
   `res` (the function position), up to `num_results` values. Pad
   with nil if fewer results than requested.
6. **Set top**: `top = res + num_moved` (or `res + num_results` if
   fixed count). Stack above this point is discarded.

### LUA_MULTRET

When `num_results == LUA_MULTRET`, all return values are kept. The
caller (typically `OP_CALL` with `C == 0`) reads `top` to determine
how many results were returned.

## Tail Call Handling

`OP_TAILCALL` optimizes tail position calls to avoid growing the
call stack.

1. **Normal precall**: A new CallInfo is pushed as usual.
2. **Close upvalues**: If any open upvalues reference the current
   frame's registers, close them.
3. **Move frame down**: Copy the new function + arguments from the
   new frame's position down to the old frame's position, overwriting
   the old frame.
4. **Adjust base**: Recompute `base` relative to the old `func`
   position.
5. **Update saved_pc**: Point to the new function's bytecode.
6. **Increment tail_calls**: Track elided frames for debug hooks.
7. **Pop extra CallInfo**: Remove the new frame (it was temporary).
   The old frame is now reused.
8. **Re-enter execution loop**: Jump to the loop entry point with the
   reused frame.

Result: no net growth of the call stack. The `tail_calls` counter
lets debug hooks report the correct call depth.

## Error Recovery (Protected Calls)

`pcall` and `xpcall` use protected calls that save and restore VM
state on error.

### Saved state

Before the protected call:

| Saved | Purpose |
|-------|---------|
| `ci` index | Restored to roll back the call stack |
| `n_ccalls` | Restored to reset Rust call depth |
| `top` (as offset) | Stack trimmed to this point on error |
| `allow_hook` | Hook state restored |
| `err_func` | Error handler stack index restored |

### On error

1. Close any open upvalues above the saved `top`.
2. Place the error object at `stack[old_top]`.
3. Restore `ci` to the saved index.
4. Restore `base` and `saved_pc` from the restored CallInfo.
5. Restore `n_ccalls`, `allow_hook`.
6. Shrink the call stack array if it grew during the failed call.

### Offset storage

Save `ci` as an index (not a pointer/reference), since `call_stack`
may reallocate during the protected call. PUC-Rio stores byte offsets
(`ptrdiff_t`); rilua uses `usize` indices into the `Vec`.

## Stack Growth

### Value stack

Grows via `Vec::resize` when `luaD_checkstack` detects insufficient
space. The stack may grow during any function call.

### Call stack

Initial capacity: `BASIC_CI_SIZE` (8). Doubles when full (checked
by `inc_ci` before each push). Hard limit: `LUAI_MAXCALLS` (20000).
Exceeding the limit raises `"stack overflow"`.

Both stacks are separate `Vec`s. Since rilua uses indices (not
pointers) into these vectors, reallocation does not invalidate any
saved state.

## Vararg Handling

When a vararg function is called, `adjust_varargs` reorganizes the
stack:

1. Move fixed parameters to new positions above the vararg area.
2. Original argument positions become the vararg storage.
3. `base` points past the fixed parameters (to the first local slot).
4. Vararg values are accessed via negative offsets from `base`.

The `is_vararg` flags in Proto control behavior:

| Flag | Value | Meaning |
|------|-------|---------|
| `VARARG_HASARG` | 1 | Legacy: function uses `arg` table |
| `VARARG_ISVARARG` | 2 | Function signature includes `...` |
| `VARARG_NEEDSARG` | 4 | Runtime must create `arg` table |

Flags are combined with bitwise OR. `VARARG_NEEDSARG` implies
`VARARG_HASARG`. The main chunk always has `VARARG_ISVARARG` set.
