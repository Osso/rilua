# Coroutines

## Decision

**Coroutines are threads (`lua_State`) with independent value stacks and
call stacks, sharing the GC heap with all other threads in the same Lua
instance.**

Reference: `lstate.h`, `lstate.c`, `ldo.c`, `lbaselib.c`, `lgc.c` in
PUC-Rio Lua 5.1.1.

## Thread Structure

Each coroutine is a `lua_State` (type `LUA_TTHREAD`). It is a
GC-managed object linked into the `rootgc` list.

### Shared State (global_State)

All threads from the same `Lua::new()` share one `global_State`:

- GC heap (rootgc list, gray/grayagain/weak lists)
- String intern table
- Registry table
- Type metatables (per-type shared metatables)
- Memory allocator
- GC parameters (pause, step multiplier, threshold)

### Per-Thread State (lua_State)

Each thread owns:

| Field | Type | Purpose |
|-------|------|---------|
| `stack` | `Vec<Val>` | Value stack |
| `base` | `usize` | Base of current function's frame |
| `top` | `usize` | First free slot |
| `call_stack` | `Vec<CallInfo>` | Call frames |
| `ci` | `usize` | Current CallInfo index |
| `saved_pc` | `usize` | Cached program counter |
| `status` | `u8` | Thread status (0, YIELD, or error code) |
| `n_ccalls` | `u16` | Nested Rust function call depth |
| `globals` | `GcRef<Table>` | Thread's global table |
| `open_upval` | linked list | Open upvalues pointing into this stack |
| `hook_mask` | `u8` | Debug hook flags |
| `hook_count` | `i32` | Instruction count for count hooks |
| `err_func` | `usize` | Error handler stack index |
| `allow_hook` | `bool` | Whether hooks are enabled |

### Initial Stack Sizes

| Resource | Initial | Maximum |
|----------|---------|---------|
| Value stack | 45 slots (`BASIC_STACK_SIZE` + `EXTRA_STACK`) | Grows dynamically |
| CallInfo array | 8 entries (`BASIC_CI_SIZE`) | 20000 (`LUAI_MAXCALLS`) |

## Status Values

```rust
const LUA_YIELD: u8 = 1;
const LUA_ERRRUN: u8 = 2;
const LUA_ERRSYNTAX: u8 = 3;
const LUA_ERRMEM: u8 = 4;
const LUA_ERRERR: u8 = 5;
```

| Status | Meaning |
|--------|---------|
| 0 | Normal: running, initial, or finished |
| 1 (`LUA_YIELD`) | Suspended (yielded) |
| 2-5 | Dead (error occurred) |

### Distinguishing Dead from Initial (both status=0)

When status is 0, check the stack:

- `stack is empty` (gettop == 0): **dead** (coroutine returned)
- `stack has function, no active frames` (ci == base_ci): **initial**
  (not yet started)
- `active frames present` (ci > base_ci): **running** or **normal**

## State Transitions

```text
Created (status=0, function on stack)
  --[first resume]--> Running (status=0, active frames)
  --[yield]--> Suspended (status=LUA_YIELD)
  --[resume]--> Running (status=0)
  --[yield]--> Suspended (status=LUA_YIELD)
  ...
  --[return from body]--> Dead (status=0, stack empty)
  --[error]--> Dead (status=error code, permanent)
```

A dead coroutine cannot be resumed. The status is permanent after an
error.

## Resume Protocol

### Validation

Three rejection conditions:

1. `status >= 2` (error code): "cannot resume dead coroutine"
2. `status == 0` and `ci != base_ci`: "cannot resume non-suspended
   coroutine" (it is running or in normal state)
3. `status == 0` and stack empty: "cannot resume dead coroutine"
   (checked at the Lua library level, not in `lua_resume`)

### First Resume (status == 0, function on stack)

1. The body function sits on the coroutine's stack (placed by
   `coroutine.create`).
2. Arguments are transferred via `lua_xmove` from the caller to the
   coroutine's stack above the function.
