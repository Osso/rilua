# Closures and Upvalues

## Decision

**Open/closed upvalue model following PUC-Rio. Closures own
`Rc<Proto>` (shared, not GC-managed). Upvalues are GC-managed
objects.**

## Overview

In Lua, functions are first-class values created at runtime. A
closure is a function combined with its captured variables
(upvalues). Upvalues allow inner functions to access variables from
enclosing scopes, even after those scopes have exited.

## Closure Types

### Lua Closures

```rust
pub struct Closure {
    pub proto: Rc<Proto>,
    pub upvalues: Vec<GcRef<Upvalue>>,
    pub env: GcRef<Table>,
}
```

A Lua closure contains:

- A reference to its function prototype (`Rc<Proto>`, shared and
  immutable)
- A list of upvalue references (GC-managed, one per captured
  variable)
- An environment table (the table used for global variable access)

### PUC-Rio Closure Layout (Reference)

```c
// Common fields for both closure types
#define ClosureHeader \
    CommonHeader; lu_byte isC; lu_byte nupvalues; \
    GCObject *gclist; struct Table *env

typedef struct LClosure {
    ClosureHeader;
    struct Proto *p;
    UpVal *upvals[1];       // Flexible array of UpVal pointers
} LClosure;

typedef struct CClosure {
    ClosureHeader;
    lua_CFunction f;
    TValue upvalue[1];      // Flexible array of inline TValues
} CClosure;
```

Key difference: PUC-Rio Lua closures store `UpVal*` pointers (shared
objects) while C closures store `TValue` directly (inline, not
shared). rilua follows the same split.

### Rust Closures (C Closures)

For native Rust functions exposed to Lua:

```rust
pub struct RustClosure {
    pub func: RustFn,                   // fn(&mut Lua) -> Result<u32>
    pub upvalues: Vec<Val>,             // Inline values (not shared)
    pub env: GcRef<Table>,
}
```

Rust closures store upvalues as inline `Val` values, matching
PUC-Rio's `CClosure`. These upvalues are independent copies -- they
are not shared between closures and do not use the `Upvalue` object.

Simple Rust functions without upvalues are stored as
`Val::RustFunction(RustFn)` without wrapping in a closure struct.

### Allocation

PUC-Rio uses flexible array members for closures:

```c
// Size with n upvalues
sizeCclosure(n) = sizeof(CClosure) + sizeof(TValue) * (n - 1)
sizeLclosure(n) = sizeof(LClosure) + sizeof(TValue*) * (n - 1)
```

rilua uses `Vec` instead, which handles the variable-size storage.

## Upvalue Model

### Open vs Closed

An upvalue has two states:

**Open**: The upvalue points to a slot on the Lua stack. The
variable is still "alive" in its declaring function's stack frame.

**Closed**: The upvalue owns the value directly. The declaring
function has returned, and the stack slot no longer exists.

```rust
pub struct Upvalue {
    state: UpvalueState,
}

enum UpvalueState {
    Open { stack_index: usize },
    Closed { value: Val },
}
```

### PUC-Rio UpVal Layout (Reference)

```c
typedef struct UpVal {
    CommonHeader;
    TValue *v;              // Points to stack slot OR to u.value
    union {
        TValue value;       // Storage when closed
        struct {            // Linked list pointers when open
            struct UpVal *prev;
            struct UpVal *next;
        } l;
    } u;
} UpVal;
```

The discriminant is implicit: if `v == &uv->u.value`, the upvalue
is closed. Otherwise it is open. The union saves memory: open
upvalues need list pointers, closed upvalues need value storage,
but never both simultaneously.

### Sharing

Multiple closures can capture the same variable. When they do,
they share the same `Upvalue` object (same `GcRef<Upvalue>`).
Mutations through one closure are visible to all others.

