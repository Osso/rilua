//! Pin an object or mark a table's children as independently-pinned.
//!
//! The two APIs are complementary:
//!
//! - [`Gc::pin_object`] sets [`Flag::Pinned`] on the underlying arena
//!   entry so sweep keeps it alive regardless of mark state. Use on
//!   long-lived objects whose reachability through the normal mark
//!   phase is either expensive (deep traversals) or not cleanly
//!   expressible (e.g. objects the host wants to keep without having
//!   to anchor them from a root table).
//!
//! - [`Gc::mark_table_no_traverse`] sets [`Flag::SkipTraverse`] on a
//!   table so the mark phase marks it but does not walk its children.
//!   Use on stable registry tables whose children are themselves
//!   pinned (so the mark phase doesn't need to walk them).
//!
//! Neither API changes the GC color. Callers that want a table to
//! survive sweep without ever being marked should also call
//! `pin_object(Val::Table(r))`.

use super::arena::Flag;
use crate::vm::state::Gc;
use crate::vm::table::Table;
use crate::vm::value::Val;

use super::arena::GcRef;

impl Gc {
    /// Pins a GC-managed value so sweep will not collect it. Silent
    /// no-op for non-collectable variants (nil, bool, number, light
    /// userdata) or stale refs.
    ///
    /// Returns whether the flag was applied (false for non-collectable
    /// values or stale refs).
    pub fn pin_object(&mut self, val: Val) -> bool {
        match val {
            Val::Str(r) => self.string_arena.set_flag(r, Flag::Pinned),
            Val::Table(r) => self.tables.set_flag(r, Flag::Pinned),
            Val::Function(r) => self.closures.set_flag(r, Flag::Pinned),
            Val::Userdata(r) => self.userdata.set_flag(r, Flag::Pinned),
            Val::Thread(r) => self.threads.set_flag(r, Flag::Pinned),
            Val::Nil | Val::Bool(_) | Val::Num(_) | Val::LightUserdata(_) => false,
        }
    }

    /// Marks a table so the mark phase walks the entry itself but
    /// skips its children (metatable, array values, hash entries).
    ///
    /// The table is NOT automatically pinned by this call — if the
    /// table is not reachable from a GC root, it will still be swept.
    /// Pair with [`Gc::pin_object`] on the table when its only
    /// reachability is through the SkipTraverse root itself.
    ///
    /// Returns whether the flag was applied (false for stale refs).
    pub fn mark_table_no_traverse(&mut self, r: GcRef<Table>) -> bool {
        self.tables.set_flag(r, Flag::SkipTraverse)
    }
}

#[cfg(test)]
mod tests {
    use crate::vm::gc::Color;
    use crate::vm::gc::arena::Flag;
    use crate::vm::state::LuaState;
    use crate::vm::table::Table;
    use crate::vm::value::Val;

    #[test]
    fn pin_object_sets_pinned_flag_on_table() {
        let mut lua = LuaState::new();
        let t = lua.gc.tables.alloc(Table::new(), Color::White0);

        assert!(lua.gc.pin_object(Val::Table(t)));
        assert!(lua.gc.tables.is_pinned(t));
    }

    #[test]
    fn pin_object_is_noop_for_non_collectable() {
        let mut lua = LuaState::new();
        assert!(!lua.gc.pin_object(Val::Nil));
        assert!(!lua.gc.pin_object(Val::Bool(true)));
        assert!(!lua.gc.pin_object(Val::Num(1.0)));
        assert!(!lua.gc.pin_object(Val::LightUserdata(0x1234)));
    }

    #[test]
    fn pin_object_returns_false_for_stale_ref() {
        let mut lua = LuaState::new();
        let t = lua.gc.tables.alloc(Table::new(), Color::White0);
        lua.gc.tables.free(t);
        assert!(!lua.gc.pin_object(Val::Table(t)));
    }

    #[test]
    fn mark_table_no_traverse_sets_skip_traverse_flag() {
        let mut lua = LuaState::new();
        let t = lua.gc.tables.alloc(Table::new(), Color::White0);

        assert!(lua.gc.mark_table_no_traverse(t));
        assert!(lua.gc.tables.is_skip_traverse(t));
        assert!(!lua.gc.tables.is_pinned(t));
        // Caller is responsible for pairing with pin_object when needed.
    }

    #[test]
    fn pinned_table_survives_sweep_without_mark() {
        let mut lua = LuaState::new();
        let t = lua.gc.tables.alloc(Table::new(), Color::White0);
        assert!(lua.gc.pin_object(Val::Table(t)));

        // Sweep with White0 as dead and White1 as the new white.
        // Without the pin flag `t` would be freed since it never got
        // marked; with it, the entry survives and resets to White1.
        lua.gc.tables.sweep(Color::White0, Color::White1);
        assert!(lua.gc.tables.is_valid(t));
        assert_eq!(lua.gc.tables.color(t), Some(Color::White1));
        assert!(lua.gc.tables.is_pinned(t));
    }

    #[test]
    fn skip_traverse_and_pinned_together_survive_full_gc_without_marking_children() {
        let mut lua = LuaState::new();

        // Create a stable-root table with a child, pin the root, set
        // SkipTraverse on it. The child is unreferenced elsewhere so a
        // normal mark would not reach it; a normal sweep would free it.
        let root = lua.gc.tables.alloc(Table::new(), lua.gc.current_white);
        let child = lua.gc.tables.alloc(Table::new(), lua.gc.current_white);
        let key = lua.gc.intern_string(b"orphan_child");
        lua.gc
            .tables
            .get_mut(root)
            .unwrap()
            .raw_set(Val::Str(key), Val::Table(child), &lua.gc.string_arena)
            .unwrap();
        assert!(lua.gc.pin_object(Val::Table(root)));
        assert!(lua.gc.mark_table_no_traverse(root));

        lua.full_gc().expect("full gc");

        // Root survived (pinned). Child was freed — the SkipTraverse
        // on root prevented the mark phase from walking into it.
        assert!(lua.gc.tables.is_valid(root));
        assert!(!lua.gc.tables.is_valid(child));
    }
}