3. `luaD_precall` sets up the call frame. Arguments become parameters.
4. `luaV_execute` enters the interpreter loop.

### Subsequent Resume (status == LUA_YIELD)

1. Clear status to 0.
2. Arguments from the resume caller sit on the coroutine's stack
   (transferred via `lua_xmove`).
3. If the yield happened during a C function call: complete the
   interrupted call via `luaD_poscall`. The resume arguments become
   the return values of `coroutine.yield()` inside the coroutine.
4. If the yield happened inside a debug hook: restore base and
   continue execution.
5. Re-enter `luaV_execute`.

### Return Values

`lua_resume` returns:

| Return | Meaning |
|--------|---------|
| 0 | Coroutine finished (returned from body) |
| `LUA_YIELD` (1) | Coroutine yielded |
| 2-5 | Error (coroutine is now dead) |

On success or yield, return/yield values are on the coroutine's stack.
The caller retrieves them via `lua_xmove`.

### Error Handling

If an error occurs during resume:

1. The coroutine's status is set to the error code (permanently dead).
2. The error object is placed on the coroutine's stack.
3. The caller retrieves the error message.

Errors in coroutines do NOT propagate to the resume caller
automatically. `coroutine.resume` returns `false, error_message`.
`coroutine.wrap` propagates errors via `lua_error`.

## Yield Protocol

### lua_yield Implementation

```
1. Check nCcalls > 0: if so, raise
   "attempt to yield across metamethod/C-call boundary"
2. Set base = top - nresults (protect yield values)
3. Set status = LUA_YIELD
4. Return -1 (signals yield to the VM)
```

### The nCcalls Restriction

`nCcalls` counts nested Rust function calls on the current thread.
It is incremented by `luaD_call` and decremented after.

`lua_resume` does NOT use `luaD_call`. It calls `luaD_precall`
directly, so `nCcalls` stays at 0 during normal coroutine execution.

Yield is blocked (`nCcalls > 0`) when:

- A metamethod is executing (metamethods go through `luaD_call`)
- A Rust function called another Rust function via `luaD_call`
- Any C-level nesting on the call stack

Yield is allowed when:

- Pure Lua code is executing
- A C function was called directly by the VM's OP_CALL handler
  (via `luaD_precall`, which does not increment `nCcalls`)

### How Yield Values Reach the Caller

1. `lua_yield` sets `base = top - nresults`, protecting the yield
   values on the stack.
2. The C function returns -1, triggering the `PCRYIELD` path in
   `luaD_precall`.
3. The VM's OP_CALL handler sees the yield and returns (unwinds).
4. Control returns to `resume()`, then to `lua_resume`, which returns
   `LUA_YIELD`.
5. The caller reads `gettop(co)` to get the count of yield values
   and retrieves them via `lua_xmove`.

### Hook Yield

Debug hooks can call `lua_yield`. When the VM detects
`status == LUA_YIELD` after returning from the hook, it saves
`pc - 1` and returns. On subsequent resume, the hook-yield path
restores base and re-enters the execution loop.

## Coroutine Library

Registered as the `coroutine` table by `luaopen_base`.

### coroutine.create(f)

- Argument: must be a Lua function (not a C function).
  Error: "Lua function expected"
- Creates a new thread via `lua_newthread`
- Moves the function to the new thread's stack
- Returns the thread

### coroutine.resume(co, ...)

- Transfers arguments to the coroutine via `lua_xmove`
- Calls `lua_resume`
- On success/yield: returns `true` followed by return/yield values
- On error: returns `false` followed by the error message
- Error: "too many arguments to resume", "too many results to resume",
  "cannot resume dead coroutine"

### coroutine.yield(...)

- All arguments become yield values
- Returns the values passed to the next `resume`

### coroutine.wrap(f)

- Creates a coroutine and returns an iterator function (closure)
- Each call to the iterator resumes the coroutine
- On error: propagates via `lua_error` (not false+message like resume)
- If error is a string, prepends source location info

### coroutine.status(co)

