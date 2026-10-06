use super::*;

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
