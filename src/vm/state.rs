//! Lua VM state: the main execution context.
//!
//! `LuaState` holds the value stack, call stack, global table, and GC
//! heap. Each coroutine gets its own `LuaThread` with a separate stack
//! but sharing the GC heap with all other threads.
//!
//! ## Gc Struct
//!
//! The `Gc` struct holds all typed arenas (strings, tables, closures,
//! upvalues) and the string interning table. In Phase 3, allocation
//! works but no collection cycle runs. Collection is added in Phase 6.
//!
//! ## Stack Layout
//!
//! The value stack is a flat `Vec<Val>`. Each function call occupies a
//! contiguous range: `[func, arg1, ..., argN, local1, ..., localM]`.
//! The `base` field points to the first local (register 0).
//!
//! Reference: `lstate.h`, `lstate.c` in PUC-Rio Lua 5.1.1.

use super::callinfo::{CallInfo, LUA_MULTRET};
use super::closure::{Closure, Upvalue};
use super::gc::Color;
use super::gc::arena::{Arena, GcRef};
use super::gc::trace::Trace;
use super::metatable::{NUM_TYPE_TAGS, TM_N, TM_NAMES};
use super::string::{LuaString, StringTable};
use super::table::Table;
use super::value::{Userdata, Val};
use crate::error::LuaResult;

// ---------------------------------------------------------------------------
// Constants (match PUC-Rio limits)
// ---------------------------------------------------------------------------

/// Maximum total call depth (Lua + Rust functions).
pub const MAXCALLS: usize = 20_000;

/// Maximum nested Rust function calls (prevents Rust stack overflow).
pub const MAXCCALLS: u16 = 200;

/// Minimum stack slots guaranteed for Rust functions.
pub const LUA_MINSTACK: usize = 20;

/// Initial value stack size (2 * LUA_MINSTACK).
const BASIC_STACK_SIZE: usize = 2 * LUA_MINSTACK;

/// Initial CallInfo array capacity.
pub(crate) const BASIC_CI_SIZE: usize = 8;

// ---------------------------------------------------------------------------
// Gc (garbage collector state -- allocation only, no sweep yet)
// ---------------------------------------------------------------------------

/// GC state: holds all typed arenas, string table, and collection state.
///
/// The `gc_state` field holds mark-sweep pacing parameters, gray lists,
/// and memory tracking. Collection runs stop-the-world via `full_gc()`.
pub struct Gc {
    /// Interned strings.
    pub strings: StringTable,
    /// String arena (LuaString storage).
    pub string_arena: Arena<LuaString>,
    /// Table arena.
    pub tables: Arena<Table>,
    /// Closure arena (Lua and Rust closures).
    pub closures: Arena<Closure>,
    /// Upvalue arena.
    pub upvalues: Arena<Upvalue>,
    /// Userdata arena.
    pub userdata: Arena<Userdata>,
    /// Thread arena (coroutines).
    pub threads: Arena<LuaThread>,
    /// Current white color for new allocations.
    pub current_white: Color,
    /// Per-type metatables. Indexed by type tag (see `metatable::type_tag`).
    /// Tables and userdata have per-instance metatables; other types use these.
    pub type_metatables: [Option<GcRef<Table>>; NUM_TYPE_TAGS],
    /// Interned metamethod name strings (one per TMS event).
    /// Initialized once during state creation.
    pub tm_names: [Option<GcRef<LuaString>>; TM_N],
    /// GC collection state: gray lists, pacing, memory tracking.
    pub gc_state: super::gc::collector::GcState,
}

impl Gc {
    /// Creates a new GC state with empty arenas.
    fn new() -> Self {
        let mut gc = Self {
            strings: StringTable::new(),
            string_arena: Arena::new(),
            tables: Arena::new(),
            closures: Arena::new(),
            upvalues: Arena::new(),
            userdata: Arena::new(),
            threads: Arena::new(),
            current_white: Color::White0,
            type_metatables: [None; NUM_TYPE_TAGS],
            tm_names: [None; TM_N],
            gc_state: super::gc::collector::GcState::new(),
        };
        gc.init_tm_names();
        gc
    }

    /// Interns all 17 metamethod name strings.
    ///
    /// Called once during state initialization. These strings are GC roots
    /// and are never collected. Matches PUC-Rio's `luaT_init`.
    fn init_tm_names(&mut self) {
        for (i, name) in TM_NAMES.iter().enumerate() {
            let r = self.intern_string(name.as_bytes());
            self.tm_names[i] = Some(r);
        }
    }

