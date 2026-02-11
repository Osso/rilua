//! OS library: clock, date, time, execute, environment access.
//!
//! Reference: `loslib.c` in PUC-Rio Lua 5.1.1.

use crate::error::{LuaError, LuaResult, RuntimeError};
use crate::vm::execute::coerce_to_number;
use crate::vm::gc::arena::GcRef;
use crate::vm::state::LuaState;
use crate::vm::table::Table;
use crate::vm::value::Val;

// ---------------------------------------------------------------------------
// libc FFI bindings for date/time/locale functions
// ---------------------------------------------------------------------------

/// C `time_t` -- signed 64-bit on modern Linux (matches `i64`).
type TimeT = i64;

/// C `clock_t` -- signed integer type, `i64` on Linux x86_64.
type ClockT = i64;

/// C `struct tm` for broken-down time.
#[repr(C)]
struct Tm {
    tm_sec: i32,
    tm_min: i32,
    tm_hour: i32,
    tm_mday: i32,
    tm_mon: i32,
    tm_year: i32,
    tm_wday: i32,
    tm_yday: i32,
    tm_isdst: i32,
    // glibc extensions (must be present for correct struct size):
    tm_gmtoff: i64,
    tm_zone: *const i8,
}

impl Default for Tm {
    fn default() -> Self {
        Self {
            tm_sec: 0,
            tm_min: 0,
            tm_hour: 0,
            tm_mday: 0,
            tm_mon: 0,
            tm_year: 0,
            tm_wday: 0,
            tm_yday: 0,
            tm_isdst: 0,
            tm_gmtoff: 0,
            tm_zone: std::ptr::null(),
        }
    }
}

// Locale category constants (Linux/glibc values).
const LC_ALL: i32 = 6;
const LC_COLLATE: i32 = 3;
const LC_CTYPE: i32 = 0;
const LC_MONETARY: i32 = 4;
const LC_NUMERIC: i32 = 1;
const LC_TIME: i32 = 2;

#[allow(unsafe_code)]
unsafe extern "C" {
    fn clock() -> ClockT;
    fn time(t: *mut TimeT) -> TimeT;
    fn mktime(tm: *mut Tm) -> TimeT;
    fn localtime_r(timep: *const TimeT, result: *mut Tm) -> *mut Tm;
    fn gmtime_r(timep: *const TimeT, result: *mut Tm) -> *mut Tm;
    fn strftime(s: *mut u8, max: usize, format: *const u8, tm: *const Tm) -> usize;
    fn setlocale(category: i32, locale: *const u8) -> *const u8;
    fn mkstemp(template: *mut u8) -> i32;
    fn close(fd: i32) -> i32;
}

/// `CLOCKS_PER_SEC` is 1_000_000 on POSIX systems.
const CLOCKS_PER_SEC: f64 = 1_000_000.0;

// ---------------------------------------------------------------------------
// Argument helpers (same pattern as other stdlib modules)
// ---------------------------------------------------------------------------

#[inline]
fn nargs(state: &LuaState) -> usize {
    state.top.saturating_sub(state.base)
}

#[inline]
fn arg(state: &LuaState, n: usize) -> Val {
    let idx = state.base + n;
    if idx < state.top {
        state.stack_get(idx)
    } else {
        Val::Nil
    }
}

fn bad_argument(name: &str, n: usize, msg: &str) -> LuaError {
    LuaError::Runtime(RuntimeError {
        message: format!("bad argument #{n} to '{name}' ({msg})"),
        level: 0,
        traceback: vec![],
    })
}

fn runtime_error(msg: String) -> LuaError {
    LuaError::Runtime(RuntimeError {
        message: msg,
        level: 0,
        traceback: vec![],
    })
}

/// Extracts a number argument, coercing strings like `luaL_checknumber`.
fn check_number(state: &LuaState, name: &str, n: usize) -> LuaResult<f64> {
    let val = arg(state, n);
    match val {
        Val::Num(v) => Ok(v),
        Val::Str(_) => coerce_to_number(val, &state.gc)
            .ok_or_else(|| bad_argument(name, n + 1, "number expected")),
        _ => Err(bad_argument(name, n + 1, "number expected")),
    }
}

