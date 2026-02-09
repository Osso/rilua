# Garbage Collector

## Decision

**Arena-based mark-sweep GC using generational indices. Zero unsafe
code. Behavioral compatibility with PUC-Rio Lua 5.1.1's GC
semantics.**

## Overview

All GC-managed objects (strings, tables, closures, userdata, threads)
are allocated in typed arenas — `Vec<Entry<T>>` containers where each
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
| Branded lifetimes | 5-10 | Excellent | Too complex, pervasive `'gc` |
| Contained unsafe | 3-5 | Excellent | Soundness by convention |
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
    White,   // Not yet visited (candidate for collection)
    Gray,    // Visited, children not yet traced
    Black,   // Fully traced
}
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
    userdata: Arena<Userdata>,
    threads: Arena<LuaThread>,

    // GC state
    gray_stack: Vec<GcObject>,  // Objects to trace
    weak_tables: Vec<GcRef<Table>>,  // Tables with __mode
    finalize_list: Vec<GcRef<Userdata>>,  // Dead userdata with __gc

    // GC tuning
    state: GcState,
    total_bytes: usize,
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

/// Type-erased reference for the gray stack
enum GcObject {
    String(GcRef<LuaString>),
    Table(GcRef<Table>),
    Closure(GcRef<Closure>),
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

The `Trace` trait is safe (no `unsafe` required). Each type
implements it to enumerate its GC references:

```rust
impl Trace for Table {
    fn trace(&self, tracer: &mut Tracer) {
        if let Some(mt) = self.metatable {
            tracer.mark(mt);
        }
        for (k, v) in self.iter() {
            k.trace(tracer);
            v.trace(tracer);
        }
    }
}
```

## Collection Algorithm

### Mark Phase (Propagate)

1. Mark roots gray: global table, registry, main thread, open
   upvalues, string metatable.
2. Pop gray objects from gray stack.
3. For each gray object, call `trace()` to mark children.
4. Mark the object black.
5. Repeat until gray stack is empty.

### Atomic Phase

After propagation completes:

1. Re-trace objects on the gray-again list (tables that were
   modified during marking via backward barriers).
2. Process weak tables: collect tables with `__mode` into
   `weak_tables` list.
3. Separate finalizable userdata: dead userdata with `__gc`
   metamethods move to `finalize_list` and are re-marked alive.
4. Clear dead weak entries.

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

**Forward barrier** (most objects): When assigning a white value
into a black object, mark the value gray.

**Backward barrier** (tables): When assigning a white value into
a black table, mark the table gray-again. Tables use the backward
barrier because they are mutated frequently — re-marking the table
is cheaper than marking every assigned value.

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
| `"stop"` | Set `enabled = false`. Returns 0. |
| `"restart"` | Set `enabled = true`, `threshold = total_bytes`. Returns 0. |
| `"collect"` | Run a full mark-sweep cycle. Returns 0. |
| `"count"` | Return `total_bytes / 1024.0` (floating point). |
| `"step"` | Perform incremental work. Return true if cycle completed. |
| `"setpause"` | Set `gc_pause`. Return previous value. |
| `"setstepmul"` | Set `gc_step_mul`. Return previous value. |

Defaults: `gc_pause = 200` (collect when memory doubles),
`gc_step_mul = 200` (GC runs at 2x allocation speed).

## Incremental Scheduling

The GC runs in small incremental steps interleaved with program
execution, triggered when `total_bytes >= threshold`:

1. After each allocation, check if threshold is exceeded.
2. If so, perform a GC step proportional to the allocation size
   multiplied by `gc_step_mul / 100`.
3. After a full cycle completes, set the next threshold:
   `threshold = (total_bytes / 100) * gc_pause`.

This matches PUC-Rio's debt-based scheduling model.

## Proto Ownership

Function prototypes (`Proto`) are NOT managed by the GC. They are
immutable after compilation and shared between closures via
`Rc<Proto>`. This simplifies ownership — the GC only traces mutable,
potentially cyclic objects.

The `Closure` type holds `Rc<Proto>` and the `Trace` implementation
for `Closure` does not trace into the Proto (it is not a GC object).
