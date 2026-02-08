use super::Chunk;
use super::Instr;
use super::Result;
use super::error::Error;
use super::error::ErrorKind;
use super::error::SyntaxError;
use super::exp_desc::ExpDesc;
use super::exp_desc::PlaceExp;
use super::exp_desc::PrefixExp;
use super::lexer::TokenStream;
use super::token::Token;
use super::token::TokenType;

use std::borrow::Borrow;
use std::cmp::Ordering;

/// Tracks whether a block is a loop (for `break` validation) and collects
/// the code indices of `break` jumps that need backpatching when the loop ends.
#[derive(Debug)]
struct BlockInfo {
    is_loop: bool,
    break_jumps: Vec<usize>,
}

/// Info about a local variable: name, nesting level, and whether it was
/// captured by a closure (requiring `Close` on scope exit).
#[derive(Debug)]
struct LocalInfo {
    name: String,
    level: i32,
    captured: bool,
}

/// Tracks the current state, to make parsing easier.
#[derive(Debug)]
struct Parser<'a> {
    /// The input token stream.
    input: TokenStream<'a>,
    chunk: Chunk,
    nest_level: i32,
    locals: Vec<LocalInfo>,
    outer_chunks: Vec<Chunk>,
    /// Stack of block scopes for `break` statement validation and backpatching.
    block_stack: Vec<BlockInfo>,
    /// When entering a nested function, the current locals count is pushed here.
    /// This marks the boundary between the outer function's locals and the inner one's.
    outer_local_counts: Vec<usize>,
    /// Stack tracking whether the current function is vararg. Pushed/popped
    /// in tandem with `outer_chunks` (one entry per function nesting level).
    is_vararg_stack: Vec<bool>,
    /// Tracks the next free stack slot relative to `stack_bottom`, analogous to
    /// PUC-Rio's `FuncState.freereg`. Equals params + locals_in_scope + temporaries.
    /// Saved/restored across nested function definitions via `outer_freeregs`.
    freereg: usize,
    /// Saved `freereg` values for enclosing functions (pushed/popped in tandem
    /// with `outer_chunks`).
    outer_freeregs: Vec<usize>,
    /// Tracks instructions whose register operand needs fixup at the end of
    /// `parse_chunk` to account for the VM's local slot pre-allocation.
    /// Each entry is `(instruction_index, num_locals_at_emission)`.
    /// The register operand gets adjusted by `final_num_locals - num_locals_at_emission`.
    /// Used for `SetListMulti` (table register) and `CallVar` (function register).
    register_fixups: Vec<(usize, u8)>,
}

/// Parses Lua source code into a `Chunk`.
pub(super) fn parse_str(source: &str) -> Result<Chunk> {
    let parser = Parser {
        input: TokenStream::new(source),
        chunk: Chunk::default(),
        nest_level: 0,
        locals: Vec::new(),
        outer_chunks: Vec::new(),
        block_stack: Vec::new(),
        outer_local_counts: Vec::new(),
        is_vararg_stack: Vec::new(),
        freereg: 0,
        outer_freeregs: Vec::new(),
        register_fixups: Vec::new(),
    };
    parser.parse_all()
}

impl<'a> Parser<'a> {
    // Helper functions

    /// Creates a new local slot at the current nest_level.
    /// Fails if we have exceeded the maximum number of locals.
    fn add_local(&mut self, name: &str) -> Result<()> {
        let base = self.outer_local_counts.last().copied().unwrap_or(0);
        if self.locals.len() - base >= u8::MAX as usize {
            Err(self.error(SyntaxError::TooManyLocals))
        } else {
            self.locals.push(LocalInfo {
                name: name.to_string(),
                level: self.nest_level,
                captured: false,
            });
            self.freereg += 1;
            let locals_in_chunk = self.locals.len() - base;
            if locals_in_chunk > self.chunk.num_locals as usize {
                self.chunk.num_locals += 1;
            }
            Ok(())
        }
    }

    /// Constructs an error of the given kind at the current position.
    // TODO: rename to error_here
    #[must_use]
    fn error(&self, kind: impl Into<ErrorKind>) -> Error {
        let pos = self.input.pos();
        self.error_at(kind, pos)
    }

    /// Constructs an error of the given kind and position.
    #[must_use]
    fn error_at(&self, kind: impl Into<ErrorKind>, pos: usize) -> Error {
        let (line, column) = self.input.line_and_column(pos);
        Error::new(kind, line, column)
    }

    /// Constructs an error for when a specific `TokenType` was expected but not found.
    #[must_use]
    fn err_unexpected(&self, token: Token, _expected: TokenType) -> Error {
        let error_kind = if token.typ == TokenType::EndOfFile {
            SyntaxError::UnexpectedEof
        } else {
            SyntaxError::UnexpectedTok
        };
        self.error_at(error_kind, token.start)
    }

    /// Pulls a token off the input and checks it against `expected`.
    /// Returns the token if it matches, `Err` otherwise.
    fn expect(&mut self, expected: TokenType) -> Result<Token> {
        let token = self.input.next()?;
        if token.typ == expected {
            Ok(token)
        } else {
            Err(self.err_unexpected(token, expected))
        }
    }

