//! Runtime helpers extracted from the main execute loop.

use crate::error::LuaResult;
use crate::vm::value::{append_lua_number_bytes, lua_number_string_len};

use super::super::gc::arena::Arena;
use super::super::string::LuaString;
use super::{
    CallResult, Closure, Gc, GcRef, LuaState, MAXTAGLOOP, Proto, TMS, Table, Val, arith_error,
    compare_error, gettmbyobj, runtime_error, runtime_error_simple, type_error, val_raw_equal,
};

/// Gets "source:line: " for a given call stack level.
///
/// Level 0 is the currently running function, level 1 is the caller, etc.
/// Returns an empty string if the level doesn't exist or has no source info.
///
/// Matches PUC-Rio's `luaL_where`.
pub(crate) fn get_where(state: &LuaState, level: u32) -> String {
    let level = level as usize;
    if state.ci < level {
        return String::new();
    }

    let target_ci = state.ci - level;
    let ci = &state.call_stack[target_ci];
    let func_val = state.stack_get(ci.func);

    if let Val::Function(r) = func_val
        && let Some(Closure::Lua(lcl)) = state.gc.closures.get(r)
    {
        let proto = &lcl.proto;
        let pc = ci.saved_pc;
        let line = if pc > 0 && pc <= proto.line_info.len() {
            proto.line_info[pc - 1]
        } else if !proto.line_info.is_empty() {
            proto.line_info[0]
        } else {
            return String::new();
        };
        return format!("{}:{line}: ", proto.short_source);
    }

    String::new()
}

/// Looks up a metamethod for the given value.
///
/// Returns the metamethod value if found, or `None`.
pub(super) fn get_tm_for_val(gc: &Gc, val: Val, event: TMS) -> Option<Val> {
    gettmbyobj(
        val,
        event,
        &gc.tables,
        &gc.string_arena,
        &gc.type_metatables,
        &gc.tm_names,
        &gc.userdata,
    )
}

/// Call a metamethod with 2 arguments, storing 1 result.
///
/// Used by arithmetic and comparison metamethods.
/// Stack layout: push tm, push arg1, push arg2, call, read result.
pub(super) fn call_tm_res(
    state: &mut LuaState,
    tm: Val,
    arg1: Val,
    arg2: Val,
    result_reg: usize,
) -> LuaResult<()> {
    let call_base = state.top;
    state.ensure_stack(call_base + 4);
    state.stack_set(call_base, tm);
    state.stack_set(call_base + 1, arg1);
    state.stack_set(call_base + 2, arg2);
    state.top = call_base + 3;

    state.call_function(call_base, 1)?;

    let result = state.stack_get(call_base);
    state.stack_set(result_reg, result);
    Ok(())
}

/// Call a metamethod with 3 arguments, no result stored.
///
/// Used by `__newindex` metamethod.
fn call_tm_void(state: &mut LuaState, tm: Val, arg1: Val, arg2: Val, arg3: Val) -> LuaResult<()> {
    let call_base = state.top;
    state.ensure_stack(call_base + 5);
    state.stack_set(call_base, tm);
    state.stack_set(call_base + 1, arg1);
    state.stack_set(call_base + 2, arg2);
    state.stack_set(call_base + 3, arg3);
    state.top = call_base + 4;

    state.call_function(call_base, 0)?;

    Ok(())
}

/// Try a binary metamethod on left operand, then right.
///
/// Looks up `event` in lhs's metatable. If not found, tries rhs's.
/// Calls the found metamethod with (lhs, rhs) and stores the result.
/// Returns an error if neither side has the metamethod.
///
/// `b_rk` and `c_rk` are the raw B/C instruction fields, used to
/// resolve variable names in error messages via `arith_error`.
#[allow(clippy::too_many_arguments)]
pub(super) fn call_bin_tm(
    state: &mut LuaState,
    lhs: Val,
    rhs: Val,
    result_reg: usize,
    event: TMS,
    proto: &Proto,
    pc: usize,
    base: usize,
    b_rk: u32,
    c_rk: u32,
) -> LuaResult<()> {
    let tm =
        get_tm_for_val(&state.gc, lhs, event).or_else(|| get_tm_for_val(&state.gc, rhs, event));

    match tm {
        Some(tm_val) => call_tm_res(state, tm_val, lhs, rhs, result_reg),
        None => Err(arith_error(state, proto, pc, base, lhs, rhs, b_rk, c_rk)),
    }
}