    /// Interns a string, returning a GcRef to the interned LuaString.
    ///
    /// Tracks memory: adds estimated size to `total_bytes` only when a
    /// new string is actually created (not on dedup hit). Debt is NOT
    /// accumulated here; PUC-Rio's `gcdept` only changes in `luaC_step`.
    pub fn intern_string(&mut self, data: &[u8]) -> GcRef<LuaString> {
        let old_count = self.string_arena.len();
        let r = self
            .strings
            .intern(data, &mut self.string_arena, self.current_white);
        // Only track memory if a new string was actually allocated.
        if self.string_arena.len() > old_count {
            let est = super::gc::collector::EST_STRING_SIZE + data.len();
            self.gc_state.total_bytes += est;
        }
        r
    }

    /// Allocates a new table in the GC arena.
    pub fn alloc_table(&mut self, table: Table) -> GcRef<Table> {
        let est = super::gc::collector::EST_TABLE_SIZE
            + table.array_slice().len() * 16
            + table.hash_size() as usize * 32;
        self.gc_state.total_bytes += est;
        self.tables.alloc(table, self.current_white)
    }

    /// Allocates a new closure in the GC arena.
    pub fn alloc_closure(&mut self, closure: Closure) -> GcRef<Closure> {
        self.gc_state.total_bytes += super::gc::collector::EST_CLOSURE_SIZE;
        self.closures.alloc(closure, self.current_white)
    }

    /// Allocates a new upvalue in the GC arena.
    pub fn alloc_upvalue(&mut self, upvalue: Upvalue) -> GcRef<Upvalue> {
        self.gc_state.total_bytes += super::gc::collector::EST_UPVALUE_SIZE;
        self.upvalues.alloc(upvalue, self.current_white)
    }

    /// Allocates a new userdata in the GC arena.
    pub fn alloc_userdata(&mut self, userdata: Userdata) -> GcRef<Userdata> {
        self.gc_state.total_bytes += super::gc::collector::EST_USERDATA_SIZE;
        self.userdata.alloc(userdata, self.current_white)
    }

    /// Allocates a new thread (coroutine) in the GC arena.
    pub fn alloc_thread(&mut self, thread: LuaThread) -> GcRef<LuaThread> {
        self.gc_state.total_bytes += super::gc::collector::EST_THREAD_SIZE;
        self.threads.alloc(thread, self.current_white)
    }

    /// Returns the interned string GcRef for a metamethod name.
    #[inline]
    pub fn tm_name(&self, event: super::metatable::TMS) -> Option<GcRef<LuaString>> {
        self.tm_names[event as usize]
    }
}

impl Default for Gc {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// ThreadStatus
// ---------------------------------------------------------------------------

/// Status of a coroutine thread.
///
/// Maps to PUC-Rio's thread status values:
/// - 0 = initial (function loaded, not yet started) or finished ok
/// - `LUA_YIELD` = suspended (yielded)
/// - Any error status = dead (errored)
///
/// We split the 0 case into `Initial` (has function, no frames) and
/// `Dead` (finished or errored) for clarity.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThreadStatus {
    /// Function loaded, not yet started. Stack has the function + args.
    Initial,
    /// Currently being executed (state is in `LuaState`, not in this struct).
    Running,
    /// Yielded, waiting to be resumed. Stack has yielded values.
    Suspended,
    /// Resumed another coroutine and waiting for it to yield/finish.
    Normal,
    /// Finished execution (returned) or errored. Cannot be resumed.
    Dead,
}

// ---------------------------------------------------------------------------
// LuaThread (coroutine)
// ---------------------------------------------------------------------------

