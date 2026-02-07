use super::Markable;
use super::Result;
use super::State;
use super::Table;
use super::object::{ObjectPtr, StringPtr};

use std::cell::RefCell;
use std::fmt;
use std::hash::{Hash, Hasher};
use std::rc::Rc;

/// Runtime representation of an upvalue (a captured outer variable).
///
/// While the enclosing function is active, the upvalue is "open" and points
/// to a stack slot. When the enclosing scope exits, the upvalue is "closed"
/// and owns the value directly.
///
/// Multiple closures can share the same upvalue via `Rc`, ensuring that
/// mutations to a shared captured variable are visible to all closures.
#[derive(Debug)]
pub(super) struct Upvalue {
    pub(super) state: RefCell<UpvalueState>,
}

#[derive(Debug)]
pub(super) enum UpvalueState {
    /// Points to a stack index (absolute).
    Open(usize),
    /// Owns the value after the enclosing scope has exited.
    Closed(Val),
}

impl Upvalue {
    pub(super) fn new_open(stack_index: usize) -> Rc<Self> {
        Rc::new(Self {
            state: RefCell::new(UpvalueState::Open(stack_index)),
        })
    }

    /// Read the upvalue's current value.
    pub(super) fn get(&self, stack: &[Val]) -> Val {
        match &*self.state.borrow() {
            UpvalueState::Open(idx) => stack[*idx].clone(),
            UpvalueState::Closed(val) => val.clone(),
        }
    }

    /// Write a new value into the upvalue.
    pub(super) fn set(&self, stack: &mut [Val], val: Val) {
        let is_open = match &*self.state.borrow() {
            UpvalueState::Open(idx) => Some(*idx),
            UpvalueState::Closed(_) => None,
        };
        if let Some(idx) = is_open {
            stack[idx] = val;
        } else {
            *self.state.borrow_mut() = UpvalueState::Closed(val);
        }
    }

    /// Transition from open to closed by reading the value from the stack.
    pub(super) fn close(&self, stack: &[Val]) {
        let val = {
            let state = self.state.borrow();
            match &*state {
                UpvalueState::Open(idx) => stack[*idx].clone(),
                UpvalueState::Closed(_) => return, // already closed
            }
        };
        *self.state.borrow_mut() = UpvalueState::Closed(val);
    }
}

impl Markable for Upvalue {
    fn mark_reachable(&self) {
        if let UpvalueState::Closed(val) = &*self.state.borrow() {
            val.mark_reachable();
        }
        // Open upvalues reference stack slots, which are already roots
    }
}

/// Format a number using Lua 5.1.1's `%.14g` convention.
///
/// Lua uses `sprintf(s, "%.14g", n)` from `luaconf.h`. The `%g` format
/// uses scientific notation when the exponent is < -4 or >= precision,
/// otherwise fixed-point. Trailing zeros after the decimal point are
/// stripped.
pub(crate) fn lua_fmt_number(n: f64) -> String {
    // Special cases
    if n.is_nan() {
        return "-nan".to_string();
    }
    if n.is_infinite() {
        return if n > 0.0 {
            "inf".to_string()
        } else {
            "-inf".to_string()
        };
    }

    // C's %.14g: use %e form to find the exponent, then decide format.
    // %g uses %e if exponent < -4 or exponent >= precision (14).
    // Otherwise uses %f style.
    // In both cases, trailing zeros after the decimal point are removed,
    // and the decimal point itself is removed if no digits follow it.
    let precision: usize = 14;

    if n == 0.0 {
        // Preserve sign of negative zero
        return if n.is_sign_negative() {
            "-0".to_string()
        } else {
            "0".to_string()
        };
    }

    // Determine the exponent (base-10)
    let abs_n = n.abs();
    let exp = abs_n.log10().floor() as i32;

    if exp < -4 || exp >= precision as i32 {
        // Use scientific notation: precision-1 digits after decimal in %e,
        // then strip trailing zeros
        let e_digits = precision - 1;
        let mut s = format!("{n:.e_digits$e}");
        // Strip trailing zeros in the mantissa part (before 'e')
        if let Some(e_pos) = s.find('e') {
            let (mantissa, exponent) = s.split_at(e_pos);
            let exponent = exponent.to_string();
            let mantissa = strip_trailing_zeros(mantissa);
            s = format!("{mantissa}{exponent}");
        }
        // C uses e+00 format (at least two digits), Rust uses e0.
        // Normalize exponent to match C: e+01, e-04, etc.
        s = normalize_exponent(&s);
        s
    } else {
        // Use fixed-point notation
        // Number of decimal places = precision - (exp + 1), but at least 0
        let decimal_places = if exp >= 0 {
            precision.saturating_sub((exp + 1) as usize)
        } else {
            precision + (-1 - exp) as usize
        };
        let s = format!("{n:.decimal_places$}");
        strip_trailing_zeros(&s).to_string()
    }
}