/// Try an order (comparison) metamethod.
///
/// Matches PUC-Rio's `call_orderTM`: looks up the event on the left
/// operand first, then checks that the right operand has the SAME
/// metamethod (raw equality). Returns `None` if no matching metamethod
/// exists, `Some(bool)` with the truthiness of the call result.
fn call_order_tm(state: &mut LuaState, lhs: Val, rhs: Val, event: TMS) -> LuaResult<Option<bool>> {
    let tm1 = get_tm_for_val(&state.gc, lhs, event);
    let Some(tm1_val) = tm1 else {
        return Ok(None);
    };

    let tm2 = get_tm_for_val(&state.gc, rhs, event);
    let tm2_val = tm2.unwrap_or(Val::Nil);

    if !val_raw_equal(tm1_val, tm2_val, &state.gc.tables, &state.gc.string_arena) {
        return Ok(None);
    }

    let call_base = state.top;
    state.ensure_stack(call_base + 4);
    state.stack_set(call_base, tm1_val);
    state.stack_set(call_base + 1, lhs);
    state.stack_set(call_base + 2, rhs);
    state.top = call_base + 3;

    state.call_depth += 1;
    let cmp_result = (|| {
        state.check_stack_overflow()?;
        match state.precall(call_base, 1)? {
            CallResult::Lua => super::execute(state),
            CallResult::Rust => Ok(()),
        }
    })();
    state.call_depth -= 1;
    cmp_result?;

    let result = state.stack_get(call_base);
    Ok(Some(result.is_truthy()))
}

/// Concatenate registers `base+b` through `base+c`, storing the result
/// in `base+b`. Matches PUC-Rio's `luaV_concat`.
///
/// Processes pairs right-to-left. For each pair, tries to coerce both to
/// strings. If coercion fails, tries the `__concat` metamethod. Coalesces
/// consecutive string/number values into a single buffer for efficiency.
pub(super) fn vm_concat(
    state: &mut LuaState,
    base: usize,
    b: usize,
    c: usize,
    proto: &Proto,
    pc: usize,
) -> LuaResult<()> {
    let mut total = c - b + 1;
    let mut last = c;

    while total > 1 {
        let top = base + last + 1;
        let lhs = state.stack_get(top - 2);
        let rhs = state.stack_get(top - 1);

        if needs_concat_metamethod(lhs, rhs, &state.gc) {
            let error_context = ConcatErrorContext {
                base,
                last,
                proto,
                pc,
            };
            concat_via_metamethod(state, lhs, rhs, top - 2, &error_context)?;
            total -= 1;
            last -= 1;
            continue;
        }

        let run_len = count_concat_run(state, top, total);
        concat_string_run(state, top, run_len, proto, pc)?;
        total -= run_len - 1;
        last -= run_len - 1;
    }
    Ok(())
}

/// Maximum string size. PUC-Rio uses MAX_SIZET which is ~4GB on 32-bit.
/// We use u32::MAX - 2 to match PUC-Rio 32-bit behavior on all platforms,
/// ensuring the PUC-Rio test suite passes regardless of host word size.
const MAX_STRING_SIZE: usize = (u32::MAX - 2) as usize;

fn needs_concat_metamethod(lhs: Val, rhs: Val, gc: &Gc) -> bool {
    !is_string_or_number(lhs, gc) || !is_string_or_number(rhs, gc)
}

struct ConcatErrorContext<'a> {
    base: usize,
    last: usize,
    proto: &'a Proto,
    pc: usize,
}

fn concat_via_metamethod(
    state: &mut LuaState,
    lhs: Val,
    rhs: Val,
    result_reg: usize,
    error_context: &ConcatErrorContext<'_>,
) -> LuaResult<()> {
    let tm = get_tm_for_val(&state.gc, lhs, TMS::Concat)
        .or_else(|| get_tm_for_val(&state.gc, rhs, TMS::Concat));
    if let Some(tm_val) = tm {
        return call_tm_res(state, tm_val, lhs, rhs, result_reg);
    }

    let reg = if is_string_or_number(lhs, &state.gc) {
        error_context.last
    } else {
        error_context.last - 1
    };
    Err(type_error(
        state,
        error_context.proto,
        error_context.pc,
        error_context.base,
        reg,
        "concatenate",
    ))
}

