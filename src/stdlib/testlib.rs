//! Test library: internal VM introspection for the PUC-Rio test suite.
//!
//! Registers a global `T` table with functions that expose internal
//! data structures. This is the Rust equivalent of PUC-Rio's `ltests.c`.
//!
//! Functions provided:
//! - `T.querytab(t [,i])` - inspect table array/hash internals
//! - `T.hash(key [,table])` - string hash or main position
//! - `T.int2fb(n)` - float-byte encoding (luaO_int2fb / luaO_fb2int)
//! - `T.log2(n)` - integer log2 (luaO_log2)
//! - `T.listcode(f)` - disassemble function bytecode

use crate::compiler::codegen::int2fb;
use crate::error::LuaResult;
use crate::vm::closure::Closure;
use crate::vm::execute::fb2int;
use crate::vm::instructions::{Instruction, OpMode};
use crate::vm::state::LuaState;
use crate::vm::value::Val;

use super::{arg_error, type_error};

// -------------------------------------------------------------------------
// T.querytab(t [, i])
// -------------------------------------------------------------------------

/// Inspects a table's internal structure.
///
/// With one argument: returns (array_size, hash_size, last_free_index).
/// With two arguments where i < array_size: returns (i, array[i], nil).
/// With two arguments where i >= array_size: returns (key, value, next)
/// for hash node at index (i - array_size).
///
/// Matches PUC-Rio's `table_query` from `ltests.c`.
pub fn t_querytab(state: &mut LuaState) -> LuaResult<u32> {
    let arg0 = if state.base < state.top {
        state.stack_get(state.base)
    } else {
        return Err(type_error(state, 0, "table"));
    };

    let Val::Table(table_ref) = arg0 else {
        return Err(type_error(state, 0, "table"));
    };

    // Optional second argument: index, default -1.
    let i: i32 = if state.base + 1 < state.top {
        match state.stack_get(state.base + 1) {
            Val::Num(n) => n as i32,
            _ => -1,
        }
    } else {
        -1
    };

    // Extract all table data before any mutable borrows.
    let (asize, hsize, last_free, array_val, node_info) = {
        let table = state
            .gc
            .tables
            .get(table_ref)
            .ok_or_else(|| type_error(state, 0, "table"))?;
        let asize = table.array_len();
        let hsize = table.hash_size();
        let last_free = table.last_free_index();

        let array_val = if i >= 0 && (i as usize) < asize {
            Some(table.array_get(i as usize).unwrap_or(Val::Nil))
        } else {
            None
        };

        let node_info = if i >= 0 && (i as usize) >= asize {
            let hash_idx = (i as usize) - asize;
            if (hash_idx as u32) < hsize {
                table.query_node(hash_idx as u32)
            } else {
                None
            }
        } else {
            None
        };

        (asize, hsize, last_free, array_val, node_info)
    };

    state.ensure_stack(3);

    if i == -1 {
        // Return (array_size, hash_size, last_free).
        state.stack_set(state.base, Val::Num(asize as f64));
        state.stack_set(state.base + 1, Val::Num(hsize as f64));
        state.stack_set(state.base + 2, Val::Num(last_free as f64));
    } else if let Some(val) = array_val {
        // Array part query: return (i, array[i], nil).
        state.stack_set(state.base, Val::Num(i as f64));
        state.stack_set(state.base + 1, val);
        state.stack_set(state.base + 2, Val::Nil);
    } else if let Some((key, value, next)) = node_info {
        // Hash part query: return (key, value, next).
        // PUC-Rio logic: show the real key unless value is nil AND
        // key is non-nil AND key is non-number (dead key case).
        // In rilua, free nodes have nil keys so this simplifies.
        let display_key = if !value.is_nil() || key.is_nil() || matches!(key, Val::Num(_)) {
            key
        } else {
            let s = state.gc.intern_string(b"<undef>");
            Val::Str(s)
        };

        state.stack_set(state.base, display_key);
        state.stack_set(state.base + 1, value);
        match next {
            Some(idx) => state.stack_set(state.base + 2, Val::Num(idx as f64)),
            None => state.stack_set(state.base + 2, Val::Nil),
        }
    } else {
        // Index out of range or hash part query with no node.
        state.stack_set(state.base, Val::Nil);
        state.stack_set(state.base + 1, Val::Nil);
        state.stack_set(state.base + 2, Val::Nil);
    }
    state.top = state.base + 3;
    Ok(3)
}

// -------------------------------------------------------------------------
// T.hash(key [, table])
// -------------------------------------------------------------------------