| Condition | Result |
|-----------|--------|
| `co` is the calling thread | "running" |
| `status == LUA_YIELD` | "suspended" |
| `status == 0`, has active stack frames | "normal" |
| `status == 0`, stack empty | "dead" |
| `status == 0`, function on stack, no frames | "suspended" (initial) |
| `status >= 2` (error) | "dead" |

The "normal" state occurs when coroutine A resumes coroutine B, and B
queries `coroutine.status(A)`. A has status 0 but active frames.

### coroutine.running()

- Returns the running coroutine thread, or nothing if called from the
  main thread.
- `lua_pushthread` returns 1 if it is the main thread, 0 otherwise.

## Value Transfer: lua_xmove

```
lua_xmove(from, to, n):
  1. Assert from and to share the same global_State
  2. Assert to has enough stack space
  3. Pop n values from 'from' (decrement top)
  4. Push n values onto 'to' (copy TValue structs)
```

Values are shallow-copied. GC object pointers are shared between
threads (safe because all threads share the same GC heap). No deep
copy occurs.

## GC Interaction

### Thread Traversal

During GC propagation, `traversestack` marks a thread's state:

1. Mark the thread's globals table
2. Compute `lim` as max of `top` and all `ci.top` values
3. Mark all values from `stack[0]` to `stack[top-1]`
4. Nil out slots from `top` to `lim` (clear stale references)
5. Check stack sizes for potential shrinking

### Threads Are Never Fully Black

After traversal, threads are moved to the `grayagain` list and marked
gray (not black). This is because stack writes do not use GC write
barriers -- the stack can be mutated between incremental GC steps.
Threads are re-traversed during the atomic phase to catch mutations.

### Atomic Phase

During the atomic phase:

1. The running thread is explicitly marked
2. Open upvalues are remarked (`remarkupvals`)
3. All objects on `grayagain` (including threads) are re-traversed

### Suspended Coroutine Safety

A suspended coroutine is safe from collection as long as something
references the thread value (in a table, on a stack, etc.). The
thread's stack is traversed, marking all saved values, yield values,
and objects referenced from saved call frames.

### Open Upvalues on Coroutine Stacks

Each thread has its own open upvalue list. Open upvalues point into
the thread's stack. They are handled specially:

- `remarkupvals` during the atomic phase re-marks gray open upvalues
- Thread destruction (`luaE_freethread`) closes all open upvalues
- `luaF_close` copies values from the stack into the upvalue's own
  storage, then links the upvalue into the rootgc list
- During sweep, each thread's open upvalue list is swept separately

### Thread as GC Root

The main thread is always a GC root. Other threads are reachable
through references from the main thread's stack, tables, closures,
or the registry.

## Implementation Notes for rilua

### Thread Representation

```rust
pub struct Thread {
    stack: Vec<Val>,
    base: usize,
    top: usize,
    call_stack: Vec<CallInfo>,
    ci: usize,
    saved_pc: usize,
    status: u8,
    n_ccalls: u16,
    globals: GcRef<Table>,
    open_upval: Option<GcRef<UpVal>>,
    // hook state, error handler, etc.
}
```

The main `Lua` struct wraps a `Thread` (the main thread) plus the
shared `GlobalState`.

### Resume/Yield Without longjmp

PUC-Rio uses `longjmp` for error handling and yield. rilua uses
`Result` propagation instead:

- `lua_resume` equivalent returns `Result<ResumeStatus>`
- `lua_yield` equivalent returns a special `YieldError` that unwinds
  the call stack via `?` propagation
- The execution loop checks for yield after each instruction that
  may trigger a C call

### Coroutine and VM Interaction

The execution loop (`luaV_execute`) takes a `nexeccalls` parameter
that tells it how many Lua frames were entered by resume. When that
many frames have been exited, execution returns to the resume caller
rather than continuing.

For yield: the OP_CALL handler checks if `luaD_precall` returned
`PCRYIELD`. If so, the execution loop returns immediately, unwinding
back to the resume entry point.
