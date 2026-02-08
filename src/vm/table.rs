use std::collections::HashMap;

use super::Error;
use super::Markable;
use super::Result;
use super::TypeError;
use super::Val;

#[derive(Debug, Default)]
pub(super) struct Table {
    map: HashMap<Val, Val>,
}

impl Table {
    pub(super) fn get(&self, key: &Val) -> Val {
        match key {
            Val::Nil => Val::Nil,
            Val::Num(n) if n.is_nan() => Val::Nil,
            _ => self.map.get(key).cloned().unwrap_or_default(),
        }
    }

    pub(super) fn insert(&mut self, key: Val, value: Val) -> Result<()> {
        match key {
            Val::Nil => Err(Error::new(TypeError::TableKeyNil, 0, 0)),
            Val::Num(n) if n.is_nan() => Err(Error::new(TypeError::TableKeyNan, 0, 0)),
            _ => {
                if matches!(value, Val::Nil) {
                    self.map.remove(&key);
                } else {
                    self.map.insert(key, value);
                }
                Ok(())
            }
        }
    }

    /// Returns the length of the sequence part of the table (the `#` operator).
    ///
    /// A sequence is `t[1], t[2], ..., t[n]` where `t[n]` is non-nil and
    /// `t[n+1]` is nil. Per Lua 5.1 semantics, returns any valid boundary.
    pub(super) fn sequence_length(&self) -> usize {
        let mut i = 1;
        loop {
            let key = Val::Num(f64::from(i));
            if matches!(self.map.get(&key), None | Some(Val::Nil)) {
                return (i - 1) as usize;
            }
            i += 1;
        }
    }

    /// Implements the `next()` function. Given a key, returns the next
    /// key-value pair in the table. With `Nil` key, returns the first pair.
    /// Returns `(Nil, Nil)` when iteration is complete.
    pub(super) fn next_pair(&self, key: &Val) -> Option<(Val, Val)> {
        if matches!(key, Val::Nil) {
            // Return the first pair
            return self.map.iter().next().map(|(k, v)| (k.clone(), v.clone()));
        }
        // Find the given key, then return the pair after it.
        // HashMap iteration order is arbitrary, which matches Lua's spec.
        let mut found = false;
        for (k, v) in &self.map {
            if found {
                return Some((k.clone(), v.clone()));
            }
            if k == key {
                found = true;
            }
        }
        None
    }
}

impl Markable for Table {
    fn mark_reachable(&self) {
        for (k, v) in &self.map {
            k.mark_reachable();
            v.mark_reachable();
        }
    }
}
