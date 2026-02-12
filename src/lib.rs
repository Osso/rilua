//! rilua — Lua 5.1.1 implemented in Rust.
//!
//! A from-scratch implementation targeting behavioral equivalence with
//! the PUC-Rio reference interpreter. Designed for embedding in Rust
//! applications, with a focus on the World of Warcraft addon variant.
//!
//! # Architecture
//!
//! Pipeline: Source -> Lexer -> Parser -> AST -> Compiler -> Proto -> VM
//!
//! See `docs/architecture.md` for design documentation.
//!
//! # Usage
//!
//! ```ignore
//! use rilua::{Lua, StdLib};
//!
//! let mut lua = Lua::new()?;
//! lua.exec("print(1 + 2)")?;
//!
//! lua.set_global("x", 42.0)?;
//! let x: f64 = lua.global("x")?;
//! ```

pub mod compiler;
pub mod conversion;
pub mod error;
pub mod handles;
pub mod stdlib;
pub mod vm;

use std::rc::Rc;

// Re-exports for public API.
pub use conversion::{FromLua, FromLuaMulti, IntoLua, IntoLuaMulti};
pub use error::{LuaError, LuaResult};
pub use handles::{Function, Table, Thread};
pub use stdlib::StdLib;
pub use vm::closure::RustFn;
pub use vm::state::ThreadStatus;
pub use vm::value::Val;

use error::RuntimeError;
use vm::callinfo::LUA_MULTRET;
use vm::closure::{Closure, LuaClosure, RustClosure};
use vm::execute::{CallResult, execute};
use vm::proto::Proto;
use vm::state::{Gc, LuaState};

// ---------------------------------------------------------------------------
// Lua struct: high-level embedding API
// ---------------------------------------------------------------------------

/// A Lua interpreter instance.
///
/// Owns the full VM state including value stack, call stack, GC heap,
/// and global table. All interaction with Lua values goes through this
/// struct.
pub struct Lua {
    state: LuaState,
}

impl Lua {
    /// Creates a new Lua state with all standard libraries loaded.
    pub fn new() -> LuaResult<Self> {
        let mut lua = Self {
            state: LuaState::new(),
        };
        stdlib::open_libs(&mut lua.state)?;
        Ok(lua)
    }

    /// Creates a new Lua state with no libraries loaded.
    ///
    /// The state has a global table and registry but no standard
    /// functions (`print`, `type`, etc.) are available.
    pub fn new_empty() -> Self {
        Self {
            state: LuaState::new(),
        }
    }

    /// Creates a new Lua state with selected standard libraries.
    ///
    /// ```ignore
    /// let lua = Lua::new_with(StdLib::BASE | StdLib::STRING)?;
    /// ```
    pub fn new_with(libs: StdLib) -> LuaResult<Self> {
        let mut lua = Self {
            state: LuaState::new(),
        };
        stdlib::open_libs_selective(&mut lua.state, libs)?;
        Ok(lua)
    }

    // -----------------------------------------------------------------------
    // Execution
    // -----------------------------------------------------------------------

    /// Compiles and executes a Lua source string.
    ///
    /// The chunk name is set to `"=(string)"`.
    pub fn exec(&mut self, source: &str) -> LuaResult<()> {
        self.exec_bytes(source.as_bytes(), "=(string)")
    }

    /// Compiles and executes Lua source bytes with a given chunk name.
    ///
    /// Source is `&[u8]` because Lua files may contain arbitrary byte
    /// sequences (e.g. `\0`, `\255` in string literals).
    pub fn exec_bytes(&mut self, source: &[u8], name: &str) -> LuaResult<()> {
        let proto = compiler::compile(source, name)?;
        self.run_proto(proto)
    }

    /// Reads a file and executes its contents as a Lua chunk.
    ///
    /// The chunk name is set to `@<path>` following PUC-Rio convention.
    pub fn exec_file(&mut self, path: &str) -> LuaResult<()> {
        let source = std::fs::read(path).map_err(|e| {
            LuaError::Runtime(RuntimeError {
                message: format!("cannot open {path}: {e}"),
                level: 0,
                traceback: vec![],
            })
        })?;
        let name = format!("@{path}");
        self.exec_bytes(&source, &name)
    }

