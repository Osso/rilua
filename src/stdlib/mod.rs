//! Standard library: modular implementation of Lua 5.1.1's built-in libraries.

pub mod base;
pub mod coroutine;
pub mod debug;
pub mod io;
pub mod math;
pub mod os;
pub mod package;
pub mod string;
pub mod table;

use crate::error::LuaResult;
use crate::vm::closure::{Closure, RustClosure, RustFn};
use crate::vm::gc::arena::GcRef;
use crate::vm::state::LuaState;
use crate::vm::table::Table;
use crate::vm::value::Val;

/// Registers all standard library functions into the global table.
pub fn open_libs(state: &mut LuaState) -> LuaResult<()> {
    // Base library functions.
    register_global_fn(state, "print", base::lua_print)?;
    register_global_fn(state, "type", base::lua_type)?;
    register_global_fn(state, "tostring", base::lua_tostring)?;
    register_global_fn(state, "tonumber", base::lua_tonumber)?;
    register_global_fn(state, "assert", base::lua_assert)?;
    register_global_fn(state, "error", base::lua_error)?;
    register_global_fn(state, "pcall", base::lua_pcall)?;
    register_global_fn(state, "xpcall", base::lua_xpcall)?;
    register_global_fn(state, "setmetatable", base::lua_setmetatable)?;
    register_global_fn(state, "getmetatable", base::lua_getmetatable)?;
    register_global_fn(state, "rawget", base::lua_rawget)?;
    register_global_fn(state, "rawset", base::lua_rawset)?;
    register_global_fn(state, "rawequal", base::lua_rawequal)?;
    register_global_fn(state, "select", base::lua_select)?;
    register_global_fn(state, "unpack", base::lua_unpack)?;
    register_global_fn(state, "next", base::lua_next)?;
    register_global_fn(state, "pairs", base::lua_pairs)?;
    register_global_fn(state, "ipairs", base::lua_ipairs)?;
    register_global_fn(state, "loadstring", base::lua_loadstring)?;
    register_global_fn(state, "loadfile", base::lua_loadfile)?;
    register_global_fn(state, "dofile", base::lua_dofile)?;
    register_global_fn(state, "load", base::lua_load)?;
    register_global_fn(state, "collectgarbage", base::lua_collectgarbage)?;
    register_global_fn(state, "setfenv", base::lua_setfenv)?;
    register_global_fn(state, "getfenv", base::lua_getfenv)?;
    register_global_fn(state, "newproxy", base::lua_newproxy)?;
    register_global_fn(state, "gcinfo", base::lua_gcinfo)?;

    // Global values: _G (self-referential) and _VERSION.
    register_global_val(state, "_G", Val::Table(state.global))?;
    let version_str = state.gc.intern_string(b"Lua 5.1");
    register_global_val(state, "_VERSION", Val::Str(version_str))?;

    // String library.
    open_string_lib(state)?;

    // Table library.
    open_table_lib(state)?;

    // Math library.
    open_math_lib(state)?;

    // OS library.
    open_os_lib(state)?;

    Ok(())
}

/// Registers the table library as `table` global.
///
/// Follows PUC-Rio's `luaopen_table` pattern from `ltablib.c`.
fn open_table_lib(state: &mut LuaState) -> LuaResult<()> {
    let table_table = state.gc.alloc_table(Table::new());
    register_table_fn(state, table_table, "concat", table::tab_concat)?;
    register_table_fn(state, table_table, "foreach", table::tab_foreach)?;
    register_table_fn(state, table_table, "foreachi", table::tab_foreachi)?;
    register_table_fn(state, table_table, "getn", table::tab_getn)?;
    register_table_fn(state, table_table, "insert", table::tab_insert)?;
    register_table_fn(state, table_table, "maxn", table::tab_maxn)?;
    register_table_fn(state, table_table, "remove", table::tab_remove)?;
    register_table_fn(state, table_table, "setn", table::tab_setn)?;
    register_table_fn(state, table_table, "sort", table::tab_sort)?;
    register_global_val(state, "table", Val::Table(table_table))?;
    Ok(())
}