fn count_concat_run(state: &LuaState, top: usize, total: usize) -> usize {
    let mut run_len = 2;
    while run_len < total && is_string_or_number(state.stack_get(top - run_len - 1), &state.gc) {
        run_len += 1;
    }
    run_len
}

fn concat_string_run(
    state: &mut LuaState,
    top: usize,
    run_len: usize,
    proto: &Proto,
    pc: usize,
) -> LuaResult<()> {
    let total_len = concat_run_length(state, top, run_len, proto, pc)?;
    let mut buffer = Vec::with_capacity(total_len);
    for i in (0..run_len).rev() {
        let value = state.stack_get(top - 1 - i);
        val_to_string_bytes(value, &state.gc, &mut buffer);
    }
    let string_ref = state.gc.intern_string(&buffer);
    state.stack_set(top - run_len, Val::Str(string_ref));
    Ok(())
}

fn concat_run_length(
    state: &LuaState,
    top: usize,
    run_len: usize,
    proto: &Proto,
    pc: usize,
) -> LuaResult<usize> {
    let mut total_len = 0;
    for i in (0..run_len).rev() {
        let value_len = val_string_len(state.stack_get(top - 1 - i), &state.gc);
        if value_len >= MAX_STRING_SIZE - total_len {
            return Err(runtime_error(proto, pc, "string length overflow"));
        }
        total_len += value_len;
    }
    Ok(total_len)
}

fn is_string_or_number(val: Val, gc: &Gc) -> bool {
    matches!(val, Val::Num(_)) || {
        if let Val::Str(r) = val {
            gc.string_arena.get(r).is_some()
        } else {
            false
        }
    }
}

fn val_string_len(val: Val, gc: &Gc) -> usize {
    match val {
        Val::Str(r) => gc.string_arena.get(r).map_or(0, |s| s.data().len()),
        Val::Num(n) => lua_number_string_len(n),
        _ => 0,
    }
}

fn val_to_string_bytes(val: Val, gc: &Gc, buffer: &mut Vec<u8>) {
    match val {
        Val::Str(r) => {
            if let Some(s) = gc.string_arena.get(r) {
                buffer.extend_from_slice(s.data());
            }
        }
        Val::Num(n) => append_lua_number_bytes(buffer, n),
        _ => {}
    }
}

/// Raw table get.
#[allow(dead_code)]
fn table_get(state: &LuaState, table_ref: GcRef<Table>, key: Val) -> LuaResult<Val> {
    let table = state
        .gc
        .tables
        .get(table_ref)
        .ok_or_else(|| runtime_error_simple("invalid table reference"))?;
    Ok(table.get(key, &state.gc.string_arena))
}

/// Raw table set with write barrier and memory tracking.
pub(super) fn table_set(
    state: &mut LuaState,
    table_ref: GcRef<Table>,
    key: Val,
    value: Val,
) -> LuaResult<()> {
    ensure_table_not_frozen(state, table_ref)?;
    let table = state
        .gc
        .tables
        .get_mut(table_ref)
        .ok_or_else(|| runtime_error_simple("invalid table reference"))?;
    let mem_before = table.estimated_memory();
    table.raw_set(key, value, &state.gc.string_arena)?;
    let mem_after = table.estimated_memory();
    if mem_after > mem_before {
        state.gc.gc_state.total_bytes += mem_after - mem_before;
    }
    if state.gc.gc_state.total_bytes > state.gc.gc_state.alloc_limit {
        return Err(crate::LuaError::Memory);
    }
    state.gc.barrier_back(table_ref);
    Ok(())
}

