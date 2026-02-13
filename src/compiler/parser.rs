//! Parser: transforms a token stream into an AST.
//!
//! Recursive descent parser following PUC-Rio's `lparser.c` structure.
//! Uses Pratt parsing for expressions with operator precedence.
//! Error messages match PUC-Rio's format for compatibility.

use crate::error::{LuaError, LuaResult, SyntaxError};

use super::ast::{BinOp, Block, Expr, FuncBody, FuncName, Stat, TableField, UnOp};
use super::lexer::Lexer;
use super::token::{Span, Token};

/// Parser state.
pub struct Parser {
    /// Lexer providing the token stream.
    lexer: Lexer,
    /// Current token.
    current: Token,
    /// Span of the current token.
    span: Span,
    /// Line of the last consumed token (PUC-Rio: `ls->lastline`).
    /// Set in `advance()` before reading the next token.
    lastline: u32,
    /// Nesting depth of loop constructs (while, repeat, for).
    /// Used to validate that `break` only appears inside a loop.
    loop_depth: u32,
}

impl Parser {
    /// Creates a new parser for the given source bytes.
    pub fn new(source: &[u8], name: &str) -> LuaResult<Self> {
        let mut lexer = Lexer::new(source, name);
        let (current, span) = lexer.next()?;
        Ok(Self {
            lexer,
            current,
            span,
            lastline: 1,
            loop_depth: 0,
        })
    }

    // -- Token helpers --

    /// Advances to the next token, returning the previous one.
    fn advance(&mut self) -> LuaResult<(Token, Span)> {
        let prev = (self.current.clone(), self.span);
        // PUC-Rio: ls->lastline = ls->linenumber (before reading next token)
        self.lastline = self.span.line;
        let (tok, span) = self.lexer.next()?;
        self.current = tok;
        self.span = span;
        Ok(prev)
    }

    /// Returns true if the current token matches the expected token.
    fn check(&self, expected: &Token) -> bool {
        self.current == *expected
    }

    /// Returns true if the current token is a `Char` with the given byte.
    fn check_char(&self, ch: u8) -> bool {
        self.current == Token::Char(ch)
    }

    /// Consumes the current token if it matches, returns true if consumed.
    fn test_next(&mut self, expected: &Token) -> LuaResult<bool> {
        if self.check(expected) {
            self.advance()?;
            Ok(true)
        } else {
            Ok(false)
        }
    }

    /// Consumes the current token if it's a `Char` matching `ch`.
    fn test_next_char(&mut self, ch: u8) -> LuaResult<bool> {
        self.test_next(&Token::Char(ch))
    }

    /// Expects the current token to match, or returns an error.
    fn expect(&mut self, expected: &Token) -> LuaResult<Span> {
        if self.check(expected) {
            let span = self.span;
            self.advance()?;
            Ok(span)
        } else {
            Err(self.error_expected(&expected.token2str()))
        }
    }

    /// Expects a `Char` token with the given byte.
    fn expect_char(&mut self, ch: u8) -> LuaResult<Span> {
        self.expect(&Token::Char(ch))
    }

    /// Expects a `Name` token and returns the name string.
    fn expect_name(&mut self) -> LuaResult<(String, Span)> {
        let span = self.span;
        match &self.current {
            Token::Name(_) => {
                let (tok, _) = self.advance()?;
                if let Token::Name(name) = tok {
                    Ok((name, span))
                } else {
                    // Unreachable: we checked above
                    Err(self.error_expected("<name>"))
                }
            }
            _ => Err(self.error_expected("<name>")),
        }
    }

    /// Expects a closing delimiter and produces PUC-Rio style error messages.
    ///
    /// If the opening and closing tokens are on the same line, emits a simple
    /// `"'X' expected"` error. Otherwise, emits
    /// `"'X' expected (to close 'Y' at line N)"`.
    fn check_match(&mut self, close: &Token, open: &Token, open_line: u32) -> LuaResult<()> {
        if !self.check(close) {
            if open_line == self.lexer.line() {
                return Err(self.error_expected(&close.token2str()));
            }
            // PUC-Rio's check_match calls luaX_syntaxerror which always
            // appends "near <token>". We must include this for the REPL's
            // incomplete chunk detection (checks for "<eof>" at end).
            return Err(self.syntax_error_near(&format!(
                "'{}' expected (to close '{}' at line {open_line})",
                close.token2str(),
                open.token2str(),
            )));
        }
        self.advance()?;
        Ok(())
    }

    // -- Error helpers --

    /// Creates a syntax error without a `near` token suffix.
    /// Use `syntax_error_near` for PUC-Rio's `luaX_syntaxerror` equivalent.
    fn syntax_error(&self, msg: &str) -> LuaError {
        LuaError::Syntax(SyntaxError {
            message: msg.to_string(),
            source: self.lexer.source_name().to_string(),
            line: self.span.line,
        })
    }

    /// Creates a syntax error with `near '<current_token>'` appended.
    /// Matches PUC-Rio's `luaX_syntaxerror` -> `luaX_lexerror(msg, token)`.
    fn syntax_error_near(&self, msg: &str) -> LuaError {
        let near = self.current.txt_token();
        self.syntax_error(&format!("{msg} near {near}"))
    }

    fn error_expected(&self, what: &str) -> LuaError {
        // PUC-Rio: luaO_pushfstring(LUA_QS " expected", token2str(token))
        // then luaX_syntaxerror adds "near" LUA_QS with txtToken.
        self.syntax_error_near(&format!("'{what}' expected"))
    }