    /// Compiles a Lua source string and returns a function handle.
    ///
    /// The returned `Function` can be stored and called later. The
    /// chunk name is set to `"=(string)"`.
    pub fn load(&mut self, source: &str) -> LuaResult<Function> {
        self.load_bytes(source.as_bytes(), "=(string)")
    }

    /// Compiles Lua source bytes and returns a function handle.
    pub fn load_bytes(&mut self, source: &[u8], name: &str) -> LuaResult<Function> {
        let proto = compiler::compile(source, name)?;
        let mut proto = Rc::try_unwrap(proto).unwrap_or_else(|rc| (*rc).clone());
        patch_string_constants(&mut proto, &mut self.state.gc);
        let proto = Rc::new(proto);

        let lua_cl = LuaClosure::new(proto, self.state.global);
        let closure_ref = self.state.gc.alloc_closure(Closure::Lua(lua_cl));
        Ok(Function(closure_ref))
    }

    // -----------------------------------------------------------------------
    // Globals
    // -----------------------------------------------------------------------

    /// Gets a global variable, converting it to the requested Rust type.
    ///
    /// Takes `&mut self` because looking up a string key may need to
    /// intern the name (idempotent if it already exists).
    pub fn global<V: FromLua>(&mut self, name: &str) -> LuaResult<V> {
        let val = self.get_global_val(name);
        V::from_lua(val, self)
    }

    /// Sets a global variable from a Rust value.
    pub fn set_global<V: IntoLua>(&mut self, name: &str, value: V) -> LuaResult<()> {
        let val = value.into_lua(self)?;
        self.set_global_val(name, val)
    }

    // -----------------------------------------------------------------------
    // Table creation
    // -----------------------------------------------------------------------

    /// Allocates a new empty table and returns a handle.
    pub fn create_table(&mut self) -> Table {
        let r = self.state.gc.alloc_table(vm::table::Table::new());
        Table(r)
    }

    // -----------------------------------------------------------------------
    // Function registration
    // -----------------------------------------------------------------------

    /// Registers a Rust function as a global Lua function.
    pub fn register_function(&mut self, name: &str, func: RustFn) -> LuaResult<()> {
        let closure = Closure::Rust(RustClosure::new(func, name));
        let closure_ref = self.state.gc.alloc_closure(closure);
        self.set_global_val(name, Val::Function(closure_ref))
    }

    // -----------------------------------------------------------------------
    // GC control
    // -----------------------------------------------------------------------

    /// Runs a full garbage collection cycle.
    pub fn gc_collect(&mut self) -> LuaResult<()> {
        self.state.full_gc()
    }

    /// Returns the total memory in use by Lua (in bytes).
    pub fn gc_count(&self) -> usize {
        self.state.gc.gc_state.total_bytes
    }

    /// Stops the garbage collector.
    pub fn gc_stop(&mut self) {
        self.state.gc.gc_state.gc_enabled = false;
    }

    /// Restarts the garbage collector.
    pub fn gc_restart(&mut self) {
        self.state.gc.gc_state.gc_enabled = true;
    }

    /// Performs an incremental GC step. Returns true if the step
    /// finished a collection cycle.
    pub fn gc_step(&mut self, step_size: i64) -> LuaResult<bool> {
        self.state.gc_step(step_size)
    }

    /// Sets the GC pause parameter (percentage). Returns the previous value.
    pub fn gc_set_pause(&mut self, pause: u32) -> u32 {
        let old = self.state.gc.gc_state.gc_pause;
        self.state.gc.gc_state.gc_pause = pause;
        old
    }

    /// Sets the GC step multiplier. Returns the previous value.
    pub fn gc_set_step_multiplier(&mut self, stepmul: u32) -> u32 {
        let old = self.state.gc.gc_state.gc_stepmul;
        self.state.gc.gc_state.gc_stepmul = stepmul;
        old
    }

    // -----------------------------------------------------------------------
    // Internal accessors (pub(crate) for stdlib, conversion, handles)
    // -----------------------------------------------------------------------

