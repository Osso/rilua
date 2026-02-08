//! Functions and types associated with converting source code into bytecode.

mod exp_desc;
mod lexer;
mod parser;
mod token;

use super::Instr;
use super::Result;
use super::error;

/// Describes how a closure captures an outer variable.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct UpvalueDesc {
    /// Name of the captured variable (for debugging).
    pub(super) name: Vec<u8>,
    /// If true, captures a local from the immediately enclosing function.
    /// If false, captures an upvalue from the immediately enclosing function.
    pub(super) is_local: bool,
    /// Index into the parent's locals (if `is_local`) or upvalues (if not).
    pub(super) index: u8,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub(super) struct Chunk {
    pub(super) code: Vec<Instr>,
    pub(super) number_literals: Vec<f64>,
    pub(super) string_literals: Vec<Vec<u8>>,
    pub(super) num_params: u8,
    pub(super) num_locals: u8,
    pub(super) is_vararg: bool,
    pub(super) nested: Vec<Self>,
    pub(super) upvalue_descs: Vec<UpvalueDesc>,
}

pub(super) fn parse_str(source: impl AsRef<str>) -> Result<Chunk> {
    parser::parse_str(source.as_ref())
}