/// Propagate the current call frame's taint to a table slot.
///
/// Called after every raw table write when `state.taint_mode` is active.
/// Only writes taint when the current frame is tainted (`Some`). Secure
/// frames (taint = `None`) do not clear existing taint — that is handled
/// separately by explicit `clear_slot_taint_str` calls.
///
/// Key dispatch: `Val::Str` → `set_slot_taint_str`, integer-valued
/// `Val::Num` → `set_slot_taint_int`. Other key types are ignored (they
/// are rare and not tracked by WoW's taint system).
pub(crate) fn propagate_slot_taint(state: &mut LuaState, table_ref: GcRef<Table>, key: Val) {
    if !state.taint_mode {
        return;
    }
    let taint = match state.call_stack[state.ci].taint.as_deref() {
        Some(taint) => taint.to_owned(),
        None => return,
    };

    let resolved = match key {
        Val::Str(r) => state
            .gc
            .string_arena
            .get(r)
            .map_or(ResolvedKey::Other, |s| ResolvedKey::Str(s.data().to_vec())),
        Val::Num(n) if n.is_finite() => resolve_numeric_taint_key(n),
        _ => ResolvedKey::Other,
    };

    let Some(table) = state.gc.tables.get_mut(table_ref) else {
        return;
    };
    match resolved {
        ResolvedKey::Str(bytes) => table.set_slot_taint_str(&bytes, &taint),
        ResolvedKey::Int(k) => table.set_slot_taint_int(k, &taint),
        ResolvedKey::Other => {}
    }
}

enum ResolvedKey {
    Str(Vec<u8>),
    Int(i64),
    Other,
}

fn resolve_numeric_taint_key(n: f64) -> ResolvedKey {
    let key = n as i64;
    #[allow(clippy::float_cmp)]
    if (key as f64) == n {
        ResolvedKey::Int(key)
    } else {
        ResolvedKey::Other
    }
}

/// Lua table get with `__index` metamethod support.
///
/// Matches PUC-Rio's `luaV_gettable`.
#[allow(clippy::too_many_arguments)]
pub(super) fn vm_gettable(
    state: &mut LuaState,
    t: Val,
    key: Val,
    result_reg: usize,
    proto: &Proto,
    pc: usize,
    base: usize,
    obj_reg: Option<usize>,
) -> LuaResult<()> {
    let mut current = t;
    let resolved_key = resolve_gettable_key(key, &state.gc);
    for _ in 0..MAXTAGLOOP {
        match gettable_step(
            state,
            current,
            key,
            resolved_key,
            result_reg,
            proto,
            pc,
            base,
            obj_reg,
        )? {
            GettableStep::Done => return Ok(()),
            GettableStep::Continue(next) => current = next,
        }
    }
    Err(runtime_error_simple("loop in gettable"))
}

enum GettableStep {
    Done,
    Continue(Val),
}

#[allow(clippy::too_many_arguments)]
fn gettable_step(
    state: &mut LuaState,
    current: Val,
    key: Val,
    resolved_key: ResolvedGetKey,
    result_reg: usize,
    proto: &Proto,
    pc: usize,
    base: usize,
    obj_reg: Option<usize>,
) -> LuaResult<GettableStep> {
    match current {
        Val::Table(table_ref) => {
            handle_table_gettable(state, current, table_ref, key, resolved_key, result_reg)
        }
        _ => handle_non_table_gettable(state, current, key, result_reg, proto, pc, base, obj_reg),
    }
}

#[derive(Clone, Copy)]
enum ResolvedGetKey {
    Str { key: GcRef<LuaString>, hash: u32 },
    Int(i64),
    Other(Val),
}

fn resolve_gettable_key(key: Val, gc: &Gc) -> ResolvedGetKey {
    match key {
        Val::Str(string_ref) => {
            gc.string_arena
                .get(string_ref)
                .map_or(ResolvedGetKey::Other(key), |string| ResolvedGetKey::Str {
                    key: string_ref,
                    hash: string.hash(),
                })
        }
        Val::Num(number) => resolve_integer_gettable_key(number)
            .map_or(ResolvedGetKey::Other(key), ResolvedGetKey::Int),
        _ => ResolvedGetKey::Other(key),
    }
}

fn resolve_integer_gettable_key(number: f64) -> Option<i64> {
    if !number.is_finite() {
        return None;
    }
    let integer_key = number as i64;
    #[allow(clippy::float_cmp)]
    if (integer_key as f64) == number {
        Some(integer_key)
    } else {
        None
    }
}

