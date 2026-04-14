//! API-facing table and metamethod operations for `LuaState`.

use crate::error::{LuaError, LuaResult, RuntimeError};
use crate::vm::value::append_lua_number_bytes;

use super::{LuaState, Table, Val};
use crate::vm::gc::arena::GcRef;
use crate::vm::metatable::{MAXTAGLOOP, TMS, fasttm, gettmbyobj, val_raw_equal};

impl LuaState {
    /// Metamethod-aware table index: `t[key]` with `__index` chain.
    ///
    /// Equivalent to PUC-Rio's `lua_gettable`. Follows `__index` metamethods
    /// up to `MAXTAGLOOP` depth. Used by stdlib code that needs full Lua
    /// table access semantics (e.g., gsub table replacement).
    pub fn gettable(&mut self, t: Val, key: Val) -> LuaResult<Val> {
        let mut current = t;
        for _ in 0..MAXTAGLOOP {
            if let Val::Table(table_ref) = current {
                let result = self
                    .gc
                    .tables
                    .get(table_ref)
                    .map_or(Val::Nil, |table| table.get(key, &self.gc.string_arena));
                if !result.is_nil() {
                    return Ok(result);
                }

                match lookup_table_tm(self, table_ref, TMS::Index)? {
                    None => return Ok(Val::Nil),
                    Some(tm_val) if matches!(tm_val, Val::Function(_)) => {
                        return self.call_tm_two_args(tm_val, current, key);
                    }
                    Some(tm_val) => current = tm_val,
                }
            } else {
                return Ok(Val::Nil);
            }
        }

        Err(loop_error("gettable"))
    }

    /// Metamethod-aware table set: `t[key] = value` with `__newindex` chain.
    ///
    /// Equivalent to PUC-Rio's `lua_settable`. Follows `__newindex`
    /// metamethods up to `MAXTAGLOOP` depth. Used by API-level code
    /// that needs full Lua table assignment semantics.
    pub fn settable(&mut self, t: Val, key: Val, value: Val) -> LuaResult<()> {
        let mut current = t;
        for _ in 0..MAXTAGLOOP {
            if let Val::Table(table_ref) = current {
                let existing = self
                    .gc
                    .tables
                    .get(table_ref)
                    .map_or(Val::Nil, |table| table.get(key, &self.gc.string_arena));
                if !existing.is_nil() {
                    self.raw_set_api_table(table_ref, key, value)?;
                    return Ok(());
                }

                match lookup_table_tm(self, table_ref, TMS::NewIndex)? {
                    None => {
                        self.raw_set_api_table(table_ref, key, value)?;
                        return Ok(());
                    }
                    Some(tm_val) if matches!(tm_val, Val::Function(_)) => {
                        self.call_tm_three_args(tm_val, current, key, value)?;
                        return Ok(());
                    }
                    Some(tm_val) => current = tm_val,
                }
            } else {
                match lookup_tm(self, current, TMS::NewIndex) {
                    None => return Err(index_error(current)),
                    Some(tm_val) if matches!(tm_val, Val::Function(_)) => {
                        self.call_tm_three_args(tm_val, current, key, value)?;
                        return Ok(());
                    }
                    Some(tm_val) => current = tm_val,
                }
            }
        }

        Err(loop_error("settable"))
    }

    /// API-level less-than comparison with metamethod support.
    ///
    /// Equivalent to PUC-Rio's `lua_lessthan`. Unlike the VM's
    /// `val_less_than`, this doesn't require proto/pc context.
    pub fn api_lessthan(&mut self, a: Val, b: Val) -> LuaResult<bool> {
        match (&a, &b) {
            (Val::Num(x), Val::Num(y)) => Ok(x < y),
            (Val::Str(x), Val::Str(y)) => {
                let sx = self.gc.string_arena.get(*x);
                let sy = self.gc.string_arena.get(*y);
                match (sx, sy) {
                    (Some(sx), Some(sy)) => Ok(crate::vm::execute::l_strcmp(sx.data(), sy.data())
                        == std::cmp::Ordering::Less),
                    _ => Err(self.compare_error(a, b)),
                }
            }
            _ => {
                if std::mem::discriminant(&a) != std::mem::discriminant(&b) {
                    return Err(self.compare_error(a, b));
                }
                match self.call_order_tm_api(a, b, TMS::Lt)? {
                    Some(result) => Ok(result),
                    None => Err(self.compare_error(a, b)),
                }
            }
        }
    }