    /// Expects an identifier token and returns the identifier as a string.
    fn expect_identifier(&mut self) -> Result<&'a str> {
        let token = self.expect(TokenType::Identifier)?;
        let name = self.get_text(token);
        Ok(name)
    }

    /// Expects an identifier and returns the id of its string literal.
    fn expect_identifier_id(&mut self) -> Result<u8> {
        let name = self.expect_identifier()?;
        self.find_or_add_string(name.as_bytes())
    }

    /// Stores a literal string (as bytes) and returns its index.
    fn find_or_add_string(&mut self, string: &[u8]) -> Result<u8> {
        find_or_add(&mut self.chunk.string_literals, string)
            .ok_or_else(|| self.error(SyntaxError::TooManyStrings))
    }

    /// Stores a literal number and returns its index.
    fn find_or_add_number(&mut self, num: f64) -> Result<u8> {
        find_or_add(&mut self.chunk.number_literals, &num)
            .ok_or_else(|| self.error(SyntaxError::TooManyNumbers))
    }

    /// Converts a literal string token into its semantic byte value.
    ///
    /// For long strings, strips delimiters and the optional leading newline.
    /// For short strings, strips quotes and processes escape sequences per
    /// the Lua 5.1.1 spec (`llex.c:276-329`).
    fn get_literal_string_contents(&self, tok: Token) -> Result<Vec<u8>> {
        let Token { start, len, typ } = tok;
        assert_eq!(typ, TokenType::LiteralString);
        let text = self.input.substring(start..(start + len as usize));
        if text.starts_with('[') {
            // Long string: count `=` signs to determine bracket level,
            // then strip the opening `[=*[` and closing `]=*]` delimiters.
            let level = text[1..].chars().take_while(|&c| c == '=').count();
            let delim_len = 2 + level; // `[` + `=`*level + `[`
            let inner = &text[delim_len..text.len() - delim_len];
            // Skip a leading newline: \n, \r, \r\n, or \n\r
            // (Lua spec: first newline after opening bracket is ignored)
            let inner = if let Some(rest) = inner.strip_prefix("\r\n") {
                rest
            } else if let Some(rest) = inner.strip_prefix("\n\r") {
                rest
            } else if let Some(rest) = inner.strip_prefix('\r') {
                rest
            } else if let Some(rest) = inner.strip_prefix('\n') {
                rest
            } else {
                inner
            };
            // Normalize \r\n and \r to \n in the body (Lua spec: llex.c:257-263)
            // Long strings come from source &str, so always valid UTF-8
            Ok(normalize_line_endings(inner).into_bytes())
        } else {
            // Short string: strip quotes and process escape sequences
            assert!(len >= 2);
            let raw = &text[1..text.len() - 1];
            process_escapes(raw, start, |pos| self.input.line_and_column(pos))
        }
    }

    /// Gets the original source code contained by a token.
    #[must_use]
    fn get_text(&self, token: Token) -> &'a str {
        self.input.substring(token.range())
    }

    /// Lowers the nesting level by one, discarding any locals from that block.
    /// Emits `Close` if any discarded locals were captured by closures.
    fn level_down(&mut self) {
        let base = self.outer_local_counts.last().copied().unwrap_or(0);
        let mut need_close = false;
        let mut close_base = self.locals.len();
        while let Some(local) = self.locals.last() {
            if local.level == self.nest_level {
                if local.captured {
                    need_close = true;
                    close_base = self.locals.len() - 1;
                }
                self.locals.pop();
                self.freereg -= 1;
            } else {
                break;
            }
        }
        if need_close {
            let slot = (close_base - base) as u8;
            self.push(Instr::Close(slot));
        }
        self.nest_level -= 1;
    }

    /// Returns true if the current function being parsed is vararg.
    fn current_chunk_is_vararg(&self) -> bool {
        self.is_vararg_stack.last().copied().unwrap_or(false)
    }

    /// Adds an instruction to the output and updates `freereg` to reflect the
    /// instruction's stack effect. Mirrors PUC-Rio's `freereg` maintenance.
    fn push(&mut self, instr: Instr) {
        self.apply_freereg_effect(&instr);
        self.chunk.code.push(instr);
    }

    /// Removes the last instruction from the code and undoes its `freereg`
    /// effect. Used when replacing a Call/VarArg instruction with a different
    /// return count.
    fn pop_instr(&mut self) -> Option<Instr> {
        let instr = self.chunk.code.pop()?;
        self.undo_freereg_effect(&instr);
        Some(instr)
    }

    /// Applies the `freereg` stack effect for the given instruction (push).
    fn apply_freereg_effect(&mut self, instr: &Instr) {
        let (pushes, pops) = Self::stack_effect(instr);
        self.freereg = self.freereg + pushes - pops;
    }

    /// Undoes the `freereg` stack effect for the given instruction (pop).
    fn undo_freereg_effect(&mut self, instr: &Instr) {
        let (pushes, pops) = Self::stack_effect(instr);
        self.freereg = self.freereg + pops - pushes;
    }

    /// Returns (pushes, pops) for a given instruction.
    fn stack_effect(instr: &Instr) -> (usize, usize) {
        match *instr {
            // Push +1
            Instr::PushNil
            | Instr::PushBool(_)
            | Instr::PushNum(_)
            | Instr::PushString(_)
            | Instr::GetLocal(_)
            | Instr::GetGlobal(_)
            | Instr::GetUpval(_)
            | Instr::NewTable
            | Instr::Closure(_) => (1, 0),

            // Pop 1, push 1: net 0
            Instr::GetField(_) | Instr::Not | Instr::Negate | Instr::Length => (1, 1),

            // Pop 2 (key + table), push 1: net -1
            Instr::GetTable => (1, 2),

            // Pop 1
            Instr::Pop | Instr::SetLocal(_) | Instr::SetGlobal(_) | Instr::SetUpval(_) => (0, 1),

            // Binary ops: pop 2, push 1
            Instr::Add
            | Instr::Subtract
            | Instr::Multiply
            | Instr::Divide
            | Instr::Mod
            | Instr::Pow
            | Instr::Concat
            | Instr::Less
            | Instr::LessEqual
            | Instr::Greater
            | Instr::GreaterEqual
            | Instr::Equal
            | Instr::NotEqual => (1, 2),

            // Table field init: pops value
            Instr::InitField(_, _) => (0, 1),
            // Table index init: pops key + value
            Instr::InitIndex(_) => (0, 2),
            // SetList: pops n values
            Instr::SetList(n) => (0, n as usize),
            // SetListMulti: dynamic, handled by caller
            Instr::SetListMulti(_) => (0, 0),

            // SetField: pops value + removes table
            Instr::SetField(_, _) => (0, 2),
            // SetTable: pops value + removes key + removes table
            Instr::SetTable(_) => (0, 3),

            // Self_: pops object, pushes method + object
            Instr::Self_(_) => (2, 1),

            // Call: pops func + args, pushes rets.
            // 255 for rets = variable return count (multi-return, handled by caller).
            Instr::Call(args, rets) => {
                let pops = args as usize + 1;
                let pushes = if rets == 255 { 0 } else { rets as usize };
                (pushes, pops)
            }

            // CallVar: variable arg count. The function register is known but
            // the arg count is dynamic. For freereg tracking, it pops everything
            // above the function register (net: reset to func_reg) then pushes rets.
            // Since freereg is already at the right position (from parse_funcargs),
            // we just account for the function itself being consumed.
            Instr::CallVar(_, rets) => {
                let pushes = if rets == 255 { 0 } else { rets as usize };
                // CallVar consumes everything from func_reg to TOS.
                // freereg was not advanced for the multi-return args, so it's
                // already at the right level. We just push the return values.
                (pushes, 1)
            }

            // VarArg: pushes n values (0 = dynamic)
            Instr::VarArg(n) => (n as usize, 0),

            // No stack effect
            Instr::Return(_)
            | Instr::Jump(_)
            | Instr::Close(_)
            | Instr::BranchTrueKeep(_)
            | Instr::BranchFalseKeep(_)
            | Instr::ForPrep(_, _)
            | Instr::ForLoop(_, _)
            | Instr::TForLoop(_, _, _) => (0, 0),

            // Branch: pops condition
            Instr::BranchFalse(_) => (0, 1),
        }
    }

    // Actual parsing

    /// The main entry point for the parser. This parses the entire input.
    fn parse_all(mut self) -> Result<Chunk> {
        // Top-level chunk is always vararg (matches PUC-Rio behavior)
        let c = self.parse_chunk(&[], true)?;
        let token = self.input.next()?;
        assert_eq!(self.nest_level, 0);
        if token.typ == TokenType::EndOfFile {
            Ok(c)
        } else {
            Err(self.err_unexpected(token, TokenType::EndOfFile))
        }
    }

    /// Parses a `Chunk`.
    fn parse_chunk(&mut self, params: &[&str], is_vararg: bool) -> Result<Chunk> {
        self.outer_chunks.push(self.chunk.clone());
        self.outer_local_counts.push(self.locals.len());
        self.is_vararg_stack.push(is_vararg);
        self.outer_freeregs.push(self.freereg);
        self.freereg = 0;
        self.chunk = Chunk::default();
        // Save and reset SetListMulti fixups for this chunk scope
        let outer_fixups = std::mem::take(&mut self.register_fixups);

        self.chunk.num_params = params.len() as u8;
        self.chunk.is_vararg = is_vararg;
        for &param in params {
            self.locals.push(LocalInfo {
                name: param.into(),
                level: self.nest_level,
                captured: false,
            });
            self.freereg += 1;
        }

        self.parse_statements()?;
        self.push(Instr::Return(0));

        // Post-patch register-based instructions: adjust register operands to
        // account for the VM's pre-allocation of `num_locals` nil slots at
        // function entry. Each fixup records the instruction index and the value
        // of num_locals at emission time. The delta (final - emission) is the
        // number of not-yet-declared locals whose pre-allocated slots shift
        // temporaries upward on the stack.
        let final_num_locals = self.chunk.num_locals;
        for &(idx, num_locals_at_emission) in &self.register_fixups {
            let delta = final_num_locals - num_locals_at_emission;
            match &mut self.chunk.code[idx] {
                Instr::SetListMulti(reg) | Instr::CallVar(reg, _) => {
                    *reg += delta;
                }
                _ => {}
            }
        }

        let tmp_chunk = self.chunk.clone();
        self.chunk = self.outer_chunks.pop().unwrap();

        // Restore locals to the boundary of the enclosing function
        let boundary = self.outer_local_counts.pop().unwrap();
        self.locals.truncate(boundary);
        self.is_vararg_stack.pop();
        self.freereg = self.outer_freeregs.pop().unwrap();
        self.register_fixups = outer_fixups;

        if option_env!("LUA_DEBUG_PARSER").is_some() {
            println!("Compiled chunk: {:#?}", &tmp_chunk);
        }

        Ok(tmp_chunk)
    }

    /// Parses 0 or more statements, possibly separated by semicolons.
    fn parse_statements(&mut self) -> Result<()> {
        loop {
            match self.input.peek_type()? {
                TokenType::Identifier | TokenType::LParen | TokenType::LParenLineStart => {
                    self.parse_assign_or_call()?;
                }
                TokenType::If => self.parse_if()?,
                TokenType::While => self.parse_while()?,
                TokenType::Repeat => self.parse_repeat()?,
                TokenType::Do => self.parse_do()?,
                TokenType::Local => self.parse_locals()?,
                TokenType::For => self.parse_for()?,
                TokenType::Function => self.parse_fndecl()?,
                TokenType::Semi => {
                    self.input.next()?;
                }
                TokenType::Break => break self.parse_break(),
                TokenType::Return => break self.parse_return(),
                _ => break Ok(()),
            }
        }
    }

    /// Parses a function declaration, which is any statement that starts with
    /// the keyword `function`.
    fn parse_fndecl(&mut self) -> Result<()> {
        self.input.next()?; // 'function' keyword
        let name = self.expect_identifier()?;
        match self.input.peek_type()? {
            TokenType::Dot => self.parse_fndecl_table(name),
            TokenType::Colon => self.parse_fndecl_method(name),
            _ => self.parse_fndecl_basic(name),
        }
    }

    /// Parses a basic function declaration, which just assigns the function to
    /// a local or global variable.
    fn parse_fndecl_basic(&mut self, name: &'a str) -> Result<()> {
        let place_exp = self.parse_prefix_identifier(name)?;
        let instr = match place_exp {
            PlaceExp::Local(i) => Instr::SetLocal(i),
            PlaceExp::Global(i) => Instr::SetGlobal(i),
            PlaceExp::Upvalue(i) => Instr::SetUpval(i),
            _ => unreachable!("place expression was not a variable"),
        };
        self.parse_fndef()?;
        self.push(instr);
        Ok(())
    }

    fn parse_fndecl_table(&mut self, table_name: &'a str) -> Result<()> {
        // Push the table onto the stack.
        let table_instr = match self.parse_prefix_identifier(table_name)? {
            PlaceExp::Local(i) => Instr::GetLocal(i),
            PlaceExp::Global(i) => Instr::GetGlobal(i),
            PlaceExp::Upvalue(i) => Instr::GetUpval(i),
            _ => unreachable!("place expression was not a variable"),
        };
        self.push(table_instr);

        // Parse all the dot-separated fields. There must be at least one.
        self.expect(TokenType::Dot)?;
        let mut last_field_id = self.expect_identifier_id()?;
        while self.input.try_pop(TokenType::Dot)?.is_some() {
            self.push(Instr::GetField(last_field_id));
            last_field_id = self.expect_identifier_id()?;
        }

        // Check for trailing `:method` (e.g. `function t.a:m() ... end`)
        if self.input.try_pop(TokenType::Colon)?.is_some() {
            self.push(Instr::GetField(last_field_id));
            let method_id = self.expect_identifier_id()?;
            self.parse_fndef_with_self()?;
            self.push(Instr::SetField(0, method_id));
        } else {
            // Parse the function params and body.
            self.parse_fndef()?;
            self.push(Instr::SetField(0, last_field_id));
        }
        Ok(())
    }

    /// Parses `function Name:method(params) body end`.
    ///
    /// Like `parse_fndecl_table` but adds an implicit `self` parameter.
    fn parse_fndecl_method(&mut self, table_name: &'a str) -> Result<()> {
        // Push the table onto the stack.
        let table_instr = match self.parse_prefix_identifier(table_name)? {
            PlaceExp::Local(i) => Instr::GetLocal(i),
            PlaceExp::Global(i) => Instr::GetGlobal(i),
            PlaceExp::Upvalue(i) => Instr::GetUpval(i),
            _ => unreachable!("place expression was not a variable"),
        };
        self.push(table_instr);

        // Consume ':' and get method name
        self.expect(TokenType::Colon)?;
        let method_id = self.expect_identifier_id()?;

        // Parse function body with implicit `self` parameter
        self.parse_fndef_with_self()?;
        self.push(Instr::SetField(0, method_id));
        Ok(())
    }

    /// Parses a function definition body with an implicit `self` first parameter.
    fn parse_fndef_with_self(&mut self) -> Result<()> {
        let mut params = vec!["self"];
        let lparen_tok = self.input.next()?;
        match lparen_tok.typ {
            TokenType::LParen | TokenType::LParenLineStart => (),
            _ => return Err(self.err_unexpected(lparen_tok, TokenType::LParen)),
        }
        if self.input.try_pop(TokenType::RParen)?.is_none() {
            params.push(self.expect_identifier()?);
            while self.input.try_pop(TokenType::Comma)?.is_some() {
                params.push(self.expect_identifier()?);
            }
            self.expect(TokenType::RParen)?;
        }

        if self.chunk.nested.len() >= u8::MAX as usize {
            return Err(self.error(SyntaxError::Complexity));
        }

        self.nest_level += 1;
        let new_chunk = self.parse_chunk(&params, false)?;
        self.level_down();

        self.chunk.nested.push(new_chunk);
        self.push(Instr::Closure(self.chunk.nested.len() as u8 - 1));
        self.expect(TokenType::End)?;
        Ok(())
    }

    /// Parses a return statement. Return statements must always come last in a
    /// block.
    fn parse_return(&mut self) -> Result<()> {
        self.input.next()?; // 'return' keyword
        let (n, last_exp) = self.parse_explist()?;
        if is_multi_return(&last_exp) && n > 0 {
            self.patch_last_for_multi_return(&last_exp);
            // 255 signals "variable return count" to the VM: return
            // everything on the stack above the frame's locals.
            self.push(Instr::Return(255));
        } else {
            self.push(Instr::Return(n));
        }
        self.input.try_pop(TokenType::Semi)?;
        Ok(())
    }

    /// Parses a `break` statement. In Lua 5.1, `break` must be the last
    /// statement in a block and can only appear inside a loop.
    fn parse_break(&mut self) -> Result<()> {
        let break_token = self.input.next()?; // 'break' keyword
        let (line, _col) = self.input.line_and_column(break_token.start);

        // Find the innermost enclosing loop block
        let found_loop = self.block_stack.iter().rev().any(|block| block.is_loop);

        if !found_loop {
            return Err(self.error_at(SyntaxError::BreakOutsideLoop(line), break_token.start));
        }

        // Emit Close if any locals in scope are captured by closures.
        // This ensures upvalues are properly closed before exiting the loop.
        self.emit_close_if_needed();

        // Emit a placeholder jump — will be backpatched when the loop ends
        let jump_index = self.chunk.code.len();
        self.push(Instr::Jump(0));

        // Record the jump index in the innermost loop block
        for block in self.block_stack.iter_mut().rev() {
            if block.is_loop {
                block.break_jumps.push(jump_index);
                break;
            }
        }

        // In Lua 5.1, `break` must be the last statement in a block.
        // An optional semicolon may follow.
        self.input.try_pop(TokenType::Semi)?;
        Ok(())
    }

    /// Emits a `Close` instruction if any locals in the current scope have
    /// been captured by closures.
    fn emit_close_if_needed(&mut self) {
        let base = self.outer_local_counts.last().copied().unwrap_or(0);
        let mut min_captured = None;
        for (i, local) in self.locals.iter().enumerate().skip(base) {
            if local.captured {
                match min_captured {
                    None => min_captured = Some(i - base),
                    Some(prev) => {
                        if i - base < prev {
                            min_captured = Some(i - base);
                        }
                    }
                }
            }
        }
        if let Some(slot) = min_captured {
            self.push(Instr::Close(slot as u8));
        }
    }

    /// Backpatches all break jumps in the given block to jump to the
    /// current code position.
    fn backpatch_breaks(&mut self, block: &BlockInfo) {
        let target = self.chunk.code.len();
        for &jump_index in &block.break_jumps {
            let offset = (target - jump_index - 1) as isize;
            self.chunk.code[jump_index] = Instr::Jump(offset);
        }
    }

    /// Parses a statement which could be a variable assignment or a function call.
    fn parse_assign_or_call(&mut self) -> Result<()> {
        match self.parse_prefix_exp()? {
            PrefixExp::Parenthesized => {
                let tok = self.input.next()?;
                Err(self.err_unexpected(tok, TokenType::Assign))
            }
            PrefixExp::FunctionCall(num_args) => {
                self.push(Instr::Call(num_args, 0));
                Ok(())
            }
            PrefixExp::FunctionCallVar(func_reg) => {
                // Variable-arg call used as a statement: discard all returns.
                // Don't use push() — see eval_prefix_exp for rationale.
                let instr_idx = self.chunk.code.len();
                self.chunk.code.push(Instr::CallVar(func_reg, 0));
                self.freereg = func_reg as usize;
                self.register_fixups
                    .push((instr_idx, self.chunk.num_locals));
                Ok(())
            }
            PrefixExp::Place(first_place) => self.parse_assign(first_place),
        }
    }

    /// Parses a variable assignment.
    fn parse_assign(&mut self, first_exp: PlaceExp) -> Result<()> {
        let mut places = vec![first_exp];
        while self.input.try_pop(TokenType::Comma)?.is_some() {
            places.push(self.parse_place_exp()?);
        }

        self.expect(TokenType::Assign)?;
        let num_lvals = places.len() as isize;
        let (num_rvals, last_exp) = self.parse_explist()?;
        let num_rvals = num_rvals as isize;
        let diff = num_lvals - num_rvals;
        if diff > 0 {
            if let ExpDesc::Prefix(PrefixExp::FunctionCall(_)) = last_exp {
                let num_args = match self.pop_instr() {
                    Some(Instr::Call(args, _)) => args,
                    i => unreachable!("PrefixExp::FunctionCall but last instruction was {:?}", i),
                };
                self.push(Instr::Call(num_args, 1 + diff as u8));
            } else if let ExpDesc::Prefix(PrefixExp::FunctionCallVar(_)) = last_exp {
                let func_reg = match self.pop_instr() {
                    Some(Instr::CallVar(reg, _)) => reg,
                    i => unreachable!("FunctionCallVar but last instruction was {:?}", i),
                };
                self.push(Instr::CallVar(func_reg, 1 + diff as u8));
            } else if matches!(last_exp, ExpDesc::VarArg) {
                // Patch VarArg(1) to VarArg(1 + diff) to return enough values
                self.pop_instr();
                self.push(Instr::VarArg(1 + diff as u8));
            } else {
                for _ in 0..diff {
                    self.push(Instr::PushNil);
                }
            }
        } else {
            // discard excess rvals
            for _ in diff..0 {
                self.push(Instr::Pop);
            }
        }

        places.reverse();
        for (i, place_exp) in places.into_iter().enumerate() {
            let instr = match place_exp {
                PlaceExp::Local(i) => Instr::SetLocal(i),
                PlaceExp::Global(i) => Instr::SetGlobal(i),
                PlaceExp::Upvalue(i) => Instr::SetUpval(i),
                PlaceExp::FieldAccess(literal_id) => {
                    let stack_offset = num_lvals as u8 - i as u8 - 1;
                    Instr::SetField(stack_offset, literal_id)
                }
                PlaceExp::TableIndex => {
                    let stack_offset = num_lvals as u8 - i as u8 - 1;
                    Instr::SetTable(stack_offset)
                }
            };
            self.push(instr);
        }

        Ok(())
    }

    /// Parses an expression which can appear on the left side of an assignment.
    fn parse_place_exp(&mut self) -> Result<PlaceExp> {
        match self.parse_prefix_exp()? {
            PrefixExp::Parenthesized
            | PrefixExp::FunctionCall(_)
            | PrefixExp::FunctionCallVar(_) => {
                let tok = self.input.next()?;
                Err(self.err_unexpected(tok, TokenType::Assign))
            }
            PrefixExp::Place(place) => Ok(place),
        }
    }

    /// Emits code to evaluate the prefix expression as a normal expression.
    fn eval_prefix_exp(&mut self, exp: &PrefixExp) {
        match exp {
            PrefixExp::FunctionCall(num_args) => {
                self.push(Instr::Call(*num_args, 1));
            }
            PrefixExp::FunctionCallVar(func_reg) => {
                // Don't use push() — CallVar's stack_effect can't account for
                // the variable number of fixed args between the function and
                // the multi-return expansion. Set freereg directly.
                let instr_idx = self.chunk.code.len();
                self.chunk.code.push(Instr::CallVar(*func_reg, 1));
                self.freereg = *func_reg as usize + 1;
                self.register_fixups
                    .push((instr_idx, self.chunk.num_locals));
            }
            PrefixExp::Parenthesized => (),
            PrefixExp::Place(place) => {
                let instr = match place {
                    PlaceExp::Local(i) => Instr::GetLocal(*i),
                    PlaceExp::Global(i) => Instr::GetGlobal(*i),
                    PlaceExp::Upvalue(i) => Instr::GetUpval(*i),
                    PlaceExp::FieldAccess(i) => Instr::GetField(*i),
                    PlaceExp::TableIndex => Instr::GetTable,
                };
                self.push(instr);
            }
        }
    }

    /// Resolves a variable name to a local, upvalue, or global.
    ///
    /// Three-stage resolution (matching PUC-Rio's `singlevaraux` in `lparser.c`):
    /// 1. Local in current function scope
    /// 2. Upvalue (walking up through enclosing functions)
    /// 3. Global
    fn parse_prefix_identifier(&mut self, name: &str) -> Result<PlaceExp> {
        let base = self.outer_local_counts.last().copied().unwrap_or(0);
        // Stage 1: Local in current function
        if let Some(abs_idx) = find_last_local(&self.locals, name, base) {
            let local_idx = (abs_idx - base) as u8;
            return Ok(PlaceExp::Local(local_idx));
        }

        // Stage 2: Upvalue (search enclosing functions)
        if let Some(upval_idx) = self.resolve_upvalue(name)? {
            return Ok(PlaceExp::Upvalue(upval_idx));
        }

        // Stage 3: Global
        let i = self.find_or_add_string(name.as_bytes())?;
        Ok(PlaceExp::Global(i))
    }

    /// Attempts to resolve `name` as an upvalue. Returns the upvalue index
    /// in the current chunk's `upvalue_descs` if found.
    ///
    /// This walks outward through enclosing function scopes. Each scope
    /// either finds the variable as a local (creating `is_local: true` desc)
    /// or as an upvalue of the next-outer scope (creating `is_local: false`
    /// desc that chains through intermediate functions).
    fn resolve_upvalue(&mut self, name: &str) -> Result<Option<u8>> {
        let num_outers = self.outer_local_counts.len();
        if num_outers == 0 {
            return Ok(None);
        }
        self.resolve_upvalue_recursive(name, num_outers)
    }

    /// Recursive helper for upvalue resolution.
    /// `depth` is the number of function boundaries to look through
    /// (depth=1 means look in the immediately enclosing function).
    fn resolve_upvalue_recursive(&mut self, name: &str, depth: usize) -> Result<Option<u8>> {
        if depth == 0 {
            return Ok(None);
        }

        // The locals base for the function at `depth - 1` levels up
        let outer_base = if depth >= 2 {
            self.outer_local_counts[depth - 2]
        } else {
            0
        };
        let outer_top = self.outer_local_counts[depth - 1];

        // Check if the variable is a local in that outer function
        if let Some(abs_idx) = find_last_local(&self.locals, name, outer_base) {
            if abs_idx < outer_top {
                let local_idx = (abs_idx - outer_base) as u8;
                // Mark the local as captured
                self.locals[abs_idx].captured = true;

                // Add upvalue descriptor to the chunk at depth
                let upval_idx = self.add_upvalue_desc(name, true, local_idx, depth)?;
                return Ok(Some(upval_idx));
            }
        }

        // Recurse: look further out
        if let Some(parent_upval_idx) = self.resolve_upvalue_recursive(name, depth - 1)? {
            // The variable was found further out. Add a chained upvalue descriptor
            // that captures the parent's upvalue.
            let upval_idx = self.add_upvalue_desc(name, false, parent_upval_idx, depth)?;
            return Ok(Some(upval_idx));
        }

        Ok(None)
    }

    /// Adds an `UpvalueDesc` to the chunk at the given depth level.
    /// `depth == num_outers` means the current chunk.
    /// Returns the index of the upvalue in that chunk's desc list.
    fn add_upvalue_desc(
        &mut self,
        name: &str,
        is_local: bool,
        index: u8,
        depth: usize,
    ) -> Result<u8> {
        let descs = if depth == self.outer_local_counts.len() {
            // Current chunk
            &mut self.chunk.upvalue_descs
        } else {
            // An outer chunk at the given depth
            &mut self.outer_chunks[depth].upvalue_descs
        };

        // Check if we already have this exact upvalue
        for (i, desc) in descs.iter().enumerate() {
            if desc.is_local == is_local && desc.index == index {
                return Ok(i as u8);
            }
        }

        // Add new descriptor
        if descs.len() >= u8::MAX as usize {
            return Err(self.error(SyntaxError::Complexity));
        }
        let idx = descs.len() as u8;
        descs.push(super::UpvalueDesc {
            name: name.as_bytes().to_vec(),
            is_local,
            index,
        });
        Ok(idx)
    }

    /// Parses a `local` declaration, including `local function`.
    fn parse_locals(&mut self) -> Result<()> {
        self.input.next().unwrap(); // `local` keyword

        // `local function Name funcbody` is syntactic sugar for
        // `local Name; Name = function funcbody end`
        if self.input.check_type(TokenType::Function)? {
            return self.parse_local_function();
        }

        let base = self.outer_local_counts.last().copied().unwrap_or(0);
        let old_local_count = (self.locals.len() - base) as u8;

        let names = self.parse_namelist()?;

        let num_names = names.len() as u8;
        if self.input.try_pop(TokenType::Assign)?.is_some() {
            // Also perform the assignment
            let (num_rvalues, last_exp) = self.parse_explist()?;
            match num_names.cmp(&num_rvalues) {
                Ordering::Less => {
                    for _ in num_names..num_rvalues {
                        self.push(Instr::Pop);
                    }
                }
                Ordering::Greater => {
                    if let ExpDesc::Prefix(PrefixExp::FunctionCall(num_args)) = last_exp {
                        self.pop_instr(); // Remove the old 'Call' instruction
                        self.push(Instr::Call(num_args, 1 + num_names - num_rvalues));
                    } else if let ExpDesc::Prefix(PrefixExp::FunctionCallVar(_)) = last_exp {
                        let func_reg = match self.pop_instr() {
                            Some(Instr::CallVar(reg, _)) => reg,
                            i => unreachable!("FunctionCallVar but last instr was {:?}", i),
                        };
                        self.push(Instr::CallVar(func_reg, 1 + num_names - num_rvalues));
                    } else if matches!(last_exp, ExpDesc::VarArg) {
                        self.pop_instr(); // Remove the old VarArg instruction
                        self.push(Instr::VarArg(1 + num_names - num_rvalues));
                    } else {
                        for _ in num_rvalues..num_names {
                            self.push(Instr::PushNil);
                        }
                    }
                }
                Ordering::Equal => (),
            }
        } else {
            // They've only been declared, just set them all nil
            for _ in &names {
                self.push(Instr::PushNil);
            }
        }

        // Actually perform the assignment
        for i in (0..num_names).rev() {
            self.push(Instr::SetLocal(i + old_local_count));
        }

        // Bring the new variables into scope. It is important they are not
        // in scope until after this statement.
        for name in names {
            self.add_local(name)?;
        }

        Ok(())
    }

    /// Parses `local function Name funcbody`.
    ///
    /// The name is added to locals **before** the body is parsed so that the
    /// function can reference itself recursively.
    fn parse_local_function(&mut self) -> Result<()> {
        self.input.next()?; // consume `function`
        let name = self.expect_identifier()?;

        // Add local BEFORE parsing body so recursion works
        self.add_local(name)?;
        let base = self.outer_local_counts.last().copied().unwrap_or(0);
        let local_slot = (self.locals.len() - 1 - base) as u8;

        // Parse function body -> emits Closure instruction
        self.parse_fndef()?;

        // Assign closure to local
        self.push(Instr::SetLocal(local_slot));
        Ok(())
    }

    /// Parse a comma-separated list of identifiers.
    fn parse_namelist(&mut self) -> Result<Vec<&'a str>> {
        let mut names = vec![self.expect_identifier()?];
        while self.input.try_pop(TokenType::Comma)?.is_some() {
            names.push(self.expect_identifier()?);
        }
        Ok(names)
    }

    /// Parses a `for` loop, dispatching to numeric or generic forms.
    fn parse_for(&mut self) -> Result<()> {
        self.input.next()?; // `for` keyword
        let name = self.expect_identifier()?;
        self.nest_level += 1;
        match self.input.peek_type()? {
            TokenType::Assign => {
                self.input.next()?;
                self.parse_numeric_for(name)?;
            }
            TokenType::Comma | TokenType::In => {
                self.parse_generic_for(name)?;
            }
            _ => return Err(self.error(SyntaxError::UnexpectedTok)),
        }
        self.level_down();
        Ok(())
    }

    /// Parses a numeric `for` loop, starting with the first expression after the `=`.
    fn parse_numeric_for(&mut self, name: &str) -> Result<()> {
        // The start(current), stop and step are stored in three "hidden" local slots.
        let base = self.outer_local_counts.last().copied().unwrap_or(0);
        let current_local_slot = (self.locals.len() - base) as u8;
        self.add_local("")?;
        self.add_local("")?;
        self.add_local("")?;

        // The actual local is in a fourth slot, so that it can be reassigned to.
        let visible_local_idx = self.locals.len();
        self.add_local(name)?;

        // First, all 3 control expressions are evaluated.
        self.parse_expr()?;
        self.expect(TokenType::Comma)?;
        self.parse_expr()?;

        // optional step value
        self.parse_numeric_for_step()?;

        // The ForPrep command pulls three values off the stack and places them
        // into locals to use in the loop.
        let loop_start_instr_index = self.chunk.code.len();
        self.push(Instr::ForPrep(current_local_slot, -1));

        // body
        self.block_stack.push(BlockInfo {
            is_loop: true,
            break_jumps: Vec::new(),
        });
        self.parse_statements()?;
        self.expect(TokenType::End)?;

        // If the for-loop variable or any body locals are captured as upvalues,
        // emit Close before ForLoop so each iteration closes its upvalues.
        // This ensures closures created in different iterations capture
        // distinct values.
        let base = self.outer_local_counts.last().copied().unwrap_or(0);
        let any_captured = self.locals[visible_local_idx..].iter().any(|l| l.captured);
        if any_captured {
            let slot = (visible_local_idx - base) as u8;
            self.push(Instr::Close(slot));
        }

        let body_length = (self.chunk.code.len() - loop_start_instr_index) as isize;
        self.push(Instr::ForLoop(current_local_slot, -(body_length)));

        // Correct the ForPrep instruction.
        self.chunk.code[loop_start_instr_index] = Instr::ForPrep(current_local_slot, body_length);

        let block = self.block_stack.pop().unwrap();
        self.backpatch_breaks(&block);

        Ok(())
    }

    /// Parses a generic `for` loop: `for var {, var} in explist do body end`.
    ///
    /// Three hidden locals hold the iterator state (generator, state, control).
    /// User-declared variables come after those.
    fn parse_generic_for(&mut self, first_name: &str) -> Result<()> {
        // Collect all loop variable names
        let mut names = vec![first_name.to_string()];
        while self.input.try_pop(TokenType::Comma)?.is_some() {
            let name = self.expect_identifier()?;
            names.push(name.to_string());
        }
        self.expect(TokenType::In)?;

        // Parse expression list (e.g. `pairs(t)`)
        let (num_exprs, last_exp) = self.parse_explist()?;

        // Adjust to exactly 3 values (generator, state, control)
        match num_exprs.cmp(&3) {
            Ordering::Less => {
                if num_exprs < 3 {
                    if let ExpDesc::Prefix(PrefixExp::FunctionCall(num_args)) = last_exp {
                        // Last expr is a function call — adjust to return enough
                        self.pop_instr(); // remove the old Call
                        self.push(Instr::Call(num_args, 3 - num_exprs + 1));
                    } else if let ExpDesc::Prefix(PrefixExp::FunctionCallVar(_)) = last_exp {
                        let func_reg = match self.pop_instr() {
                            Some(Instr::CallVar(reg, _)) => reg,
                            i => unreachable!("FunctionCallVar but last instr was {:?}", i),
                        };
                        self.push(Instr::CallVar(func_reg, 3 - num_exprs + 1));
                    } else if matches!(last_exp, ExpDesc::VarArg) {
                        self.pop_instr();
                        self.push(Instr::VarArg(3 - num_exprs + 1));
                    } else {
                        for _ in num_exprs..3 {
                            self.push(Instr::PushNil);
                        }
                    }
                }
            }
            Ordering::Greater => {
                for _ in 3..num_exprs {
                    self.push(Instr::Pop);
                }
            }
            Ordering::Equal => (),
        }

        self.expect(TokenType::Do)?;

        // Add three hidden locals: (for generator), (for state), (for control)
        let base = self.outer_local_counts.last().copied().unwrap_or(0);
        let hidden_base = (self.locals.len() - base) as u8;
        self.add_local("")?; // generator
        self.add_local("")?; // state
        self.add_local("")?; // control

        // Store the 3 iterator values into hidden locals
        for i in (0..3).rev() {
            self.push(Instr::SetLocal(hidden_base + i));
        }

        // Add user-declared loop variables
        let num_vars = names.len() as u8;
        for name in &names {
            self.add_local(name)?;
        }
        // Initialize user variables to nil
        for _ in 0..num_vars {
            self.push(Instr::PushNil);
        }
        for i in (0..num_vars).rev() {
            self.push(Instr::SetLocal(hidden_base + 3 + i));
        }

        // Jump to the TForLoop instruction (skip body on first entry)
        let jump_index = self.chunk.code.len();
        self.push(Instr::Jump(0)); // placeholder

        // Parse body
        self.block_stack.push(BlockInfo {
            is_loop: true,
            break_jumps: Vec::new(),
        });
        let body_start = self.chunk.code.len();
        self.parse_statements()?;
        self.expect(TokenType::End)?;

        // If any loop variables are captured as upvalues, emit Close
        let user_var_start = base + hidden_base as usize + 3;
        let any_captured = self.locals[user_var_start..].iter().any(|l| l.captured);
        if any_captured {
            let slot = (user_var_start - base) as u8;
            self.push(Instr::Close(slot));
        }

        // Emit TForLoop with embedded jump-back offset.
        // After get_instr advances IP past TForLoop, jump(offset) must land
        // exactly at body_start. So offset = body_start - (tforloop_index + 1).
        let tforloop_index = self.chunk.code.len();
        let jump_back = -((tforloop_index - body_start) as isize) - 1;
        self.push(Instr::TForLoop(hidden_base, num_vars, jump_back));

        // Backpatch the initial jump to skip to the TForLoop
        let skip_offset = (tforloop_index - jump_index - 1) as isize;
        self.chunk.code[jump_index] = Instr::Jump(skip_offset);

        let block = self.block_stack.pop().unwrap();
        self.backpatch_breaks(&block);

        Ok(())
    }

    /// Parses the optional step value of a numeric `for` loop.
    fn parse_numeric_for_step(&mut self) -> Result<()> {
        let next_token = self.input.next()?;
        match next_token.typ {
            TokenType::Comma => {
                self.parse_expr()?;
                self.expect(TokenType::Do)?;
                Ok(())
            }
            TokenType::Do => {
                let i = self.find_or_add_number(1.0)?;
                self.push(Instr::PushNum(i));
                Ok(())
            }
            _ => Err(self.err_unexpected(next_token, TokenType::Do)),
        }
    }

    /// Parses a `do ... end` statement.
    fn parse_do(&mut self) -> Result<()> {
        self.input.next()?; // `do` keyword
        self.nest_level += 1;
        self.block_stack.push(BlockInfo {
            is_loop: false,
            break_jumps: Vec::new(),
        });
        self.parse_statements()?;
        self.expect(TokenType::End)?;
        self.block_stack.pop();
        self.level_down();
        Ok(())
    }

    /// Parses a `repeat ... until` statement.
    fn parse_repeat(&mut self) -> Result<()> {
        self.input.next()?; // `repeat` keyword
        self.nest_level += 1;
        self.block_stack.push(BlockInfo {
            is_loop: true,
            break_jumps: Vec::new(),
        });
        let body_start = self.chunk.code.len() as isize;
        self.parse_statements()?;
        self.expect(TokenType::Until)?;
        self.parse_expr()?;
        let expr_end = self.chunk.code.len() as isize;
        self.push(Instr::BranchFalse(body_start - (expr_end + 1)));

        let block = self.block_stack.pop().unwrap();
        self.backpatch_breaks(&block);

        self.level_down();
        Ok(())
    }

    /// Parses a `while ... do ... end` statement.
    fn parse_while(&mut self) -> Result<()> {
        // Structure of while loop instructions:
        // - Condition instructions
        // - `BranchFalse` to evaluate condition and skip body
        // - Body instructions
        // - `Jump` back to condition start
        // - (break jumps land here)
        self.input.next()?;
        self.nest_level += 1;
        let condition_start = self.chunk.code.len();
        self.parse_expr()?;
        self.expect(TokenType::Do)?;

        let test_position = self.chunk.code.len();
        self.push(Instr::BranchFalse(0));

        self.block_stack.push(BlockInfo {
            is_loop: true,
            break_jumps: Vec::new(),
        });
        self.parse_statements()?;
        self.expect(TokenType::End)?;

        let body_end = self.chunk.code.len();
        self.push(Instr::Jump(-((body_end + 1 - condition_start) as isize)));

        let body_len = body_end - test_position;
        self.chunk.code[test_position] = Instr::BranchFalse(body_len as isize);

        let block = self.block_stack.pop().unwrap();
        self.backpatch_breaks(&block);

        self.level_down();

        Ok(())
    }

    /// Parses an if-then statement, including any attached `else` or `elseif` branches.
    fn parse_if(&mut self) -> Result<()> {
        self.parse_if_arm()
    }

    /// Parses an `if` or `elseif` block and any subsequent `elseif` or `else`
    /// blocks in the same chain.
    fn parse_if_arm(&mut self) -> Result<()> {
        self.input.next()?; // `if` or `elseif` keyword
        self.parse_expr()?;
        self.expect(TokenType::Then)?;
        self.nest_level += 1;

        let branch_instr_index = self.chunk.code.len();
        self.push(Instr::BranchFalse(0));

        self.parse_statements()?;
        let mut branch_target = self.chunk.code.len();

        self.close_if_arm()?;
        if self.chunk.code.len() > branch_target {
            // If the size has changed, the first instruction added was a
            // Jump, so we need to skip it.
            branch_target += 1;
        }

        let branch_offset = (branch_target - branch_instr_index - 1) as isize;
        self.chunk.code[branch_instr_index] = Instr::BranchFalse(branch_offset);
        Ok(())
    }

    /// Parses the closing keyword of an `if` or `elseif` arms, and any arms
    /// that may follow.
    fn close_if_arm(&mut self) -> Result<()> {
        self.level_down();
        match self.input.peek_type()? {
            TokenType::ElseIf => self.parse_else_or_elseif(true),
            TokenType::Else => self.parse_else_or_elseif(false),
            _ => {
                self.expect(TokenType::End)?;
                Ok(())
            }
        }
    }

    /// Parses an `elseif` or `else` block, and handles the `Jump` instruction
    /// for the end of the preceding block.
    fn parse_else_or_elseif(&mut self, elseif: bool) -> Result<()> {
        let jump_instr_index = self.chunk.code.len();
        self.push(Instr::Jump(0));
        if elseif {
            self.parse_if_arm()?;
        } else {
            self.parse_else()?;
        }
        let new_len = self.chunk.code.len();
        let jump_len = new_len - jump_instr_index - 1;
        self.chunk.code[jump_instr_index] = Instr::Jump(jump_len as isize);
        Ok(())
    }

    /// Parses an `else` block.
    fn parse_else(&mut self) -> Result<()> {
        self.nest_level += 1;
        self.input.next()?; // `else` keyword
        self.parse_statements()?;
        self.expect(TokenType::End)?;
        self.level_down();
        Ok(())
    }

    /// Parses a comma-separated list of expressions. Trailing and leading
    /// commas are not allowed. Returns how many expressions were parsed and
    /// a descriptor of the last expression.
    fn parse_explist(&mut self) -> Result<(u8, ExpDesc)> {
        // An explist has to have at least one expression.
        let mut last_exp_desc = self.parse_expr()?;
        let mut num_expressions = 1;
        while let Some(token) = self.input.try_pop(TokenType::Comma)? {
            if num_expressions == u8::MAX {
                return Err(self.error_at(SyntaxError::Complexity, token.start));
            }
            last_exp_desc = self.parse_expr()?;
            num_expressions += 1;
        }

        Ok((num_expressions, last_exp_desc))
    }

    /// Parses a single expression.
    fn parse_expr(&mut self) -> Result<ExpDesc> {
        self.parse_or()
    }

    /// Parses an `or` expression. Precedence 8.
    fn parse_or(&mut self) -> Result<ExpDesc> {
        let mut exp_desc = self.parse_and()?;

        while self.input.try_pop(TokenType::Or)?.is_some() {
            exp_desc = ExpDesc::Other;
            let branch_instr_index = self.chunk.code.len();
            self.push(Instr::BranchTrueKeep(0));
            // If we don't short-circuit, pop the left-hand expression
            self.push(Instr::Pop);
            self.parse_and()?;
            let branch_offset = (self.chunk.code.len() - branch_instr_index - 1) as isize;
            self.chunk.code[branch_instr_index] = Instr::BranchTrueKeep(branch_offset);
        }

        Ok(exp_desc)
    }

    /// Parses `and` expression. Precedence 7.
    fn parse_and(&mut self) -> Result<ExpDesc> {
        let mut exp_desc = self.parse_comparison()?;

        while self.input.try_pop(TokenType::And)?.is_some() {
            exp_desc = ExpDesc::Other;
            let branch_instr_index = self.chunk.code.len();
            self.push(Instr::BranchFalseKeep(0));
            // If we don't short-circuit, pop the left-hand expression
            self.push(Instr::Pop);
            self.parse_comparison()?;
            let branch_offset = (self.chunk.code.len() - branch_instr_index - 1) as isize;
            self.chunk.code[branch_instr_index] = Instr::BranchFalseKeep(branch_offset);
        }

        Ok(exp_desc)
    }

    /// Parses a comparison expression. Precedence 6.
    ///
    /// `==`, `~=`, `<`, `<=`, `>`, `>=`
    fn parse_comparison(&mut self) -> Result<ExpDesc> {
        let mut exp_desc = self.parse_concat()?;
        loop {
            let instr = match self.input.peek_type()? {
                TokenType::Less => Instr::Less,
                TokenType::LessEqual => Instr::LessEqual,
                TokenType::Greater => Instr::Greater,
                TokenType::GreaterEqual => Instr::GreaterEqual,
                TokenType::Equal => Instr::Equal,
                TokenType::NotEqual => Instr::NotEqual,
                _ => break,
            };
            exp_desc = ExpDesc::Other;
            self.input.next()?;
            self.parse_concat()?;
            self.push(instr);
        }
        Ok(exp_desc)
    }

    /// Parses a string concatenation expression (`..`). Precedence 5.
    fn parse_concat(&mut self) -> Result<ExpDesc> {
        let mut exp_desc = self.parse_addition()?;
        if self.input.try_pop(TokenType::DotDot)?.is_some() {
            exp_desc = ExpDesc::Other;
            self.parse_concat()?;
            self.push(Instr::Concat);
        }

        Ok(exp_desc)
    }

    /// Parses an addition expression (`+`, `-`). Precedence 4.
    fn parse_addition(&mut self) -> Result<ExpDesc> {
        let mut exp_desc = self.parse_multiplication()?;
        loop {
            let instr = match self.input.peek_type()? {
                TokenType::Plus => Instr::Add,
                TokenType::Minus => Instr::Subtract,
                _ => break,
            };
            exp_desc = ExpDesc::Other;
            self.input.next()?;
            self.parse_multiplication()?;
            self.push(instr);
        }
        Ok(exp_desc)
    }

    /// Parses a multiplication expression (`*`, `/`, `%`). Precedence 3.
    fn parse_multiplication(&mut self) -> Result<ExpDesc> {
        let mut exp_desc = self.parse_unary()?;
        loop {
            let instr = match self.input.peek_type()? {
                TokenType::Star => Instr::Multiply,
                TokenType::Slash => Instr::Divide,
                TokenType::Mod => Instr::Mod,
                _ => break,
            };
            exp_desc = ExpDesc::Other;
            self.input.next()?;
            self.parse_unary()?;
            self.push(instr);
        }
        Ok(exp_desc)
    }

    /// Parses a unary expression (`not`, `#`, `-`). Precedence 2.
    fn parse_unary(&mut self) -> Result<ExpDesc> {
        let instr = match self.input.peek_type()? {
            TokenType::Not => Instr::Not,
            TokenType::Hash => Instr::Length,
            TokenType::Minus => Instr::Negate,
            _ => {
                return self.parse_pow();
            }
        };
        self.input.next()?;
        self.parse_unary()?;
        self.push(instr);

        Ok(ExpDesc::Other)
    }

    /// Parse an exponentiation expression (`^`). Right-associative, Precedence 1.
    fn parse_pow(&mut self) -> Result<ExpDesc> {
        let mut exp_desc = self.parse_primary()?;
        if self.input.try_pop(TokenType::Caret)?.is_some() {
            exp_desc = ExpDesc::Other;
            self.parse_unary()?;
            self.push(Instr::Pow);
        }

        Ok(exp_desc)
    }

    /// Parses a 'primary' expression. See `parse_prefix_exp` and `parse_expr_base` for details.
    fn parse_primary(&mut self) -> Result<ExpDesc> {
        match self.input.peek_type()? {
            TokenType::Identifier | TokenType::LParen | TokenType::LParenLineStart => {
                let prefix = self.parse_prefix_exp()?;
                self.eval_prefix_exp(&prefix);
                Ok(prefix.into())
            }
            _ => self.parse_expr_base(),
        }
    }

    /// Parses a `prefix expression`. Prefix expressions are the expressions
    /// which can appear on the left side of a function call, table index, or
    /// field access.
    fn parse_prefix_exp(&mut self) -> Result<PrefixExp> {
        let tok = self.input.next()?;
        let prefix = match tok.typ {
            TokenType::Identifier => {
                let text = self.get_text(tok);
                let place = self.parse_prefix_identifier(text)?;
                place.into()
            }
            TokenType::LParen | TokenType::LParenLineStart => {
                self.parse_expr()?;
                self.expect(TokenType::RParen)?;
                PrefixExp::Parenthesized
            }
            _ => {
                return Err(self.err_unexpected(tok, TokenType::Identifier));
            }
        };
        self.parse_prefix_extension(prefix)
    }

    /// Attempts to parse an extension to a prefix expression: a field access,
    /// table index, or function/method call.
    fn parse_prefix_extension(&mut self, base_expr: PrefixExp) -> Result<PrefixExp> {
        match self.input.peek_type()? {
            TokenType::Dot => {
                self.eval_prefix_exp(&base_expr);
                self.input.next()?;
                let field_name = self.expect_identifier()?;
                let name_idx = self.find_or_add_string(field_name.as_bytes())?;
                let prefix = PlaceExp::FieldAccess(name_idx).into();
                self.parse_prefix_extension(prefix)
            }
            TokenType::LSquare => {
                self.eval_prefix_exp(&base_expr);
                self.input.next()?;
                self.parse_expr()?;
                self.expect(TokenType::RSquare)?;
                let prefix = PlaceExp::TableIndex.into();
                self.parse_prefix_extension(prefix)
            }
            TokenType::LParen | TokenType::LiteralString | TokenType::LCurly => {
                // Capture the function's register before eval_prefix_exp pushes it.
                #[allow(clippy::cast_possible_truncation)]
                let func_reg = self.freereg as u8;
                self.eval_prefix_exp(&base_expr);
                let (num_args, last_exp) = self.parse_funcargs()?;
                // If the last argument is a multi-return expression (function
                // call or vararg), patch it to return all values and use
                // CallVar with the function's register for correct runtime
                // arg-count computation.
                let prefix = if is_multi_return(&last_exp) && num_args > 0 {
                    self.patch_last_for_multi_return(&last_exp);
                    PrefixExp::FunctionCallVar(func_reg)
                } else {
                    PrefixExp::FunctionCall(num_args)
                };
                self.parse_prefix_extension(prefix)
            }
            TokenType::LParenLineStart => {
                let pos = self.input.next()?.start;
                Err(self.error_at(SyntaxError::LParenLineStart, pos))
            }
            TokenType::Colon => {
                self.eval_prefix_exp(&base_expr);
                self.input.next()?; // consume ':'
                let method_name = self.expect_identifier()?;
                let name_idx = self.find_or_add_string(method_name.as_bytes())?;
                // Self_ pops object, pushes method then object.
                // The method function is at freereg before Self_ is emitted.
                #[allow(clippy::cast_possible_truncation)]
                let func_reg = self.freereg as u8;
                self.push(Instr::Self_(name_idx));
                let (num_args, last_exp) = self.parse_funcargs()?;
                let prefix = if is_multi_return(&last_exp) && num_args > 0 {
                    self.patch_last_for_multi_return(&last_exp);
                    PrefixExp::FunctionCallVar(func_reg)
                } else {
                    PrefixExp::FunctionCall(num_args + 1) // +1 for self
                };
                self.parse_prefix_extension(prefix)
            }
            _ => Ok(base_expr),
        }
    }

    /// Parses function arguments in all three Lua forms:
    /// - `(explist)` — parenthesized arguments
    /// - `"string"` — single string literal argument
    /// - `{fields}` — single table constructor argument
    ///
    /// Returns `(num_args, last_exp)`: the argument count and the `ExpDesc`
    /// of the last argument (used for multi-return expansion).
    fn parse_funcargs(&mut self) -> Result<(u8, ExpDesc)> {
        match self.input.peek_type()? {
            TokenType::LParen => {
                self.input.next()?;
                self.parse_call()
            }
            TokenType::LiteralString => {
                let tok = self.input.next()?;
                let bytes = self.get_literal_string_contents(tok)?;
                let idx = self.find_or_add_string(&bytes)?;
                self.push(Instr::PushString(idx));
                Ok((1, ExpDesc::Other))
            }
            TokenType::LCurly => {
                self.input.next()?; // consume '{'
                self.parse_table()?;
                Ok((1, ExpDesc::Other))
            }
            _ => {
                let tok = self.input.next()?;
                Err(self.err_unexpected(tok, TokenType::LParen))
            }
        }
    }

    /// Parses a 'base' expression, after eliminating any operators. This can be:
    /// * A literal number
    /// * A literal string
    /// * A function definition
    /// * One of the keywords `nil`, `false` or `true
    /// * A table constructor
    fn parse_expr_base(&mut self) -> Result<ExpDesc> {
        let tok = self.input.next()?;
        match tok.typ {
            TokenType::LCurly => self.parse_table()?,
            TokenType::LiteralNumber => {
                let text = self.get_text(tok);
                let number = text.parse().unwrap();
                let idx = self.find_or_add_number(number)?;
                self.push(Instr::PushNum(idx));
            }
            TokenType::LiteralHexNumber => {
                // Cut off the "0x" or "0X" prefix
                let text = &self.get_text(tok)[2..];
                let number = u128::from_str_radix(text, 16).unwrap() as f64;
                let idx = self.find_or_add_number(number)?;
                self.push(Instr::PushNum(idx));
            }
            TokenType::LiteralString => {
                let bytes = self.get_literal_string_contents(tok)?;
                let idx = self.find_or_add_string(&bytes)?;
                self.push(Instr::PushString(idx));
            }
            TokenType::Function => self.parse_fndef()?,
            TokenType::Nil => self.push(Instr::PushNil),
            TokenType::False => self.push(Instr::PushBool(false)),
            TokenType::True => self.push(Instr::PushBool(true)),
            TokenType::DotDotDot => {
                if !self.current_chunk_is_vararg() {
                    return Err(self.error(SyntaxError::VarargOutsideVarargFunc));
                }
                self.push(Instr::VarArg(1)); // default: single value
                return Ok(ExpDesc::VarArg);
            }
            _ => {
                return Err(self.err_unexpected(tok, TokenType::Nil));
            }
        }
        Ok(ExpDesc::Other)
    }

    /// Parses the parameters in a function definition.
    /// Returns the list of named parameters and whether the function is vararg.
    fn parse_params(&mut self) -> Result<(Vec<&'a str>, bool)> {
        let lparen_tok = self.input.next()?;
        match lparen_tok.typ {
            TokenType::LParen | TokenType::LParenLineStart => (),
            _ => return Err(self.err_unexpected(lparen_tok, TokenType::LParen)),
        }
        let mut args = Vec::new();
        let mut is_vararg = false;
        if self.input.try_pop(TokenType::RParen)?.is_some() {
            return Ok((args, false));
        }
        // Check if first token is `...` (vararg-only function)
        if self.input.check_type(TokenType::DotDotDot)? {
            self.input.next()?;
            is_vararg = true;
        } else {
            args.push(self.expect_identifier()?);
            while self.input.try_pop(TokenType::Comma)?.is_some() {
                if self.input.check_type(TokenType::DotDotDot)? {
                    self.input.next()?;
                    is_vararg = true;
                    break;
                }
                args.push(self.expect_identifier()?);
            }
        }
        self.expect(TokenType::RParen)?;
        Ok((args, is_vararg))
    }

    /// Parses the parameters and body of a function definition.
    fn parse_fndef(&mut self) -> Result<()> {
        let (params, is_vararg) = self.parse_params()?;
        if self.chunk.nested.len() >= u8::MAX as usize {
            return Err(self.error(SyntaxError::Complexity));
        }

        self.nest_level += 1;
        let new_chunk = self.parse_chunk(&params, is_vararg)?;
        self.level_down();

        self.chunk.nested.push(new_chunk);
        self.push(Instr::Closure(self.chunk.nested.len() as u8 - 1));
        self.expect(TokenType::End)?;
        Ok(())
    }

    /// Parses a table constructor. Uses `freereg` (analogous to PUC-Rio's
    /// `FuncState.freereg`) to track the table's stack position for
    /// `SetListMulti` when the last array entry is a multi-return expression.
    fn parse_table(&mut self) -> Result<()> {
        // Record the table's stack position before NewTable pushes it.
        // This is `freereg` before the push, i.e., the register/slot where
        // the table will live. Mirrors PUC-Rio's `cc->t->u.s.info`.
        let table_reg = self.freereg;
        self.push(Instr::NewTable);
        if self.input.try_pop(TokenType::RCurly)?.is_none() {
            // i is the number of array-style entries.
            let mut i = 0u8;
            let mut last_array_exp = ExpDesc::Other;
            let (new_i, exp) = self.parse_table_entry(i)?;
            if new_i > i {
                last_array_exp = exp;
            }
            i = new_i;
            while let TokenType::Comma | TokenType::Semi = self.input.peek_type()? {
                self.input.next()?;
                if self.input.check_type(TokenType::RCurly)? {
                    break;
                }
                let (new_i, exp) = self.parse_table_entry(i)?;
                if new_i > i {
                    last_array_exp = exp;
                }
                i = new_i;
            }
            self.expect(TokenType::RCurly)?;

            if i > 0 {
                if is_multi_return(&last_array_exp) {
                    // The last array entry is a multi-return expression
                    // (function call or `...`). Patch it to return all values,
                    // then use SetListMulti so the VM collects everything
                    // between the table and TOS as array entries.
                    self.patch_last_for_multi_return(&last_array_exp);
                    // table_reg is the table's compile-time freereg position.
                    // Record a fixup so parse_chunk can adjust for the VM's
                    // num_locals pre-allocation (which shifts temporaries).
                    let instr_idx = self.chunk.code.len();
                    self.push(Instr::SetListMulti(table_reg as u8));
                    self.register_fixups
                        .push((instr_idx, self.chunk.num_locals));
                    // After SetListMulti, only the table remains at table_reg.
                    self.freereg = table_reg + 1;
                } else {
                    self.push(Instr::SetList(i));
                }
            }
        }
        Ok(())
    }

    /// Parses a table entry. Returns the updated array-style counter and the
    /// `ExpDesc` of the entry (meaningful only for array-style entries).
    fn parse_table_entry(&mut self, counter: u8) -> Result<(u8, ExpDesc)> {
        match self.input.peek_type()? {
            TokenType::Identifier if self.input.peek_next_type()? == TokenType::Assign => {
                // Field assignment: Name '=' expr
                let index = self.expect_identifier_id()?;
                self.expect(TokenType::Assign)?;
                self.parse_expr()?;
                self.push(Instr::InitField(counter, index));
                Ok((counter, ExpDesc::Other))
            }
            TokenType::LSquare => {
                self.input.next().unwrap();
                self.parse_expr()?;
                self.expect(TokenType::RSquare)?;
                self.expect(TokenType::Assign)?;
                self.parse_expr()?;
                self.push(Instr::InitIndex(counter));
                Ok((counter, ExpDesc::Other))
            }
            _ => {
                if counter == u8::MAX {
                    return Err(self.error(SyntaxError::Complexity));
                }
                let exp = self.parse_expr()?;
                Ok((counter + 1, exp))
            }
        }
    }

    /// Patches the last emitted instruction so a multi-return expression
    /// (`FunctionCall` or `VarArg`) returns all values instead of just one.
    /// Also adjusts `freereg` to undo the +1 from the original instruction,
    /// since the actual return count is now dynamic (unknown at compile time).
    fn patch_last_for_multi_return(&mut self, exp: &ExpDesc) {
        match exp {
            ExpDesc::Prefix(PrefixExp::FunctionCall(_)) => {
                // Replace the Call(args, 1) with Call(args, 255) to return all values.
                // 255 signals "variable return count" (multi-return), distinct from
                // Call(args, 0) which means "discard all returns" (expression statement).
                if let Some(Instr::Call(args, old_rets)) = self.chunk.code.last().copied() {
                    let last = self.chunk.code.len() - 1;
                    self.chunk.code[last] = Instr::Call(args, 255);
                    // Undo the freereg adjustment from the original rets count
                    self.freereg -= old_rets as usize;
                }
            }
            ExpDesc::Prefix(PrefixExp::FunctionCallVar(_)) => {
                // Replace CallVar(reg, 1) with CallVar(reg, 255) for multi-return.
                if let Some(Instr::CallVar(reg, old_rets)) = self.chunk.code.last().copied() {
                    let last = self.chunk.code.len() - 1;
                    self.chunk.code[last] = Instr::CallVar(reg, 255);
                    self.freereg -= old_rets as usize;
                }
            }
            ExpDesc::VarArg => {
                // Replace VarArg(1) with VarArg(0) to push all varargs
                if let Some(Instr::VarArg(old_n)) = self.chunk.code.last().copied() {
                    let last = self.chunk.code.len() - 1;
                    self.chunk.code[last] = Instr::VarArg(0);
                    // Undo the freereg adjustment from the original count
                    self.freereg -= old_n as usize;
                }
            }
            _ => {}
        }
    }

    /// Parses a function call. Returns the number of arguments.
    fn parse_call(&mut self) -> Result<(u8, ExpDesc)> {
        let tup = if self.input.check_type(TokenType::RParen)? {
            (0, ExpDesc::Other)
        } else {
            self.parse_explist()?
        };
        self.expect(TokenType::RParen)?;
        Ok(tup)
    }
}