/// A Lua thread (coroutine) with its own stack and call stack.
///
/// Each coroutine has independent per-thread state but shares the GC
/// heap (`Gc`) with all other threads. When a coroutine is not running,
/// its state is stored here. When running, its state is swapped into
/// `LuaState` (the "swap model") and this struct holds the resumer's
/// saved state or default values.
///
/// Reference: `lua_State` in `lstate.h` (per-thread fields).
pub struct LuaThread {
    /// Value stack.
    pub stack: Vec<Val>,
    /// Base of current function's frame.
    pub base: usize,
    /// First free slot in the value stack.
    pub top: usize,
    /// Call stack.
    pub call_stack: Vec<CallInfo>,
    /// Current call stack index.
    pub ci: usize,
    /// Nested C-call boundary depth (for yield boundary check).
    pub n_ccalls: u16,
    /// Recursive execute() depth counter (for Rust stack overflow detection).
    pub call_depth: u16,
    /// Set when ci reaches MAXCALLS. Cleared when ci drops below MAXCALLS.
    /// Allows headroom for error handlers after stack overflow.
    pub ci_overflow: bool,
    /// Open upvalues.
    pub open_upvalues: Vec<GcRef<Upvalue>>,
    /// Upvalues that were open when the thread was suspended.
    /// Each entry stores (upvalue_ref, original_stack_index).
    /// On resume, these are reopened: the closed value is written back
    /// to the stack slot and the upvalue is marked Open again.
    /// This is necessary because rilua's swap model moves the stack
    /// between threads, which would leave open upvalues pointing at
    /// the wrong stack.
    pub suspended_upvals: Vec<(GcRef<Upvalue>, usize)>,
    /// Error object for error propagation.
    pub error_object: Option<Val>,
    /// Thread status.
    pub status: ThreadStatus,
    /// Per-thread global table. Each thread can have its own global
    /// environment, set via `setfenv(thread, table)`.
    pub global: GcRef<Table>,
}

impl LuaThread {
    /// Creates a new thread with an initial stack and the given function.
    ///
    /// The function is placed at stack[0], with base=1 and top=1.
    /// Status is `Initial` (ready to be resumed for the first time).
    /// The thread inherits the given global table from its creator.
    pub fn new(func_val: Val, global: GcRef<Table>) -> Self {
        let mut stack = vec![Val::Nil; BASIC_STACK_SIZE];
        stack[0] = func_val;

        let initial_ci = CallInfo::new(0, 1, 1 + LUA_MINSTACK, LUA_MULTRET);
        let mut call_stack = Vec::with_capacity(BASIC_CI_SIZE);
        call_stack.push(initial_ci);

        Self {
            stack,
            base: 1,
            top: 1,
            call_stack,
            ci: 0,
            n_ccalls: 0,
            call_depth: 0,
            ci_overflow: false,
            open_upvalues: Vec::new(),
            suspended_upvals: Vec::new(),
            error_object: None,
            status: ThreadStatus::Initial,
            global,
        }
    }
}

impl Trace for LuaThread {
    fn trace(&self) {
        // Phase 7: mark all Val references in the stack, open upvalues, etc.
    }
}

// ---------------------------------------------------------------------------
// LuaState
// ---------------------------------------------------------------------------

/// The main VM state.
///
/// Holds the value stack, call stack, GC heap, global table, registry,
/// and open upvalue list. This is the central data structure for
/// executing Lua bytecode.
pub struct LuaState {
    /// Value stack. All Lua values live here during execution.
    pub stack: Vec<Val>,

    /// Base of current function's frame (first local / register 0).
    /// Always mirrors `call_stack[ci].base`.
    pub base: usize,

    /// First free slot in the value stack.
    pub top: usize,

    /// Call stack: one entry per active function call.
    pub call_stack: Vec<CallInfo>,

    /// Index into `call_stack` for the current frame.
    pub ci: usize,

    /// Nested Rust call depth counter (yield boundary: yield only when 0).
    pub n_ccalls: u16,

    /// Recursive execute() depth counter (Rust stack overflow detection).
    pub call_depth: u16,

    /// Set when ci reaches MAXCALLS. Cleared when ci drops below MAXCALLS.
    pub ci_overflow: bool,

    /// Global table (_G). Used by GETGLOBAL/SETGLOBAL.
    pub global: GcRef<Table>,

    /// Registry table. Internal storage for the VM.
    pub registry: GcRef<Table>,

    /// Open upvalues sorted by stack index (descending).
    /// Used by find_upvalue and close_upvalues.
    pub open_upvalues: Vec<GcRef<Upvalue>>,

    /// GC state (all arenas and string table).
    pub gc: Gc,

    /// Error object for `pcall`/`xpcall` error propagation.
    ///
    /// When `error()` throws a value, it's stored here so `pcall` can
    /// retrieve it. `None` for VM-generated errors (pcall uses the
    /// message string instead). Cleared after pcall reads it.
    pub error_object: Option<Val>,

    /// Random number generator state for `math.random` / `math.randomseed`.
    ///
    /// Uses a linear congruential generator matching common C `rand()`
    /// implementations (glibc constants). State is initialized as if
    /// `srand(1)` was called, per the C standard default.
    pub rng_state: u64,

