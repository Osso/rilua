# Garbage Collector

## Decision

**Arena-based mark-sweep GC using generational indices. Zero unsafe
code. Behavioral compatibility with PUC-Rio Lua 5.1.1's GC
semantics.**

## Overview

All GC-managed objects (strings, tables, closures, upvalues, userdata,
threads) are allocated in typed arenas — `Vec<Entry<T>>` containers where each
slot has a generation counter. Objects are referenced by `GcRef<T>`
indices that include both the slot index and the expected generation.

The collector uses mark-sweep to reclaim unreachable objects. The
algorithm is modeled on PUC-Rio Lua 5.1.1's incremental tri-color
collector, adapted to work with arena storage instead of intrusive
linked lists.

## Why Arena + Generational Indices

Six approaches were evaluated:

| Approach | Unsafe | Lua Compat | Verdict |
|----------|--------|------------|---------|
| Rc + cycle detection | 0 | Poor | Different collection semantics |
| Arena + gen. indices | 0 | Good | Selected |
| Branded lifetimes | 5-10 | Full | Too complex, pervasive `'gc` |
| Contained unsafe | 3-5 | Full | Soundness by convention |
| Hybrid Rc + tracing | 3-5 | Partial | Two systems, semantic mismatch |

The arena approach was chosen because:

1. **Zero unsafe** — All access is bounds-checked through `Vec`
   indexing. Generational counters detect stale references.
2. **Full Lua compatibility** — Mark-sweep maps directly to Lua's
   GC algorithm. Incremental collection, weak tables, finalizers,
   and the `collectgarbage()` API are all implementable.
3. **Manageable complexity** — The arena is a simple data structure.
   The GC algorithm is a well-understood adaptation of PUC-Rio's code.
4. **Good performance** — Cache-friendly linear iteration during sweep.
   O(1) allocation from free list. Index-based access adds one bounds
   check per dereference (branch-predicted, negligible in practice).

## Data Structures

### Arena

```rust
pub struct Arena<T> {
    entries: Vec<Entry<T>>,
    free_head: Option<u32>,
    len: u32,
}

struct Entry<T> {
    generation: u32,
    state: SlotState<T>,
}

enum SlotState<T> {
    Occupied { value: T, color: Color },
    Free { next: Option<u32> },
}

#[derive(Clone, Copy)]
enum Color {
    White0,  // White (generation 0)
    White1,  // White (generation 1)
    // Which white is "current" alternates each cycle via current_white.
    // Newly allocated objects get the current white.
    // Objects bearing the "other" white after marking are dead.
    Gray,    // Visited, children not yet traced
    Black,   // Fully traced
}

// PUC-Rio uses two white bits (WHITE0BIT, WHITE1BIT) to
// distinguish objects allocated during sweep from dead objects.
// At the end of the atomic phase, `current_white` flips.
// During sweep, objects bearing the "other white" are dead;
// objects bearing the "current white" survive.
```

### GcRef

```rust
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct GcRef<T> {
    index: u32,
    generation: u32,
    _marker: PhantomData<T>,
}
```

A `GcRef` is valid only if the arena slot at `index` has the same
`generation`. If the slot has been freed and reused, the generation
will differ, and `get()` returns `None`.

### GcHeap

```rust
pub struct GcHeap {
    strings: Arena<LuaString>,
    tables: Arena<Table>,
    closures: Arena<Closure>,
    upvalues: Arena<Upvalue>,
    userdata: Arena<Userdata>,
    threads: Arena<LuaThread>,

    // GC state
    current_white: Color,               // Alternates White0/White1
    gray_stack: Vec<GcObject>,          // Objects to trace
    gray_again: Vec<GcObject>,          // Re-trace in atomic phase
    weak_tables: Vec<GcRef<Table>>,     // Tables with __mode
    finalize_list: Vec<GcRef<Userdata>>,// Dead userdata with __gc

    // GC tuning
    state: GcState,
    total_bytes: usize,
    estimate: usize,    // Memory at end of last cycle
    threshold: usize,
    gc_pause: u32,      // Default: 200
    gc_step_mul: u32,   // Default: 200
    enabled: bool,
}

enum GcState {
    Pause,
    Propagate,
    SweepString,
    Sweep,
    Finalize,
}

/// Type-erased reference for the gray stack.
/// Note: strings are never pushed to the gray stack (they go
/// white-to-black directly in reallymarkobject).
enum GcObject {
    Table(GcRef<Table>),
    Closure(GcRef<Closure>),
    Upvalue(GcRef<Upvalue>),
    Userdata(GcRef<Userdata>),
    Thread(GcRef<LuaThread>),
}
```

### Trace Trait

```rust
/// Implemented by all types that may contain GC references.
pub trait Trace {
    fn trace(&self, tracer: &mut Tracer);
}

pub struct Tracer<'a> {
    heap: &'a mut GcHeap,
}

impl Tracer<'_> {
    pub fn mark<T: Trace>(&mut self, reference: GcRef<T>) {
        // If white, mark gray and push to gray stack
    }
}
```

The `Trace` trait requires no `unsafe` code. Each type
implements it to enumerate its GC references:

```rust
impl Trace for Table {
    fn trace(&self, tracer: &mut Tracer) {
        if let Some(mt) = self.metatable {
            tracer.mark(mt);
        }
        // Check __mode for weak table behavior.
        // Weak keys/values are NOT marked during normal traversal;
        // they are processed separately in the atomic phase.
        let mode = self.weak_mode(); // None, WeakKeys, WeakValues, WeakBoth
        for (k, v) in self.iter() {
            if !mode.has_weak_keys() {
                k.trace(tracer);
            }
            if !mode.has_weak_values() {
                v.trace(tracer);
            }
        }
    }
}
```

