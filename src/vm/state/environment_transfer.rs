//! Host-controlled value conversion at Lua closure environment boundaries.

use super::LuaState;
use crate::LuaResult;
use crate::vm::gc::arena::GcRef;
use crate::vm::table::Table;
use std::ops::Range;

/// Convert direct arguments or results between distinct Lua environments.
///
/// The hook may replace values in the supplied range, but must preserve the
/// active call frames and stack top. Native functions do not themselves form
/// an environment boundary. Nested calls made by the hook do not invoke it.
pub type EnvironmentTransferHook =
    fn(&mut LuaState, GcRef<Table>, GcRef<Table>, Range<usize>) -> LuaResult<()>;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Lua, LuaApi, LuaApiMut, Val};

    fn convert_token(
        state: &mut LuaState,
        _source: GcRef<Table>,
        target: GcRef<Table>,
        values: Range<usize>,
    ) -> LuaResult<()> {
        let (from, to) = if target == state.global {
            (1042.0, 42.0)
        } else {
            (42.0, 1042.0)
        };
        for slot in values {
            if state.stack_get(slot) == Val::Num(from) {
                state.stack_set(slot, Val::Num(to));
            }
        }
        Ok(())
    }

    fn environment_fixture() -> Lua {
        let mut lua = Lua::new().unwrap();
        lua.exec("privateEnv = setmetatable({}, {__index=_G})")
            .unwrap();
        lua.state_mut().environment_transfer_hook = Some(convert_token);
        lua
    }

    #[test]
    fn environment_transfer_preserves_fixed_vararg_and_multiple_results() {
        let mut lua = environment_fixture();
        lua.exec(
            r#"
            local function f(first, ...)
                assert(first == 1042)
                assert(select('#', ...) == 3 and select(1, ...) == 1042)
                return first, nil, ...
            end
            setfenv(f, privateEnv)
            local a,b,c,d,e = f(42,42,nil,'plain')
            assert(a == 42 and b == nil and c == 42 and d == nil and e == 'plain')
            local ok, value = pcall(f, 42,42,nil,'plain')
            assert(ok and value == 42)
        "#,
        )
        .unwrap();
    }

    fn native_token(state: &mut LuaState) -> LuaResult<u32> {
        state.push(Val::Num(42.0));
        Ok(1)
    }

    #[test]
    fn environment_transfer_handles_local_outbound_tail_return() {
        let mut lua = environment_fixture();
        lua.register_function("native_token", native_token).unwrap();
        lua.exec(
            r#"
            local function outbound() return native_token() end
            local function private()
                assert(outbound() == 1042)
                assert(native_token() == 42, 'native raw return is explicit')
                return outbound()
            end
            setfenv(private, privateEnv)
            assert(private() == 42)
        "#,
        )
        .unwrap();
    }

    fn rejecting_transfer(
        state: &mut LuaState,
        source: GcRef<Table>,
        target: GcRef<Table>,
        values: Range<usize>,
    ) -> LuaResult<()> {
        if values
            .clone()
            .any(|slot| state.stack_get(slot) == Val::Num(13.0))
        {
            return Err(crate::runtime_error("transfer denied"));
        }
        convert_token(state, source, target, values)
    }

    #[test]
    fn environment_transfer_errors_are_caught_and_hook_restored() {
        let mut lua = environment_fixture();
        lua.state_mut().environment_transfer_hook = Some(rejecting_transfer);
        lua.exec(
            r#"
            local f = setfenv(function(value) return value end, privateEnv)
            local ok, err = pcall(f,13)
            assert(not ok and string.find(err,'transfer denied',1,true))
            assert(f(42) == 42)
            local result = setfenv(function() return 13 end, privateEnv)
            ok, err = pcall(result)
            assert(not ok and string.find(err,'transfer denied',1,true))
            assert(f(42) == 42)
        "#,
        )
        .unwrap();
    }

    fn reentrant_transfer(
        state: &mut LuaState,
        source: GcRef<Table>,
        target: GcRef<Table>,
        values: Range<usize>,
    ) -> LuaResult<()> {
        state.exec("setfenv(function() return 13 end, privateEnv)()")?;
        convert_token(state, source, target, values)
    }

    #[test]
    fn environment_transfer_allows_host_reentry_without_recursing_hook() {
        let mut lua = environment_fixture();
        lua.state_mut().environment_transfer_hook = Some(reentrant_transfer);
        lua.exec(
            r#"
            local f = setfenv(function(value) assert(value==1042); return value end, privateEnv)
            assert(f(42)==42)
        "#,
        )
        .unwrap();
    }

    #[test]
    fn environment_transfer_default_and_same_environment_are_unchanged() {
        let mut lua = Lua::new().unwrap();
        lua.exec("local e=setmetatable({},{__index=_G}); assert(setfenv(function(x) return x end,e)(42)==42)").unwrap();
        lua.state_mut().environment_transfer_hook = Some(rejecting_transfer);
        lua.exec("local function f(x) return x end; assert(f(13)==13)")
            .unwrap();
    }
}