/// Returns true if the expression can expand to multiple values.
fn is_multi_return(exp: &ExpDesc) -> bool {
    matches!(
        exp,
        ExpDesc::VarArg
            | ExpDesc::Prefix(PrefixExp::FunctionCall(_))
            | ExpDesc::Prefix(PrefixExp::FunctionCallVar(_))
    )
}

/// Processes escape sequences in a short string's raw content (after quote
/// stripping). Implements the Lua 5.1.1 escape rules from `llex.c:287-316`:
///
/// - Named escapes: `\a`, `\b`, `\f`, `\n`, `\r`, `\t`, `\v`
/// - Decimal byte escapes: `\ddd` (up to 3 digits, value 0-255)
/// Normalizes line endings in long string/comment bodies.
/// Converts `\r\n` and standalone `\r` to `\n` per Lua spec (llex.c:257-263).
fn normalize_line_endings(s: &str) -> String {
    let mut result = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\r' {
            result.push('\n');
            // Consume a following \n if present (\r\n -> single \n)
            if chars.peek() == Some(&'\n') {
                chars.next();
            }
        } else {
            result.push(c);
        }
    }
    result
}

/// - Backslash-newline: `\` followed by `\n` or `\r` inserts a newline
/// - Default: unknown escapes drop the backslash (e.g. `\z` -> `z`)
fn process_escapes(
    raw: &str,
    tok_start: usize,
    line_col: impl Fn(usize) -> (usize, usize),
) -> Result<Vec<u8>> {
    let mut result = Vec::with_capacity(raw.len());
    let mut chars = raw.chars().peekable();

    while let Some(c) = chars.next() {
        if c != '\\' {
            // Non-escape characters: encode as UTF-8 bytes (source is text)
            let mut buf = [0u8; 4];
            result.extend_from_slice(c.encode_utf8(&mut buf).as_bytes());
            continue;
        }

        // Consume the character after the backslash
        let Some(esc) = chars.next() else {
            // Backslash at end of string — shouldn't happen since the
            // lexer ensures the closing quote exists, but handle gracefully
            result.push(b'\\');
            break;
        };

        match esc {
            'a' => result.push(0x07),
            'b' => result.push(0x08),
            'f' => result.push(0x0C),
            'n' => result.push(b'\n'),
            'r' => result.push(b'\r'),
            't' => result.push(b'\t'),
            'v' => result.push(0x0B),
            '\n' | '\r' => {
                // Backslash-newline: insert a newline. For \r\n or \n\r
                // pairs, consume the second character too.
                result.push(b'\n');
                if let Some(&next) = chars.peek() {
                    if (esc == '\r' && next == '\n') || (esc == '\n' && next == '\r') {
                        chars.next();
                    }
                }
            }
            '0'..='9' => {
                // Decimal byte escape: up to 3 digits, value 0-255
                let mut value: u32 = u32::from(esc as u8 - b'0');
                let mut count = 1;
                while count < 3 {
                    if let Some(&next) = chars.peek() {
                        if next.is_ascii_digit() {
                            value = value * 10 + u32::from(next as u8 - b'0');
                            chars.next();
                            count += 1;
                        } else {
                            break;
                        }
                    } else {
                        break;
                    }
                }
                if value > 255 {
                    let (line, col) = line_col(tok_start);
                    return Err(Error::new(SyntaxError::EscapeTooLarge, line, col));
                }
                // THE FIX: push exactly one byte, not a multi-byte UTF-8 char
                result.push(value as u8);
            }
            // Default: drop the backslash, keep the character.
            // This handles \\, \", \', and any unknown escape.
            other => {
                let mut buf = [0u8; 4];
                result.extend_from_slice(other.encode_utf8(&mut buf).as_bytes());
            }
        }
    }

    Ok(result)
}