## Collection Algorithm

### Mark Phase (Propagate)

1. Mark roots gray: main thread, global table, registry, all type
   metatables (not just string — one per type that supports it).
2. Pop gray objects from gray stack.
3. For each gray object, call `trace()` to mark children.
4. Mark the object black.
5. Repeat until gray stack is empty.

### Atomic Phase

After propagation completes (runs without interleaving). The
ordering below matches PUC-Rio's `atomic()` in `lgc.c`:

1. Re-mark open upvalues (`remarkupvals`). Upvalues of dead
   threads are not roots in `markroot` but must be re-marked here.
2. Propagate all objects marked from step 1 and from write
   barriers that fired during the propagate phase.
3. Set up weak tables for re-traversal (move weak list to gray).
4. Re-mark the currently running thread.
5. Re-mark all type metatables.
6. Propagate all from steps 3-5.
7. Re-trace the gray-again list (tables and threads modified
   during marking via backward barriers). Propagate.
8. Separate finalizable userdata: dead userdata with `__gc`
   metamethods move to `finalize_list`.
9. Mark preserved userdata alive (`marktmu`). Propagate.
10. Clear dead weak entries (`cleartable`).
11. Flip `current_white` (White0 becomes White1 or vice versa).
    New allocations from this point use the new current white.
12. Set state to `SweepString`.

### Sweep Phase

Iterate each arena linearly. For each occupied slot:

- If white (not marked): free the slot (return to free list).
- If black (marked): reset to white for next cycle.

### Finalize Phase

For each userdata in `finalize_list`:

1. Call its `__gc` metamethod.
2. Mark it as finalized (will not finalize again).
3. The finalizer may resurrect the object by storing it somewhere
   reachable. It will be collected in the next cycle if still dead.

## Write Barriers

During the propagate phase, mutations can violate the tri-color
invariant (a black object must not point to a white object). Write
barriers restore the invariant:

**Forward barrier** (closures, upvalues, userdata): When assigning
a white value into a black object during the Propagate phase, mark
the value gray. During SweepString and Sweep phases, the barrier
instead marks the parent white to avoid unnecessary marking. (The
forward barrier is never called during Finalize or Pause — PUC-Rio
asserts this in `luaC_barrierf`.)

**Backward barrier** (tables, threads): When assigning a white value
into a black table, mark the table gray-again (push onto the
`gray_again` list for re-traversal in the atomic phase). Tables use
the backward barrier because they are mutated frequently — re-marking
the table is cheaper than marking every assigned value. Threads also
use a backward-like mechanism because their stacks can be mutated at
any time.

## Weak Tables

Lua 5.1.1 weak table semantics:

- `__mode = "k"` — weak keys. Dead keys (and their values) are
  cleared after marking.
- `__mode = "v"` — weak values. Dead values (and their keys) are
  cleared after marking.
- `__mode = "kv"` — both weak.
- **Strings are never cleared** from weak tables. They are treated
  as values, not collectible objects, for weak table purposes.
- **Finalized userdata** are cleared from weak values but kept in
  weak keys (Lua 5.1.1 quirk).
- Lua 5.1.1 does NOT implement ephemeron tables (added in 5.2).

## collectgarbage() API

| Option | Behavior |
|--------|----------|
| `"stop"` | Disable GC. PUC-Rio sets `GCthreshold = MAX_LUMEM`; rilua sets `enabled = false`. Returns 0. |
| `"restart"` | Re-enable GC. PUC-Rio sets `GCthreshold = totalbytes`; rilua sets `enabled = true`. Returns 0. |
| `"collect"` | Run a full mark-sweep cycle. Returns 0. |
| `"count"` | Return memory in use in KB (float). PUC-Rio computes `(totalbytes >> 10) + (totalbytes & 0x3ff) / 1024.0`. |
| `"step"` | Perform incremental work. Return true if cycle completed. |
| `"setpause"` | Set `gc_pause`. Return previous value. |
| `"setstepmul"` | Set `gc_step_mul`. Return previous value. |

Defaults: `gc_pause = 200` (collect when memory doubles),
`gc_step_mul = 200` (GC runs at 2x allocation speed).

## Incremental Scheduling

The GC runs in small incremental steps interleaved with program
execution, triggered when `total_bytes >= threshold`:

1. After each allocation, check if `total_bytes >= threshold`.
2. If so, perform a GC step with a fixed work limit of
   `(GCSTEPSIZE / 100) * gc_step_mul` (where `GCSTEPSIZE = 1024`).
   The step size is NOT proportional to the allocation size.
3. After a full cycle completes, set the next threshold:
   `threshold = (estimate / 100) * gc_pause`, where `estimate`
   is the memory usage at the end of the cycle (which may differ
   from `total_bytes` due to finalized userdata and freed memory).

This matches PUC-Rio's debt-based scheduling model.

## Proto Ownership

In PUC-Rio, `Proto` is a GC-managed object — it has `CommonHeader`,
is linked into the root GC list via `luaC_link()`, traversed in
`propagatemark()`, and freed in `freeobj()`.

In rilua, function prototypes are managed via `Rc<Proto>` instead.
This is a deliberate divergence because Proto is immutable after
compilation and its ownership graph is a tree (parent protos own
child protos), never cyclic. `Rc` provides automatic deallocation
when the last closure referencing a proto is collected.

The `Closure` type holds `Rc<Proto>` and the `Trace` implementation
for `Closure` does not trace into the Proto. Proto references to
interned strings (source name, local names, upvalue names) are stored
as owned `String` values rather than `GcRef<LuaString>`, so the GC
does not need to trace through protos to keep strings alive.
