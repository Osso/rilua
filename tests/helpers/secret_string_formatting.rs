use rilua::api::state_is_secure;
use rilua::table_security::{is_secret_value, unwrap_secret, wrap_host_secret_string};
use rilua::{Lua, LuaApiMut, Val};

fn fixture() -> Lua {
    let mut lua = Lua::new().unwrap();
    rilua::table_security::register_table_security(&mut lua).unwrap();
    let secret = wrap_host_secret_string(lua.state_mut(), "abcdefgh");
    lua.state_mut().push(secret);
    lua.set_global_val("HostSecret", secret).unwrap();
    lua.state_mut().pop();
    lua.exec("debug.settaintmode(true); collectgarbage('collect'); collectgarbage('collect')")
        .unwrap();
    lua
}

fn assert_payload(lua: &mut Lua, name: &str, expected: &[u8]) {
    // Trusted Rust inspection only; no decoded callback or taint reset.
    let result = lua.get_global_val(name);
    let state = lua.state_mut();
    assert!(state_is_secure(state));
    assert!(is_secret_value(state, result), "{name} must remain secret");
    let Val::Str(reference) = unwrap_secret(state, result).unwrap() else {
        panic!("formatted payload must be a string");
    };
    assert_eq!(
        state.gc.string_arena.get(reference).unwrap().data(),
        expected
    );
}

#[test]
fn secret_format_precision_preserves_full_payload() {
    let mut lua = fixture();
    lua.exec("Result = string.format('%.5s', HostSecret)")
        .unwrap();
    assert_payload(&mut lua, "Result", b"abcdefgh");
}

#[test]
fn secret_format_width_and_alignment_add_no_padding() {
    let mut lua = fixture();
    lua.exec(
        "Right = string.format('%12s', HostSecret); Left = string.format('%-12s', HostSecret)",
    )
    .unwrap();
    assert_payload(&mut lua, "Right", b"abcdefgh");
    assert_payload(&mut lua, "Left", b"abcdefgh");
}

#[test]
fn secret_format_public_controls_keep_width_and_precision() {
    fixture()
        .exec(
            r#"
        assert(string.format('%.5s', 'abcdefgh') == 'abcde')
        assert(string.format('%12s', 'abcdefgh') == '    abcdefgh')
        assert(string.format('%-12s', 'abcdefgh') == 'abcdefgh    ')
        assert(not issecretvalue(string.format('%s:%02d', 'public', 7)))
    "#,
        )
        .unwrap();
}

#[test]
fn secret_format_mixed_arguments_and_gc_retain_typed_result() {
    let mut lua = fixture();
    lua.exec(
        r#"
        Result = string.format('[%5.2s|%.5s|%02d|%12s]', 'public', HostSecret, 7, HostSecret)
        HostSecret = nil
        collectgarbage('collect')
        collectgarbage('collect')
    "#,
    )
    .unwrap();
    assert_payload(&mut lua, "Result", b"[   pu|abcdefgh|07|abcdefgh]");
}

#[test]
fn secret_format_unused_secret_input_still_marks_output() {
    let mut lua = fixture();
    lua.exec("Result = string.format('public:%s', 'value', HostSecret)")
        .unwrap();
    assert_payload(&mut lua, "Result", b"public:value");
}

#[test]
fn secret_format_inferred_tainted_operation_preserves_guards_and_stack_taint() {
    let mut lua = fixture();
    // Addon opaque-format permission is inferred, not native-verified.
    lua.exec(
        r#"
        local function addon_format()
            assert(debug.getstacktaint() == 'FormatAddon')
            assert(not pcall(secretunwrap, HostSecret))
            local result = string.format('%.5s/%12s', HostSecret, HostSecret)
            assert(debug.getstacktaint() == 'FormatAddon')
            assert(issecretvalue(result))
            assert(not pcall(secretunwrap, result))
            assert(debug.getstacktaint() == 'FormatAddon')
            return result
        end
        debug.setobjecttaint(addon_format, 'FormatAddon')
        Result = addon_format()
        assert(debug.getstacktaint() == nil)
        collectgarbage('collect')
    "#,
    )
    .unwrap();
    assert_payload(&mut lua, "Result", b"abcdefgh/abcdefgh");
}