/// Returns a string's hash value (1 arg) or the main position index of
/// a key in a table's hash part (2 args).
///
/// Matches PUC-Rio's `hash_query` from `ltests.c`.
pub fn t_hash(state: &mut LuaState) -> LuaResult<u32> {
    let arg0 = if state.base < state.top {
        state.stack_get(state.base)
    } else {
        return Err(type_error(state, 0, "value"));
    };

    let has_arg2 = state.base + 1 < state.top;

    let result = if !has_arg2 {
        // 1 argument: return the string's hash value.
        let Val::Str(str_ref) = arg0 else {
            return Err(arg_error(state, 1, "string expected"));
        };
        let hash = state.gc.string_arena.get(str_ref).map_or(0, |s| s.hash());
        hash as f64
    } else {
        // 2 arguments: return main_position(key, table).
        let arg1 = state.stack_get(state.base + 1);
        let Val::Table(table_ref) = arg1 else {
            return Err(type_error(state, 1, "table"));
        };
        let table = state
            .gc
            .tables
            .get(table_ref)
            .ok_or_else(|| type_error(state, 1, "table"))?;
        let mp = if table.hash_size() == 0 {
            0
        } else {
            table.main_position(&arg0, &state.gc.string_arena)
        };
        mp as f64
    };

    state.stack_set(state.base, Val::Num(result));
    state.top = state.base + 1;
    Ok(1)
}

// -------------------------------------------------------------------------
// T.int2fb(n)
// -------------------------------------------------------------------------

/// Converts an integer to float-byte encoding and back.
///
/// Returns two values: the encoded byte and the decoded integer.
/// Matches PUC-Rio's `int2fb_aux` from `ltests.c`.
pub fn t_int2fb(state: &mut LuaState) -> LuaResult<u32> {
    let arg0 = if state.base < state.top {
        state.stack_get(state.base)
    } else {
        return Err(type_error(state, 0, "number"));
    };

    let Val::Num(n) = arg0 else {
        return Err(type_error(state, 0, "number"));
    };

    let x = n as u32;
    let encoded = int2fb(x);
    let decoded = fb2int(encoded);

    state.ensure_stack(2);
    state.stack_set(state.base, Val::Num(encoded as f64));
    state.stack_set(state.base + 1, Val::Num(decoded as f64));
    state.top = state.base + 2;
    Ok(2)
}

// -------------------------------------------------------------------------
// T.log2(n)
// -------------------------------------------------------------------------

/// PUC-Rio's luaO_log2: integer log base 2 using a lookup table.
///
/// Matches PUC-Rio's `log2_aux` from `ltests.c`.
pub fn t_log2(state: &mut LuaState) -> LuaResult<u32> {
    let arg0 = if state.base < state.top {
        state.stack_get(state.base)
    } else {
        return Err(type_error(state, 0, "number"));
    };

    let Val::Num(n) = arg0 else {
        return Err(type_error(state, 0, "number"));
    };

    let result = lua_o_log2(n as u32);
    state.stack_set(state.base, Val::Num(result as f64));
    state.top = state.base + 1;
    Ok(1)
}

/// PUC-Rio's `luaO_log2` from `lobject.c`: integer log2 via lookup table.
///
/// The table-based approach matches PUC-Rio exactly, producing identical
/// results for all inputs.
fn lua_o_log2(mut x: u32) -> i32 {
    #[rustfmt::skip]
    const LOG_2: [u8; 256] = [
        0,1,2,2,3,3,3,3,4,4,4,4,4,4,4,4,5,5,5,5,5,5,5,5,5,5,5,5,5,5,5,5,
        6,6,6,6,6,6,6,6,6,6,6,6,6,6,6,6,6,6,6,6,6,6,6,6,6,6,6,6,6,6,6,6,
        7,7,7,7,7,7,7,7,7,7,7,7,7,7,7,7,7,7,7,7,7,7,7,7,7,7,7,7,7,7,7,7,
        7,7,7,7,7,7,7,7,7,7,7,7,7,7,7,7,7,7,7,7,7,7,7,7,7,7,7,7,7,7,7,7,
        8,8,8,8,8,8,8,8,8,8,8,8,8,8,8,8,8,8,8,8,8,8,8,8,8,8,8,8,8,8,8,8,
        8,8,8,8,8,8,8,8,8,8,8,8,8,8,8,8,8,8,8,8,8,8,8,8,8,8,8,8,8,8,8,8,
        8,8,8,8,8,8,8,8,8,8,8,8,8,8,8,8,8,8,8,8,8,8,8,8,8,8,8,8,8,8,8,8,
        8,8,8,8,8,8,8,8,8,8,8,8,8,8,8,8,8,8,8,8,8,8,8,8,8,8,8,8,8,8,8,8,
    ];
    let mut l: i32 = -1;
    while x >= 256 {
        l += 8;
        x >>= 8;
    }
    l + i32::from(LOG_2[x as usize])
}

// -------------------------------------------------------------------------
// T.listcode(f)
// -------------------------------------------------------------------------