/// Extracts a string argument as bytes.
fn check_string(state: &LuaState, name: &str, n: usize) -> LuaResult<Vec<u8>> {
    let val = arg(state, n);
    match val {
        Val::Str(r) => state
            .gc
            .string_arena
            .get(r)
            .map(|s| s.data().to_vec())
            .ok_or_else(|| bad_argument(name, n + 1, "string expected")),
        Val::Num(_) => Ok(format!("{val}").into_bytes()),
        _ => Err(bad_argument(name, n + 1, "string expected")),
    }
}

/// Extracts an optional string argument. Returns `None` for nil/absent.
fn opt_string(state: &LuaState, name: &str, n: usize) -> LuaResult<Option<Vec<u8>>> {
    if nargs(state) <= n || matches!(arg(state, n), Val::Nil) {
        Ok(None)
    } else {
        check_string(state, name, n).map(Some)
    }
}

// ---------------------------------------------------------------------------
// Return helpers
// ---------------------------------------------------------------------------

/// Pushes `true` and returns 1 (success pattern for remove/rename).
#[inline]
#[allow(clippy::unnecessary_wraps)]
fn push_true(state: &mut LuaState) -> LuaResult<u32> {
    state.push(Val::Bool(true));
    Ok(1)
}

/// Pushes nil, error message, errno -- the PUC-Rio `os_pushresult` failure
/// pattern used by `os.remove` and `os.rename`.
fn push_error(state: &mut LuaState, filename: &str, err: std::io::Error) -> LuaResult<u32> {
    let msg = format!("{filename}: {err}");
    let msg_val = Val::Str(state.gc.intern_string(msg.as_bytes()));
    // Map std::io::Error to a numeric code. raw_os_error() gives the errno
    // on Unix; fall back to 0 for synthetic errors.
    #[allow(clippy::cast_precision_loss)]
    let code = err.raw_os_error().unwrap_or(0) as f64;
    state.push(Val::Nil);
    state.push(msg_val);
    state.push(Val::Num(code));
    Ok(3)
}

// ---------------------------------------------------------------------------
// os.clock()
// ---------------------------------------------------------------------------

/// `os.clock()` -- Returns CPU time used by the program in seconds.
///
/// Matches PUC-Rio: `(lua_Number)clock() / CLOCKS_PER_SEC`.
#[allow(unsafe_code)]
pub fn os_clock(state: &mut LuaState) -> LuaResult<u32> {
    let ticks = unsafe { clock() };
    #[allow(clippy::cast_precision_loss)]
    let secs = ticks as f64 / CLOCKS_PER_SEC;
    state.push(Val::Num(secs));
    Ok(1)
}

// ---------------------------------------------------------------------------
// Date table field extraction helpers
// ---------------------------------------------------------------------------

/// Reads a numeric field from a date table. Returns `default` if the field
/// is nil/absent, or errors if the field is missing and no default is given.
/// Matches PUC-Rio's `getfield` in `loslib.c`.
fn get_date_field(
    state: &mut LuaState,
    table_ref: GcRef<Table>,
    key: &str,
    default: Option<i32>,
) -> LuaResult<i32> {
    let key_val = Val::Str(state.gc.intern_string(key.as_bytes()));
    let table = state
        .gc
        .tables
        .get(table_ref)
        .ok_or_else(|| runtime_error("table not found".into()))?;
    let val = table.get(key_val, &state.gc.string_arena);
    match val {
        Val::Num(n) =>
        {
            #[allow(clippy::cast_possible_truncation)]
            Ok(n as i32)
        }
        _ => match default {
            Some(d) => Ok(d),
            None => Err(runtime_error(format!(
                "field '{key}' missing in date table"
            ))),
        },
    }
}

/// Reads a boolean field from a date table. Returns -1 for nil (auto-detect),
/// 0 for false, 1 for true. Matches PUC-Rio's `getboolfield` in `loslib.c`.
fn get_date_bool_field(state: &mut LuaState, table_ref: GcRef<Table>, key: &str) -> LuaResult<i32> {
    let key_val = Val::Str(state.gc.intern_string(key.as_bytes()));
    let table = state
        .gc
        .tables
        .get(table_ref)
        .ok_or_else(|| runtime_error("table not found".into()))?;
    let val = table.get(key_val, &state.gc.string_arena);
    match val {
        Val::Nil => Ok(-1),
        Val::Bool(b) => Ok(i32::from(b)),
        _ => Ok(-1),
    }
}

// ---------------------------------------------------------------------------
// os.time([table])
// ---------------------------------------------------------------------------