/// Strip trailing zeros after a decimal point. If all fractional digits
/// are zeros, remove the decimal point too.
fn strip_trailing_zeros(s: &str) -> &str {
    if !s.contains('.') {
        return s;
    }
    let s = s.trim_end_matches('0');
    s.strip_suffix('.').unwrap_or(s)
}

/// Normalize Rust's scientific notation exponent to C's format.
/// Rust: `1.23e5`, `1.23e-5`; C: `1.23e+05`, `1.23e-05`
fn normalize_exponent(s: &str) -> String {
    if let Some(e_pos) = s.find('e') {
        let (mantissa, exp_part) = s.split_at(e_pos);
        let exp_str = &exp_part[1..]; // skip 'e'
        let (sign, digits) = if let Some(d) = exp_str.strip_prefix('-') {
            ("-", d)
        } else if let Some(d) = exp_str.strip_prefix('+') {
            ("+", d)
        } else {
            ("+", exp_str)
        };
        // Ensure at least two digits
        let exp_num: i32 = digits.parse().unwrap_or(0);
        format!("{mantissa}e{sign}{exp_num:02}")
    } else {
        s.to_string()
    }
}

pub type RustFunc = fn(&mut State) -> Result<u8>;

/// Parse a byte slice to an f64 using Lua 5.1.1 semantics.
///
/// Mirrors `luaO_str2d()` from `lobject.c`:
/// - Leading/trailing ASCII whitespace is trimmed
/// - Decimal numbers parsed via standard float parsing (including
///   scientific notation like `1.5e2`)
/// - Hex integers with `0x`/`0X` prefix parsed via radix-16 integer
///   conversion
/// - Returns `None` on any parse failure
pub(super) fn str_to_number(bytes: &[u8]) -> Option<f64> {
    // Trim ASCII whitespace (matching C's isspace: space, tab, newline,
    // vertical tab, form feed, carriage return)
    let trimmed = trim_ascii_whitespace(bytes);
    if trimmed.is_empty() {
        return None;
    }

    // Must be valid UTF-8 for parsing (numeric chars are always ASCII)
    let s = std::str::from_utf8(trimmed).ok()?;

    // Try standard float parsing first (handles decimal, scientific notation)
    if let Ok(n) = s.parse::<f64>() {
        return Some(n);
    }

    // Try hex integer: 0x or 0X prefix
    let hex = s.strip_prefix("0x").or_else(|| s.strip_prefix("0X"))?;
    if hex.is_empty() {
        return None;
    }
    let n = u64::from_str_radix(hex, 16).ok()?;
    Some(n as f64)
}

/// Parse a string with a given base (2-36) to a number.
///
/// Used by `tonumber(s, base)`. Only accepts strings consisting of digits
/// valid for the given base (plus optional leading/trailing whitespace).
/// Letters can be upper or lower case.
pub(super) fn str_to_number_base(bytes: &[u8], base: u32) -> Option<f64> {
    let trimmed = trim_ascii_whitespace(bytes);
    if trimmed.is_empty() {
        return None;
    }
    let s = std::str::from_utf8(trimmed).ok()?;
    let n = u64::from_str_radix(s, base).ok()?;
    Some(n as f64)
}

fn trim_ascii_whitespace(bytes: &[u8]) -> &[u8] {
    let start = bytes
        .iter()
        .position(|&b| !b.is_ascii_whitespace())
        .unwrap_or(bytes.len());
    let end = bytes
        .iter()
        .rposition(|&b| !b.is_ascii_whitespace())
        .map_or(start, |p| p + 1);
    &bytes[start..end]
}

#[derive(Clone, Default)]
pub(super) enum Val {
    #[default]
    Nil,
    Bool(bool),
    Num(f64),
    Str(StringPtr),
    RustFn(RustFunc),
    Obj(ObjectPtr),
}
use Val::*;

