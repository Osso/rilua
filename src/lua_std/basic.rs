//! Lua's Standard Library

use crate::LuaType;
use crate::State;
use crate::error::ErrorKind;

pub(crate) fn open_base(state: &mut State) {
    let mut add = |name, func| {
        state.push_rust_fn(func);
        state.set_global(name);
    };

    // Issues an error when the value of its first argument is false; otherwise,
    // returns all its arguments. `message` is an error message; when absent,
    // it defaults to "assertion failed!".
    add("assert", |state| {
        state.check_any(1)?;
        let cond = state.to_boolean(1);
        state.remove(1);
        if cond {
            Ok(state.get_top() as u8)
        } else if state.get_top() == 0 {
            Err(state.error(ErrorKind::AssertionFail))
        } else {
            let s = state.to_string(1);
            Err(state.error(ErrorKind::WithMessage(s)))
        }
    });

    add("ipairs", |state| {
        state.check_type(1, LuaType::Table)?;
        state.set_top(1);
        state.push_rust_fn(|state| {
            state.check_type(1, LuaType::Table)?;
            state.check_type(2, LuaType::Number)?;
            state.set_top(2);
            let old_index = state.to_number(2).unwrap();
            let new_index = old_index + 1.0;
            state.pop(1); // pop the old number
            state.push_number(new_index);
            state.get_table(1)?;
            if state.to_boolean(-1) {
                state.push_number(new_index);
                state.replace(1); // Replaces the table with the index
                Ok(2)
            } else {
                state.set_top(0);
                state.push_nil();
                Ok(1)
            }
        });
        // Swap the table and function
        state.push_value(1);
        state.remove(1);
        // Push the initial index
        state.push_number(0.0);
        Ok(3)
    });

    // Receives any number of arguments, and prints their values to `stdout`.
    add("print", |state| {
        let range = 1..=state.get_top();
        let mut strings = range.map(|i| state.to_string(i as isize));
        if let Some(s) = strings.next() {
            print!("{s}");
            for s in strings {
                print!("\t{s}");
            }
        }
        println!();
        Ok(0)
    });

    // Returns the type of its only argument, coded as a string.
    add("type", |state| {
        state.check_any(1)?;
        let typ = state.typ(1);
        let type_str = typ.to_string();
        state.pop(state.get_top() as isize);
        state.push_string(type_str);
        Ok(1)
    });

    // tonumber(e [, base])
    //
    // Tries to convert its argument to a number. If the argument is already
    // a number or a string convertible to a number, then tonumber returns
    // this number; otherwise, it returns nil.
    //
    // An optional argument specifies the base to interpret the numeral.
    // The base can be any integer between 2 and 36, inclusive. In bases
    // above 10, the letter 'A' (in either upper or lower case) represents
    // 10, 'B' represents 11, and so forth.
    add("tonumber", |state| {
        state.check_any(1)?;
        if state.get_top() >= 2 {
            // tonumber(e, base) — explicit base conversion
            let base_f = state.to_number(2)?;
            #[allow(clippy::cast_possible_truncation)]
            let base = base_f as i64;
            if !(2..=36).contains(&base) {
                return Err(state.error(ErrorKind::WithMessage(
                    "bad argument #2 to 'tonumber' (invalid base)".to_string(),
                )));
            }
            if state.typ(1) != LuaType::String {
                // With explicit base, first arg must be a string
                state.set_top(0);
                state.push_nil();
                return Ok(1);
            }
            #[allow(clippy::cast_sign_loss)]
            let result = state.to_number_base(1, base as u32);
            state.set_top(0);
            match result {
                Some(n) => state.push_number(n),
                None => state.push_nil(),
            }
        } else {
            // tonumber(e) — default base
            let result = state.to_number_opt(1);
            state.set_top(0);
            match result {
                Some(n) => state.push_number(n),
                None => state.push_nil(),
            }
        }
        Ok(1)
    });

    // tostring(e)
    //
    // Receives an argument of any type and converts it to a string in a
    // reasonable format. For complete control of how numbers are converted,
    // use string.format.
    add("tostring", |state| {
        state.check_any(1)?;
        if state.typ(1) == LuaType::String {
            // String stays as-is — just return it
            state.set_top(1);
            return Ok(1);
        }
        let s = state.to_string(1);
        state.set_top(0);
        state.push_string(s);
        Ok(1)
    });

    // unpack(list)
    //
    // Returns list[1], list[2], ... list[#list]. The Lua version can take
    // additional arguments to return only part of the list, but that isn't
    // supported yet.
    add("unpack", |state| {
        state.check_type(1, LuaType::Table)?;
        let mut i = 1.0;
        loop {
            state.push_number(i);
            state.get_table(1)?;
            if state.typ(-1) == LuaType::Nil {
                state.pop(1);
                break;
            } else {
                i += 1.0;
            }
        }
        Ok(i as u8 - 1)
    });
}