    // -- Parsing entry point --

    /// Parses a complete Lua chunk (the top-level block).
    pub fn parse_chunk(&mut self) -> LuaResult<Block> {
        let block = self.parse_block()?;
        if !self.check(&Token::Eos) {
            return Err(self.error_expected("<eof>"));
        }
        Ok(block)
    }

    /// Parses a block (sequence of statements).
    fn parse_block(&mut self) -> LuaResult<Block> {
        // chunk -> { stat [`;'] }
        // Lua 5.1: semicolons are optional separators after statements,
        // NOT empty statements. PUC-Rio: testnext(ls, ';') only after stat.
        let mut stmts = Vec::new();
        loop {
            if self.current.is_block_follow() {
                break;
            }

            let stmt = self.parse_stat()?;
            let is_last = matches!(stmt, Stat::Return { .. } | Stat::Break { .. });
            stmts.push(stmt);

            // Optional semicolon after statement
            self.test_next_char(b';')?;

            if is_last {
                break;
            }
        }
        Ok(stmts)
    }

    // -- Statement parsing --

    fn parse_stat(&mut self) -> LuaResult<Stat> {
        let span = self.span;
        match &self.current {
            Token::If => self.parse_if(span),
            Token::While => self.parse_while(span),
            Token::Do => self.parse_do(span),
            Token::For => self.parse_for(span),
            Token::Repeat => self.parse_repeat(span),
            Token::Function => self.parse_func_decl(span),
            Token::Local => self.parse_local(span),
            Token::Return => self.parse_return(span),
            Token::Break => self.parse_break(span),
            _ => self.parse_expr_stat(span),
        }
    }

    fn parse_if(&mut self, span: Span) -> LuaResult<Stat> {
        // if expr then block {elseif expr then block} [else block] end
        self.advance()?; // consume 'if'
        let open_line = span.line;

        let mut conditions = Vec::new();
        let mut bodies = Vec::new();

        // First condition + body
        conditions.push(self.parse_expr()?);
        self.expect(&Token::Then)?;
        bodies.push(self.parse_block()?);

        // elseif chains
        while self.check(&Token::ElseIf) {
            self.advance()?; // consume 'elseif'
            conditions.push(self.parse_expr()?);
            self.expect(&Token::Then)?;
            bodies.push(self.parse_block()?);
        }

        // Optional else
        let else_body = if self.test_next(&Token::Else)? {
            Some(self.parse_block()?)
        } else {
            None
        };

        self.check_match(&Token::End, &Token::If, open_line)?;

        Ok(Stat::If {
            conditions,
            bodies,
            else_body,
            span,
        })
    }

    fn parse_while(&mut self, span: Span) -> LuaResult<Stat> {
        // while expr do block end
        self.advance()?; // consume 'while'
        let open_line = span.line;

        let condition = self.parse_expr()?;
        self.expect(&Token::Do)?;
        self.loop_depth += 1;
        let body = self.parse_block()?;
        self.loop_depth -= 1;
        self.check_match(&Token::End, &Token::While, open_line)?;

        Ok(Stat::While {
            condition,
            body,
            span,
        })
    }

    fn parse_do(&mut self, span: Span) -> LuaResult<Stat> {
        // do block end
        self.advance()?; // consume 'do'
        let open_line = span.line;

        let body = self.parse_block()?;
        self.check_match(&Token::End, &Token::Do, open_line)?;

        Ok(Stat::Do { body, span })
    }

    fn parse_for(&mut self, span: Span) -> LuaResult<Stat> {
        // for Name '=' ... (numeric) or for namelist in ... (generic)
        self.advance()?; // consume 'for'
        let open_line = span.line;

        let (name, _) = self.expect_name()?;

        match &self.current {
            Token::Char(b'=') => self.parse_numeric_for(name, open_line, span),
            Token::Char(b',') | Token::In => self.parse_generic_for(name, open_line, span),
            _ => Err(self.syntax_error_near("'=' or 'in' expected")),
        }
    }

    fn parse_numeric_for(&mut self, name: String, open_line: u32, span: Span) -> LuaResult<Stat> {
        // for name = start, stop [, step] do block end
        self.expect_char(b'=')?;
        let start = self.parse_expr()?;
        self.expect_char(b',')?;
        let stop = self.parse_expr()?;
        let step = if self.test_next_char(b',')? {
            Some(self.parse_expr()?)
        } else {
            None
        };
        self.expect(&Token::Do)?;
        self.loop_depth += 1;
        let body = self.parse_block()?;
        self.loop_depth -= 1;
        self.check_match(&Token::End, &Token::For, open_line)?;

        Ok(Stat::NumericFor {
            name,
            start,
            stop,
            step,
            body,
            span,
        })
    }

    fn parse_generic_for(
        &mut self,
        first_name: String,
        open_line: u32,
        span: Span,
    ) -> LuaResult<Stat> {
        // for namelist in explist do block end
        let mut names = vec![first_name];
        while self.test_next_char(b',')? {
            let (name, _) = self.expect_name()?;
            names.push(name);
        }
        self.expect(&Token::In)?;
        let iterators = self.parse_expr_list()?;
        self.expect(&Token::Do)?;
        self.loop_depth += 1;
        let body = self.parse_block()?;
        self.loop_depth -= 1;
        self.check_match(&Token::End, &Token::For, open_line)?;

        Ok(Stat::GenericFor {
            names,
            iterators,
            body,
            span,
        })
    }