    /// API-level equality comparison with metamethod support.
    ///
    /// Equivalent to PUC-Rio's `lua_equal`. Triggers `__eq` metamethod
    /// for tables and userdata of the same type.
    pub fn api_equal(&mut self, a: Val, b: Val) -> LuaResult<bool> {
        if val_raw_equal(a, b, &self.gc.tables, &self.gc.string_arena) {
            return Ok(true);
        }
        if std::mem::discriminant(&a) != std::mem::discriminant(&b) {
            return Ok(false);
        }
        if !matches!(a, Val::Table(_) | Val::Userdata(_)) {
            return Ok(false);
        }

        let Some(lhs_tm) = lookup_tm(self, a, TMS::Eq) else {
            return Ok(false);
        };
        let rhs_tm = lookup_tm(self, b, TMS::Eq).unwrap_or(Val::Nil);
        if !val_raw_equal(lhs_tm, rhs_tm, &self.gc.tables, &self.gc.string_arena) {
            return Ok(false);
        }

        let result = self.call_tm_two_args(lhs_tm, a, b)?;
        Ok(result.is_truthy())
    }

    /// API-level concatenation of `count` values at top of stack.
    ///
    /// Concatenates values at positions `(top - count)..top`, placing
    /// the result at `top - count` and adjusting `top`.
    pub fn api_concat(&mut self, count: usize) -> LuaResult<()> {
        if count == 0 {
            let string_ref = self.gc.intern_string(b"");
            self.push(Val::Str(string_ref));
            return Ok(());
        }
        if count == 1 {
            return Ok(());
        }

        let mut total = count;
        let result_pos = self.top - count;

        while total > 1 {
            total = self.concat_step(result_pos, total)?;
        }

        self.top = result_pos + 1;
        Ok(())
    }

    fn concat_step(&mut self, result_pos: usize, total: usize) -> LuaResult<usize> {
        let top = result_pos + total;
        let lhs = self.stack_get(top - 2);
        let rhs = self.stack_get(top - 1);

        if self.needs_concat_metamethod(lhs, rhs) {
            return self
                .concat_via_metamethod(top, lhs, rhs)
                .map(|()| total - 1);
        }

        let run_len = self.count_concat_run(top, total);
        self.concat_string_run(top, run_len);
        Ok(total - (run_len - 1))
    }

    fn needs_concat_metamethod(&self, lhs: Val, rhs: Val) -> bool {
        !self.is_string_or_number(lhs) || !self.is_string_or_number(rhs)
    }

    fn concat_via_metamethod(&mut self, top: usize, lhs: Val, rhs: Val) -> LuaResult<()> {
        let Some(tm_val) =
            lookup_tm(self, lhs, TMS::Concat).or_else(|| lookup_tm(self, rhs, TMS::Concat))
        else {
            return Err(concat_error(lhs, rhs, self));
        };

        let result = self.call_tm_two_args(tm_val, lhs, rhs)?;
        self.stack_set(top - 2, result);
        self.top = top - 1;
        Ok(())
    }

    fn count_concat_run(&self, top: usize, total: usize) -> usize {
        let mut run_len = 2;
        while run_len < total && self.is_string_or_number(self.stack_get(top - run_len - 1)) {
            run_len += 1;
        }
        run_len
    }

    fn concat_string_run(&mut self, top: usize, run_len: usize) {
        let mut buffer = Vec::new();
        for i in (0..run_len).rev() {
            let value = self.stack_get(top - 1 - i);
            self.val_to_string_bytes(value, &mut buffer);
        }
        let string_ref = self.gc.intern_string(&buffer);
        self.stack_set(top - run_len, Val::Str(string_ref));
    }

    fn raw_set_api_table(
        &mut self,
        table_ref: GcRef<Table>,
        key: Val,
        value: Val,
    ) -> LuaResult<()> {
        let table = self
            .gc
            .tables
            .get_mut(table_ref)
            .ok_or_else(invalid_table_reference)?;
        table.raw_set(key, value, &self.gc.string_arena)?;
        self.gc.barrier_back(table_ref);
        Ok(())
    }