/// Disassembles a Lua function's bytecode into a table.
///
/// Returns a table where:
/// - `t.maxstack` = max stack size
/// - `t.numparams` = number of parameters
/// - `t[1]` .. `t[n]` = instruction strings in PUC-Rio's `buildop` format
///
/// Format: `"(%4d) %4d - %-12s%4d %4d %4d"` (iABC)
///     or: `"(%4d) %4d - %-12s%4d %4d"` (iABx/iAsBx)
///
/// Matches PUC-Rio's `listcode` + `buildop` from `ltests.c`.
pub fn t_listcode(state: &mut LuaState) -> LuaResult<u32> {
    let arg0 = if state.base < state.top {
        state.stack_get(state.base)
    } else {
        return Err(arg_error(state, 1, "Lua function expected"));
    };

    // Must be a Lua function (not a Rust closure).
    let Val::Function(func_ref) = arg0 else {
        return Err(arg_error(state, 1, "Lua function expected"));
    };

    let proto = {
        let closure = state
            .gc
            .closures
            .get(func_ref)
            .ok_or_else(|| arg_error(state, 1, "Lua function expected"))?;
        match closure {
            Closure::Lua(lc) => lc.proto.clone(),
            Closure::Rust(_) => {
                return Err(arg_error(state, 1, "Lua function expected"));
            }
        }
    };

    // Create the result table.
    let result_table = state.gc.alloc_table(crate::vm::table::Table::new());

    // Set t.maxstack.
    let maxstack_key = state.gc.intern_string(b"maxstack");
    if let Some(t) = state.gc.tables.get_mut(result_table) {
        t.raw_set(
            Val::Str(maxstack_key),
            Val::Num(f64::from(proto.max_stack_size)),
            &state.gc.string_arena,
        )?;
    }

    // Set t.numparams.
    let numparams_key = state.gc.intern_string(b"numparams");
    if let Some(t) = state.gc.tables.get_mut(result_table) {
        t.raw_set(
            Val::Str(numparams_key),
            Val::Num(f64::from(proto.num_params)),
            &state.gc.string_arena,
        )?;
    }

    // Emit instruction strings: t[pc+1] = buildop(pc).
    for pc in 0..proto.code.len() {
        let instr = Instruction::from_raw(proto.code[pc]);
        let line = proto.line_info.get(pc).copied().unwrap_or(0);
        let op_str = buildop(instr, line, pc);

        let key = Val::Num((pc + 1) as f64);
        let val_str = state.gc.intern_string(op_str.as_bytes());
        let val = Val::Str(val_str);

        if let Some(t) = state.gc.tables.get_mut(result_table) {
            t.raw_set(key, val, &state.gc.string_arena)?;
        }
    }

    state.stack_set(state.base, Val::Table(result_table));
    state.top = state.base + 1;
    Ok(1)
}

/// Formats a single instruction in PUC-Rio's `buildop` format.
///
/// Format:
/// - iABC:  `"(%4d) %4d - %-12s%4d %4d %4d"`
/// - iABx:  `"(%4d) %4d - %-12s%4d %4d"`
/// - iAsBx: `"(%4d) %4d - %-12s%4d %4d"`
fn buildop(instr: Instruction, line: u32, pc: usize) -> String {
    let op = instr.opcode();
    let name = op.name();
    let a = instr.a();

    match op.mode() {
        OpMode::IABC => {
            let b = instr.b();
            let c = instr.c();
            format!("({line:4}) {pc:4} - {name:<12}{a:4} {b:4} {c:4}")
        }
        OpMode::IABx => {
            let bx = instr.bx();
            format!("({line:4}) {pc:4} - {name:<12}{a:4} {bx:4}")
        }
        OpMode::IAsBx => {
            let sbx = instr.sbx();
            format!("({line:4}) {pc:4} - {name:<12}{a:4} {sbx:4}")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vm::instructions::OpCode;

    #[test]
    fn log2_values() {
        assert_eq!(lua_o_log2(0), -1);
        assert_eq!(lua_o_log2(1), 0);
        assert_eq!(lua_o_log2(2), 1);
        assert_eq!(lua_o_log2(3), 1);
        assert_eq!(lua_o_log2(4), 2);
        assert_eq!(lua_o_log2(7), 2);
        assert_eq!(lua_o_log2(8), 3);
        assert_eq!(lua_o_log2(255), 7);
        assert_eq!(lua_o_log2(256), 8);
        assert_eq!(lua_o_log2(1024), 10);
    }

    #[test]
    fn buildop_iabc() {
        let instr = Instruction::abc(OpCode::Move, 1, 2, 0);
        let s = buildop(instr, 5, 0);
        assert!(s.contains("MOVE"));
        assert!(s.starts_with("(   5)    0 - "));
    }

    #[test]
    fn buildop_iabx() {
        let instr = Instruction::a_bx(OpCode::LoadK, 0, 1);
        let s = buildop(instr, 1, 0);
        assert!(s.contains("LOADK"));
    }

    #[test]
    fn buildop_iasbx() {
        let instr = Instruction::a_sbx(OpCode::Jmp, 0, 10);
        let s = buildop(instr, 1, 0);
        assert!(s.contains("JMP"));
        assert!(s.contains("  10"));
    }
}