    /// Currently running coroutine thread, or `None` if this is the main
    /// thread's direct execution context.
    ///
    /// Used by `coroutine.running()` and `coroutine.status()` to identify
    /// which thread is active. When `Some(ref)`, the `LuaState`'s per-thread
    /// fields (stack, call_stack, etc.) belong to that coroutine.
    pub current_thread: Option<GcRef<LuaThread>>,
}

impl LuaState {
    /// Creates a new VM state with an empty stack and initial CallInfo.
    ///
    /// Allocates the global table and registry in the GC arena,
    /// initializes the value stack to `BASIC_STACK_SIZE` slots (all nil),
    /// and pushes the initial (bottom) CallInfo frame.
    #[must_use]
    pub fn new() -> Self {
        let mut gc = Gc::new();

        // Allocate global and registry tables.
        let global = gc.alloc_table(Table::new());
        let registry = gc.alloc_table(Table::new());

        // Initialize value stack: BASIC_STACK_SIZE slots, all nil.
        let stack = vec![Val::Nil; BASIC_STACK_SIZE];

        // Initial CallInfo: func=0, base=1 (slot 0 holds the "entry" function).
        // Top is set to base + LUA_MINSTACK to provide minimum stack space.
        let initial_ci = CallInfo::new(0, 1, 1 + LUA_MINSTACK, LUA_MULTRET);

        let mut call_stack = Vec::with_capacity(BASIC_CI_SIZE);
        call_stack.push(initial_ci);

        Self {
            stack,
            base: 1,
            top: 1,
            call_stack,
            ci: 0,
            n_ccalls: 0,
            call_depth: 0,
            ci_overflow: false,
            global,
            registry,
            open_upvalues: Vec::new(),
            gc,
            error_object: None,
            rng_state: 1, // C standard: default as if srand(1) was called.
            current_thread: None,
        }
    }

    // ----- Stack operations -----

    /// Returns the value at the given absolute stack index.
    ///
    /// Returns `Val::Nil` if the index is out of bounds.
    #[inline]
    pub fn stack_get(&self, idx: usize) -> Val {
        if idx < self.stack.len() {
            self.stack[idx]
        } else {
            Val::Nil
        }
    }

    /// Sets the value at the given absolute stack index.
    ///
    /// Grows the stack with nil values if the index is beyond current
    /// capacity.
    #[inline]
    pub fn stack_set(&mut self, idx: usize, val: Val) {
        if idx >= self.stack.len() {
            self.stack.resize(idx + 1, Val::Nil);
        }
        self.stack[idx] = val;
    }

    /// Ensures at least `n` free slots above `top`.
    ///
    /// Grows the stack if necessary.
    pub fn ensure_stack(&mut self, n: usize) {
        let needed = self.top + n;
        if needed > self.stack.len() {
            self.stack.resize(needed, Val::Nil);
        }
    }

    /// Pushes a value onto the stack at `top` and increments `top`.
    pub fn push(&mut self, val: Val) {
        if self.top >= self.stack.len() {
            self.stack.resize(self.top + 1, Val::Nil);
        }
        self.stack[self.top] = val;
        self.top += 1;
    }

    /// Pops the top value from the stack and returns it.
    ///
    /// Returns `Val::Nil` if the stack is empty.
    pub fn pop(&mut self) -> Val {
        if self.top > 0 {
            self.top -= 1;
            self.stack[self.top]
        } else {
            Val::Nil
        }
    }

    /// Metamethod-aware table index: `t[key]` with `__index` chain.
    ///
    /// Equivalent to PUC-Rio's `lua_gettable`. Follows `__index` metamethods
    /// up to `MAXTAGLOOP` depth. Used by stdlib code that needs full Lua
    /// table access semantics (e.g., gsub table replacement).
    pub fn gettable(&mut self, t: Val, key: Val) -> LuaResult<Val> {
        use super::metatable::{MAXTAGLOOP, TMS, gettmbyobj};

        let mut current = t;
        for _ in 0..MAXTAGLOOP {
            if let Val::Table(table_ref) = current {
                let result = self
                    .gc
                    .tables
                    .get(table_ref)
                    .map_or(Val::Nil, |tbl| tbl.get(key, &self.gc.string_arena));
                if !result.is_nil() {
                    return Ok(result);
                }
                // Check __index metamethod.
                let tm = gettmbyobj(
                    current,
                    TMS::Index,
                    &self.gc.tables,
                    &self.gc.string_arena,
                    &self.gc.type_metatables,
                    &self.gc.tm_names,
                    &self.gc.userdata,
                );
                match tm {
                    None => return Ok(Val::Nil),
                    Some(tm_val) if matches!(tm_val, Val::Function(_)) => {
                        let saved_top = self.top;
                        let call_base = self.top;
                        self.ensure_stack(call_base + 4);
                        self.stack_set(call_base, tm_val);
                        self.stack_set(call_base + 1, current);
                        self.stack_set(call_base + 2, key);
                        self.top = call_base + 3;
                        self.call_function(call_base, 1)?;
                        let result = self.stack_get(call_base);
                        self.top = saved_top;
                        return Ok(result);
                    }
                    Some(tm_val) => {
                        current = tm_val;
                    }
                }
            } else {
                return Ok(Val::Nil);
            }
        }
        Err(crate::error::LuaError::Runtime(
            crate::error::RuntimeError {
                message: "loop in gettable".into(),
                level: 0,
                traceback: vec![],
            },
        ))
    }