    /// Immutable access to the underlying `LuaState`.
    pub(crate) fn state(&self) -> &LuaState {
        &self.state
    }

    /// Mutable access to the underlying `LuaState`.
    pub(crate) fn state_mut(&mut self) -> &mut LuaState {
        &mut self.state
    }

    // -----------------------------------------------------------------------
    // Private helpers
    // -----------------------------------------------------------------------

    /// Patches string constants, creates a main closure, and executes it.
    fn run_proto(&mut self, proto: Rc<Proto>) -> LuaResult<()> {
        let mut proto = Rc::try_unwrap(proto).unwrap_or_else(|rc| (*rc).clone());
        patch_string_constants(&mut proto, &mut self.state.gc);
        let proto = Rc::new(proto);

        let lua_cl = LuaClosure::new(proto, self.state.global);
        let closure_ref = self.state.gc.alloc_closure(Closure::Lua(lua_cl));

        self.state.stack_set(0, Val::Function(closure_ref));
        self.state.top = 1;
        self.state.base = 1;

        match self.state.precall(0, LUA_MULTRET)? {
            CallResult::Lua => execute(&mut self.state)?,
            CallResult::Rust => {}
        }

        Ok(())
    }

    /// Reads a value from the global table by name.
    ///
    /// Uses `intern_string` (idempotent) to get the key reference, then
    /// does a direct table lookup.
    fn get_global_val(&mut self, name: &str) -> Val {
        let key_ref = self.state.gc.intern_string(name.as_bytes());
        let Some(global_table) = self.state.gc.tables.get(self.state.global) else {
            return Val::Nil;
        };
        global_table.get_str(key_ref, &self.state.gc.string_arena)
    }

    /// Sets a value in the global table by name.
    fn set_global_val(&mut self, name: &str, val: Val) -> LuaResult<()> {
        let key_ref = self.state.gc.intern_string(name.as_bytes());
        let key = Val::Str(key_ref);
        let global = self.state.global;
        let table = self.state.gc.tables.get_mut(global).ok_or_else(|| {
            LuaError::Runtime(RuntimeError {
                message: "global table not found".into(),
                level: 0,
                traceback: vec![],
            })
        })?;
        table.raw_set(key, val, &self.state.gc.string_arena)
    }
}

// ---------------------------------------------------------------------------
// Backward-compatible free functions
// ---------------------------------------------------------------------------

/// Executes Lua source bytes as `"=(string)"`.
///
/// Compiles the source, registers the standard library, and runs the
/// resulting chunk. Equivalent to `exec_with_name(source, "=(string)")`.
pub fn exec(source: &[u8]) -> LuaResult<()> {
    exec_with_name(source, "=(string)")
}

/// Executes Lua source bytes with the given chunk name.
///
/// Source is accepted as `&[u8]` because Lua files may contain arbitrary
/// byte sequences (e.g. `\0`, `\255` in string literals).
///
/// Pipeline: compile -> patch string constants -> create state ->
/// register stdlib -> create main closure -> precall -> execute.
pub fn exec_with_name(source: &[u8], name: &str) -> LuaResult<()> {
    let mut lua = Lua::new()?;
    lua.exec_bytes(source, name)
}

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

