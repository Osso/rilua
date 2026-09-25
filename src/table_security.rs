//! Opt-in table-access policy and opaque secret keys for WoW consumers.
//!
//! This is not a general secret-value VM: wrappers cannot participate in Lua
//! arithmetic. Secret booleans support guarded control-flow and comparisons;
//! secret numbers support guarded ordering. Other wrapped payloads remain opaque.
//! The private payload is GC-traced and never stored in fenv.
//! Access-boundary callers must invoke `check_table_access` before reading or
//! mutating a table. Low-level arena/table operations remain trusted host APIs.

use crate::api::{LuaApiMut, state_is_secure};
use crate::vm::gc::arena::GcRef;
use crate::vm::state::LuaState;
use crate::vm::table::Table;
use crate::vm::value::Userdata;
use crate::{Lua, LuaResult, Val, runtime_error};

const DISALLOW_TAINTED_ACCESS: u8 = 1;
const DISALLOW_SECRET_KEYS: u8 = 2;

/// Register the optional globals. Ordinary Lua states retain their default API.
pub fn register_table_security(lua: &mut Lua) -> LuaResult<()> {
    lua.register_function("settablesecurity", lua_set_table_security)?;
    lua.register_function("secretwrap", lua_secret_wrap)?;
    lua.register_function("secretunwrap", lua_secret_unwrap)?;
    lua.register_function("issecretvalue", lua_is_secret_value)?;
    Ok(())
}

/// Reject forbidden caller/key access. `None` covers keyless operations.
pub fn check_table_access(
    state: &LuaState,
    table: GcRef<Table>,
    key: Option<Val>,
) -> LuaResult<()> {
    let flags = state
        .gc
        .tables
        .get(table)
        .ok_or_else(|| runtime_error("table has been collected"))?
        .security_flags;
    if flags & DISALLOW_TAINTED_ACCESS != 0 && !state_is_secure(state) {
        return Err(runtime_error("tainted access to secured table"));
    }
    if flags & DISALLOW_SECRET_KEYS != 0 && key.is_some_and(|key| is_secret_value(state, key)) {
        return Err(runtime_error("secret key access to secured table"));
    }
    Ok(())
}

/// Add a restriction; options accumulate because one table can need both.
/// Option 2 is deliberately unsupported rather than silently ignored.
pub fn set_table_security(state: &mut LuaState, table: GcRef<Table>, option: u32) -> LuaResult<()> {
    ensure_secure_caller(state)?;
    let flag = match option {
        0 => DISALLOW_TAINTED_ACCESS,
        1 => DISALLOW_SECRET_KEYS,
        2 => return Err(runtime_error("SecretWrapContents is not supported")),
        _ => return Err(runtime_error("invalid TableSecurityOption")),
    };
    let target = state
        .gc
        .tables
        .get_mut(table)
        .ok_or_else(|| runtime_error("table has been collected"))?;
    target.security_flags |= flag;
    Ok(())
}

/// Inspect only VM-owned wrappers, not arbitrary host userdata.
pub fn is_secret_value(state: &LuaState, value: Val) -> bool {
    let Val::Userdata(reference) = value else {
        return false;
    };
    state
        .gc
        .userdata
        .get(reference)
        .is_some_and(|value| value.secret_value().is_some())
}

/// Wrap without changing an already wrapped value. Returned values must be
/// rooted by the caller before any subsequent GC safe point.
pub fn wrap_secret(state: &mut LuaState, value: Val) -> LuaResult<Val> {
    ensure_secure_caller(state)?;
    if is_secret_value(state, value) {
        return Ok(value);
    }
    Ok(Val::Userdata(
        state.gc.alloc_userdata(Userdata::secret(value)),
    ))
}

/// Create a secret result from a boolean computed by trusted Rust host code.
/// Unlike `wrap_secret`, this does not accept a Lua value or change caller taint.
/// The caller must root the result (for example, push it) before another GC safe point.
pub fn wrap_host_secret_bool(state: &mut LuaState, value: bool) -> Val {
    Val::Userdata(state.gc.alloc_userdata(Userdata::secret(Val::Bool(value))))
}

/// Return a secret boolean's payload through the existing secure-caller guard.
/// Non-boolean secret wrappers and ordinary Lua values keep their original behavior.
fn checked_secret_bool(state: &LuaState, value: Val) -> LuaResult<Option<bool>> {
    let Val::Userdata(reference) = value else {
        return Ok(None);
    };
    let Some(Val::Bool(_)) = state
        .gc
        .userdata
        .get(reference)
        .and_then(Userdata::secret_value)
    else {
        return Ok(None);
    };
    let Val::Bool(value) = unwrap_secret(state, value)? else {
        unreachable!("a secret boolean's payload changed during inspection")
    };
    Ok(Some(value))
}

/// Evaluate truthiness without allowing a tainted caller to inspect a secret boolean.
/// Other opaque secret values retain their previous userdata truthiness.
pub(crate) fn checked_truthiness(state: &LuaState, value: Val) -> LuaResult<bool> {
    Ok(checked_secret_bool(state, value)?.unwrap_or_else(|| value.is_truthy()))
}