```lua
function make_counter()
    local n = 0
    local function inc() n = n + 1; return n end
    local function get() return n end
    return inc, get
end

local inc, get = make_counter()
inc()           -- n is now 1
print(get())    -- prints 1 (shared upvalue)
```

Both `inc` and `get` hold a `GcRef<Upvalue>` pointing to the same
`Upvalue` object. When `make_counter` returns, the upvalue is closed
and the value (`n = 1`) moves into the `Upvalue` storage.

## Open Upvalue Lists

Open upvalues are tracked in two data structures:

### Per-Thread List (`L->openupval`)

Each thread maintains a singly-linked list of open upvalues pointing
into its stack. The list is **sorted by stack index in descending
order** (highest stack address first).

```text
L->openupval -> [slot 15] -> [slot 10] -> [slot 3] -> NULL
```

This ordering enables efficient closing: when a block exits at level
N, all upvalues at the head of the list (with stack_index >= N) are
closed in sequence.

In rilua, this is a `Vec<GcRef<Upvalue>>` maintained in sorted order,
or a linked structure through the arena.

### Global List (`uvhead` Sentinel)

All open upvalues across all threads are linked into a global
doubly-linked list through the `uvhead` sentinel in `global_State`.

```text
uvhead <-> [uv_A] <-> [uv_B] <-> [uv_C] <-> uvhead  (circular)
```

This list exists for the GC: during the atomic phase,
`remarkupvals()` walks this list to re-mark gray open upvalues.
Without it, the GC would need to traverse all threads to find open
upvalues.

PUC-Rio initializes the sentinel as a circular list:

```c
g->uvhead.u.l.prev = &g->uvhead;
g->uvhead.u.l.next = &g->uvhead;
```

In rilua, this can be a `Vec<GcRef<Upvalue>>` in the GC state, or
a doubly-linked list through the upvalue arena slots.

## Finding and Creating Upvalues

### `luaF_findupval` Algorithm

When the VM executes OP_CLOSURE and processes a MOVE pseudo-
instruction (capturing a local variable from the enclosing function),
it calls `findupval` to either reuse an existing open upvalue for
that stack slot or create a new one.

```text
function findupval(L, stack_level):
    // Walk per-thread open upvalue list (sorted descending by level)
    pp = &L.openupval
    while pp is not empty and pp.level >= stack_level:
        uv = pp.current
        if uv.stack_index == stack_level:
            // Found: reuse existing upvalue
            if is_dead(uv):
                resurrect(uv)      // flip to current white
            return uv
        pp = pp.next

    // Not found: create new open upvalue
    uv = arena.alloc(Upvalue {
        state: Open { stack_index: stack_level }
    })

    // Insert into per-thread list (maintains sorted order)
    // uv goes between pp.prev and pp.current
    insert uv into L.openupval at position pp

    // Insert into global uvhead list (at head)
    uv.global_prev = uvhead
    uv.global_next = uvhead.next
    uvhead.next.prev = uv
    uvhead.next = uv

    return uv
```

Key details:

1. **Sorted walk**: the list is sorted descending by stack index.
   The walk stops as soon as it finds a slot below the target level,
   because all remaining entries have even lower indices.
2. **Reuse**: if an upvalue for this exact stack slot already exists
   (created by a previous closure), return it. This is how sharing
   works.
3. **Resurrection**: if the found upvalue was marked dead by GC but
   not yet swept, flip its color to keep it alive.
4. **Insertion point**: the new upvalue is inserted at the position
   where the walk stopped, maintaining sorted order.
5. **Global list**: new upvalues are inserted at the head of the
   `uvhead` doubly-linked list for GC access.

### Why Sorted Order Matters

The descending sort serves two purposes:

1. **Efficient closing**: `luaF_close(level)` only needs to pop
   entries from the head of the list until it reaches one below the
   level. No scanning needed.
2. **Efficient search**: `findupval` walks from highest to lowest.
   Since closures typically capture nearby locals, the target is
   often near the head.

## Closing Upvalues

### `luaF_close` Algorithm

