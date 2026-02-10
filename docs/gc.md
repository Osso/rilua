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
/// white-to-gray directly in reallymarkobject — no children to trace).
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

Triggered by transitioning from `Pause` to `Propagate`. Runs
incrementally via `single_step()`.

**Root marking** (`mark_root`): Mark the following objects gray:

- Main thread
- Global table
- Registry
- All per-type metatables (one per Lua type)

**Propagation** (`propagate_mark`): Each incremental step:

1. Pop one gray object from `gray_stack`.
2. Mark it black (set color to `Black`).
3. Call `trace()` on the object, which marks its children:
   - **Table**: Mark metatable. Check `__mode` for weak table
     handling. If weak, add to `weak_tables` list and keep gray
     (do not mark children covered by weakness). Non-weak keys and
     values are marked.
   - **Closure (Lua)**: Mark environment table, prototype (Rc, no
     GC mark needed), and all upvalue GcRefs.
   - **Closure (Rust)**: Mark environment table and all upvalue
     values.
   - **Thread**: Mark globals table. Mark all values in the stack
     from `stack[0]` to the maximum of all CallInfo `top` values.
     Nil out the inactive portion. Strings in the stack are marked
     directly (not pushed to gray).
   - **Proto** (if GC-managed): Mark source name, constant strings,
     nested protos, local variable names, upvalue names. In rilua,
     Proto is `Rc`-managed, so this does not apply.
4. Return the estimated work cost (object memory size in bytes).
5. Repeat until gray stack is empty, then transition to atomic
   phase.

**Strings**: Never pushed to gray stack. They have no children to
trace. `mark_string` clears the white bits directly (white to
non-white in one step).

### Atomic Phase

Runs without interleaving after propagation drains the gray stack.
This is the only stop-the-world portion of the GC cycle.

The ordering matches PUC-Rio's `atomic()` in `lgc.c`. Each step
includes the rationale:

1. **Re-mark open upvalues** (`remarkupvals`). Iterate the global
   open upvalue list. For each gray upvalue, mark its pointed-to
   value. *Rationale*: A thread may have died during the mark phase,
   but another closure still holds an upvalue pointing to the dead
   thread's stack. The upvalue's value must stay alive.

2. **Propagate** from step 1. Drain any gray objects created by
   upvalue re-marking.

3. **Move weak tables to gray** (`gray_stack = weak_tables`; clear
   `weak_tables`). *Rationale*: Weak tables were kept gray during
   propagation. Now re-traverse them to determine which entries are
   still reachable via non-weak paths.

4. **Re-mark current thread** and **re-mark all type metatables**.
   *Rationale*: Objects may have been created or mutated since the
   main mark phase started. The current thread and metatables are
   always reachable.

5. **Propagate** from steps 3-4.

6. **Re-trace gray-again list** (`gray_stack = gray_again`; clear
   `gray_again`). Propagate. *Rationale*: The gray-again list
   contains tables that were marked black but subsequently mutated
   (caught by the backward write barrier). They must be re-traversed
   to ensure all their new children are marked.

7. **Separate finalizable userdata** (`separate_udata`). Iterate
   all userdata. For each white (dead) userdata that has a `__gc`
   metamethod and has not been finalized yet:
   - Mark it as finalized (set flag).
   - Move it to `finalize_list`.
   *Rationale*: Identifies which userdata need finalizer calls.

8. **Mark finalizable userdata alive** (`mark_tmu`). For each
   userdata in `finalize_list`:
   - Reset to current white (resurrect it).
   - Mark it and trace its children.
   Propagate. *Rationale*: `__gc` methods can reference other
   objects. Those objects must not be collected this cycle. This is
   the "resurrection" mechanism.

9. **Clear dead weak entries** (`clear_table`). For each table in
   the weak tables list, remove entries where:
   - Weak key is white (dead) -- remove entry.
   - Weak value is white (dead) -- remove entry.
   - Exception: **strings are never cleared** from weak tables
     (the `is_cleared` check re-marks strings immediately).
   *Rationale*: Weak references to dead objects must be removed.

10. **Flip current white**. `current_white` toggles between
    `White0` and `White1`. All new allocations from this point use
    the new current white. Objects still bearing the old white are
    dead and will be freed during sweep.

11. **Initialize sweep**: Set `state = SweepString`, reset sweep
    cursors, compute `estimate` (live memory estimate, excluding
    finalizable userdata size).

