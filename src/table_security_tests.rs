use super::*;
use crate::LuaApi;

#[test]
fn secret_payload_kind_covers_tags_and_contents_flag_without_unwrap() -> LuaResult<()> {
    let mut state = LuaState::new();
    let table = state.gc.alloc_table(Table::new());
    let text = state.gc.intern_string(b"payload");
    let function = state.gc.alloc_closure(crate::vm::closure::Closure::Rust(
        crate::vm::closure::RustClosure::new(|_| Ok(0), "kind_test"),
    ));
    let userdata = state.gc.alloc_userdata(Userdata::new(Box::new(17_u32)));
    let thread = state.gc.alloc_thread(crate::vm::state::LuaThread::new(
        Val::Function(function),
        state.global,
    ));
    let values = [
        (Val::Nil, SecretPayloadKind::Nil),
        (Val::Bool(false), SecretPayloadKind::Boolean),
        (Val::Num(3.0), SecretPayloadKind::Number),
        (Val::Str(text), SecretPayloadKind::String),
        (Val::Table(table), SecretPayloadKind::Table),
        (Val::LightUserdata(4), SecretPayloadKind::LightUserdata),
        (Val::Function(function), SecretPayloadKind::Function),
        (Val::Userdata(userdata), SecretPayloadKind::Userdata),
        (Val::Thread(thread), SecretPayloadKind::Thread),
    ];
    for (value, kind) in values {
        let wrapped = wrap_secret(&mut state, value)?;
        assert_eq!(secret_payload_kind(&state, wrapped), Some(kind));
        assert!(is_secret_value(&state, wrapped));
        assert_eq!(
            is_secret_table(&state, wrapped),
            kind == SecretPayloadKind::Table
        );
        assert_eq!(secret_payload_kind(&state, value), None);
    }
    assert!(!is_secret_table(&state, Val::Table(table)));
    set_table_security(&mut state, table, 2)?;
    assert!(is_secret_table(&state, Val::Table(table)));
    state.call_stack[0].taint = Some("Addon".to_owned());
    assert!(is_secret_table(&state, Val::Table(table)));
    assert!(set_table_security(&mut state, table, 2).is_err());
    Ok(())
}

#[test]
fn secret_access_revocation_tracks_frames_without_changing_taint() -> LuaResult<()> {
    use crate::vm::callinfo::CallInfo;
    let mut state = LuaState::new();
    let secret = wrap_host_secret_number(&mut state, 7.0);
    state.push(secret);
    assert!(revoke_secret_access(&mut state).is_err());
    let mut frame = CallInfo::new(0, 1, 20, 0);
    frame.is_lua = true;
    state.push_ci(frame);
    revoke_secret_access(&mut state)?;
    assert!(state_is_secure(&state));
    assert!(!can_access_secrets(&state));
    assert!(unwrap_secret(&state, secret).is_err());
    state.push_ci(CallInfo::new(0, 1, 20, 0));
    assert!(!can_access_secrets(&state));
    state.pop_ci();
    assert!(!can_access_secrets(&state));
    state.pop_ci();
    assert!(can_access_secrets(&state));
    assert_eq!(unwrap_secret(&state, secret)?, Val::Num(7.0));
    Ok(())
}

#[test]
fn secret_transform_binary_payload_errors_and_kind_validation() -> LuaResult<()> {
    let mut state = LuaState::new();
    let bytes = state.gc.intern_string(&[0, 255, b'A']);
    let input = wrap_secret(&mut state, Val::Str(bytes))?;
    state.push(input);
    let output = transform_host_secret_string(&mut state, input, |bytes| {
        Ok(bytes.iter().rev().copied().collect())
    })?;
    state.push(output);
    let Val::Str(result) = unwrap_secret(&state, output)? else {
        return Err(runtime_error("string expected"));
    };
    assert_eq!(
        state
            .gc
            .string_arena
            .get(result)
            .map(crate::vm::string::LuaString::data),
        Some([b'A', 255, 0].as_slice())
    );
    assert!(
        transform_host_secret_string(&mut state, input, |_| Err(runtime_error("host failure")))
            .is_err()
    );
    assert_eq!(unwrap_secret(&state, input)?, Val::Str(bytes));
    let number = wrap_host_secret_number(&mut state, 3.0);
    assert!(transform_host_secret_string(&mut state, number, |_| Ok(vec![])).is_err());
    Ok(())
}