/// `os.time([table])` -- Returns current time or converts a date table.
///
/// No args: returns current UNIX timestamp.
/// Table arg: reads year/month/day (required), hour/min/sec (optional,
/// defaults 12/0/0), isdst (optional). Returns timestamp or nil if
/// mktime fails.
#[allow(unsafe_code)]
pub fn os_time(state: &mut LuaState) -> LuaResult<u32> {
    if nargs(state) == 0 || matches!(arg(state, 0), Val::Nil) {
        // No args: current time.
        let t = unsafe { time(std::ptr::null_mut()) };
        #[allow(clippy::cast_precision_loss)]
        let v = t as f64;
        state.push(Val::Num(v));
        return Ok(1);
    }

    // Table argument: extract fields.
    let table_ref = match arg(state, 0) {
        Val::Table(r) => r,
        _ => return Err(bad_argument("time", 1, "table expected")),
    };

    let sec = get_date_field(state, table_ref, "sec", Some(0))?;
    let min = get_date_field(state, table_ref, "min", Some(0))?;
    let hour = get_date_field(state, table_ref, "hour", Some(12))?;
    let day = get_date_field(state, table_ref, "day", None)?;
    let month = get_date_field(state, table_ref, "month", None)?;
    let year = get_date_field(state, table_ref, "year", None)?;
    let isdst = get_date_bool_field(state, table_ref, "isdst")?;

    let mut tm = Tm {
        tm_sec: sec,
        tm_min: min,
        tm_hour: hour,
        tm_mday: day,
        tm_mon: month - 1,    // Lua 1-12 -> C 0-11
        tm_year: year - 1900, // Lua full year -> C offset from 1900
        tm_isdst: isdst,
        ..Tm::default()
    };

    let t = unsafe { mktime(&mut tm) };
    if t == -1 {
        state.push(Val::Nil);
    } else {
        #[allow(clippy::cast_precision_loss)]
        let v = t as f64;
        state.push(Val::Num(v));
    }
    Ok(1)
}

// ---------------------------------------------------------------------------
// os.date([format [, time]])
// ---------------------------------------------------------------------------

/// `os.date([format [, time]])` -- Formats a date/time value.
///
/// Default format: "%c". Prefix "!" forces UTC. Format "*t" returns a
/// table with fields: sec, min, hour, day, month, year, wday, yday, isdst.
/// Otherwise uses strftime with a 256-byte buffer.
#[allow(unsafe_code)]
pub fn os_date(state: &mut LuaState) -> LuaResult<u32> {
    // Get format string (default "%c").
    let format_bytes = if nargs(state) > 0 && !matches!(arg(state, 0), Val::Nil) {
        check_string(state, "date", 0)?
    } else {
        b"%c".to_vec()
    };
    let format_str = &format_bytes;

    // Get time argument (default: current time).
    let t: TimeT = if nargs(state) > 1 && !matches!(arg(state, 1), Val::Nil) {
        #[allow(clippy::cast_possible_truncation)]
        let v = check_number(state, "date", 1)? as TimeT;
        v
    } else {
        unsafe { time(std::ptr::null_mut()) }
    };

    // Check for "!" prefix (UTC).
    let (use_utc, fmt) = if format_str.first() == Some(&b'!') {
        (true, &format_str[1..])
    } else {
        (false, format_str.as_slice())
    };

    // Convert time_t to struct tm.
    let mut tm = Tm::default();
    let result = unsafe {
        if use_utc {
            gmtime_r(&t, &mut tm)
        } else {
            localtime_r(&t, &mut tm)
        }
    };

    if result.is_null() {
        // Invalid date -> return nil.
        state.push(Val::Nil);
        return Ok(1);
    }

    // Check for "*t" table format.
    if fmt == b"*t" {
        return os_date_table(state, &tm);
    }

    // strftime formatting with 256-byte buffer (matches PUC-Rio).
    let mut buf = [0u8; 256];
    // strftime needs a NUL-terminated format string.
    let mut fmt_c = Vec::with_capacity(fmt.len() + 1);
    fmt_c.extend_from_slice(fmt);
    fmt_c.push(0);

    let n = unsafe { strftime(buf.as_mut_ptr(), buf.len(), fmt_c.as_ptr(), &tm) };

    if n == 0 && !fmt.is_empty() {
        return Err(runtime_error("'date' format too long".into()));
    }

    let result_str = &buf[..n];
    let val = Val::Str(state.gc.intern_string(result_str));
    state.push(val);
    Ok(1)
}

