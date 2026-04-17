//! Freeze-after-bootstrap: walk a root table and pin every reachable
//! GC-managed object with [`Flag::Frozen`].
//!
//! Intended use: call once at the end of bootstrap on `_G` / `__secureenv`
//! so the mark phase can short-circuit over the stable-root trees. The
//! walk does not allocate arenas, mutate values, or trigger GC.
//!
//! Frozen entries remain frozen until explicitly cleared or freed. The
//! flag survives `sweep` only when the entry itself survives — once a
//! slot is reclaimed, [`Arena::free`] and both sweep paths clear the
//! flag byte so a reused slot starts clean.
//!
//! The walk visits: tables (including metatable + array + hash entries),
//! closures (both Lua and Rust, with their upvalues and env), upvalues
//! (closed upvalue values), and userdata (metatable + env). Strings,
//! threads, and light userdata are skipped.

use super::arena::Flag;
use crate::vm::closure::{Closure, UpvalueState};
use crate::vm::state::Gc;
use crate::vm::value::Val;

use super::arena::GcRef;
use crate::vm::closure::Upvalue;
use crate::vm::table::Table;
use crate::vm::value::Userdata;

/// Counters returned by [`Gc::freeze_table`] for observability.
///
/// Tracks how many live objects of each kind were newly pinned. Revisits
/// of already-frozen entries are not counted — they short-circuit during
/// the BFS.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct FreezeStats {
    pub tables: u32,
    pub closures: u32,
    pub upvalues: u32,
    pub userdata: u32,
}

impl FreezeStats {
    /// Total number of live objects pinned across all arenas.
    pub fn total(&self) -> u32 {
        self.tables + self.closures + self.upvalues + self.userdata
    }
}

/// Work item for the freeze BFS.
enum FreezeTarget {
    Table(GcRef<Table>),
    Closure(GcRef<Closure>),
    Upvalue(GcRef<Upvalue>),
    Userdata(GcRef<Userdata>),
}

impl Gc {
    /// Freezes every GC-managed object reachable from `root`.
    ///
    /// BFS walk that sets [`Flag::Frozen`] on tables, closures, upvalues,
    /// and userdata. Already-frozen entries are skipped (so cycles are
    /// safe and repeat calls are idempotent). Strings, threads, and
    /// light userdata are not pinned.
    ///
    /// Intended to be called once after bootstrap completes.
    pub fn freeze_table(&mut self, root: GcRef<Table>) -> FreezeStats {
        let mut stats = FreezeStats::default();
        let mut queue: Vec<FreezeTarget> = vec![FreezeTarget::Table(root)];

        while let Some(target) = queue.pop() {
            match target {
                FreezeTarget::Table(r) => self.freeze_table_entry(r, &mut queue, &mut stats),
                FreezeTarget::Closure(r) => self.freeze_closure_entry(r, &mut queue, &mut stats),
                FreezeTarget::Upvalue(r) => self.freeze_upvalue_entry(r, &mut queue, &mut stats),
                FreezeTarget::Userdata(r) => self.freeze_userdata_entry(r, &mut queue, &mut stats),
            }
        }

        stats
    }

    fn freeze_table_entry(
        &mut self,
        r: GcRef<Table>,
        queue: &mut Vec<FreezeTarget>,
        stats: &mut FreezeStats,
    ) {
        if self.tables.is_frozen(r) {
            return;
        }
        if !self.tables.set_flag(r, Flag::Frozen) {
            return;
        }
        stats.tables += 1;

        let (metatable, children) = {
            let Some(table) = self.tables.get(r) else {
                return;
            };
            let metatable = table.metatable();
            let mut children: Vec<Val> = Vec::new();
            for i in 0..table.array_len() {
                if let Some(val) = table.array_get(i) {
                    children.push(val);
                }
            }
            for i in 0..table.hash_node_count() {
                let Some((key, val)) = table.hash_node_kv(i) else {
                    continue;
                };
                children.push(key);
                children.push(val);
            }
            (metatable, children)
        };

        if let Some(mt) = metatable {
            queue.push(FreezeTarget::Table(mt));
        }
        for val in children {
            enqueue_val(val, queue);
        }
    }