fn host_number_inspector_lua() -> LuaResult<Lua> {
    let mut lua = Lua::new()?;
    register_table_security(&mut lua)?;
    lua.register_function("inspect_number", inspect_restricted_host_number)?;
    lua.register_function("drop_number_access", |state| {
        revoke_secret_access(state)?;
        Ok(0)
    })?;
    let secret = wrap_host_secret_number(lua.state_mut(), 42.0);
    lua.state_mut().push(secret);
    lua.set_global_val("HostNumber", secret)?;
    lua.state_mut().pop();
    lua.exec("debug.settaintmode(true)")?;
    Ok(lua)
}

#[derive(Debug, PartialEq, Eq)]
enum InspectedNumberSelector {
    Fixture,
}

fn snapshot_number_inspector_taints(state: &LuaState) -> Vec<Option<String>> {
    state.call_stack[..=state.ci]
        .iter()
        .map(|frame| frame.taint.clone())
        .collect()
}

fn inspect_restricted_host_number(state: &mut LuaState) -> LuaResult<u32> {
    let input = state.stack_get(state.base);
    let taints_before = snapshot_number_inspector_taints(state);
    let secure_before = state_is_secure(state);
    assert!(!can_access_secrets(state));
    let denial = unwrap_secret(state, input).unwrap_err();
    assert!(
        denial
            .to_string()
            .contains("secret access requires an untainted caller without revoked access")
    );
    let selector = inspect_host_secret_number(state, input, |number| {
        assert_eq!(number, 42.0);
        Ok(InspectedNumberSelector::Fixture)
    })?;
    assert_eq!(selector, InspectedNumberSelector::Fixture);
    let taints_after = snapshot_number_inspector_taints(state);
    assert_eq!(taints_after, taints_before);
    assert_eq!(state_is_secure(state), secure_before);
    assert!(!can_access_secrets(state));
    assert!(unwrap_secret(state, input).is_err());
    // No payload or host selector is pushed back to Lua.
    Ok(0)
}

#[test]
fn host_secret_number_inspector_accepts_tainted_host_callback_without_unwrap() -> LuaResult<()> {
    let mut lua = host_number_inspector_lua()?;
    lua.exec(
        r"
        local function addon()
            assert(debug.getstacktaint() == 'NumberAddon')
            assert(select('#', inspect_number(HostNumber)) == 0)
            assert(debug.getstacktaint() == 'NumberAddon')
            assert(not pcall(secretunwrap, HostNumber))
        end
        debug.setobjecttaint(addon, 'NumberAddon')
        addon()
        assert(debug.getstacktaint() == nil)
        assert(secretunwrap(HostNumber) == 42)
    ",
    )?;
    let secret = lua.get_global_val("HostNumber");
    assert!(can_access_secrets(lua.state()));
    assert_eq!(unwrap_secret(lua.state(), secret)?, Val::Num(42.0));
    Ok(())
}