    fn parse_repeat(&mut self, span: Span) -> LuaResult<Stat> {
        // repeat block until expr
        self.advance()?; // consume 'repeat'
        let open_line = span.line;

        self.loop_depth += 1;
        let body = self.parse_block()?;
        self.check_match(&Token::Until, &Token::Repeat, open_line)?;
        let condition = self.parse_expr()?;
        self.loop_depth -= 1;

        Ok(Stat::Repeat {
            body,
            condition,
            span,
        })
    }

    fn parse_func_decl(&mut self, span: Span) -> LuaResult<Stat> {
        // function funcname funcbody
        self.advance()?; // consume 'function'

        let name = self.parse_func_name()?;
        let body = self.parse_func_body(span)?;

        Ok(Stat::FuncDecl { name, body, span })
    }

    fn parse_func_name(&mut self) -> LuaResult<FuncName> {
        let span = self.span;
        let (first, _) = self.expect_name()?;
        let mut parts = vec![first];

        // foo.bar.baz
        while self.test_next_char(b'.')? {
            let (name, _) = self.expect_name()?;
            parts.push(name);
        }

        // Optional :method
        let method = if self.test_next_char(b':')? {
            let (name, _) = self.expect_name()?;
            Some(name)
        } else {
            None
        };

        Ok(FuncName {
            parts,
            method,
            span,
        })
    }

    fn parse_local(&mut self, span: Span) -> LuaResult<Stat> {
        // local function Name funcbody
        // local namelist ['=' explist]
        self.advance()?; // consume 'local'

        if self.test_next(&Token::Function)? {
            let (name, _) = self.expect_name()?;
            let body = self.parse_func_body(span)?;
            return Ok(Stat::LocalFunc { name, body, span });
        }

        // local namelist ['=' explist]
        let (first_name, _) = self.expect_name()?;
        let mut names = vec![first_name];
        while self.test_next_char(b',')? {
            let (name, _) = self.expect_name()?;
            names.push(name);
        }

        let values = if self.test_next_char(b'=')? {
            self.parse_expr_list()?
        } else {
            Vec::new()
        };

        Ok(Stat::LocalDecl {
            names,
            values,
            span,
        })
    }

    fn parse_return(&mut self, span: Span) -> LuaResult<Stat> {
        // stat -> RETURN explist
        // PUC-Rio's retstat does NOT consume a trailing ';'.
        // The single optional ';' is consumed by chunk() after the stat.
        self.advance()?; // consume 'return'

        let values = if self.current.is_block_follow() || self.check_char(b';') {
            Vec::new()
        } else {
            self.parse_expr_list()?
        };

        Ok(Stat::Return { values, span })
    }

    fn parse_break(&mut self, span: Span) -> LuaResult<Stat> {
        self.advance()?; // consume 'break'
        if self.loop_depth == 0 {
            // PUC-Rio's breakstat validates that break appears inside a loop.
            // The error includes "near <current_token>" matching luaX_syntaxerror.
            return Err(self.syntax_error_near("no loop to break"));
        }
        Ok(Stat::Break { span })
    }

    fn parse_expr_stat(&mut self, span: Span) -> LuaResult<Stat> {
        // Assignment or function call
        let expr = self.parse_suffixed_expr()?;

        if self.check_char(b'=') || self.check_char(b',') {
            // Assignment: target1, target2, ... = expr1, expr2, ...
            let mut targets = vec![expr];
            while self.test_next_char(b',')? {
                targets.push(self.parse_suffixed_expr()?);
            }
            self.expect_char(b'=')?;
            let values = self.parse_expr_list()?;
            Ok(Stat::Assign {
                targets,
                values,
                span,
            })
        } else {
            // Function call used as statement.
            // PUC-Rio's exprstat only accepts calls; anything else
            // (bare name, literal, binop, etc.) is a syntax error.
            match &expr {
                super::ast::Expr::Call { .. } | super::ast::Expr::MethodCall { .. } => {
                    Ok(Stat::ExprStat { expr, span })
                }
                _ => Err(self.error_expected("=")),
            }
        }
    }

    // -- Expression parsing --
    // Full Pratt parsing is in chunk 2f. This provides the minimal
    // infrastructure needed for statement parsing.

    /// Parses a comma-separated list of one or more expressions.
    fn parse_expr_list(&mut self) -> LuaResult<Vec<Expr>> {
        let mut exprs = vec![self.parse_expr()?];
        while self.test_next_char(b',')? {
            exprs.push(self.parse_expr()?);
        }
        Ok(exprs)
    }

    /// Parses an expression using Pratt parsing.
    pub(crate) fn parse_expr(&mut self) -> LuaResult<Expr> {
        self.parse_sub_expr(0)
    }

    /// Pratt parser: parse sub-expression with minimum precedence `limit`.
    fn parse_sub_expr(&mut self, limit: u8) -> LuaResult<Expr> {
        let span = self.span;

        // Unary prefix operators
        let mut expr = if let Some(op) = self.get_unary_op() {
            self.advance()?;
            let operand = self.parse_sub_expr(UNARY_PRIORITY)?;
            Expr::UnOp {
                op,
                operand: Box::new(operand),
                span,
            }
        } else {
            self.parse_simple_expr()?
        };

        // Binary operators (loop while priority > limit)
        while let Some(op) = self.get_binary_op() {
            let (left_prio, right_prio) = binary_priority(op);
            if left_prio <= limit {
                break;
            }
            self.advance()?; // consume operator
            let right = self.parse_sub_expr(right_prio)?;
            let new_span = span;
            expr = Expr::BinOp {
                op,
                left: Box::new(expr),
                right: Box::new(right),
                span: new_span,
            };
        }

        Ok(expr)
    }