/// Registers the math library as `math` global.
///
/// Follows PUC-Rio's `luaopen_math` pattern from `lmathlib.c`:
/// 28 functions + `math.pi` + `math.huge` + `math.mod` alias.
fn open_math_lib(state: &mut LuaState) -> LuaResult<()> {
    let math_table = state.gc.alloc_table(Table::new());

    register_table_fn(state, math_table, "abs", math::math_abs)?;
    register_table_fn(state, math_table, "acos", math::math_acos)?;
    register_table_fn(state, math_table, "asin", math::math_asin)?;
    register_table_fn(state, math_table, "atan", math::math_atan)?;
    register_table_fn(state, math_table, "atan2", math::math_atan2)?;
    register_table_fn(state, math_table, "ceil", math::math_ceil)?;
    register_table_fn(state, math_table, "cos", math::math_cos)?;
    register_table_fn(state, math_table, "cosh", math::math_cosh)?;
    register_table_fn(state, math_table, "deg", math::math_deg)?;
    register_table_fn(state, math_table, "exp", math::math_exp)?;
    register_table_fn(state, math_table, "floor", math::math_floor)?;
    register_table_fn(state, math_table, "fmod", math::math_fmod)?;
    register_table_fn(state, math_table, "frexp", math::math_frexp)?;
    register_table_fn(state, math_table, "ldexp", math::math_ldexp)?;
    register_table_fn(state, math_table, "log", math::math_log)?;
    register_table_fn(state, math_table, "log10", math::math_log10)?;
    register_table_fn(state, math_table, "max", math::math_max)?;
    register_table_fn(state, math_table, "min", math::math_min)?;
    register_table_fn(state, math_table, "modf", math::math_modf)?;
    register_table_fn(state, math_table, "pow", math::math_pow)?;
    register_table_fn(state, math_table, "rad", math::math_rad)?;
    register_table_fn(state, math_table, "random", math::math_random)?;
    register_table_fn(state, math_table, "randomseed", math::math_randomseed)?;
    register_table_fn(state, math_table, "sin", math::math_sin)?;
    register_table_fn(state, math_table, "sinh", math::math_sinh)?;
    register_table_fn(state, math_table, "sqrt", math::math_sqrt)?;
    register_table_fn(state, math_table, "tan", math::math_tan)?;
    register_table_fn(state, math_table, "tanh", math::math_tanh)?;

    // Deprecated alias: mod = fmod (LUA_COMPAT_MOD, enabled by default).
    register_table_fn(state, math_table, "mod", math::math_fmod)?;

    // Constants: math.pi and math.huge.
    let pi_key = state.gc.intern_string(b"pi");
    let huge_key = state.gc.intern_string(b"huge");
    let mt = state.gc.tables.get_mut(math_table).ok_or_else(|| {
        crate::error::LuaError::Runtime(crate::error::RuntimeError {
            message: "math table not found".into(),
            level: 0,
            traceback: vec![],
        })
    })?;
    mt.raw_set(
        Val::Str(pi_key),
        Val::Num(std::f64::consts::PI),
        &state.gc.string_arena,
    )?;
    mt.raw_set(
        Val::Str(huge_key),
        Val::Num(f64::INFINITY),
        &state.gc.string_arena,
    )?;

    register_global_val(state, "math", Val::Table(math_table))?;
    Ok(())
}

/// Registers the OS library as `os` global.
///
/// Follows PUC-Rio's `luaopen_os` pattern from `loslib.c`:
/// 11 functions.
fn open_os_lib(state: &mut LuaState) -> LuaResult<()> {
    let os_table = state.gc.alloc_table(Table::new());

    register_table_fn(state, os_table, "clock", os::os_clock)?;
    register_table_fn(state, os_table, "date", os::os_date)?;
    register_table_fn(state, os_table, "difftime", os::os_difftime)?;
    register_table_fn(state, os_table, "execute", os::os_execute)?;
    register_table_fn(state, os_table, "exit", os::os_exit)?;
    register_table_fn(state, os_table, "getenv", os::os_getenv)?;
    register_table_fn(state, os_table, "remove", os::os_remove)?;
    register_table_fn(state, os_table, "rename", os::os_rename)?;
    register_table_fn(state, os_table, "setlocale", os::os_setlocale)?;
    register_table_fn(state, os_table, "time", os::os_time)?;
    register_table_fn(state, os_table, "tmpname", os::os_tmpname)?;

    register_global_val(state, "os", Val::Table(os_table))?;
    Ok(())
}

