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

### Rust Closures

Native Rust functions are stored as function pointers in `Val::RustFunction`.
They do not have upvalues in the Lua sense — they access Rust state
through the `&mut Lua` parameter passed at call time.

For Rust functions that need persistent state (Lua-visible upvalues),
a separate `RustClosure` type wraps a function pointer with inline
upvalue storage, mirroring PUC-Rio's `CClosure`.

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

### Sharing

Multiple closures can capture the same variable. When they do,
they share the same `Upvalue` object (same `GcRef<Upvalue>`).
Mutations through one closure are visible to all others.

This is achieved by maintaining a list of open upvalues in the
VM state, sorted by stack index. When creating a closure, the
compiler emits instructions that either:

- Reuse an existing open upvalue for that stack slot
- Create a new open upvalue if none exists

### Closing

When a function returns (or a block exits with locals going out of
scope), the CLOSE instruction closes all upvalues pointing at or
above register A:

1. Walk the open upvalue list.
2. For each upvalue pointing at stack index >= A:
   - Copy the current stack value into the upvalue.
   - Switch state from `Open` to `Closed`.

After closing, the upvalue is self-contained — it no longer
references the stack.

## Proto Ownership

Function prototypes (`Proto`) are immutable after compilation. They
contain the bytecode, constant pool, debug info, and nested Proto
references.

Protos are shared between closures via `Rc<Proto>`:

- Creating a closure (CLOSURE instruction) clones the `Rc`, not the
  Proto itself.
- Multiple closures from the same function definition share one
  Proto allocation.
- Protos are NOT managed by the GC — `Rc` handles their lifetime.

This is a deliberate divergence from PUC-Rio, where Proto is a GC
object. Since Proto is immutable and acyclic (it only references
other Protos and constants, not tables or closures), `Rc` is
sufficient and simpler.

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

The CLOSURE instruction is followed by pseudo-instructions that
specify where each upvalue comes from:

- MOVE: upvalue is a local in the enclosing function (capture from
  stack)
- GETUPVAL: upvalue is an upvalue of the enclosing function (copy
  reference)

## GC Interaction

- `Closure` is a GC-managed object. Its `Trace` implementation
  marks its upvalues and environment table (but NOT the `Rc<Proto>`).
- `Upvalue` is a GC-managed object. When closed, its `Trace`
  implementation marks the contained value.
- Open upvalues are roots — they are reachable from the VM state's
  open upvalue list.