    /// Parse a simple (non-operator) expression.
    fn parse_simple_expr(&mut self) -> LuaResult<Expr> {
        let span = self.span;
        match &self.current {
            Token::Number(_) => {
                let (tok, _) = self.advance()?;
                if let Token::Number(n) = tok {
                    Ok(Expr::Number(n, span))
                } else {
                    Err(self.syntax_error("unexpected token"))
                }
            }
            Token::Str(_) => {
                let (tok, _) = self.advance()?;
                if let Token::Str(s) = tok {
                    Ok(Expr::Str(s, span))
                } else {
                    Err(self.syntax_error("unexpected token"))
                }
            }
            Token::Nil => {
                self.advance()?;
                Ok(Expr::Nil(span))
            }
            Token::True => {
                self.advance()?;
                Ok(Expr::True(span))
            }
            Token::False => {
                self.advance()?;
                Ok(Expr::False(span))
            }
            Token::Dots => {
                self.advance()?;
                Ok(Expr::VarArg(span))
            }
            Token::Function => {
                self.advance()?; // consume 'function'
                let body = self.parse_func_body(span)?;
                Ok(Expr::FuncDef { body, span })
            }
            Token::Char(b'{') => self.parse_table_ctor(),
            _ => self.parse_suffixed_expr(),
        }
    }

    /// Parse primary expression with suffix chain (calls, indexing, fields).
    fn parse_suffixed_expr(&mut self) -> LuaResult<Expr> {
        let mut expr = self.parse_primary_expr()?;

        loop {
            match &self.current {
                Token::Char(b'.') => {
                    // field access: expr.name
                    self.advance()?;
                    let (field, _) = self.expect_name()?;
                    let span = expr.span();
                    expr = Expr::Field {
                        table: Box::new(expr),
                        field,
                        span,
                    };
                }
                Token::Char(b'[') => {
                    // index: expr[key]
                    let span = expr.span();
                    self.advance()?;
                    let key = self.parse_expr()?;
                    self.expect_char(b']')?;
                    expr = Expr::Index {
                        table: Box::new(expr),
                        key: Box::new(key),
                        span,
                    };
                }
                Token::Char(b':') => {
                    // method call: expr:name(args)
                    let span = expr.span();
                    self.advance()?;
                    let (method, _) = self.expect_name()?;
                    let args = self.parse_func_args()?;
                    expr = Expr::MethodCall {
                        table: Box::new(expr),
                        method,
                        args,
                        span,
                    };
                }
                Token::Char(b'(') | Token::Str(_) | Token::Char(b'{') => {
                    // function call: expr(args) or expr"str" or expr{table}
                    let span = expr.span();
                    let args = self.parse_func_args()?;
                    expr = Expr::Call {
                        func: Box::new(expr),
                        args,
                        span,
                    };
                }
                _ => break,
            }
        }

        Ok(expr)
    }

    /// Parse a primary expression: Name or '(' expr ')'.
    fn parse_primary_expr(&mut self) -> LuaResult<Expr> {
        let span = self.span;
        match &self.current {
            Token::Name(_) => {
                let (tok, _) = self.advance()?;
                if let Token::Name(name) = tok {
                    Ok(Expr::Name(name, span))
                } else {
                    Err(self.syntax_error("unexpected token"))
                }
            }
            Token::Char(b'(') => {
                let paren_span = self.span;
                self.advance()?;
                let expr = self.parse_expr()?;
                self.expect_char(b')')?;
                Ok(Expr::Paren(Box::new(expr), paren_span))
            }
            _ => Err(self.syntax_error_near("unexpected symbol")),
        }
    }

    /// Parse function arguments: '(' [exprlist] ')' | tableconstructor | string
    fn parse_func_args(&mut self) -> LuaResult<Vec<Expr>> {
        match &self.current {
            Token::Char(b'(') => {
                // PUC-Rio: if (line != ls->lastline)
                //   luaX_syntaxerror("ambiguous syntax (function call x new statement)")
                if self.span.line != self.lastline {
                    return Err(
                        self.syntax_error_near("ambiguous syntax (function call x new statement)")
                    );
                }
                let open_line = self.span.line;
                self.advance()?;
                let args = if self.check_char(b')') {
                    Vec::new()
                } else {
                    self.parse_expr_list()?
                };
                self.check_match(&Token::Char(b')'), &Token::Char(b'('), open_line)?;
                Ok(args)
            }
            Token::Char(b'{') => {
                let table = self.parse_table_ctor()?;
                Ok(vec![table])
            }
            Token::Str(_) => {
                let span = self.span;
                let (tok, _) = self.advance()?;
                if let Token::Str(s) = tok {
                    Ok(vec![Expr::Str(s, span)])
                } else {
                    Err(self.syntax_error("unexpected token"))
                }
            }
            _ => Err(self.syntax_error_near("function arguments expected")),
        }
    }