When a block exits with locals that were captured as upvalues
(detected by the compiler's `markupval` flag on the block), or when
a function returns, upvalues pointing at or above the exit level
must be closed.

```text
function close_upvalues(L, level):
    while L.openupval is not empty:
        uv = L.openupval.first      // highest stack index
        if uv.stack_index < level:
            break                    // below the exit level, stop

        // Remove from per-thread open list
        L.openupval.remove_first()

        if is_dead(uv):
            // Already unreachable: free immediately
            free_upvalue(uv)
        else:
            // Remove from global uvhead list
            unlink_from_global_list(uv)

            // Copy value from stack to upvalue storage
            uv.closed_value = stack[uv.stack_index]
            uv.state = Closed { value: uv.closed_value }

            // Link into GC rootgc list (now a regular GC object)
            link_into_rootgc(uv)
    end
```

Step by step:

1. **Pop from open list**: since the list is sorted descending,
   upvalues at or above `level` are all at the head.
2. **Dead check**: if the upvalue is already dead (unreachable by
   any closure), skip the closing and free it directly.
3. **Unlink from global list**: remove from `uvhead` doubly-linked
   list (O(1) with prev/next pointers).
4. **Copy value**: read the current value from the stack slot and
   store it in the upvalue's own storage. After this, the upvalue
   no longer references the stack.
5. **Transition state**: change from `Open` to `Closed`.
6. **Link into rootgc**: closed upvalues become regular GC objects
   in the `rootgc` list (they were previously tracked only in the
   open upvalue lists).

### When Closing Happens

- **OP_RETURN**: closes upvalues at the function's base register.
  Done inside the return handling code, not via a separate CLOSE
  instruction.
- **OP_TAILCALL**: same as OP_RETURN (closes before the tail call).
- **OP_CLOSE**: explicitly emitted by the compiler when a block with
  captured locals exits (e.g., the end of a `do...end` block, the
  end of a `for` loop body). The operand A specifies the stack level.
- **Thread destruction**: `luaF_close(L, L->stack)` closes all
  upvalues for the thread.

### The `markupval` Mechanism

The compiler determines whether OP_CLOSE is needed by tracking
which blocks have captured locals. When `singlevaraux` resolves a
variable as a local in an enclosing function (not the innermost),
it calls `markupval(fs, register)`:

```text
function markupval(fs, level):
    // Walk up the block chain to find the block owning this register
    bl = fs.current_block
    while bl is not null and bl.nactvar > level:
        bl = bl.parent
    if bl is not null:
        bl.upval = true     // This block has captured locals
```

When `leaveblock` runs for a block with `upval == true`, it emits
`OP_CLOSE` before removing the locals.

## OP_CLOSURE Processing

When the VM executes OP_CLOSURE, it creates a new Lua closure and
populates its upvalue array by reading pseudo-instructions that
follow the CLOSURE instruction in the bytecode.

### Step-by-Step Algorithm

```text
function execute_closure(instruction):
    proto_index = GETARG_Bx(instruction)
    child_proto = current_closure.proto.nested[proto_index]
    nups = child_proto.nups

    // Create new closure
    new_closure = Closure {
        proto: Rc::clone(&child_proto),
        upvalues: Vec::with_capacity(nups),
        env: current_closure.env,
    }

    // Process nups pseudo-instructions
    for j in 0..nups:
        pseudo = code[pc]
        pc += 1              // consume the pseudo-instruction

        if opcode(pseudo) == OP_GETUPVAL:
            // Share parent closure's upvalue
            parent_uv_index = GETARG_B(pseudo)
            new_closure.upvalues[j] = current_closure.upvalues[parent_uv_index]

        else:   // opcode must be OP_MOVE
            // Capture local variable from enclosing function's stack
            local_register = GETARG_B(pseudo)
            stack_slot = base + local_register
            new_closure.upvalues[j] = findupval(L, stack_slot)

    // Store result
    R(A) = new_closure

    // GC check (closure allocation may trigger collection)
    gc_check()
```

### Pseudo-Instruction Encoding

The pseudo-instructions are real instructions in the bytecode array
but are never executed by the normal dispatch loop. They are consumed
by the OP_CLOSURE handler:

- **OP_MOVE 0, B, 0**: upvalue comes from local register B in the
  current function. The A and C fields are unused (set to 0).
  `findupval` is called to get or create an open upvalue for
  `base + B`.

- **OP_GETUPVAL 0, B, 0**: upvalue comes from upvalue slot B in
  the current closure. The A and C fields are unused (set to 0).
  The new closure shares the same `GcRef<Upvalue>` as the parent.

### Example

```lua
local x = 1
local function outer()
    local y = 2
    local function inner()
        return x + y
    end
    return inner
end
```

The compiler emits for `inner`:

```text
OP_CLOSURE  A, proto_index_of_inner
OP_GETUPVAL 0, 0, 0    -- inner.upvalues[0] = outer.upvalues[0] (x)
OP_MOVE     0, 0, 0    -- inner.upvalues[1] = findupval(base + 0) (y)
```

Here `x` is an upvalue of `outer` (captured from the top level), so
`inner` shares it via GETUPVAL. `y` is a local of `outer`, so
`inner` captures it fresh via MOVE/findupval.

## Proto Ownership

Function prototypes (`Proto`) are immutable after compilation. They
contain the bytecode, constant pool, debug info, and nested Proto
references.

Protos are shared between closures via `Rc<Proto>`:

- Creating a closure (CLOSURE instruction) clones the `Rc`, not the
  Proto itself.
- Multiple closures from the same function definition share one
  Proto allocation.
- Protos are NOT managed by the GC -- `Rc` handles their lifetime.

This is a deliberate divergence from PUC-Rio, where Proto is a GC
object. Since Proto is immutable after compilation and its constant
pool only contains nil, booleans, numbers, and strings (not tables,
closures, or other GC objects that could create cycles), `Rc` is
sufficient and simpler.

### Proto Fields

```rust
pub struct Proto {
    pub code: Vec<Instruction>,
    pub constants: Vec<Val>,
    pub nested: Vec<Rc<Proto>>,
    pub num_upvalues: u8,
    pub num_params: u8,
    pub is_vararg: u8,
    pub max_stack_size: u8,
    // Debug info
    pub source: GcRef<LuaString>,
    pub line_defined: u32,
    pub last_line_defined: u32,
    pub line_info: Vec<u32>,
    pub local_vars: Vec<LocalVar>,
    pub upvalue_names: Vec<GcRef<LuaString>>,
}
```

The `num_upvalues` field (`nups` in PUC-Rio) tells the VM how many
pseudo-instructions follow each OP_CLOSURE that creates this
function.

## Compiler Support

The compiler resolves upvalue references during compilation. For
each upvalue in a function, it records:

```rust
pub struct UpvalueDesc {
    pub name: String,
    pub in_stack: bool,  // true = parent's local, false = parent's upvalue
    pub index: u8,       // local slot or upvalue index in parent
}
```

### Upvalue Resolution Chain (`singlevaraux`)

When the parser encounters a variable name, `singlevaraux` resolves
it by searching outward through enclosing functions:

```text
function singlevaraux(fs, name):
    if fs is null:
        return VGLOBAL              // no more scopes: it's a global

    // Search locals in this function
    reg = searchvar(fs, name)
    if reg >= 0:
        if not in base function:
            markupval(fs, reg)      // flag block for OP_CLOSE
        return VLOCAL(reg)

    // Not a local: try enclosing function
    result = singlevaraux(fs.parent, name)
    if result == VGLOBAL:
        return VGLOBAL              // still global, propagate

    // Found in an outer scope: register as upvalue in this function
    uv_index = indexupvalue(fs, name, result)
    return VUPVAL(uv_index)
```

### `indexupvalue` Deduplication

Each function's upvalue list is deduplicated. When registering an
upvalue, `indexupvalue` first checks if an identical entry already
exists (same `in_stack` flag and same `index`):

```text
function indexupvalue(fs, name, desc):
    // Check for duplicate
    for i in 0..fs.num_upvalues:
        if fs.upvalues[i].in_stack == desc.in_stack
           and fs.upvalues[i].index == desc.index:
            return i                // reuse existing slot

    // New upvalue
    check limit (LUAI_MAXUPVALUES = 60)
    fs.upvalues[fs.num_upvalues] = UpvalueDesc {
        name, in_stack: desc.in_stack, index: desc.index
    }
    return fs.num_upvalues++
```

### Limits

- `LUAI_MAXUPVALUES` = 60 per function
- Error message: `"too many upvalues"` (syntax error)

## GC Interaction

### Closure Traversal

When the GC propagates (marks) a closure:

```text
function traverse_closure(cl):
    mark(cl.env)                    // mark environment table
    if cl is Lua closure:
        mark(cl.proto)              // mark Proto (or skip if Rc)
        for uv in cl.upvalues:
            mark(uv)                // mark each UpVal object
    else if cl is C/Rust closure:
        for val in cl.upvalues:
            mark_value(val)         // mark inline TValues
```

For Lua closures, the GC marks the `GcRef<Upvalue>` objects, not the
values inside them. The upvalues are traversed separately.

For C/Rust closures, the upvalues are inline `Val` values and are
marked directly.

### Upvalue Marking

When the GC marks an upvalue:

```text
function mark_upvalue(uv):
    mark_value(uv.current_value)    // mark whatever v points to
    if uv is closed:
        set_black(uv)               // fully processed
    else:
        // Open upvalues stay GRAY (not black)
        // because the stack value they point to may change
```

Open upvalues remain gray because between incremental GC steps, the
stack value they reference can be overwritten. The GC re-marks them
during the atomic phase.

### Atomic Phase: `remarkupvals`

During the atomic phase (stop-the-world), the GC walks the global
`uvhead` list and re-marks all gray open upvalues:

```text
function remarkupvals():
    for uv in uvhead list:
        if is_gray(uv):
            mark_value(*uv.v)       // re-mark the stack value
```

This ensures that values pointed to by open upvalues are not
incorrectly swept. Without this step, a value written to a stack
slot after the slot was already traversed would be missed.

### Sweep Phase

During sweep, each thread's `openupval` list is swept:

```text
function sweep_thread(thread):
    sweep_list(thread.openupval)    // remove dead open upvalues
```

Dead upvalues are freed via `luaF_freeupval`, which unlinks them
from the global list if still open, then deallocates.

Closed upvalues live in the `rootgc` list and are swept with all
other objects.

### `luaC_linkupval`: Closed Upvalue GC Transition

When an upvalue is closed, it transitions from the open upvalue
tracking system to the regular GC object list:

```text
function link_closed_upvalue(uv):
    add uv to rootgc list

    if is_gray(uv) and gc_state == Propagate:
        // During mark phase: finish marking
        set_black(uv)
        barrier(uv, uv.value)      // forward barrier on the value
    else if is_gray(uv):
        // During sweep phase: make white (will be swept normally)
        make_white(uv)
```

### Write Barriers for Upvalues

When a Lua closure's upvalue is modified (via OP_SETUPVAL), a write
barrier is needed:

```text
function execute_setupval(closure, uv_index, value):
    uv = closure.upvalues[uv_index]
    *uv.v = value               // write through the pointer
    barrier(uv, value)          // maintain tri-color invariant
```

For C/Rust closures, upvalue writes through the API
(`lua_setupvalue`) also require a barrier on the closure object:

```text
function api_setupvalue(closure, index, value):
    closure.upvalues[index] = value
    barrier(closure, value)
```