/// Resolves string constant placeholders in a Proto using the GC.
///
/// During compilation, string constants are stored as `Val::Nil` with
/// their raw bytes recorded in `proto.string_pool`. This function
/// interns each string via the GC and replaces the placeholder with
/// the real `Val::Str` value. Recurses into child protos.
pub(crate) fn patch_string_constants(proto: &mut Proto, gc: &mut Gc) {
    for (idx, bytes) in proto.string_pool.drain(..) {
        let str_ref = gc.intern_string(&bytes);
        proto.constants[idx as usize] = Val::Str(str_ref);
    }
    for child in &mut proto.protos {
        patch_string_constants(Rc::make_mut(child), gc);
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lua_new_creates_working_state() {
        let lua = Lua::new();
        assert!(lua.is_ok());
    }

    #[test]
    fn lua_new_empty_has_no_libs() {
        let mut lua = Lua::new_empty();
        // `print` should not be defined.
        let val: LuaResult<Val> = lua.global("print");
        assert!(val.is_ok());
        assert_eq!(val.ok(), Some(Val::Nil));
    }

    #[test]
    fn lua_new_with_selective() {
        let lua = Lua::new_with(StdLib::BASE | StdLib::STRING);
        assert!(lua.is_ok());
    }

    #[test]
    fn lua_exec_string() {
        let mut lua = Lua::new().ok().unwrap_or_else(Lua::new_empty);
        let result = lua.exec("local x = 1 + 2");
        assert!(result.is_ok());
    }

    #[test]
    fn lua_exec_syntax_error() {
        let mut lua = Lua::new().ok().unwrap_or_else(Lua::new_empty);
        let result = lua.exec("if then end");
        assert!(result.is_err());
    }

    #[test]
    fn lua_set_and_get_global_f64() {
        let mut lua = Lua::new_empty();
        lua.set_global("x", 42.0f64).ok();
        let val: LuaResult<f64> = lua.global("x");
        assert_eq!(val.ok(), Some(42.0));
    }

    #[test]
    fn lua_set_and_get_global_string() {
        let mut lua = Lua::new_empty();
        lua.set_global("name", "hello").ok();
        let val: LuaResult<String> = lua.global("name");
        assert_eq!(val.ok(), Some("hello".to_string()));
    }

    #[test]
    fn lua_set_and_get_global_bool() {
        let mut lua = Lua::new_empty();
        lua.set_global("flag", true).ok();
        let val: LuaResult<bool> = lua.global("flag");
        assert_eq!(val.ok(), Some(true));
    }

    #[test]
    fn lua_set_and_get_global_nil() {
        let mut lua = Lua::new_empty();
        lua.set_global("x", 42.0f64).ok();
        lua.set_global::<Option<f64>>("x", None).ok();
        let val: LuaResult<Option<f64>> = lua.global("x");
        assert_eq!(val.ok(), Some(None));
    }

    #[test]
    fn lua_create_table() {
        let mut lua = Lua::new_empty();
        let t = lua.create_table();
        t.raw_set(&mut lua.state, Val::Num(1.0), Val::Num(10.0))
            .ok();
        let v = t.raw_get(&lua.state, Val::Num(1.0));
        assert_eq!(v.ok(), Some(Val::Num(10.0)));
    }

    #[test]
    fn lua_register_function() {
        let mut lua = Lua::new_empty();
        let result = lua.register_function("myfn", |state| {
            state.push(Val::Num(99.0));
            Ok(1)
        });
        assert!(result.is_ok());
        let val: LuaResult<Val> = lua.global("myfn");
        assert!(matches!(val.ok(), Some(Val::Function(_))));
    }

    #[test]
    fn lua_gc_methods() {
        let mut lua = Lua::new_empty();
        let count = lua.gc_count();
        assert!(count > 0);

        lua.gc_stop();
        assert!(!lua.state.gc.gc_state.gc_enabled);

        lua.gc_restart();
        assert!(lua.state.gc.gc_state.gc_enabled);

        let old_pause = lua.gc_set_pause(300);
        assert_eq!(old_pause, 200); // default
        assert_eq!(lua.state.gc.gc_state.gc_pause, 300);

        let old_mul = lua.gc_set_step_multiplier(400);
        assert_eq!(old_mul, 200); // default
        assert_eq!(lua.state.gc.gc_state.gc_stepmul, 400);
    }

    #[test]
    fn lua_gc_collect() {
        let mut lua = Lua::new_empty();
        let result = lua.gc_collect();
        assert!(result.is_ok());
    }

    #[test]
    fn lua_load_and_check() {
        let mut lua = Lua::new_empty();
        let func = lua.load("return 42");
        assert!(func.is_ok());
        assert!(matches!(func.ok(), Some(Function(_))));
    }

    #[test]
    fn backward_compat_exec() {
        let result = exec(b"local x = 1 + 2");
        assert!(result.is_ok());
    }

    #[test]
    fn backward_compat_exec_with_name() {
        let result = exec_with_name(b"local x = 1 + 2", "=test");
        assert!(result.is_ok());
    }
}