    /// Parse function body: '(' [parlist] ')' block 'end'
    fn parse_func_body(&mut self, def_span: Span) -> LuaResult<FuncBody> {
        let open_line = self.span.line;
        self.expect_char(b'(')?;

        let mut params = Vec::new();
        let mut has_varargs = false;

        if !self.check_char(b')') {
            loop {
                match &self.current {
                    Token::Name(_) => {
                        let (name, _) = self.expect_name()?;
                        params.push(name);
                    }
                    Token::Dots => {
                        self.advance()?;
                        has_varargs = true;
                        break;
                    }
                    _ => {
                        // PUC-Rio: "<name> or '...' expected" via luaX_syntaxerror
                        return Err(self.syntax_error_near("<name> or '...' expected"));
                    }
                }
                if !self.test_next_char(b',')? {
                    break;
                }
            }
        }

        self.expect_char(b')')?;
        // Reset loop depth inside function bodies -- break can't cross
        // function boundaries (PUC-Rio creates a new FuncState).
        let saved_loop_depth = self.loop_depth;
        self.loop_depth = 0;
        let body = self.parse_block()?;
        self.loop_depth = saved_loop_depth;
        // Capture the `end` keyword's line before consuming it.
        // PUC-Rio: f->lastlinedefined = ls->linenumber (at `end`)
        let end_line = self.span.line;
        self.check_match(&Token::End, &Token::Function, open_line)?;

        Ok(FuncBody {
            params,
            has_varargs,
            body,
            span: def_span,
            end_line,
        })
    }

    /// Parse table constructor: '{' [fieldlist] '}'
    fn parse_table_ctor(&mut self) -> LuaResult<Expr> {
        let span = self.span;
        let open_line = span.line;
        self.expect_char(b'{')?;

        let mut fields = Vec::new();

        while !self.check_char(b'}') {
            let field = self.parse_field()?;
            fields.push(field);

            // Field separator: ',' or ';'
            if !self.test_next_char(b',')? && !self.test_next_char(b';')? {
                break;
            }
        }

        self.check_match(&Token::Char(b'}'), &Token::Char(b'{'), open_line)?;

        Ok(Expr::TableCtor { fields, span })
    }

    /// Parse a single table field.
    fn parse_field(&mut self) -> LuaResult<TableField> {
        let span = self.span;
        match &self.current {
            // [expr] = expr
            Token::Char(b'[') => {
                self.advance()?;
                let key = self.parse_expr()?;
                self.expect_char(b']')?;
                self.expect_char(b'=')?;
                let value = self.parse_expr()?;
                Ok(TableField::IndexField { key, value, span })
            }
            // name = expr (need lookahead to distinguish from positional expr)
            Token::Name(_) => {
                // Check if this is name = expr or just expr
                if self.lexer.lookahead()? == &Token::Char(b'=') {
                    let (name, _) = self.expect_name()?;
                    self.expect_char(b'=')?;
                    let value = self.parse_expr()?;
                    Ok(TableField::NameField { name, value, span })
                } else {
                    let value = self.parse_expr()?;
                    Ok(TableField::ValueField { value, span })
                }
            }
            // Positional value
            _ => {
                let value = self.parse_expr()?;
                Ok(TableField::ValueField { value, span })
            }
        }
    }

    // -- Operator helpers --

    fn get_unary_op(&self) -> Option<UnOp> {
        match &self.current {
            Token::Not => Some(UnOp::Not),
            Token::Char(b'-') => Some(UnOp::Neg),
            Token::Char(b'#') => Some(UnOp::Len),
            _ => None,
        }
    }

    fn get_binary_op(&self) -> Option<BinOp> {
        match &self.current {
            Token::Char(b'+') => Some(BinOp::Add),
            Token::Char(b'-') => Some(BinOp::Sub),
            Token::Char(b'*') => Some(BinOp::Mul),
            Token::Char(b'/') => Some(BinOp::Div),
            Token::Char(b'%') => Some(BinOp::Mod),
            Token::Char(b'^') => Some(BinOp::Pow),
            Token::Concat => Some(BinOp::Concat),
            Token::Ne => Some(BinOp::Ne),
            Token::Eq => Some(BinOp::Eq),
            Token::Char(b'<') => Some(BinOp::Lt),
            Token::Le => Some(BinOp::Le),
            Token::Char(b'>') => Some(BinOp::Gt),
            Token::Ge => Some(BinOp::Ge),
            Token::And => Some(BinOp::And),
            Token::Or => Some(BinOp::Or),
            _ => None,
        }
    }
}

/// Unary operator priority (higher than all binary operators).
const UNARY_PRIORITY: u8 = 8;

/// Returns (left priority, right priority) for a binary operator.
///
/// Right-associative operators (`..` and `^`) have right < left.
/// PUC-Rio priority table from `lparser.c`.
fn binary_priority(op: BinOp) -> (u8, u8) {
    match op {
        BinOp::Or => (1, 1),
        BinOp::And => (2, 2),
        BinOp::Lt | BinOp::Gt | BinOp::Le | BinOp::Ge | BinOp::Ne | BinOp::Eq => (3, 3),
        BinOp::Concat => (5, 4), // right-associative
        BinOp::Add | BinOp::Sub => (6, 6),
        BinOp::Mul | BinOp::Div | BinOp::Mod => (7, 7),
        BinOp::Pow => (10, 9), // right-associative
    }
}

/// Parses a Lua source string into an AST block.
pub fn parse(source: &[u8], name: &str) -> LuaResult<Block> {
    let mut parser = Parser::new(source, name)?;
    parser.parse_chunk()
}

#[cfg(test)]
mod tests {
    use super::*;

    // -- Helper --

    fn parse_ok(source: &str) -> Block {
        parse(source.as_bytes(), "test").unwrap()
    }

