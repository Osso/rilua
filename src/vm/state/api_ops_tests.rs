use super::LuaState;
use crate::vm::{
    closure::{Closure, RustClosure},
    table::Table,
    value::Userdata,
};
use crate::{LuaResult, Val};

fn object_with_index(state: &mut LuaState, index: Val) -> Val {
    let meta = state.gc.alloc_table(Table::new());
    let key = state.gc.intern_string_static(b"__index");
    state
        .gc
        .tables
        .get_mut(meta)
        .unwrap()
        .raw_set(Val::Str(key), index, &state.gc.string_arena)
        .unwrap();
    Val::Userdata(
        state
            .gc
            .alloc_userdata(Userdata::with_metatable(Box::new(()), meta)),
    )
}

#[test]
fn api_gettable_follows_userdata_index_table() {
    let mut state = LuaState::new();
    let methods = state.gc.alloc_table(Table::new());
    let key = Val::Str(state.gc.intern_string_static(b"method"));
    state
        .gc
        .tables
        .get_mut(methods)
        .unwrap()
        .raw_set(key, Val::Num(17.0), &state.gc.string_arena)
        .unwrap();
    let object = object_with_index(&mut state, Val::Table(methods));
    assert_eq!(state.gettable(object, key).unwrap(), Val::Num(17.0));
}

fn index_function(state: &mut LuaState) -> LuaResult<u32> {
    state.push(Val::Num(23.0));
    Ok(1)
}

#[test]
fn api_gettable_calls_userdata_index_function() {
    let mut state = LuaState::new();
    let function = state
        .gc
        .alloc_closure(Closure::Rust(RustClosure::new(index_function, "index")));
    let object = object_with_index(&mut state, Val::Function(function));
    let key = Val::Str(state.gc.intern_string_static(b"method"));
    assert_eq!(state.gettable(object, key).unwrap(), Val::Num(23.0));
}

#[test]
fn api_gettable_rejects_non_indexable_values() {
    let mut state = LuaState::new();
    let key = Val::Str(state.gc.intern_string_static(b"method"));
    assert!(state.gettable(Val::Num(1.0), key).is_err());
}