/// Finds the index of the last local entry which matches `name`,
/// searching only within the current function's scope (from `base` to end).
#[must_use]
fn find_last_local(locals: &[LocalInfo], name: &str, base: usize) -> Option<usize> {
    let mut i = locals.len();
    while i > base {
        i -= 1;
        if locals[i].name == name {
            return Some(i);
        }
    }

    None
}

/// Returns the index of an entry in the literals list, adding it if it does not exist.
fn find_or_add<T, E>(queue: &mut Vec<T>, x: &E) -> Option<u8>
where
    T: Borrow<E> + PartialEq<E>,
    E: PartialEq<T> + ToOwned<Owned = T> + ?Sized,
{
    if let Some(i) = queue.iter().position(|y| y == x) {
        Some(i as u8)
    } else {
        let i = queue.len();
        if i == u8::MAX as usize {
            None
        } else {
            queue.push(x.to_owned());
            Some(i as u8)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::Chunk;
    use super::Instr::{self, *};
    use super::parse_str;

    fn check_it(input: &str, mut output: Chunk) {
        // Top-level chunks are always vararg
        output.is_vararg = true;
        assert_eq!(parse_str(input).unwrap(), output);
    }

    #[test]
    fn test01() {
        let text = "x = 5 + 6";
        let out = Chunk {
            code: vec![PushNum(0), PushNum(1), Add, SetGlobal(0), Return(0)],
            number_literals: vec![5.0, 6.0],
            string_literals: vec![b"x".to_vec()],
            ..Chunk::default()
        };
        check_it(text, out);
    }

    #[test]
    fn test02() {
        let text = "x = -5^2";
        let out = Chunk {
            code: vec![PushNum(0), PushNum(1), Pow, Negate, SetGlobal(0), Return(0)],
            number_literals: vec![5.0, 2.0],
            string_literals: vec![b"x".to_vec()],
            ..Chunk::default()
        };
        check_it(text, out);
    }

    #[test]
    fn test03() {
        let text = "x = 5 + true .. 'hi'";
        let out = Chunk {
            code: vec![
                PushNum(0),
                PushBool(true),
                Add,
                PushString(1),
                Concat,
                SetGlobal(0),
                Return(0),
            ],
            number_literals: vec![5.0],
            string_literals: vec![b"x".to_vec(), b"hi".to_vec()],
            ..Chunk::default()
        };
        check_it(text, out);
    }

    #[test]
    fn test04() {
        let text = "x = 1 .. 2 + 3";
        let output = Chunk {
            code: vec![
                PushNum(0),
                PushNum(1),
                PushNum(2),
                Add,
                Concat,
                SetGlobal(0),
                Return(0),
            ],
            number_literals: vec![1.0, 2.0, 3.0],
            string_literals: vec![b"x".to_vec()],
            ..Chunk::default()
        };
        check_it(text, output);
    }

    #[test]
    fn test05() {
        let text = "x = 2^-3";
        let output = Chunk {
            code: vec![PushNum(0), PushNum(1), Negate, Pow, SetGlobal(0), Return(0)],
            number_literals: vec![2.0, 3.0],
            string_literals: vec![b"x".to_vec()],
            ..Chunk::default()
        };
        check_it(text, output);
    }

    #[test]
    fn test06() {
        let text = "x=  not not 1";
        let output = Chunk {
            code: vec![PushNum(0), Instr::Not, Instr::Not, SetGlobal(0), Return(0)],
            number_literals: vec![1.0],
            string_literals: vec![b"x".to_vec()],
            ..Chunk::default()
        };
        check_it(text, output);
    }

    #[test]
    fn test07() {
        let text = "a = 5";
        let output = Chunk {
            code: vec![PushNum(0), SetGlobal(0), Return(0)],
            number_literals: vec![5.0],
            string_literals: vec![b"a".to_vec()],
            ..Chunk::default()
        };
        check_it(text, output);
    }

    #[test]
    fn test08() {
        let text = "x = true and false";
        let output = Chunk {
            code: vec![
                PushBool(true),
                BranchFalseKeep(2),
                Pop,
                PushBool(false),
                SetGlobal(0),
                Return(0),
            ],
            string_literals: vec![b"x".to_vec()],
            ..Chunk::default()
        };
        check_it(text, output);
    }

    #[test]
    fn test09() {
        let text = "x =  5 or nil and true";
        let code = vec![
            PushNum(0),
            BranchTrueKeep(5),
            Pop,
            PushNil,
            BranchFalseKeep(2),
            Pop,
            PushBool(true),
            SetGlobal(0),
            Return(0),
        ];
        let output = Chunk {
            code,
            number_literals: vec![5.0],
            string_literals: vec![b"x".to_vec()],
            ..Chunk::default()
        };
        check_it(text, output);
    }

    #[test]
    fn test10() {
        let text = "if true then a = 5 end";
        let code = vec![
            PushBool(true),
            BranchFalse(2),
            PushNum(0),
            SetGlobal(0),
            Return(0),
        ];
        let chunk = Chunk {
            code,
            number_literals: vec![5.0],
            string_literals: vec![b"a".to_vec()],
            ..Chunk::default()
        };
        check_it(text, chunk);
    }

    #[test]
    fn test11() {
        let text = "if true then a = 5 if true then b = 4 end end";
        let code = vec![
            PushBool(true),
            BranchFalse(6),
            PushNum(0),
            SetGlobal(0),
            PushBool(true),
            BranchFalse(2),
            PushNum(1),
            SetGlobal(1),
            Return(0),
        ];
        let chunk = Chunk {
            code,
            number_literals: vec![5.0, 4.0],
            string_literals: vec![b"a".to_vec(), b"b".to_vec()],
            ..Chunk::default()
        };
        check_it(text, chunk);
    }

    #[test]
    fn test12() {
        let text = "if true then a = 5 else a = 4 end";
        let code = vec![
            PushBool(true),
            BranchFalse(3),
            PushNum(0),
            SetGlobal(0),
            Jump(2),
            PushNum(1),
            SetGlobal(0),
            Return(0),
        ];
        let chunk = Chunk {
            code,
            number_literals: vec![5.0, 4.0],
            string_literals: vec![b"a".to_vec()],
            ..Chunk::default()
        };
        check_it(text, chunk);
    }

    #[test]
    fn test13() {
        let text = "if true then a = 5 elseif 6 == 7 then a = 3 else a = 4 end";
        let code = vec![
            PushBool(true),
            BranchFalse(3),
            PushNum(0),
            SetGlobal(0),
            Jump(9),
            PushNum(1),
            PushNum(2),
            Instr::Equal,
            BranchFalse(3),
            PushNum(3),
            SetGlobal(0),
            Jump(2),
            PushNum(4),
            SetGlobal(0),
            Return(0),
        ];
        let chunk = Chunk {
            code,
            number_literals: vec![5.0, 6.0, 7.0, 3.0, 4.0],
            string_literals: vec![b"a".to_vec()],
            ..Chunk::default()
        };
        check_it(text, chunk);
    }

    #[test]
    fn test14() {
        let text = "while a < 10 do a = a + 1 end";
        let code = vec![
            GetGlobal(0),
            PushNum(0),
            Instr::Less,
            BranchFalse(5),
            GetGlobal(0),
            PushNum(1),
            Add,
            SetGlobal(0),
            Jump(-9),
            Return(0),
        ];
        let chunk = Chunk {
            code,
            number_literals: vec![10.0, 1.0],
            string_literals: vec![b"a".to_vec()],
            ..Chunk::default()
        };
        check_it(text, chunk);
    }

    #[test]
    fn test15() {
        let text = "repeat local x = 5 until a == b y = 4";
        let code = vec![
            PushNum(0),
            SetLocal(0),
            GetGlobal(0),
            GetGlobal(1),
            Instr::Equal,
            BranchFalse(-6),
            PushNum(1),
            SetGlobal(2),
            Return(0),
        ];
        let chunk = Chunk {
            code,
            number_literals: vec![5.0, 4.0],
            string_literals: vec![b"a".to_vec(), b"b".to_vec(), b"y".to_vec()],
            num_locals: 1,
            ..Chunk::default()
        };
        check_it(text, chunk);
    }

    #[test]
    fn test16() {
        let text = "local i i = 2";
        let code = vec![PushNil, SetLocal(0), PushNum(0), SetLocal(0), Return(0)];
        let chunk = Chunk {
            code,
            number_literals: vec![2.0],
            num_locals: 1,
            ..Chunk::default()
        };
        check_it(text, chunk);
    }

    #[test]
    fn test17() {
        let text = "local i, j print(j)";
        let code = vec![
            PushNil,
            PushNil,
            SetLocal(1),
            SetLocal(0),
            GetGlobal(0),
            GetLocal(1),
            Call(1, 0),
            Return(0),
        ];
        let chunk = Chunk {
            code,
            string_literals: vec![b"print".to_vec()],
            num_locals: 2,
            ..Chunk::default()
        };
        check_it(text, chunk);
    }

    #[test]
    fn test18() {
        let text = "local i do local i x = i end x = i";
        let code = vec![
            PushNil,
            SetLocal(0),
            PushNil,
            SetLocal(1),
            GetLocal(1),
            SetGlobal(0),
            GetLocal(0),
            SetGlobal(0),
            Return(0),
        ];
        let chunk = Chunk {
            code,
            string_literals: vec![b"x".to_vec()],
            num_locals: 2,
            ..Chunk::default()
        };
        check_it(text, chunk);
    }

    #[test]
    fn test19() {
        let text = "do local i x = i end x = i";
        let code = vec![
            PushNil,
            SetLocal(0),
            GetLocal(0),
            SetGlobal(0),
            GetGlobal(1),
            SetGlobal(0),
            Return(0),
        ];
        let chunk = Chunk {
            code,
            string_literals: vec![b"x".to_vec(), b"i".to_vec()],
            num_locals: 1,
            ..Chunk::default()
        };
        check_it(text, chunk);
    }

    #[test]
    fn test20() {
        let text = "local i if false then local i else x = i end";
        let code = vec![
            PushNil,
            SetLocal(0),
            PushBool(false),
            BranchFalse(3),
            PushNil,
            SetLocal(1),
            Jump(2),
            GetLocal(0),
            SetGlobal(0),
            Return(0),
        ];
        let chunk = Chunk {
            code,
            string_literals: vec![b"x".to_vec()],
            num_locals: 2,
            ..Chunk::default()
        };
        check_it(text, chunk);
    }

    #[test]
    fn test21() {
        let text = "for i = 1,5 do x = i end";
        let code = vec![
            PushNum(0),
            PushNum(1),
            PushNum(0),
            ForPrep(0, 3),
            GetLocal(3),
            SetGlobal(0),
            ForLoop(0, -3),
            Return(0),
        ];
        let chunk = Chunk {
            code,
            number_literals: vec![1.0, 5.0],
            string_literals: vec![b"x".to_vec()],
            num_locals: 4,
            ..Chunk::default()
        };
        check_it(text, chunk);
    }

    #[test]
    fn test22() {
        let text = "a, b = 1";
        let code = vec![PushNum(0), PushNil, SetGlobal(1), SetGlobal(0), Return(0)];
        let chunk = Chunk {
            code,
            number_literals: vec![1.0],
            string_literals: vec![b"a".to_vec(), b"b".to_vec()],
            ..Chunk::default()
        };
        check_it(text, chunk);
    }

    #[test]
    fn test23() {
        let text = "a, b = 1, 2";
        let code = vec![
            PushNum(0),
            PushNum(1),
            SetGlobal(1),
            SetGlobal(0),
            Return(0),
        ];
        let chunk = Chunk {
            code,
            number_literals: vec![1.0, 2.0],
            string_literals: vec![b"a".to_vec(), b"b".to_vec()],
            ..Chunk::default()
        };
        check_it(text, chunk);
    }

    #[test]
    fn test24() {
        let text = "a, b = 1, 2, 3";
        let code = vec![
            PushNum(0),
            PushNum(1),
            PushNum(2),
            Pop,
            SetGlobal(1),
            SetGlobal(0),
            Return(0),
        ];
        let chunk = Chunk {
            code,
            number_literals: vec![1.0, 2.0, 3.0],
            string_literals: vec![b"a".to_vec(), b"b".to_vec()],
            ..Chunk::default()
        };
        check_it(text, chunk);
    }

    #[test]
    fn test25() {
        let text = "puts()";
        let code = vec![GetGlobal(0), Call(0, 0), Return(0)];
        let chunk = Chunk {
            code,
            string_literals: vec![b"puts".to_vec()],
            ..Chunk::default()
        };
        check_it(text, chunk);
    }

    #[test]
    fn test26() {
        let text = "y = {x = 5,}";
        let code = vec![
            NewTable,
            PushNum(0),
            InitField(0, 1),
            SetGlobal(0),
            Return(0),
        ];
        let chunk = Chunk {
            code,
            number_literals: vec![5.0],
            string_literals: vec![b"y".to_vec(), b"x".to_vec()],
            ..Chunk::default()
        };
        check_it(text, chunk);
    }

    #[test]
    fn test27() {
        let text = "local x = t.x.y";
        let code = vec![
            GetGlobal(0),
            GetField(1),
            GetField(2),
            SetLocal(0),
            Return(0),
        ];
        let chunk = Chunk {
            code,
            string_literals: vec![b"t".to_vec(), b"x".to_vec(), b"y".to_vec()],
            num_locals: 1,
            ..Chunk::default()
        };
        check_it(text, chunk);
    }

    #[test]
    fn test28() {
        let text = "x = function () end";
        let code = vec![Closure(0), SetGlobal(0), Return(0)];
        let string_literals = vec![b"x".to_vec()];
        let nested = vec![Chunk {
            code: vec![Return(0)],
            ..Chunk::default()
        }];
        let chunk = Chunk {
            code,
            string_literals,
            nested,
            ..Chunk::default()
        };
        check_it(text, chunk);
    }

    #[test]
    fn test29() {
        let text = "x = function () local y = 7 end";
        let inner_chunk = Chunk {
            code: vec![PushNum(0), SetLocal(0), Return(0)],
            number_literals: vec![7.0],
            num_locals: 1,
            ..Chunk::default()
        };
        let outer_chunk = Chunk {
            code: vec![Closure(0), SetGlobal(0), Return(0)],
            string_literals: vec![b"x".to_vec()],
            nested: vec![inner_chunk],
            ..Chunk::default()
        };
        check_it(text, outer_chunk);
    }

    #[test]
    fn test30() {
        let text = "
        z = function () local z = 21 end
        x = function ()
            local y = function () end
            print(y)
        end";
        let z = Chunk {
            code: vec![PushNum(0), SetLocal(0), Return(0)],
            number_literals: vec![21.0],
            num_locals: 1,
            ..Chunk::default()
        };
        let y = Chunk {
            code: vec![Return(0)],
            ..Chunk::default()
        };
        let x = Chunk {
            code: vec![
                Closure(0),
                SetLocal(0),
                GetGlobal(0),
                GetLocal(0),
                Call(1, 0),
                Return(0),
            ],
            string_literals: vec![b"print".to_vec()],
            nested: vec![y],
            num_locals: 1,
            ..Chunk::default()
        };
        let outer_chunk = Chunk {
            code: vec![
                Closure(0),
                SetGlobal(0),
                Closure(1),
                SetGlobal(1),
                Return(0),
            ],
            nested: vec![z, x],
            string_literals: vec![b"z".to_vec(), b"x".to_vec()],
            ..Chunk::default()
        };
        check_it(text, outer_chunk);
    }

    #[test]
    fn test31() {
        let text = "local s = type(4)";
        let code = vec![GetGlobal(0), PushNum(0), Call(1, 1), SetLocal(0), Return(0)];
        let chunk = Chunk {
            code,
            num_locals: 1,
            number_literals: vec![4.0],
            string_literals: vec![b"type".to_vec()],
            ..Chunk::default()
        };
        check_it(text, chunk);
    }

    #[test]
    fn test32() {
        // `print(type(nil))`: type(nil) is the last arg to print, so it
        // triggers multi-return expansion: Call(1, 255) for type() and
        // CallVar(2, 0) for print() (func_reg=2 because locals occupy 0,1).
        let text = "local type, print print(type(nil))";
        let code = vec![
            PushNil,
            PushNil,
            SetLocal(1),
            SetLocal(0),
            GetLocal(1),
            GetLocal(0),
            PushNil,
            Call(1, 255),
            CallVar(2, 0),
            Return(0),
        ];
        let chunk = Chunk {
            code,
            num_locals: 2,
            ..Chunk::default()
        };
        check_it(text, chunk);
    }

    #[test]
    fn test33() {
        use super::*;
        let text = "print()\n(foo)()\n";
        match parse_str(text) {
            Err(Error {
                kind: ErrorKind::SyntaxError(SyntaxError::LParenLineStart),
                line_num,
                column,
            }) => {
                assert_eq!(line_num, 2);
                assert_eq!(column, 1);
            }
            _ => panic!("Should detect ambiguous function call because of linebreak"),
        }
    }

    #[test]
    fn test34() {
        let text = "while false do local b end b()";
        let code = vec![
            PushBool(false),
            BranchFalse(3),
            PushNil,
            SetLocal(0),
            Jump(-5),
            GetGlobal(0),
            Call(0, 0),
            Return(0),
        ];
        let chunk = Chunk {
            code,
            num_locals: 1,
            string_literals: vec![b"b".to_vec()],
            ..Chunk::default()
        };
        check_it(text, chunk);
    }
}