    fn parse_err(source: &str) -> String {
        parse(source.as_bytes(), "test").unwrap_err().to_string()
    }

    // -- Block and empty program --

    #[test]
    fn empty_program() {
        let block = parse_ok("");
        assert!(block.is_empty());
    }

    #[test]
    fn semicolons_only_is_error() {
        // Lua 5.1: semicolons are optional separators after statements,
        // NOT empty statements. ";;;" alone is a syntax error.
        let err = parse_err(";;;");
        assert!(err.contains("expected"), "error: {err}");
    }

    // -- Do block --

    #[test]
    fn do_end() {
        let block = parse_ok("do end");
        assert_eq!(block.len(), 1);
        assert!(matches!(block[0], Stat::Do { .. }));
    }

    #[test]
    fn nested_do() {
        let block = parse_ok("do do end end");
        assert_eq!(block.len(), 1);
        if let Stat::Do { body, .. } = &block[0] {
            assert_eq!(body.len(), 1);
            assert!(matches!(body[0], Stat::Do { .. }));
        } else {
            panic!("expected Do");
        }
    }

    // -- While --

    #[test]
    fn while_loop() {
        let block = parse_ok("while true do end");
        assert_eq!(block.len(), 1);
        if let Stat::While {
            condition, body, ..
        } = &block[0]
        {
            assert!(matches!(condition, Expr::True(_)));
            assert!(body.is_empty());
        } else {
            panic!("expected While");
        }
    }

    // -- Repeat --

    #[test]
    fn repeat_until() {
        let block = parse_ok("repeat until false");
        assert_eq!(block.len(), 1);
        if let Stat::Repeat {
            condition, body, ..
        } = &block[0]
        {
            assert!(matches!(condition, Expr::False(_)));
            assert!(body.is_empty());
        } else {
            panic!("expected Repeat");
        }
    }

    // -- If --

    #[test]
    fn if_then_end() {
        let block = parse_ok("if true then end");
        assert_eq!(block.len(), 1);
        if let Stat::If {
            conditions,
            bodies,
            else_body,
            ..
        } = &block[0]
        {
            assert_eq!(conditions.len(), 1);
            assert_eq!(bodies.len(), 1);
            assert!(else_body.is_none());
        } else {
            panic!("expected If");
        }
    }

    #[test]
    fn if_elseif_else() {
        let block = parse_ok("if true then elseif false then else end");
        if let Stat::If {
            conditions,
            bodies,
            else_body,
            ..
        } = &block[0]
        {
            assert_eq!(conditions.len(), 2);
            assert_eq!(bodies.len(), 2);
            assert!(else_body.is_some());
        } else {
            panic!("expected If");
        }
    }

    // -- Numeric for --

    #[test]
    fn numeric_for() {
        let block = parse_ok("for i = 1, 10 do end");
        if let Stat::NumericFor {
            name, step, body, ..
        } = &block[0]
        {
            assert_eq!(name, "i");
            assert!(step.is_none());
            assert!(body.is_empty());
        } else {
            panic!("expected NumericFor");
        }
    }

    #[test]
    fn numeric_for_with_step() {
        let block = parse_ok("for i = 1, 10, 2 do end");
        if let Stat::NumericFor { step, .. } = &block[0] {
            assert!(step.is_some());
        } else {
            panic!("expected NumericFor");
        }
    }

    // -- Generic for --

    #[test]
    fn generic_for() {
        let block = parse_ok("for k, v in pairs(t) do end");
        if let Stat::GenericFor { names, .. } = &block[0] {
            assert_eq!(names, &["k", "v"]);
        } else {
            panic!("expected GenericFor");
        }
    }

    // -- Local --

    #[test]
    fn local_decl() {
        let block = parse_ok("local x");
        if let Stat::LocalDecl { names, values, .. } = &block[0] {
            assert_eq!(names, &["x"]);
            assert!(values.is_empty());
        } else {
            panic!("expected LocalDecl");
        }
    }

    #[test]
    fn local_decl_with_init() {
        let block = parse_ok("local x, y = 1, 2");
        if let Stat::LocalDecl { names, values, .. } = &block[0] {
            assert_eq!(names, &["x", "y"]);
            assert_eq!(values.len(), 2);
        } else {
            panic!("expected LocalDecl");
        }
    }

    #[test]
    fn local_function() {
        let block = parse_ok("local function foo() end");
        if let Stat::LocalFunc { name, .. } = &block[0] {
            assert_eq!(name, "foo");
        } else {
            panic!("expected LocalFunc");
        }
    }

    // -- Function declaration --

    #[test]
    fn func_decl_simple() {
        let block = parse_ok("function foo() end");
        if let Stat::FuncDecl { name, .. } = &block[0] {
            assert_eq!(name.parts, vec!["foo"]);
            assert!(name.method.is_none());
        } else {
            panic!("expected FuncDecl");
        }
    }

    #[test]
    fn func_decl_dotted() {
        let block = parse_ok("function a.b.c() end");
        if let Stat::FuncDecl { name, .. } = &block[0] {
            assert_eq!(name.parts, vec!["a", "b", "c"]);
            assert!(name.method.is_none());
        } else {
            panic!("expected FuncDecl");
        }
    }

    #[test]
    fn func_decl_method() {
        let block = parse_ok("function a.b:c() end");
        if let Stat::FuncDecl { name, .. } = &block[0] {
            assert_eq!(name.parts, vec!["a", "b"]);
            assert_eq!(name.method.as_deref(), Some("c"));
        } else {
            panic!("expected FuncDecl");
        }
    }