    fn freeze_closure_entry(
        &mut self,
        r: GcRef<Closure>,
        queue: &mut Vec<FreezeTarget>,
        stats: &mut FreezeStats,
    ) {
        if self.closures.is_frozen(r) {
            return;
        }
        if !self.closures.set_flag(r, Flag::Frozen) {
            return;
        }
        stats.closures += 1;

        let Some(closure) = self.closures.get(r) else {
            return;
        };
        match closure {
            Closure::Lua(cl) => {
                let env = cl.env;
                let upvalues: Vec<GcRef<Upvalue>> = cl.upvalues.clone();
                queue.push(FreezeTarget::Table(env));
                for upv in upvalues {
                    queue.push(FreezeTarget::Upvalue(upv));
                }
            }
            Closure::Rust(cl) => {
                let env = cl.env;
                let upvalues: Vec<Val> = cl.upvalues.clone();
                if let Some(env) = env {
                    queue.push(FreezeTarget::Table(env));
                }
                for val in upvalues {
                    enqueue_val(val, queue);
                }
            }
        }
    }

    fn freeze_upvalue_entry(
        &mut self,
        r: GcRef<Upvalue>,
        queue: &mut Vec<FreezeTarget>,
        stats: &mut FreezeStats,
    ) {
        if self.upvalues.is_frozen(r) {
            return;
        }
        if !self.upvalues.set_flag(r, Flag::Frozen) {
            return;
        }
        stats.upvalues += 1;

        // Only closed upvalues carry a GC-visible value. Open upvalues
        // point into a live stack slot; their referent is the caller's
        // responsibility to keep alive, not ours to pin.
        let Some(upv) = self.upvalues.get(r) else {
            return;
        };
        if let UpvalueState::Closed { value } = upv.state {
            enqueue_val(value, queue);
        }
    }

    fn freeze_userdata_entry(
        &mut self,
        r: GcRef<Userdata>,
        queue: &mut Vec<FreezeTarget>,
        stats: &mut FreezeStats,
    ) {
        if self.userdata.is_frozen(r) {
            return;
        }
        if !self.userdata.set_flag(r, Flag::Frozen) {
            return;
        }
        stats.userdata += 1;

        let Some(ud) = self.userdata.get(r) else {
            return;
        };
        let metatable = ud.metatable();
        let env = ud.env();
        if let Some(mt) = metatable {
            queue.push(FreezeTarget::Table(mt));
        }
        if let Some(env) = env {
            queue.push(FreezeTarget::Table(env));
        }
    }
}