#[test]
fn host_secret_number_inspector_revoked_host_callback_keeps_denial() -> LuaResult<()> {
    let mut lua = host_number_inspector_lua()?;
    lua.exec(
        r"
        local function revoked()
            drop_number_access()
            assert(debug.getstacktaint() == nil and issecure())
            assert(select('#', inspect_number(HostNumber)) == 0)
            securecall(function()
                assert(debug.getstacktaint() == nil and issecure())
                inspect_number(HostNumber)
                assert(not pcall(secretunwrap, HostNumber))
            end)
            assert(debug.getstacktaint() == nil and issecure())
            assert(not pcall(secretunwrap, HostNumber))
        end
        revoked()
        assert(debug.getstacktaint() == nil)
        assert(secretunwrap(HostNumber) == 42)
    ",
    )?;
    let secret = lua.get_global_val("HostNumber");
    assert!(can_access_secrets(lua.state()));
    assert_eq!(unwrap_secret(lua.state(), secret)?, Val::Num(42.0));
    Ok(())
}

fn assert_host_number_inspection_rejected(state: &LuaState, input: Val) {
    let calls = std::cell::Cell::new(0);
    let error = inspect_host_secret_number(state, input, |_| {
        calls.set(calls.get() + 1);
        Ok(InspectedNumberSelector::Fixture)
    })
    .unwrap_err();
    let crate::LuaError::Runtime(error) = error else {
        panic!("expected numeric wrapper validation error");
    };
    assert_eq!(error.message, "secret number expected");
    assert_eq!(calls.get(), 0);
}

#[test]
fn host_secret_number_inspector_rejects_public_values_and_wrong_userdata_payloads() {
    let mut state = LuaState::new();
    let text = Val::Str(state.gc.intern_string(b"42"));
    let table = Val::Table(state.gc.alloc_table(Table::new()));
    let ordinary_userdata = Val::Userdata(state.gc.alloc_userdata(Userdata::new(Box::new(42.0))));
    let boolean = wrap_host_secret_bool(&mut state, true);
    let string = wrap_host_secret_string(&mut state, "42");
    for input in [
        Val::Nil,
        Val::Num(42.0),
        Val::Bool(true),
        text,
        table,
        ordinary_userdata,
        boolean,
        string,
    ] {
        assert_host_number_inspection_rejected(&state, input);
    }
}

#[test]
fn host_secret_number_inspector_rejects_collected_wrapper() -> LuaResult<()> {
    let mut lua = Lua::new()?;
    let secret = wrap_host_secret_number(lua.state_mut(), 42.0);
    let Val::Userdata(reference) = secret else {
        panic!("expected secret wrapper");
    };
    // A Rust Val is not a GC root. Arena lookup safely rejects its stale generation.
    lua.exec("collectgarbage('collect'); collectgarbage('collect')")?;
    assert!(lua.state().gc.userdata.get(reference).is_none());
    assert_host_number_inspection_rejected(lua.state(), secret);
    Ok(())
}

#[test]
fn host_secret_number_inspector_closure_error_preserves_secure_unwrap() -> LuaResult<()> {
    let mut state = LuaState::new();
    let secret = wrap_host_secret_number(&mut state, 42.0);
    state.push(secret);
    let calls = std::cell::Cell::new(0);
    let error = inspect_host_secret_number::<InspectedNumberSelector>(&state, secret, |number| {
        calls.set(calls.get() + 1);
        assert_eq!(number, 42.0);
        let mut error = crate::error::RuntimeError::new("trusted callback rejected operation");
        error.level = 17;
        Err(crate::LuaError::Runtime(error))
    })
    .unwrap_err();
    assert_eq!(calls.get(), 1);
    let crate::LuaError::Runtime(error) = error else {
        panic!("expected unchanged callback runtime error");
    };
    assert_eq!(error.message, "trusted callback rejected operation");
    assert_eq!(error.level, 17);
    assert!(error.traceback.is_empty());
    assert!(can_access_secrets(&state));
    assert_eq!(unwrap_secret(&state, secret)?, Val::Num(42.0));
    let selector = inspect_host_secret_number(&state, secret, |number| {
        assert_eq!(number, 42.0);
        Ok(InspectedNumberSelector::Fixture)
    })?;
    assert_eq!(selector, InspectedNumberSelector::Fixture);
    Ok(())
}