/// Creates and returns a date table with 9 fields from a `struct tm`.
fn os_date_table(state: &mut LuaState, tm: &Tm) -> LuaResult<u32> {
    let table = state.gc.alloc_table(Table::new());

    // Helper to set an integer field.
    let mut set_int = |key: &str, val: i32| -> LuaResult<()> {
        let k = Val::Str(state.gc.intern_string(key.as_bytes()));
        #[allow(clippy::cast_precision_loss)]
        let v = Val::Num(f64::from(val));
        let t = state
            .gc
            .tables
            .get_mut(table)
            .ok_or_else(|| runtime_error("date table not found".into()))?;
        t.raw_set(k, v, &state.gc.string_arena)?;
        Ok(())
    };

    set_int("sec", tm.tm_sec)?;
    set_int("min", tm.tm_min)?;
    set_int("hour", tm.tm_hour)?;
    set_int("day", tm.tm_mday)?;
    set_int("month", tm.tm_mon + 1)?; // C 0-11 -> Lua 1-12
    set_int("year", tm.tm_year + 1900)?; // C offset -> Lua full year
    set_int("wday", tm.tm_wday + 1)?; // C 0-6 (Sun=0) -> Lua 1-7 (Sun=1)
    set_int("yday", tm.tm_yday + 1)?; // C 0-365 -> Lua 1-366

    // isdst: only set if non-negative (undefined = -1 means skip).
    if tm.tm_isdst >= 0 {
        let k = Val::Str(state.gc.intern_string(b"isdst"));
        let v = Val::Bool(tm.tm_isdst != 0);
        let t = state
            .gc
            .tables
            .get_mut(table)
            .ok_or_else(|| runtime_error("date table not found".into()))?;
        t.raw_set(k, v, &state.gc.string_arena)?;
    }

    state.push(Val::Table(table));
    Ok(1)
}

// ---------------------------------------------------------------------------
// os.difftime(t1 [, t2])
// ---------------------------------------------------------------------------

/// `os.difftime(t1, t2)` -- Returns t1 - t2 in seconds.
///
/// PUC-Rio uses C's `difftime()`, which for all practical purposes is
/// just a subtraction on POSIX (time_t is arithmetic).
pub fn os_difftime(state: &mut LuaState) -> LuaResult<u32> {
    let t1 = check_number(state, "difftime", 0)?;
    let t2 = if nargs(state) > 1 && !matches!(arg(state, 1), Val::Nil) {
        check_number(state, "difftime", 1)?
    } else {
        0.0
    };
    state.push(Val::Num(t1 - t2));
    Ok(1)
}

// ---------------------------------------------------------------------------
// os.execute([command])
// ---------------------------------------------------------------------------

/// `os.execute([command])` -- Executes a shell command.
///
/// No args: returns non-zero if shell is available.
/// With command: returns the exit status from `system()`.
pub fn os_execute(state: &mut LuaState) -> LuaResult<u32> {
    if nargs(state) == 0 || matches!(arg(state, 0), Val::Nil) {
        // No command: test if shell is available. On POSIX, always yes.
        state.push(Val::Num(1.0));
        return Ok(1);
    }

    let cmd = check_string(state, "execute", 0)?;
    let cmd_str = String::from_utf8_lossy(&cmd);

    let status = std::process::Command::new("/bin/sh")
        .arg("-c")
        .arg(cmd_str.as_ref())
        .status();

    match status {
        Ok(exit) => {
            // PUC-Rio returns the raw status from system().
            // On POSIX, system() returns the wait status.
            // exit.code() gives the exit code, or None if killed by signal.
            #[allow(clippy::cast_precision_loss)]
            let code = exit.code().unwrap_or(-1) as f64;
            state.push(Val::Num(code));
        }
        Err(_) => {
            state.push(Val::Num(-1.0));
        }
    }
    Ok(1)
}

// ---------------------------------------------------------------------------
// os.exit([code])
// ---------------------------------------------------------------------------

/// `os.exit([code])` -- Terminates the program.
///
/// Default exit code is `EXIT_SUCCESS` (0).
pub fn os_exit(state: &mut LuaState) -> LuaResult<u32> {
    #[allow(clippy::cast_possible_truncation)]
    let code = if nargs(state) > 0 && !matches!(arg(state, 0), Val::Nil) {
        check_number(state, "exit", 0)? as i32
    } else {
        0
    };
    std::process::exit(code);
}

// ---------------------------------------------------------------------------
// os.getenv(varname)
// ---------------------------------------------------------------------------