/// Compare wrapped booleans and nil before userdata identity reveals a secret.
/// `None` delegates other values to the existing raw/metamethod path.
pub(crate) fn checked_secret_equality(
    state: &LuaState,
    left: Val,
    right: Val,
) -> LuaResult<Option<bool>> {
    let left_bool = checked_secret_bool(state, left)?;
    let right_bool = checked_secret_bool(state, right)?;
    if left_bool.is_none() && right_bool.is_none() {
        let left_nil = checked_secret_nil(state, left)?;
        let right_nil = checked_secret_nil(state, right)?;
        if !left_nil && !right_nil {
            return Ok(None);
        }
        return Ok(Some(
            (left_nil || left.is_nil()) && (right_nil || right.is_nil()),
        ));
    }
    let left = left_bool.or_else(|| match left {
        Val::Bool(value) => Some(value),
        _ => None,
    });
    let right = right_bool.or_else(|| match right {
        Val::Bool(value) => Some(value),
        _ => None,
    });
    Ok(Some(left.is_some() && left == right))
}

fn checked_secret_nil(state: &LuaState, value: Val) -> LuaResult<bool> {
    let Val::Userdata(reference) = value else {
        return Ok(false);
    };
    if !matches!(
        state
            .gc
            .userdata
            .get(reference)
            .and_then(Userdata::secret_value),
        Some(Val::Nil)
    ) {
        return Ok(false);
    }
    Ok(unwrap_secret(state, value)?.is_nil())
}

/// Resolve a wrapped table only for ordinary Lua indexing. Fields returned
/// by the table or its __index chain remain plain Lua values; this is not
/// recursive secret propagation or a native-verified information-flow policy.
pub(crate) fn checked_secret_table_read(state: &LuaState, value: Val) -> LuaResult<Val> {
    let Val::Userdata(reference) = value else {
        return Ok(value);
    };
    if !matches!(
        state
            .gc
            .userdata
            .get(reference)
            .and_then(Userdata::secret_value),
        Some(Val::Table(_))
    ) {
        return Ok(value);
    }
    unwrap_secret(state, value)
}

/// Inspect only wrapped booleans and numbers before order comparisons.
/// Lua still rejects ordering booleans; numeric payloads use ordinary Lua
/// ordering only after the stack-wide secure-caller check.
pub(crate) fn checked_order_operand(state: &LuaState, value: Val) -> LuaResult<Val> {
    if let Some(boolean) = checked_secret_bool(state, value)? {
        return Ok(Val::Bool(boolean));
    }
    let Val::Userdata(reference) = value else {
        return Ok(value);
    };
    if matches!(
        state
            .gc
            .userdata
            .get(reference)
            .and_then(Userdata::secret_value),
        Some(Val::Num(_))
    ) {
        return unwrap_secret(state, value);
    }
    Ok(value)
}

/// Unwrap to the original value, preserving table and ordinary-key identity.
pub fn unwrap_secret(state: &LuaState, value: Val) -> LuaResult<Val> {
    let Val::Userdata(reference) = value else {
        return Ok(value);
    };
    let Some(payload) = state
        .gc
        .userdata
        .get(reference)
        .and_then(Userdata::secret_value)
    else {
        return Ok(value);
    };
    ensure_secure_caller(state)?;
    Ok(payload)
}

fn ensure_secure_caller(state: &LuaState) -> LuaResult<()> {
    if state_is_secure(state) {
        Ok(())
    } else {
        Err(runtime_error(
            "table security operation requires an untainted caller",
        ))
    }
}

fn lua_set_table_security(state: &mut LuaState) -> LuaResult<u32> {
    let Val::Table(table) = state.stack_get(state.base) else {
        return Err(runtime_error("settablesecurity requires a table"));
    };
    let option = state.stack_get(state.base + 1);
    let option = match option {
        Val::Num(0.0) => 0,
        Val::Num(1.0) => 1,
        Val::Num(2.0) => 2,
        _ => return Err(runtime_error("invalid TableSecurityOption")),
    };
    set_table_security(state, table, option)?;
    Ok(0)
}

fn lua_secret_wrap(state: &mut LuaState) -> LuaResult<u32> {
    ensure_secure_caller(state)?;
    let count = state.top - state.base;
    for index in 0..count {
        let value = wrap_secret(state, state.stack_get(state.base + index))?;
        state.push(value);
    }
    Ok(count as u32)
}

fn lua_secret_unwrap(state: &mut LuaState) -> LuaResult<u32> {
    let count = state.top - state.base;
    for index in 0..count {
        let value = unwrap_secret(state, state.stack_get(state.base + index))?;
        state.push(value);
    }
    Ok(count as u32)
}

fn lua_is_secret_value(state: &mut LuaState) -> LuaResult<u32> {
    let secret = is_secret_value(state, state.stack_get(state.base));
    state.push(Val::Bool(secret));
    Ok(1)
}