    // ----- CallInfo helpers -----

    /// Returns a reference to the current CallInfo.
    #[inline]
    pub fn ci(&self) -> &CallInfo {
        &self.call_stack[self.ci]
    }

    /// Returns a mutable reference to the current CallInfo.
    #[inline]
    pub fn ci_mut(&mut self) -> &mut CallInfo {
        &mut self.call_stack[self.ci]
    }

    /// Pushes a new CallInfo frame onto the call stack.
    ///
    /// Writes at `ci + 1`, reusing stale slots left by previous `pop_ci`
    /// calls. Only appends when no reusable slot exists. This matches
    /// PUC-Rio's linked-list reuse pattern for `CallInfo` frames.
    pub fn push_ci(&mut self, ci: CallInfo) -> &mut CallInfo {
        let new_idx = self.ci + 1;
        if new_idx < self.call_stack.len() {
            self.call_stack[new_idx] = ci;
        } else {
            self.call_stack.push(ci);
        }
        self.ci = new_idx;
        &mut self.call_stack[self.ci]
    }

    /// Pops the current CallInfo frame from the call stack.
    ///
    /// Restores `ci` to point to the previous frame.
    pub fn pop_ci(&mut self) {
        if self.ci > 0 {
            self.ci -= 1;
        }
    }

    /// Returns the number of arguments currently on the stack above `func`.
    ///
    /// Computed as `top - func - 1` (the function itself is not an argument).
    #[inline]
    pub fn get_nargs(&self, func_idx: usize) -> usize {
        if self.top > func_idx + 1 {
            self.top - func_idx - 1
        } else {
            0
        }
    }

    // ----- Coroutine thread swap -----

    /// Saves the current per-thread state into a `LuaThread`.
    ///
    /// Used by `coroutine.resume()` to save the resumer's state before
    /// loading the coroutine's state into `LuaState`.
    pub fn save_thread_state(&mut self) -> LuaThread {
        LuaThread {
            stack: std::mem::take(&mut self.stack),
            base: self.base,
            top: self.top,
            call_stack: std::mem::take(&mut self.call_stack),
            ci: self.ci,
            n_ccalls: self.n_ccalls,
            call_depth: self.call_depth,
            ci_overflow: self.ci_overflow,
            open_upvalues: std::mem::take(&mut self.open_upvalues),
            suspended_upvals: Vec::new(),
            error_object: self.error_object.take(),
            status: ThreadStatus::Normal,
            global: self.global,
        }
    }