    // -- Return --

    #[test]
    fn return_no_values() {
        let block = parse_ok("return");
        if let Stat::Return { values, .. } = &block[0] {
            assert!(values.is_empty());
        } else {
            panic!("expected Return");
        }
    }

    #[test]
    fn return_values() {
        let block = parse_ok("return 1, 2, 3");
        if let Stat::Return { values, .. } = &block[0] {
            assert_eq!(values.len(), 3);
        } else {
            panic!("expected Return");
        }
    }

    #[test]
    fn return_must_be_last() {
        // Return followed by more statements is invalid
        let block = parse_ok("return 1");
        assert_eq!(block.len(), 1);
    }

    // -- Break --

    #[test]
    fn break_statement() {
        // Break must appear inside a loop; test inside while.
        let block = parse_ok("while true do break end");
        assert_eq!(block.len(), 1);
        if let Stat::While { body, .. } = &block[0] {
            assert_eq!(body.len(), 1);
            assert!(matches!(body[0], Stat::Break { .. }));
        } else {
            panic!("expected while statement");
        }
    }

    #[test]
    fn break_outside_loop() {
        // PUC-Rio: "no loop to break near '<eof>'"
        let err = parse_err("break");
        assert!(err.contains("no loop to break"), "got: {err}");
    }

    // -- Assignment --

    #[test]
    fn simple_assignment() {
        let block = parse_ok("x = 1");
        assert!(matches!(block[0], Stat::Assign { .. }));
    }

    #[test]
    fn multi_assignment() {
        let block = parse_ok("x, y = 1, 2");
        if let Stat::Assign {
            targets, values, ..
        } = &block[0]
        {
            assert_eq!(targets.len(), 2);
            assert_eq!(values.len(), 2);
        } else {
            panic!("expected Assign");
        }
    }

    // -- Expression statements (function calls) --

    #[test]
    fn call_statement() {
        let block = parse_ok("print(1)");
        assert!(matches!(block[0], Stat::ExprStat { .. }));
    }

    #[test]
    fn method_call_statement() {
        let block = parse_ok("obj:method()");
        if let Stat::ExprStat { expr, .. } = &block[0] {
            assert!(matches!(expr, Expr::MethodCall { .. }));
        } else {
            panic!("expected ExprStat");
        }
    }

    // -- Expression parsing --

    #[test]
    fn binary_precedence() {
        // 1 + 2 * 3 should parse as 1 + (2 * 3)
        let block = parse_ok("return 1 + 2 * 3");
        if let Stat::Return { values, .. } = &block[0] {
            if let Expr::BinOp {
                op: BinOp::Add,
                right,
                ..
            } = &values[0]
            {
                assert!(matches!(right.as_ref(), Expr::BinOp { op: BinOp::Mul, .. }));
            } else {
                panic!("expected Add at top");
            }
        } else {
            panic!("expected Return");
        }
    }

    #[test]
    fn right_associative_pow() {
        // 2^3^4 should parse as 2^(3^4)
        let block = parse_ok("return 2^3^4");
        if let Stat::Return { values, .. } = &block[0] {
            if let Expr::BinOp {
                op: BinOp::Pow,
                right,
                ..
            } = &values[0]
            {
                assert!(matches!(right.as_ref(), Expr::BinOp { op: BinOp::Pow, .. }));
            } else {
                panic!("expected Pow at top");
            }
        } else {
            panic!("expected Return");
        }
    }