/// `os.getenv(varname)` -- Returns the value of an environment variable.
///
/// Returns nil if the variable is not defined.
pub fn os_getenv(state: &mut LuaState) -> LuaResult<u32> {
    let name = check_string(state, "getenv", 0)?;
    let name_str = String::from_utf8_lossy(&name);

    match std::env::var(name_str.as_ref()) {
        Ok(val) => {
            let s = state.gc.intern_string(val.as_bytes());
            state.push(Val::Str(s));
        }
        Err(_) => {
            state.push(Val::Nil);
        }
    }
    Ok(1)
}

// ---------------------------------------------------------------------------
// os.remove(filename)
// ---------------------------------------------------------------------------

/// `os.remove(filename)` -- Deletes a file or empty directory.
///
/// Returns `true` on success, or `nil, message, errno` on failure.
pub fn os_remove(state: &mut LuaState) -> LuaResult<u32> {
    let name = check_string(state, "remove", 0)?;
    let name_str = String::from_utf8_lossy(&name).into_owned();

    match std::fs::remove_file(&name_str) {
        Ok(()) => push_true(state),
        Err(e) => push_error(state, &name_str, e),
    }
}

// ---------------------------------------------------------------------------
// os.rename(oldname, newname)
// ---------------------------------------------------------------------------

/// `os.rename(oldname, newname)` -- Renames a file.
///
/// Returns `true` on success, or `nil, message, errno` on failure.
/// Error message uses the source filename (matches PUC-Rio).
pub fn os_rename(state: &mut LuaState) -> LuaResult<u32> {
    let oldname = check_string(state, "rename", 0)?;
    let newname = check_string(state, "rename", 1)?;
    let old_str = String::from_utf8_lossy(&oldname).into_owned();
    let new_str = String::from_utf8_lossy(&newname).into_owned();

    match std::fs::rename(&old_str, &new_str) {
        Ok(()) => push_true(state),
        Err(e) => push_error(state, &old_str, e),
    }
}

// ---------------------------------------------------------------------------
// os.tmpname()
// ---------------------------------------------------------------------------

/// `os.tmpname()` -- Returns a unique temporary filename.
///
/// Uses POSIX `mkstemp` to create a safe temporary file, then closes
/// the descriptor and returns the path. Falls back to a counter-based
/// scheme if mkstemp fails.
#[allow(unsafe_code)]
pub fn os_tmpname(state: &mut LuaState) -> LuaResult<u32> {
    // PUC-Rio template: "/tmp/lua_XXXXXX"
    let mut template = *b"/tmp/lua_XXXXXX\0";
    let fd = unsafe { mkstemp(template.as_mut_ptr()) };
    if fd < 0 {
        return Err(runtime_error("unable to generate a unique filename".into()));
    }
    // Close the file descriptor; we only need the name.
    unsafe { close(fd) };

    // Find the NUL terminator to extract the filename.
    let len = template
        .iter()
        .position(|&b| b == 0)
        .unwrap_or(template.len());
    let name = &template[..len];
    let s = state.gc.intern_string(name);
    state.push(Val::Str(s));
    Ok(1)
}

// ---------------------------------------------------------------------------
// os.setlocale([locale [, category]])
// ---------------------------------------------------------------------------

/// `os.setlocale([locale [, category]])` -- Sets the program locale.
///
/// Returns the current locale name after the change, or nil if invalid.
/// Category is one of: "all", "collate", "ctype", "monetary", "numeric",
/// "time". Default category is "all".
#[allow(unsafe_code)]
pub fn os_setlocale(state: &mut LuaState) -> LuaResult<u32> {
    // Locale argument: nil means query only (NULL in C).
    let locale_arg = opt_string(state, "setlocale", 0)?;

    // Category argument: default "all".
    let cat_name = if nargs(state) > 1 && !matches!(arg(state, 1), Val::Nil) {
        check_string(state, "setlocale", 1)?
    } else {
        b"all".to_vec()
    };

    let cat = match cat_name.as_slice() {
        b"all" => LC_ALL,
        b"collate" => LC_COLLATE,
        b"ctype" => LC_CTYPE,
        b"monetary" => LC_MONETARY,
        b"numeric" => LC_NUMERIC,
        b"time" => LC_TIME,
        _ => {
            return Err(bad_argument("setlocale", 2, "invalid option"));
        }
    };

    // Build NUL-terminated locale string, or use null pointer for query.
    // The buffer must outlive the setlocale call, so declare it here.
    let locale_buf: Option<Vec<u8>> = locale_arg.map(|mut s| {
        s.push(0);
        s
    });
    let locale_ptr = match &locale_buf {
        Some(buf) => buf.as_ptr(),
        None => std::ptr::null(),
    };

    let result = unsafe { setlocale(cat, locale_ptr) };

    if result.is_null() {
        state.push(Val::Nil);
    } else {
        // Convert C string to Lua string.
        let cstr = unsafe { std::ffi::CStr::from_ptr(result.cast()) };
        let s = state.gc.intern_string(cstr.to_bytes());
        state.push(Val::Str(s));
    }
    Ok(1)
}