    /// Loads per-thread state from a GC-managed `LuaThread` into this
    /// `LuaState`, and sets the thread's status.
    ///
    /// The thread's fields are moved into `LuaState` via `mem::take`
    /// (the thread is left in a default/empty state). This method takes
    /// a `GcRef` to avoid borrow conflicts -- the arena access happens
    /// inside `&mut self`, so the borrow checker sees a single mutable
    /// reference.
    ///
    /// Used to activate a coroutine for execution.
    pub fn load_thread_by_ref(&mut self, co_ref: GcRef<LuaThread>, new_status: ThreadStatus) {
        if let Some(thread) = self.gc.threads.get_mut(co_ref) {
            thread.status = new_status;
            self.stack = std::mem::take(&mut thread.stack);
            self.base = thread.base;
            self.top = thread.top;
            self.call_stack = std::mem::take(&mut thread.call_stack);
            self.ci = thread.ci;
            self.n_ccalls = thread.n_ccalls;
            self.call_depth = thread.call_depth;
            self.ci_overflow = thread.ci_overflow;
            self.open_upvalues = std::mem::take(&mut thread.open_upvalues);
            self.error_object = thread.error_object.take();
            self.global = thread.global;

            // Reopen upvalues that were closed on suspension.
            // Write their captured values back to the stack slots and
            // mark them as Open again so the running function and its
            // closures share the same variable through the stack.
            let suspended = std::mem::take(&mut thread.suspended_upvals);
            for (uv_ref, idx) in &suspended {
                if let Some(uv) = self.gc.upvalues.get(*uv_ref) {
                    if let crate::vm::closure::UpvalueState::Closed { value } = uv.state {
                        if *idx < self.stack.len() {
                            self.stack[*idx] = value;
                        }
                    }
                }
                if let Some(uv) = self.gc.upvalues.get_mut(*uv_ref) {
                    uv.state = crate::vm::closure::UpvalueState::Open { stack_index: *idx };
                }
                // Re-add to open_upvalues list if not already present.
                if !self.open_upvalues.contains(uv_ref) {
                    self.open_upvalues.push(*uv_ref);
                }
            }
            // Re-sort open_upvalues by stack index descending.
            self.open_upvalues.sort_by(|a, b| {
                let a_idx = self
                    .gc
                    .upvalues
                    .get(*a)
                    .and_then(|uv| uv.stack_index())
                    .unwrap_or(0);
                let b_idx = self
                    .gc
                    .upvalues
                    .get(*b)
                    .and_then(|uv| uv.stack_index())
                    .unwrap_or(0);
                b_idx.cmp(&a_idx)
            });
        }
    }

    /// Saves the current per-thread state into a GC-managed `LuaThread`
    /// (with a given status), then restores this `LuaState` from the
    /// saved resumer state.
    ///
    /// Takes a `GcRef` to avoid borrow conflicts. Used after coroutine
    /// execution completes (return, yield, or error).
    pub fn save_and_restore_by_ref(
        &mut self,
        co_ref: GcRef<LuaThread>,
        co_status: ThreadStatus,
        resumer: LuaThread,
    ) {
        // Close open upvalues before the stack swap.
        //
        // In rilua's swap model, the coroutine's stack is about to be saved
        // to the GC arena and the resumer's stack loaded. Open upvalues
        // pointing into the coroutine's stack would then read from the
        // wrong stack. We close them (capturing values) and record their
        // original stack indices so they can be reopened on resume.
        let mut suspended = Vec::new();
        for &uv_ref in &self.open_upvalues {
            if let Some(uv) = self.gc.upvalues.get(uv_ref) {
                if let Some(idx) = uv.stack_index() {
                    suspended.push((uv_ref, idx));
                }
            }
        }
        for &(uv_ref, _) in &suspended {
            if let Some(uv) = self.gc.upvalues.get_mut(uv_ref) {
                uv.close(&self.stack);
            }
        }

        // Save current state into the coroutine.
        if let Some(co_thread) = self.gc.threads.get_mut(co_ref) {
            co_thread.stack = std::mem::take(&mut self.stack);
            co_thread.base = self.base;
            co_thread.top = self.top;
            co_thread.call_stack = std::mem::take(&mut self.call_stack);
            co_thread.ci = self.ci;
            co_thread.n_ccalls = self.n_ccalls;
            co_thread.call_depth = self.call_depth;
            co_thread.ci_overflow = self.ci_overflow;
            co_thread.open_upvalues = std::mem::take(&mut self.open_upvalues);
            co_thread.suspended_upvals = suspended;
            co_thread.error_object = self.error_object.take();
            co_thread.global = self.global;
            co_thread.status = co_status;
        }

        // Restore resumer's state.
        self.stack = resumer.stack;
        self.base = resumer.base;
        self.top = resumer.top;
        self.call_stack = resumer.call_stack;
        self.ci = resumer.ci;
        self.n_ccalls = resumer.n_ccalls;
        self.call_depth = resumer.call_depth;
        self.ci_overflow = resumer.ci_overflow;
        self.open_upvalues = resumer.open_upvalues;
        self.error_object = resumer.error_object;
        self.global = resumer.global;

        // Reopen the resumer's suspended upvalues. These were closed before
        // the stack swap to prevent cross-thread reads. Now that the
        // resumer's stack is active again, write the captured values back
        // to the stack slots and mark the upvalues as Open.
        for (uv_ref, idx) in resumer.suspended_upvals {
            if let Some(uv) = self.gc.upvalues.get(uv_ref) {
                if let crate::vm::closure::UpvalueState::Closed { value } = uv.state {
                    if idx < self.stack.len() {
                        self.stack[idx] = value;
                    }
                }
            }
            if let Some(uv) = self.gc.upvalues.get_mut(uv_ref) {
                uv.state = crate::vm::closure::UpvalueState::Open { stack_index: idx };
            }
            if !self.open_upvalues.contains(&uv_ref) {
                self.open_upvalues.push(uv_ref);
            }
        }
    }
}