    #[test]
    fn right_associative_concat() {
        // "a".."b".."c" should parse as "a"..("b".."c")
        let block = parse_ok(r#"return "a".."b".."c""#);
        if let Stat::Return { values, .. } = &block[0] {
            if let Expr::BinOp {
                op: BinOp::Concat,
                right,
                ..
            } = &values[0]
            {
                assert!(matches!(
                    right.as_ref(),
                    Expr::BinOp {
                        op: BinOp::Concat,
                        ..
                    }
                ));
            } else {
                panic!("expected Concat at top");
            }
        } else {
            panic!("expected Return");
        }
    }

    #[test]
    fn unary_operators() {
        let block = parse_ok("return -1, not true, #t");
        if let Stat::Return { values, .. } = &block[0] {
            assert!(matches!(values[0], Expr::UnOp { op: UnOp::Neg, .. }));
            assert!(matches!(values[1], Expr::UnOp { op: UnOp::Not, .. }));
            assert!(matches!(values[2], Expr::UnOp { op: UnOp::Len, .. }));
        } else {
            panic!("expected Return");
        }
    }

    #[test]
    fn all_binary_ops() {
        let block = parse_ok(
            "return 1+2, 1-2, 1*2, 1/2, 1%2, 1^2, \"a\"..\"b\", 1<2, 1<=2, 1>2, 1>=2, 1==2, 1~=2, true and false, true or false",
        );
        if let Stat::Return { values, .. } = &block[0] {
            assert_eq!(values.len(), 15);
        } else {
            panic!("expected Return");
        }
    }

    // -- Suffixed expressions --

    #[test]
    fn field_access() {
        let block = parse_ok("return a.b.c");
        if let Stat::Return { values, .. } = &block[0] {
            assert!(matches!(values[0], Expr::Field { .. }));
        } else {
            panic!("expected Return");
        }
    }

    #[test]
    fn index_access() {
        let block = parse_ok("return a[1]");
        if let Stat::Return { values, .. } = &block[0] {
            assert!(matches!(values[0], Expr::Index { .. }));
        } else {
            panic!("expected Return");
        }
    }

    #[test]
    fn function_call_expr() {
        let block = parse_ok("return f(1, 2)");
        if let Stat::Return { values, .. } = &block[0] {
            if let Expr::Call { args, .. } = &values[0] {
                assert_eq!(args.len(), 2);
            } else {
                panic!("expected Call");
            }
        } else {
            panic!("expected Return");
        }
    }

    #[test]
    fn call_with_string_arg() {
        let block = parse_ok(r#"print "hello""#);
        if let Stat::ExprStat { expr, .. } = &block[0] {
            if let Expr::Call { args, .. } = expr {
                assert_eq!(args.len(), 1);
                assert!(matches!(&args[0], Expr::Str(s, _) if s == b"hello"));
            } else {
                panic!("expected Call");
            }
        } else {
            panic!("expected ExprStat");
        }
    }

    #[test]
    fn call_with_table_arg() {
        let block = parse_ok("f{1, 2}");
        if let Stat::ExprStat { expr, .. } = &block[0] {
            if let Expr::Call { args, .. } = expr {
                assert_eq!(args.len(), 1);
                assert!(matches!(&args[0], Expr::TableCtor { .. }));
            } else {
                panic!("expected Call");
            }
        } else {
            panic!("expected ExprStat");
        }
    }

    // -- Table constructors --

    #[test]
    fn empty_table() {
        let block = parse_ok("return {}");
        if let Stat::Return { values, .. } = &block[0] {
            if let Expr::TableCtor { fields, .. } = &values[0] {
                assert!(fields.is_empty());
            } else {
                panic!("expected TableCtor");
            }
        } else {
            panic!("expected Return");
        }
    }

    #[test]
    fn table_with_fields() {
        let block = parse_ok("return {x = 1, [2] = 3, 4}");
        if let Stat::Return { values, .. } = &block[0] {
            if let Expr::TableCtor { fields, .. } = &values[0] {
                assert_eq!(fields.len(), 3);
                assert!(matches!(fields[0], TableField::NameField { .. }));
                assert!(matches!(fields[1], TableField::IndexField { .. }));
                assert!(matches!(fields[2], TableField::ValueField { .. }));
            } else {
                panic!("expected TableCtor");
            }
        } else {
            panic!("expected Return");
        }
    }

    // -- Function body --

    #[test]
    fn func_with_params() {
        let block = parse_ok("function foo(a, b, c) end");
        if let Stat::FuncDecl { body, .. } = &block[0] {
            assert_eq!(body.params, vec!["a", "b", "c"]);
            assert!(!body.has_varargs);
        } else {
            panic!("expected FuncDecl");
        }
    }

    #[test]
    fn func_with_varargs() {
        let block = parse_ok("function foo(a, ...) end");
        if let Stat::FuncDecl { body, .. } = &block[0] {
            assert_eq!(body.params, vec!["a"]);
            assert!(body.has_varargs);
        } else {
            panic!("expected FuncDecl");
        }
    }

    #[test]
    fn anonymous_func() {
        let block = parse_ok("return function(x) return x end");
        if let Stat::Return { values, .. } = &block[0] {
            assert!(matches!(values[0], Expr::FuncDef { .. }));
        } else {
            panic!("expected Return");
        }
    }

    // -- Parenthesized expressions --

    #[test]
    fn paren_expr() {
        let block = parse_ok("return (1 + 2) * 3");
        if let Stat::Return { values, .. } = &block[0] {
            assert!(matches!(values[0], Expr::BinOp { op: BinOp::Mul, .. }));
        } else {
            panic!("expected Return");
        }
    }

    // -- Vararg --

    #[test]
    fn vararg_expr() {
        let block = parse_ok("return ...");
        if let Stat::Return { values, .. } = &block[0] {
            assert!(matches!(values[0], Expr::VarArg(_)));
        } else {
            panic!("expected Return");
        }
    }

    // -- Literals --

    #[test]
    fn literal_exprs() {
        let block = parse_ok("return nil, true, false, 42, \"hello\"");
        if let Stat::Return { values, .. } = &block[0] {
            assert!(matches!(values[0], Expr::Nil(_)));
            assert!(matches!(values[1], Expr::True(_)));
            assert!(matches!(values[2], Expr::False(_)));
            assert!(matches!(values[3], Expr::Number(n, _) if n == 42.0));
            assert!(matches!(&values[4], Expr::Str(s, _) if s == b"hello"));
        } else {
            panic!("expected Return");
        }
    }

    // -- Error cases --

    #[test]
    fn missing_end() {
        let err = parse_err("if true then");
        assert!(err.contains("'end' expected"));
    }

    #[test]
    fn missing_end_multiline() {
        let err = parse_err("if true then\n  x = 1\n");
        assert!(err.contains("'end' expected"));
        assert!(err.contains("to close"));
    }

    #[test]
    fn missing_then() {
        let err = parse_err("if true end");
        assert!(err.contains("'then' expected"));
    }

    #[test]
    fn for_missing_in_or_eq() {
        let err = parse_err("for x do end");
        assert!(err.contains("'=' or 'in' expected"));
    }

    #[test]
    fn unexpected_symbol() {
        let err = parse_err("+");
        assert!(err.contains("unexpected symbol"));
    }

    #[test]
    fn missing_closing_paren() {
        let err = parse_err("return (1 + 2");
        assert!(err.contains("')' expected"));
    }
}