impl Val {
    pub(super) fn as_closure(&self) -> Option<&super::object::LuaClosure> {
        if let Obj(o) = self {
            o.as_closure()
        } else {
            None
        }
    }

    pub(super) fn as_num(&self) -> Option<f64> {
        match self {
            Num(f) => Some(*f),
            _ => None,
        }
    }

    /// Attempt to coerce this value to a number (Lua 5.1.1 semantics).
    ///
    /// Numbers return as-is. Strings are parsed following the same rules
    /// as `luaO_str2d` in the C source: decimal via float parse, hex via
    /// `0x` prefix, leading/trailing whitespace allowed.
    pub(super) fn to_number(&self) -> Option<f64> {
        match self {
            Num(f) => Some(*f),
            Str(s) => str_to_number(s.as_bytes()),
            _ => None,
        }
    }

    /// Returns the raw bytes of a string value. This is the primary accessor
    /// for Lua strings, which are arbitrary byte sequences.
    pub(super) fn as_bytes(&self) -> Option<&[u8]> {
        if let Str(s) = self {
            Some(s.as_bytes())
        } else {
            None
        }
    }

    pub(super) fn as_table(&mut self) -> Option<&mut Table> {
        if let Obj(o) = self {
            o.as_table()
        } else {
            None
        }
    }

    pub(super) fn truthy(&self) -> bool {
        !matches!(self, Nil | Bool(false))
    }

    /// Returns the value's type.
    pub(super) fn typ(&self) -> LuaType {
        match self {
            Nil => LuaType::Nil,
            Bool(_) => LuaType::Boolean,
            Num(_) => LuaType::Number,
            RustFn(_) => LuaType::Function,
            Str(_) => LuaType::String,
            Obj(o) => o.typ(),
        }
    }
}

impl fmt::Debug for Val {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Nil => write!(f, "nil"),
            Bool(b) => b.fmt(f),
            Num(n) => n.fmt(f),
            RustFn(func) => write!(f, "<function: {func:p}>"),
            Obj(o) => o.fmt(f),
            Str(s) => s.fmt(f),
        }
    }
}

impl fmt::Display for Val {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Nil => write!(f, "nil"),
            Bool(b) => b.fmt(f),
            Num(n) => write!(f, "{}", lua_fmt_number(*n)),
            Str(s) => s.fmt(f),
            Obj(o) => o.fmt(f),
            RustFn(func) => write!(f, "function: {func:p}"),
        }
    }
}

/// This is very dangerous, since f64 doesn't implement Eq.
impl Eq for Val {}

impl Hash for Val {
    fn hash<H: Hasher>(&self, hasher: &mut H) {
        match self {
            Nil => (),
            Bool(b) => b.hash(hasher),
            Obj(o) => o.hash(hasher),
            Num(n) => {
                debug_assert!(!n.is_nan(), "Can't hash NaN");
                let mut bits = n.to_bits();
                if bits == 1 << 63 {
                    bits = 0;
                }
                bits.hash(hasher);
            }
            RustFn(func) => {
                let f: *const RustFunc = func;
                f.hash(hasher);
            }
            Str(s) => s.hash(hasher),
        }
    }
}

impl PartialEq for Val {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Nil, Nil) => true,
            (Bool(a), Bool(b)) => a == b,
            (Num(a), Num(b)) => a == b,
            (RustFn(a), RustFn(b)) => {
                let x: *const RustFunc = a;
                let y: *const RustFunc = b;
                x == y
            }
            (Obj(a), Obj(b)) => a == b,
            (Str(a), Str(b)) => StringPtr::eq_physical(a, b),
            _ => false,
        }
    }
}

impl Markable for Val {
    fn mark_reachable(&self) {
        match self {
            Obj(o) => o.mark_reachable(),
            Str(s) => s.mark_reachable(),
            _ => (),
        }
    }
}

#[derive(Debug, Eq, PartialEq)]
pub enum LuaType {
    Nil,
    Boolean,
    Number,
    String,
    Table,
    Function,
}

impl LuaType {
    pub fn as_str(&self) -> &'static str {
        use LuaType::*;
        match self {
            Nil => "nil",
            Boolean => "boolean",
            Number => "number",
            String => "string",
            Table => "table",
            Function => "function",
        }
    }
}

impl fmt::Display for LuaType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.as_str().fmt(f)
    }
}