### Sweep Phase

Two sub-phases, each runs incrementally:

**SweepString** (`state = SweepString`): Iterate the string intern
table one hash bucket per step. For each string in the bucket:
- If bearing the "other white" (dead): remove from intern table
  and free.
- If alive: reset to current white.

String sweep is separate because strings are stored in the intern
table (hash map), not in the main object list.

When all string buckets are swept, transition to `Sweep`.

**Sweep** (`state = Sweep`): Iterate each typed arena linearly, up
to `GCSWEEPMAX` (40) objects per step. For each occupied slot:
- If bearing the "other white" (dead): free the slot (return to
  arena free list), decrement `total_bytes`.
- If alive (black or current white): reset color to current white
  for the next cycle.

When all arenas are swept, transition to `Finalize`.

**Dead object test**: An object is dead if its white color does not
match `current_white`. After the atomic phase flips white, the old
white becomes the "other white" and marks dead objects.

### Finalize Phase

Runs one finalizer per incremental step:

1. Pop the first userdata from `finalize_list`.
2. Move it back to the main arena (it may be collected next cycle
   if not resurrected).
3. Reset its color to current white.
4. Look up `__gc` on its metatable.
5. If `__gc` exists:
   - Disable debug hooks during the call.
   - Set a high GC threshold (prevent nested GC: `threshold =
     2 * total_bytes`).
   - Call `__gc(userdata)` with zero expected results.
   - Restore hooks and threshold.
6. When `finalize_list` is empty, transition to `Pause`.
   Set `threshold = (estimate / 100) * gc_pause`.

## Write Barriers

During the propagate phase, mutations can violate the tri-color
invariant (a black object must not point to a white object). Write
barriers restore the invariant:

**Forward barrier** (`barrier_f`) — used by closures, upvalues,
userdata:

```
fn barrier_f(parent: &mut GcObject, child: GcRef) {
    assert!(parent.is_black() && child.is_white());
    assert!(state != Finalize && state != Pause);
    if state == Propagate {
        mark_object(child);  // Mark the white child gray
    } else {
        // During sweep phases: make the parent white instead.
        // Cheaper than marking since we are near end of cycle.
        parent.set_color(current_white);
    }
}
```

Triggered when:
- Setting an upvalue's value
- Setting a closure's environment

**Backward barrier** (`barrier_back`) — used by tables:

```
fn barrier_back(table: &mut Table) {
    assert!(table.is_black());
    assert!(state != Finalize && state != Pause);
    table.set_color(Gray);        // Demote black to gray
    gray_again.push(table.ref);   // Re-traverse in atomic phase
}
```

Triggered when:
- Setting a table element (`t[k] = v`)
- Setting a table's metatable

Tables use the backward barrier because they are mutated frequently.
Re-traversing the entire table once in the atomic phase is cheaper
than marking each individual value as it is assigned. The gray-again
list is drained in atomic phase step 6.

**Why two barrier types**: Forward barriers (mark the child) are
appropriate for small objects with few references. Backward barriers
(re-gray the parent) are appropriate for large objects that are
frequently mutated. The choice follows PUC-Rio exactly.

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

### Scheduling Constants

| Constant | Value | Purpose |
|----------|-------|---------|
| `GCSTEPSIZE` | 1024 | Base work quantum (bytes) |
| `GCSWEEPMAX` | 40 | Max objects swept per step |
| `GCSWEEPCOST` | 10 | Estimated cost per string bucket |
| `GCFINALIZECOST` | 100 | Estimated cost per finalizer |

**Work budget per step**: `(GCSTEPSIZE / 100) * gc_step_mul`.
With default `gc_step_mul = 200`: budget = 2048 bytes of work.

**Cost reporting per phase**:
- Propagate: returns the memory size of the traversed object.
- SweepString: returns `GCSWEEPCOST` per bucket.
- Sweep: returns `GCSWEEPMAX * GCSWEEPCOST` per step.
- Finalize: returns `GCFINALIZECOST` per finalizer.

**Debt tracking**: The GC accumulates "debt" when allocations
exceed the threshold faster than the GC can keep up. Each step
reduces debt by the work performed. When debt exceeds `GCSTEPSIZE`,
the next step runs immediately (threshold set to `total_bytes`).

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