/// Registers the string library as `string` global and sets up the string
/// type metatable so that `("hello"):upper()` method syntax works.
///
/// Follows PUC-Rio's `luaopen_string` + `createmetatable` pattern from
/// `lstrlib.c`.
fn open_string_lib(state: &mut LuaState) -> LuaResult<()> {
    // Create the string library table and populate it with functions.
    let string_table = state.gc.alloc_table(Table::new());
    register_table_fn(state, string_table, "len", string::str_len)?;
    register_table_fn(state, string_table, "byte", string::str_byte)?;
    register_table_fn(state, string_table, "char", string::str_char)?;
    register_table_fn(state, string_table, "sub", string::str_sub)?;
    register_table_fn(state, string_table, "rep", string::str_rep)?;
    register_table_fn(state, string_table, "reverse", string::str_reverse)?;
    register_table_fn(state, string_table, "lower", string::str_lower)?;
    register_table_fn(state, string_table, "upper", string::str_upper)?;
    register_table_fn(state, string_table, "format", string::str_format)?;
    register_table_fn(state, string_table, "find", string::str_find)?;
    register_table_fn(state, string_table, "match", string::str_match)?;
    register_table_fn(state, string_table, "gmatch", string::str_gmatch)?;
    register_table_fn(state, string_table, "gsub", string::str_gsub)?;
    register_table_fn(state, string_table, "dump", string::str_dump)?;
    // Deprecated alias: gfind = gmatch (LUA_COMPAT_GFIND, enabled by default).
    register_table_fn(state, string_table, "gfind", string::str_gmatch)?;

    // Register as global "string".
    register_global_val(state, "string", Val::Table(string_table))?;

    // Create a metatable for the string type: { __index = string_table }.
    // This enables method syntax: ("hello"):upper() resolves via __index.
    let mt = state.gc.alloc_table(Table::new());
    let index_key = state.gc.intern_string(b"__index");
    let mt_table = state.gc.tables.get_mut(mt).ok_or_else(|| {
        crate::error::LuaError::Runtime(crate::error::RuntimeError {
            message: "string metatable not found".into(),
            level: 0,
            traceback: vec![],
        })
    })?;
    mt_table.raw_set(
        Val::Str(index_key),
        Val::Table(string_table),
        &state.gc.string_arena,
    )?;

    // Set the string type metatable (type tag 3 = String).
    state.gc.type_metatables[3] = Some(mt);

    Ok(())
}

/// Registers an arbitrary value in the global table.
fn register_global_val(state: &mut LuaState, name: &str, val: Val) -> LuaResult<()> {
    let key_ref = state.gc.intern_string(name.as_bytes());
    let key = Val::Str(key_ref);
    let global = state.global;
    let table = state.gc.tables.get_mut(global).ok_or_else(|| {
        crate::error::LuaError::Runtime(crate::error::RuntimeError {
            message: "global table not found".into(),
            level: 0,
            traceback: vec![],
        })
    })?;
    table.raw_set(key, val, &state.gc.string_arena)?;
    Ok(())
}

/// Creates a `RustClosure`, interns the name string, and sets it
/// in the global table.
fn register_global_fn(state: &mut LuaState, name: &str, func: RustFn) -> LuaResult<()> {
    let global = state.global;
    register_table_fn(state, global, name, func)
}

/// Creates a `RustClosure` and sets it in an arbitrary table.
fn register_table_fn(
    state: &mut LuaState,
    table_ref: GcRef<Table>,
    name: &str,
    func: RustFn,
) -> LuaResult<()> {
    let closure = Closure::Rust(RustClosure::new(func, name));
    let closure_ref = state.gc.alloc_closure(closure);

    let key_ref = state.gc.intern_string(name.as_bytes());
    let key = Val::Str(key_ref);
    let val = Val::Function(closure_ref);

    let table = state.gc.tables.get_mut(table_ref).ok_or_else(|| {
        crate::error::LuaError::Runtime(crate::error::RuntimeError {
            message: "table not found for function registration".into(),
            level: 0,
            traceback: vec![],
        })
    })?;
    table.raw_set(key, val, &state.gc.string_arena)?;

    Ok(())
}