fn handle_table_gettable(
    state: &mut LuaState,
    current: Val,
    table_ref: GcRef<Table>,
    key: Val,
    resolved_key: ResolvedGetKey,
    result_reg: usize,
) -> LuaResult<GettableStep> {
    let table = state
        .gc
        .tables
        .get(table_ref)
        .ok_or_else(|| runtime_error_simple("invalid table reference"))?;
    let result = gettable_fast_result(table, resolved_key, &state.gc.string_arena);

    if !result.is_nil() {
        state.stack_set(result_reg, result);
        return Ok(GettableStep::Done);
    }

    let tm = table
        .metatable()
        .and_then(|mt_ref| get_tm_for_table(&state.gc, mt_ref, TMS::Index));
    match tm {
        None => {
            state.stack_set(result_reg, Val::Nil);
            Ok(GettableStep::Done)
        }
        Some(tm_val) if matches!(tm_val, Val::Function(_)) => {
            call_tm_res(state, tm_val, current, key, result_reg)?;
            Ok(GettableStep::Done)
        }
        Some(tm_val) => Ok(GettableStep::Continue(tm_val)),
    }
}

fn gettable_fast_result(table: &Table, key: ResolvedGetKey, strings: &Arena<LuaString>) -> Val {
    match key {
        ResolvedGetKey::Str { key, hash } => table.get_str_hashed(key, hash),
        ResolvedGetKey::Int(integer_key) => table.get_int(integer_key),
        ResolvedGetKey::Other(raw_key) => table.get(raw_key, strings),
    }
}

#[allow(clippy::too_many_arguments)]
fn handle_non_table_gettable(
    state: &mut LuaState,
    current: Val,
    key: Val,
    result_reg: usize,
    proto: &Proto,
    pc: usize,
    base: usize,
    obj_reg: Option<usize>,
) -> LuaResult<GettableStep> {
    let tm = get_tm_for_val(&state.gc, current, TMS::Index);
    match tm {
        None => {
            if let Some(reg) = obj_reg {
                return Err(type_error(state, proto, pc, base, reg, "index"));
            }
            Err(runtime_error(
                proto,
                pc,
                &format!("attempt to index a {} value", current.type_name()),
            ))
        }
        Some(tm_val) if matches!(tm_val, Val::Function(_)) => {
            call_tm_res(state, tm_val, current, key, result_reg)?;
            Ok(GettableStep::Done)
        }
        Some(tm_val) => Ok(GettableStep::Continue(tm_val)),
    }
}

/// Lua table set with `__newindex` metamethod support.
///
/// Matches PUC-Rio's `luaV_settable`.
#[allow(clippy::too_many_arguments)]
pub(super) fn vm_settable(
    state: &mut LuaState,
    t: Val,
    key: Val,
    value: Val,
    proto: &Proto,
    pc: usize,
    base: usize,
    obj_reg: Option<usize>,
) -> LuaResult<()> {
    let mut current = t;
    for _ in 0..MAXTAGLOOP {
        match settable_step(state, current, key, value, proto, pc, base, obj_reg)? {
            SettableStep::Done => return Ok(()),
            SettableStep::Continue(next) => current = next,
        }
    }
    Err(runtime_error_simple("loop in settable"))
}

enum SettableStep {
    Done,
    Continue(Val),
}

#[allow(clippy::too_many_arguments)]
fn settable_step(
    state: &mut LuaState,
    current: Val,
    key: Val,
    value: Val,
    proto: &Proto,
    pc: usize,
    base: usize,
    obj_reg: Option<usize>,
) -> LuaResult<SettableStep> {
    match current {
        Val::Table(table_ref) => handle_table_settable(state, current, table_ref, key, value),
        _ => handle_non_table_settable(state, current, key, value, proto, pc, base, obj_reg),
    }
}

fn handle_table_settable(
    state: &mut LuaState,
    current: Val,
    table_ref: GcRef<Table>,
    key: Val,
    value: Val,
) -> LuaResult<SettableStep> {
    if table_has_no_metatable(state, table_ref)? {
        raw_set_new_slot(state, table_ref, key, value)?;
        return Ok(SettableStep::Done);
    }

    if !table_get(state, table_ref, key)?.is_nil() {
        ensure_table_not_frozen(state, table_ref)?;
        raw_set_existing_slot(state, table_ref, key, value)?;
        return Ok(SettableStep::Done);
    }

    match get_table_newindex_tm(state, table_ref)? {
        None => {
            ensure_table_not_frozen(state, table_ref)?;
            raw_set_new_slot(state, table_ref, key, value)?;
            Ok(SettableStep::Done)
        }
        Some(tm_val) => resolve_settable_tm(state, current, key, value, tm_val),
    }
}

