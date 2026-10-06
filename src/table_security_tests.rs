use super::*;

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
        state.gc.string_arena.get(result).map(|s| s.data()),
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