impl Default for LuaState {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // ----- LuaState construction -----

    #[test]
    fn new_state_has_stack() {
        let state = LuaState::new();
        assert_eq!(state.stack.len(), BASIC_STACK_SIZE);
        assert_eq!(state.top, 1);
        assert_eq!(state.base, 1);
    }

    #[test]
    fn new_state_has_initial_ci() {
        let state = LuaState::new();
        assert_eq!(state.call_stack.len(), 1);
        assert_eq!(state.ci, 0);
        let ci = state.ci();
        assert_eq!(ci.func, 0);
        assert_eq!(ci.base, 1);
        assert_eq!(ci.top, 1 + LUA_MINSTACK);
        assert_eq!(ci.num_results, LUA_MULTRET);
    }

    #[test]
    fn new_state_has_global_table() {
        let state = LuaState::new();
        assert!(state.gc.tables.is_valid(state.global));
    }

    #[test]
    fn new_state_has_registry() {
        let state = LuaState::new();
        assert!(state.gc.tables.is_valid(state.registry));
    }

    #[test]
    fn new_state_gc_initialized() {
        let state = LuaState::new();
        // Two tables allocated (global + registry).
        assert_eq!(state.gc.tables.len(), 2);
        // 17 interned metamethod name strings from init_tm_names.
        assert_eq!(state.gc.string_arena.len(), TM_N as u32);
        assert_eq!(state.gc.closures.len(), 0);
        assert_eq!(state.gc.upvalues.len(), 0);
        assert_eq!(state.gc.userdata.len(), 0);
    }

    #[test]
    fn new_state_no_ccalls() {
        let state = LuaState::new();
        assert_eq!(state.n_ccalls, 0);
    }

    #[test]
    fn new_state_no_open_upvalues() {
        let state = LuaState::new();
        assert!(state.open_upvalues.is_empty());
    }

    // ----- Stack operations -----

    #[test]
    fn stack_get_valid_index() {
        let mut state = LuaState::new();
        state.stack[5] = Val::Num(42.0);
        assert_eq!(state.stack_get(5), Val::Num(42.0));
    }

    #[test]
    fn stack_get_out_of_bounds() {
        let state = LuaState::new();
        assert!(state.stack_get(1000).is_nil());
    }

    #[test]
    fn stack_set_within_bounds() {
        let mut state = LuaState::new();
        state.stack_set(5, Val::Num(99.0));
        assert_eq!(state.stack[5], Val::Num(99.0));
    }

    #[test]
    fn stack_set_grows_stack() {
        let mut state = LuaState::new();
        let old_len = state.stack.len();
        state.stack_set(old_len + 10, Val::Bool(true));
        assert!(state.stack.len() > old_len);
        assert_eq!(state.stack[old_len + 10], Val::Bool(true));
    }

    #[test]
    fn ensure_stack_no_growth_needed() {
        let mut state = LuaState::new();
        let old_len = state.stack.len();
        state.ensure_stack(5);
        // top=1, need 1+5=6, stack is already BASIC_STACK_SIZE (40).
        assert_eq!(state.stack.len(), old_len);
    }

    #[test]
    fn ensure_stack_grows() {
        let mut state = LuaState::new();
        state.top = BASIC_STACK_SIZE - 2;
        state.ensure_stack(10);
        assert!(state.stack.len() >= BASIC_STACK_SIZE - 2 + 10);
    }

    #[test]
    fn push_and_pop() {
        let mut state = LuaState::new();
        state.push(Val::Num(1.0));
        state.push(Val::Num(2.0));
        state.push(Val::Num(3.0));
        assert_eq!(state.top, 4); // base was 1, pushed 3
        assert_eq!(state.pop(), Val::Num(3.0));
        assert_eq!(state.pop(), Val::Num(2.0));
        assert_eq!(state.pop(), Val::Num(1.0));
        assert_eq!(state.top, 1);
    }

    #[test]
    fn pop_empty_returns_nil() {
        let mut state = LuaState::new();
        state.top = 0;
        assert!(state.pop().is_nil());
    }