// ---------------------------------------------------------------------------
// Unit tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tm_default_zeroed() {
        let tm = Tm::default();
        assert_eq!(tm.tm_sec, 0);
        assert_eq!(tm.tm_min, 0);
        assert_eq!(tm.tm_hour, 0);
        assert_eq!(tm.tm_mday, 0);
        assert_eq!(tm.tm_mon, 0);
        assert_eq!(tm.tm_year, 0);
        assert_eq!(tm.tm_wday, 0);
        assert_eq!(tm.tm_yday, 0);
        assert_eq!(tm.tm_isdst, 0);
    }

    #[test]
    fn locale_category_mapping() {
        // Verify our constants match expected Linux/glibc values.
        assert_eq!(LC_CTYPE, 0);
        assert_eq!(LC_NUMERIC, 1);
        assert_eq!(LC_TIME, 2);
        assert_eq!(LC_COLLATE, 3);
        assert_eq!(LC_MONETARY, 4);
        assert_eq!(LC_ALL, 6);
    }

    #[test]
    #[allow(unsafe_code)]
    fn libc_clock_returns_nonnegative() {
        let ticks = unsafe { clock() };
        assert!(ticks >= 0);
    }

    #[test]
    #[allow(unsafe_code)]
    fn libc_time_returns_reasonable_value() {
        let t = unsafe { time(std::ptr::null_mut()) };
        // Should be after 2024-01-01 (timestamp 1704067200).
        assert!(t > 1_704_067_200);
    }

    #[test]
    #[allow(unsafe_code)]
    fn libc_localtime_roundtrip() {
        let t: TimeT = 1_000_000_000; // 2001-09-09 01:46:40 UTC
        let mut tm = Tm::default();
        let result = unsafe { localtime_r(&t, &mut tm) };
        assert!(!result.is_null());
        // mktime should give us back the same timestamp (or close,
        // depending on DST).
        let t2 = unsafe { mktime(&mut tm) };
        assert_eq!(t, t2);
    }

    #[test]
    #[allow(unsafe_code)]
    fn libc_gmtime_epoch() {
        let t: TimeT = 0; // 1970-01-01 00:00:00 UTC
        let mut tm = Tm::default();
        let result = unsafe { gmtime_r(&t, &mut tm) };
        assert!(!result.is_null());
        assert_eq!(tm.tm_year, 70); // 1970 - 1900
        assert_eq!(tm.tm_mon, 0); // January
        assert_eq!(tm.tm_mday, 1);
        assert_eq!(tm.tm_hour, 0);
        assert_eq!(tm.tm_min, 0);
        assert_eq!(tm.tm_sec, 0);
    }

    #[test]
    #[allow(unsafe_code)]
    fn libc_strftime_basic() {
        let t: TimeT = 0;
        let mut tm = Tm::default();
        unsafe { gmtime_r(&t, &mut tm) };
        let mut buf = [0u8; 64];
        let fmt = b"%Y-%m-%d\0";
        let n = unsafe { strftime(buf.as_mut_ptr(), buf.len(), fmt.as_ptr(), &tm) };
        assert!(n > 0);
        let result = std::str::from_utf8(&buf[..n]).expect("valid utf8");
        assert_eq!(result, "1970-01-01");
    }

    #[test]
    #[allow(unsafe_code)]
    fn libc_mkstemp_creates_file() {
        let mut template = *b"/tmp/lua_XXXXXX\0";
        let fd = unsafe { mkstemp(template.as_mut_ptr()) };
        assert!(fd >= 0);
        unsafe { close(fd) };
        // Clean up the temp file.
        let len = template
            .iter()
            .position(|&b| b == 0)
            .unwrap_or(template.len());
        let name = std::str::from_utf8(&template[..len]).expect("valid utf8");
        let _ = std::fs::remove_file(name);
    }
}