fn enqueue_val(val: Val, queue: &mut Vec<FreezeTarget>) {
    match val {
        Val::Table(r) => queue.push(FreezeTarget::Table(r)),
        Val::Function(r) => queue.push(FreezeTarget::Closure(r)),
        Val::Userdata(r) => queue.push(FreezeTarget::Userdata(r)),
        // Strings are shared/interned and have no reachable children;
        // threads intentionally remain collectable; nil/bool/number/
        // light userdata carry no GC refs.
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vm::gc::Color;
    use crate::vm::state::LuaState;
    use crate::vm::value::UserDataBox;

    #[test]
    fn freezes_single_table() {
        let mut lua = LuaState::new();
        let t = lua.gc.tables.alloc(Table::new(), Color::White0);

        let stats = lua.gc.freeze_table(t);

        assert!(lua.gc.tables.is_frozen(t));
        assert_eq!(stats.tables, 1);
        assert_eq!(stats.total(), 1);
    }

    fn set_table_entry(
        lua: &mut LuaState,
        table_ref: GcRef<Table>,
        key: Val,
        value: Val,
    ) {
        lua.gc
            .tables
            .get_mut(table_ref)
            .expect("table gone")
            .raw_set(key, value, &lua.gc.string_arena)
            .expect("raw_set failed");
    }

    #[test]
    fn freezes_nested_tables() {
        let mut lua = LuaState::new();
        let outer = lua.gc.tables.alloc(Table::new(), Color::White0);
        let inner = lua.gc.tables.alloc(Table::new(), Color::White0);
        let key = lua.gc.intern_string(b"child");
        set_table_entry(&mut lua, outer, Val::Str(key), Val::Table(inner));

        let stats = lua.gc.freeze_table(outer);

        assert!(lua.gc.tables.is_frozen(outer));
        assert!(lua.gc.tables.is_frozen(inner));
        assert_eq!(stats.tables, 2);
    }

    #[test]
    fn freeze_is_idempotent() {
        let mut lua = LuaState::new();
        let t = lua.gc.tables.alloc(Table::new(), Color::White0);

        let first = lua.gc.freeze_table(t);
        let second = lua.gc.freeze_table(t);

        assert_eq!(first.tables, 1);
        // Already frozen on the second call — nothing new to count.
        assert_eq!(second.tables, 0);
        assert!(lua.gc.tables.is_frozen(t));
    }

    #[test]
    fn freeze_breaks_cycles() {
        let mut lua = LuaState::new();
        let a = lua.gc.tables.alloc(Table::new(), Color::White0);
        let b = lua.gc.tables.alloc(Table::new(), Color::White0);
        let a_to_b = lua.gc.intern_string(b"b");
        let b_to_a = lua.gc.intern_string(b"a");
        set_table_entry(&mut lua, a, Val::Str(a_to_b), Val::Table(b));
        set_table_entry(&mut lua, b, Val::Str(b_to_a), Val::Table(a));

        let stats = lua.gc.freeze_table(a);

        assert!(lua.gc.tables.is_frozen(a));
        assert!(lua.gc.tables.is_frozen(b));
        assert_eq!(stats.tables, 2);
    }

    #[test]
    fn freeze_follows_metatable() {
        let mut lua = LuaState::new();
        let base = lua.gc.tables.alloc(Table::new(), Color::White0);
        let mt = lua.gc.tables.alloc(Table::new(), Color::White0);
        if let Some(t) = lua.gc.tables.get_mut(base) {
            t.set_metatable(Some(mt));
        }

        let stats = lua.gc.freeze_table(base);

        assert!(lua.gc.tables.is_frozen(base));
        assert!(lua.gc.tables.is_frozen(mt));
        assert_eq!(stats.tables, 2);
    }

    #[test]
    fn freeze_pins_userdata_and_metatable() {
        let mut lua = LuaState::new();
        let root = lua.gc.tables.alloc(Table::new(), Color::White0);
        let ud_mt = lua.gc.tables.alloc(Table::new(), Color::White0);
        let ud = lua.gc.userdata.alloc(
            Userdata::with_metatable(Box::new(()) as UserDataBox, ud_mt),
            Color::White0,
        );
        let key = lua.gc.intern_string(b"handle");
        set_table_entry(&mut lua, root, Val::Str(key), Val::Userdata(ud));

        let stats = lua.gc.freeze_table(root);

        assert!(lua.gc.userdata.is_frozen(ud));
        assert!(lua.gc.tables.is_frozen(ud_mt));
        assert_eq!(stats.userdata, 1);
        assert_eq!(stats.tables, 2); // root + ud_mt
    }

    #[test]
    fn freeze_does_not_pin_strings() {
        let mut lua = LuaState::new();
        let root = lua.gc.tables.alloc(Table::new(), Color::White0);
        let key = lua.gc.intern_string(b"name");
        let value = lua.gc.intern_string(b"Alessio");
        set_table_entry(&mut lua, root, Val::Str(key), Val::Str(value));

        lua.gc.freeze_table(root);

        // Strings are intentionally not frozen — they are shared via
        // the intern table and do not have reachable children.
        assert!(!lua.gc.string_arena.is_frozen(value));
    }

    #[test]
    fn freeze_returns_zero_for_stale_ref() {
        let mut lua = LuaState::new();
        let t = lua.gc.tables.alloc(Table::new(), Color::White0);
        lua.gc.tables.free(t);

        let stats = lua.gc.freeze_table(t);
        assert_eq!(stats.total(), 0);
    }

    #[test]
    fn frozen_root_survives_full_gc_without_children_walk() {
        // Freeze a root whose only reference to its child tree is
        // through the root itself. If full_gc honoured Frozen correctly
        // the child must survive (transitively pinned by the freeze
        // walk); if sweep and mark did NOT honour Frozen, the whole
        // tree would be freed because nothing else reaches the root.
        let mut lua = LuaState::new();
        let root = lua.gc.tables.alloc(Table::new(), lua.gc.current_white);
        let child = lua.gc.tables.alloc(Table::new(), lua.gc.current_white);
        let key = lua.gc.intern_string(b"child");
        lua.gc
            .tables
            .get_mut(root)
            .unwrap()
            .raw_set(Val::Str(key), Val::Table(child), &lua.gc.string_arena)
            .unwrap();

        let stats = lua.gc.freeze_table(root);
        assert_eq!(stats.tables, 2);

        lua.full_gc().expect("full gc");

        assert!(lua.gc.tables.is_valid(root));
        assert!(lua.gc.tables.is_valid(child));
        assert!(lua.gc.tables.is_frozen(root));
        assert!(lua.gc.tables.is_frozen(child));
    }

    #[test]
    fn mark_value_short_circuits_frozen_table() {
        use crate::vm::gc::arena::Flag;
        let mut lua = LuaState::new();
        let t = lua.gc.tables.alloc(Table::new(), lua.gc.current_white);
        assert!(lua.gc.tables.set_flag(t, Flag::Frozen));

        let gray_before = lua.gc.gc_state.gray.len();
        lua.gc.mark_value(Val::Table(t));
        let gray_after = lua.gc.gc_state.gray.len();

        // Frozen entry is neither pushed onto the gray list nor coloured
        // gray — mark_value short-circuits immediately.
        assert_eq!(gray_after, gray_before);
    }
}