    #[test]
    fn push_grows_stack_if_needed() {
        let mut state = LuaState::new();
        state.top = state.stack.len();
        state.push(Val::Num(42.0));
        assert_eq!(state.stack_get(state.top - 1), Val::Num(42.0));
    }

    // ----- CallInfo helpers -----

    #[test]
    fn ci_returns_current_frame() {
        let state = LuaState::new();
        assert_eq!(state.ci().func, 0);
        assert_eq!(state.ci().base, 1);
    }

    #[test]
    fn ci_mut_allows_modification() {
        let mut state = LuaState::new();
        state.ci_mut().saved_pc = 10;
        assert_eq!(state.ci().saved_pc, 10);
    }

    #[test]
    fn push_and_pop_ci() {
        let mut state = LuaState::new();
        assert_eq!(state.ci, 0);

        let new_ci = CallInfo::new(5, 6, 26, 1);
        state.push_ci(new_ci);
        assert_eq!(state.ci, 1);
        assert_eq!(state.ci().func, 5);
        assert_eq!(state.ci().base, 6);

        state.pop_ci();
        assert_eq!(state.ci, 0);
        assert_eq!(state.ci().func, 0);
    }

    #[test]
    fn nested_ci_push_pop() {
        let mut state = LuaState::new();
        state.push_ci(CallInfo::new(5, 6, 26, 1));
        state.push_ci(CallInfo::new(10, 11, 31, 2));
        state.push_ci(CallInfo::new(15, 16, 36, 3));
        assert_eq!(state.ci, 3);
        assert_eq!(state.call_stack.len(), 4);

        state.pop_ci();
        assert_eq!(state.ci, 2);
        assert_eq!(state.ci().func, 10);

        state.pop_ci();
        assert_eq!(state.ci, 1);
        assert_eq!(state.ci().func, 5);

        state.pop_ci();
        assert_eq!(state.ci, 0);
        assert_eq!(state.ci().func, 0);
    }

    #[test]
    fn pop_ci_does_not_underflow() {
        let mut state = LuaState::new();
        state.pop_ci(); // already at 0
        assert_eq!(state.ci, 0);
    }

    // ----- Gc operations -----

    #[test]
    fn gc_intern_string() {
        let mut state = LuaState::new();
        let r = state.gc.intern_string(b"hello");
        assert!(state.gc.string_arena.is_valid(r));
        let s = state.gc.string_arena.get(r);
        assert!(s.is_some());
        assert_eq!(s.map(LuaString::data), Some(b"hello".as_ref()));
    }

    #[test]
    fn gc_intern_string_dedup() {
        let mut state = LuaState::new();
        let before = state.gc.string_arena.len();
        let r1 = state.gc.intern_string(b"test");
        let r2 = state.gc.intern_string(b"test");
        assert_eq!(r1, r2);
        // Only one new string interned (deduplication).
        assert_eq!(state.gc.string_arena.len(), before + 1);
    }

    #[test]
    fn gc_alloc_table() {
        let mut state = LuaState::new();
        let t = state.gc.alloc_table(Table::new());
        assert!(state.gc.tables.is_valid(t));
        // 2 from new() + 1 just allocated.
        assert_eq!(state.gc.tables.len(), 3);
    }

    #[test]
    fn get_nargs_with_args() {
        let mut state = LuaState::new();
        // Simulate: func at index 5, args at 6,7,8, top=9
        state.top = 9;
        assert_eq!(state.get_nargs(5), 3);
    }

    #[test]
    fn get_nargs_no_args() {
        let mut state = LuaState::new();
        // func at index 5, top=6 (only the function itself)
        state.top = 6;
        assert_eq!(state.get_nargs(5), 0);
    }

    #[test]
    fn get_nargs_top_before_func() {
        let mut state = LuaState::new();
        state.top = 3;
        assert_eq!(state.get_nargs(5), 0);
    }

    // ----- Constants -----

    #[test]
    fn constants_match_puc_rio() {
        assert_eq!(MAXCALLS, 20_000);
        assert_eq!(MAXCCALLS, 200);
        assert_eq!(LUA_MINSTACK, 20);
        assert_eq!(BASIC_STACK_SIZE, 40);
        assert_eq!(BASIC_CI_SIZE, 8);
    }

    #[test]
    fn default_creates_new_state() {
        let state = LuaState::default();
        assert_eq!(state.call_stack.len(), 1);
        assert_eq!(state.stack.len(), BASIC_STACK_SIZE);
    }
}