fn table_has_no_metatable(state: &LuaState, table_ref: GcRef<Table>) -> LuaResult<bool> {
    let table = state
        .gc
        .tables
        .get(table_ref)
        .ok_or_else(|| runtime_error_simple("invalid table reference"))?;
    Ok(table.metatable().is_none())
}

/// Gate a table write on the Frozen flag.
///
/// Frozen tables are expected to be pinned as part of the Track 2
/// freeze-after-bootstrap plan; writing into one would silently break
/// the invariant that every reachable descendant is also frozen. We
/// raise a Lua error so the caller notices at the write site rather
/// than discovering the broken invariant through a later GC crash.
///
/// The metamethod path through `resolve_settable_tm` is NOT gated —
/// the metamethod may legitimately redirect writes to a mutable
/// shadow table.
#[inline]
pub(super) fn ensure_table_not_frozen(state: &LuaState, table_ref: GcRef<Table>) -> LuaResult<()> {
    if state.gc.tables.is_frozen(table_ref) {
        return Err(runtime_error_simple("attempt to modify a frozen table"));
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn handle_non_table_settable(
    state: &mut LuaState,
    current: Val,
    key: Val,
    value: Val,
    proto: &Proto,
    pc: usize,
    base: usize,
    obj_reg: Option<usize>,
) -> LuaResult<SettableStep> {
    let Some(tm_val) = get_tm_for_val(&state.gc, current, TMS::NewIndex) else {
        return Err(settable_type_error(
            state, current, proto, pc, base, obj_reg,
        ));
    };
    resolve_settable_tm(state, current, key, value, tm_val)
}

fn raw_set_existing_slot(
    state: &mut LuaState,
    table_ref: GcRef<Table>,
    key: Val,
    value: Val,
) -> LuaResult<()> {
    let table = state
        .gc
        .tables
        .get_mut(table_ref)
        .ok_or_else(|| runtime_error_simple("invalid table reference"))?;
    table.raw_set(key, value, &state.gc.string_arena)?;
    state.gc.barrier_back(table_ref);
    propagate_slot_taint(state, table_ref, key);
    Ok(())
}

fn raw_set_new_slot(
    state: &mut LuaState,
    table_ref: GcRef<Table>,
    key: Val,
    value: Val,
) -> LuaResult<()> {
    table_set(state, table_ref, key, value)?;
    propagate_slot_taint(state, table_ref, key);
    Ok(())
}

fn get_table_newindex_tm(state: &LuaState, table_ref: GcRef<Table>) -> LuaResult<Option<Val>> {
    let table = state
        .gc
        .tables
        .get(table_ref)
        .ok_or_else(|| runtime_error_simple("invalid table reference"))?;
    Ok(table
        .metatable()
        .and_then(|mt_ref| get_tm_for_table(&state.gc, mt_ref, TMS::NewIndex)))
}

fn resolve_settable_tm(
    state: &mut LuaState,
    current: Val,
    key: Val,
    value: Val,
    tm_val: Val,
) -> LuaResult<SettableStep> {
    if matches!(tm_val, Val::Function(_)) {
        call_tm_void(state, tm_val, current, key, value)?;
        return Ok(SettableStep::Done);
    }
    Ok(SettableStep::Continue(tm_val))
}

#[allow(clippy::too_many_arguments)]
fn settable_type_error(
    state: &LuaState,
    current: Val,
    proto: &Proto,
    pc: usize,
    base: usize,
    obj_reg: Option<usize>,
) -> crate::LuaError {
    if let Some(reg) = obj_reg {
        return type_error(state, proto, pc, base, reg, "index");
    }
    runtime_error(
        proto,
        pc,
        &format!("attempt to index a {} value", current.type_name()),
    )
}

fn get_tm_for_table(gc: &Gc, mt_ref: GcRef<Table>, event: TMS) -> Option<Val> {
    use super::super::metatable::fasttm;

    fasttm(&gc.tables, &gc.string_arena, mt_ref, event, &gc.tm_names)
}

/// Lua raw equality comparison (no metamethods).
pub(super) fn val_equal(a: Val, b: Val, gc: &Gc) -> bool {
    val_raw_equal(a, b, &gc.tables, &gc.string_arena)
}

/// Compare two Lua strings using `strcoll` (locale-aware), matching
/// PUC-Rio's `l_strcmp` in `lvm.c`. Handles embedded null bytes by
/// iterating over null-terminated segments.
#[allow(unsafe_code)]
pub(crate) fn l_strcmp(left: &[u8], right: &[u8]) -> std::cmp::Ordering {
    let mut l = left;
    let mut r = right;
    loop {
        let l_nul = l.iter().position(|&b| b == 0).unwrap_or(l.len());
        let r_nul = r.iter().position(|&b| b == 0).unwrap_or(r.len());

        let mut l_buf = Vec::with_capacity(l_nul + 1);
        l_buf.extend_from_slice(&l[..l_nul]);
        l_buf.push(0);
        let mut r_buf = Vec::with_capacity(r_nul + 1);
        r_buf.extend_from_slice(&r[..r_nul]);
        r_buf.push(0);

        let temp = unsafe { super::strcoll(l_buf.as_ptr(), r_buf.as_ptr()) };
        if temp != 0 {
            return if temp < 0 {
                std::cmp::Ordering::Less
            } else {
                std::cmp::Ordering::Greater
            };
        }
        if r_nul >= r.len() {
            return if l_nul >= l.len() {
                std::cmp::Ordering::Equal
            } else {
                std::cmp::Ordering::Greater
            };
        } else if l_nul >= l.len() {
            return std::cmp::Ordering::Less;
        }
        let skip = l_nul + 1;
        l = &l[skip..];
        r = &r[skip..];
    }
}

/// Lua less-than comparison with metamethod support.
pub(super) fn val_less_than(
    a: Val,
    b: Val,
    state: &mut LuaState,
    proto: &Proto,
    pc: usize,
) -> LuaResult<bool> {
    match (&a, &b) {
        (Val::Num(x), Val::Num(y)) => Ok(x < y),
        (Val::Str(x), Val::Str(y)) => {
            let sx = state
                .gc
                .string_arena
                .get(*x)
                .ok_or_else(|| compare_error(proto, pc, a, b))?;
            let sy = state
                .gc
                .string_arena
                .get(*y)
                .ok_or_else(|| compare_error(proto, pc, a, b))?;
            Ok(l_strcmp(sx.data(), sy.data()) == std::cmp::Ordering::Less)
        }
        _ => {
            if std::mem::discriminant(&a) != std::mem::discriminant(&b) {
                return Err(compare_error(proto, pc, a, b));
            }
            match call_order_tm(state, a, b, TMS::Lt)? {
                Some(result) => Ok(result),
                None => Err(compare_error(proto, pc, a, b)),
            }
        }
    }
}

/// Lua less-or-equal comparison with metamethod support.
pub(super) fn val_less_equal(
    a: Val,
    b: Val,
    state: &mut LuaState,
    proto: &Proto,
    pc: usize,
) -> LuaResult<bool> {
    match (&a, &b) {
        (Val::Num(x), Val::Num(y)) => Ok(x <= y),
        (Val::Str(x), Val::Str(y)) => {
            let sx = state
                .gc
                .string_arena
                .get(*x)
                .ok_or_else(|| compare_error(proto, pc, a, b))?;
            let sy = state
                .gc
                .string_arena
                .get(*y)
                .ok_or_else(|| compare_error(proto, pc, a, b))?;
            Ok(l_strcmp(sx.data(), sy.data()) != std::cmp::Ordering::Greater)
        }
        _ => {
            if std::mem::discriminant(&a) != std::mem::discriminant(&b) {
                return Err(compare_error(proto, pc, a, b));
            }
            if let Some(result) = call_order_tm(state, a, b, TMS::Le)? {
                return Ok(result);
            }
            if let Some(result) = call_order_tm(state, b, a, TMS::Lt)? {
                return Ok(!result);
            }
            Err(compare_error(proto, pc, a, b))
        }
    }
}