    fn call_tm_two_args(&mut self, tm: Val, arg1: Val, arg2: Val) -> LuaResult<Val> {
        let saved_top = self.top;
        let call_base = self.top;
        self.ensure_stack(call_base + 4);
        self.stack_set(call_base, tm);
        self.stack_set(call_base + 1, arg1);
        self.stack_set(call_base + 2, arg2);
        self.top = call_base + 3;
        self.call_function(call_base, 1)?;
        let result = self.stack_get(call_base);
        self.top = saved_top;
        Ok(result)
    }

    fn call_tm_three_args(&mut self, tm: Val, arg1: Val, arg2: Val, arg3: Val) -> LuaResult<()> {
        let saved_top = self.top;
        let call_base = self.top;
        self.ensure_stack(call_base + 5);
        self.stack_set(call_base, tm);
        self.stack_set(call_base + 1, arg1);
        self.stack_set(call_base + 2, arg2);
        self.stack_set(call_base + 3, arg3);
        self.top = call_base + 4;
        self.call_function(call_base, 0)?;
        self.top = saved_top;
        Ok(())
    }

    /// Check if a value is a string or number (coercible for concatenation).
    fn is_string_or_number(&self, val: Val) -> bool {
        matches!(val, Val::Num(_))
            || matches!(val, Val::Str(r) if self.gc.string_arena.get(r).is_some())
    }

    /// Append the string representation of a value to a buffer.
    fn val_to_string_bytes(&self, val: Val, buffer: &mut Vec<u8>) {
        match val {
            Val::Str(r) => {
                if let Some(s) = self.gc.string_arena.get(r) {
                    buffer.extend_from_slice(s.data());
                }
            }
            Val::Num(n) => append_lua_number_bytes(buffer, n),
            _ => {}
        }
    }

    /// Generate a comparison error (no proto/pc context).
    #[allow(clippy::unused_self)]
    fn compare_error(&self, a: Val, b: Val) -> LuaError {
        LuaError::Runtime(RuntimeError {
            message: format!(
                "attempt to compare {} with {}",
                a.type_name(),
                b.type_name()
            ),
            level: 0,
            traceback: vec![],
        })
    }

    /// Try an order metamethod without proto/pc context.
    fn call_order_tm_api(&mut self, lhs: Val, rhs: Val, event: TMS) -> LuaResult<Option<bool>> {
        let Some(lhs_tm) = lookup_tm(self, lhs, event) else {
            return Ok(None);
        };
        let rhs_tm = lookup_tm(self, rhs, event).unwrap_or(Val::Nil);
        if !val_raw_equal(lhs_tm, rhs_tm, &self.gc.tables, &self.gc.string_arena) {
            return Ok(None);
        }

        let result = self.call_tm_two_args(lhs_tm, lhs, rhs)?;
        Ok(Some(result.is_truthy()))
    }
}

fn lookup_tm(state: &LuaState, value: Val, event: TMS) -> Option<Val> {
    gettmbyobj(
        value,
        event,
        &state.gc.tables,
        &state.gc.string_arena,
        &state.gc.type_metatables,
        &state.gc.tm_names,
        &state.gc.userdata,
    )
}

fn lookup_table_tm(
    state: &LuaState,
    table_ref: GcRef<Table>,
    event: TMS,
) -> LuaResult<Option<Val>> {
    let table = state
        .gc
        .tables
        .get(table_ref)
        .ok_or_else(invalid_table_reference)?;
    match table.metatable() {
        Some(mt_ref) => Ok(fasttm(
            &state.gc.tables,
            &state.gc.string_arena,
            mt_ref,
            event,
            &state.gc.tm_names,
        )),
        None => Ok(None),
    }
}

fn invalid_table_reference() -> LuaError {
    LuaError::Runtime(RuntimeError {
        message: "invalid table reference".into(),
        level: 0,
        traceback: vec![],
    })
}

fn loop_error(operation: &str) -> LuaError {
    LuaError::Runtime(RuntimeError {
        message: format!("loop in {operation}"),
        level: 0,
        traceback: vec![],
    })
}

fn index_error(value: Val) -> LuaError {
    LuaError::Runtime(RuntimeError {
        message: format!("attempt to index a {} value", value.type_name()),
        level: 0,
        traceback: vec![],
    })
}

fn concat_error(lhs: Val, rhs: Val, state: &LuaState) -> LuaError {
    let type_name = if state.is_string_or_number(lhs) {
        rhs.type_name()
    } else {
        lhs.type_name()
    };
    LuaError::Runtime(RuntimeError {
        message: format!("attempt to concatenate a {type_name} value"),
        level: 0,
        traceback: vec![],
    })
}
