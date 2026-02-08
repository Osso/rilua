//! This module contains functions which can tokenize a string input.

use super::Result;
use super::error::Error;
use super::error::SyntaxError;
use super::token::Token;
use super::token::TokenType::{self, *};

use std::iter::Peekable;
use std::slice::SliceIndex;
use std::str::CharIndices;

/// A `TokenStream` is a wrapper around a `Lexer`. It provides a lookahead buffer and several
/// helper methods.
#[derive(Debug)]
pub(super) struct TokenStream<'a> {
    lexer: Lexer<'a>,
    lookahead: Option<Token>,
    /// Second lookahead token, used only for table constructor disambiguation
    /// (PUC-Rio's `luaX_lookahead`). Filled by `peek_next_type`.
    lookahead2: Option<Token>,
}

/// A `Lexer` handles the raw conversion of characters to tokens.
#[derive(Debug)]
pub(super) struct Lexer<'a> {
    /// The starting position of the next character.
    pos: usize,
    /// `linebreaks[i]` is the byte offset of the start of line `i`.
    linebreaks: Vec<usize>,
    iter: Peekable<CharIndices<'a>>,
    source: &'a str,
}

impl<'a> TokenStream<'a> {
    /// Constructs a new `TokenStream`.
    #[must_use]
    pub(super) fn new(source: &'a str) -> Self {
        Self {
            lexer: Lexer::new(source),
            lookahead: None,
            lookahead2: None,
        }
    }

    /// Returns the next `Token`.
    pub(super) fn next(&mut self) -> Result<Token> {
        match self.lookahead.take() {
            Some(token) => {
                // Promote lookahead2 to lookahead if present.
                self.lookahead = self.lookahead2.take();
                Ok(token)
            }
            None => self.lexer.next_token(),
        }
    }

    /// Returns the next `Token`, without popping it from the stream.
    pub(super) fn peek(&mut self) -> Result<&Token> {
        if self.lookahead.is_none() {
            self.lookahead = Some(self.lexer.next_token()?);
        }
        Ok(self.lookahead.as_ref().unwrap())
    }

    /// Returns the type of the next token.
    pub(super) fn peek_type(&mut self) -> Result<TokenType> {
        Ok(self.peek()?.typ)
    }

    /// Returns the type of the token *after* the next token (two-token lookahead).
    /// Used only for table constructor disambiguation: `Name '='` vs expression.
    /// Mirrors PUC-Rio's `luaX_lookahead`.
    pub(super) fn peek_next_type(&mut self) -> Result<TokenType> {
        // Ensure the first lookahead is filled.
        if self.lookahead.is_none() {
            self.lookahead = Some(self.lexer.next_token()?);
        }
        // Fill the second lookahead if needed.
        if self.lookahead2.is_none() {
            self.lookahead2 = Some(self.lexer.next_token()?);
        }
        Ok(self.lookahead2.as_ref().unwrap().typ)
    }

    /// Returns whether the next token is of the given type.
    pub(super) fn check_type(&mut self, expected_type: TokenType) -> Result<bool> {
        Ok(self.peek_type()? == expected_type)
    }

    /// Checks the next token's type. If it matches `expected_type`, it is popped off and
    /// returned as `Some`. Otherwise, returns `None`.
    pub(super) fn try_pop(&mut self, expected_type: TokenType) -> Result<Option<Token>> {
        if self.check_type(expected_type)? {
            Ok(Some(self.next().unwrap()))
        } else {
            Ok(None)
        }
    }

    /// Returns the current position of the `TokenStream`.
    #[must_use]
    pub(super) fn line_and_column(&self, pos: usize) -> (usize, usize) {
        self.lexer.line_and_col(pos)
    }

    /// Returns how many bytes have been read.
    #[must_use]
    pub(super) fn pos(&self) -> usize {
        match &self.lookahead {
            Some(token) => token.start,
            None => self.lexer.pos,
        }
    }

    /// Returns a substring from the source code.
    #[must_use]
    pub(super) fn substring(&self, index: impl SliceIndex<str, Output = str>) -> &'a str {
        &self.lexer.source[index]
    }
}

impl<'a> Lexer<'a> {
    /// Constructs a new `Lexer`.
    #[must_use]
    pub(super) fn new(source: &'a str) -> Self {
        let linebreaks = vec![0];
        Self {
            iter: source.char_indices().peekable(),
            linebreaks,
            pos: 0,
            source,
        }
    }

    /// Returns the next `Token`.
    pub(super) fn next_token(&mut self) -> Result<Token> {
        let starts_line = self.consume_whitespace();
        let tok_start = self.pos;
        if let Some(first_char) = self.next_char() {
            let tok_type = match first_char {
                '+' => Plus,
                '*' => Star,
                '/' => Slash,
                '%' => Mod,
                '^' => Caret,
                '#' => Hash,
                ';' => Semi,
                ':' => Colon,
                ',' => Comma,
                '(' if starts_line => LParenLineStart,
                '(' => LParen,
                ')' => RParen,
                '{' => LCurly,
                '}' => RCurly,
                ']' => RSquare,

                '.' => self.peek_dot(tok_start)?,

                '=' | '<' | '>' | '~' => self.peek_equals(tok_start, first_char)?,

                '-' => {
                    if self.try_next('-') {
                        return self.comment();
                    } else {
                        Minus
                    }
                }

                '\'' | '\"' => self.lex_string(first_char, tok_start)?,
                '[' => match self.peek_char() {
                    Some('[') | Some('=') => match self.check_long_bracket_open() {
                        Some(level) => {
                            self.read_long_bracket_body(level, true)?;
                            LiteralString
                        }
                        None => return Err(self.error(SyntaxError::UnexpectedTok)),
                    },
                    _ => LSquare,
                },

                '0'..='9' => self.lex_full_number(tok_start, first_char)?,

                'a'..='z' | 'A'..='Z' | '_' => self.lex_word(first_char),

                _ => return Err(self.error(SyntaxError::InvalidCharacter(first_char))),
            };
            let len = (self.pos - tok_start) as u32;
            let token = Token {
                typ: tok_type,
                start: tok_start,
                len,
            };
            Ok(token)
        } else {
            Ok(self.end_of_file())
        }
    }

    /// Skips over the characters in a comment.
    fn comment(&mut self) -> Result<Token> {
        if self.peek_char() == Some('[') {
            self.next_char();
            if let Some(level) = self.check_long_bracket_open() {
                self.read_long_bracket_body(level, false)?;
                return self.next_token();
            }
            // Not a long bracket (e.g. `--[not a long comment`).
            // The `[` and any `=` signs were consumed, but that's fine —
            // we just fall through to consume the rest of the line.
        }
        // Short comment: scan until newline or EOF. The newline is consumed
        // by next_char() which handles line tracking (including \r\n pairs).
        while let Some(c) = self.peek_char() {
            if c == '\n' || c == '\r' {
                self.next_char(); // consume the newline (handles \r\n pairs)
                return self.next_token();
            }
            self.next_char();
        }
        Ok(self.end_of_file())
    }

    /// Called after the first `[` of a potential long bracket is consumed.
    /// Counts `=` signs and checks for the second `[`.
    ///
    /// Returns `Some(level)` if the full `[=*[` pattern is found, consuming
    /// all matched characters. Returns `None` otherwise (any consumed `=`
    /// signs are lost, which is acceptable in the comment and error paths).
    fn check_long_bracket_open(&mut self) -> Option<usize> {
        let mut level = 0;
        while self.peek_char() == Some('=') {
            self.next_char();
            level += 1;
        }
        if self.peek_char() == Some('[') {
            self.next_char();
            Some(level)
        } else {
            None
        }
    }

    /// Called after a `]` is consumed inside a long bracket body. Counts `=`
    /// signs and checks for the closing `]` at the matching level.
    ///
    /// Returns `true` and consumes the closing `]` if levels match. Returns
    /// `false` otherwise (consumed `=` signs become part of the content).
    fn check_long_bracket_close(&mut self, level: usize) -> bool {
        let mut count = 0;
        while self.peek_char() == Some('=') {
            self.next_char();
            count += 1;
        }
        if count == level && self.peek_char() == Some(']') {
            self.next_char();
            true
        } else {
            false
        }
    }

    /// Reads the body of a long bracket (string or comment) after the opening
    /// `[=*[` has been consumed. Scans until the matching `]=*]` is found.
    ///
    /// Per the Lua spec, a leading newline immediately after the opening
    /// bracket is skipped.
    fn read_long_bracket_body(&mut self, level: usize, is_string: bool) -> Result<()> {
        // Skip an optional leading newline (\n, \r, \r\n, or \n\r).
        // next_char handles the \r\n / \n\r pair consumption.
        if matches!(self.peek_char(), Some('\n') | Some('\r')) {
            self.next_char();
        }
        loop {
            match self.next_char() {
                None => {
                    let kind = if is_string {
                        SyntaxError::UnfinishedLongString
                    } else {
                        SyntaxError::UnfinishedLongComment
                    };
                    return Err(self.error(kind));
                }
                Some(']') => {
                    if self.check_long_bracket_close(level) {
                        return Ok(());
                    }
                }
                Some(_) => {}
            }
        }
    }

    /// Peeks the next character.
    #[must_use]
    fn peek_char(&mut self) -> Option<char> {
        self.iter.peek().map(|(_, c)| *c)
    }

    /// Pops and returns the next character. Handles line ending tracking:
    /// `\n`, `\r`, `\r\n`, and `\n\r` are all single line endings (matching
    /// Lua 5.1.1's `inclinenumber` in `llex.c:127-135`).
    fn next_char(&mut self) -> Option<char> {
        match self.iter.next() {
            Some((pos, c)) => {
                self.pos = pos + c.len_utf8();
                if c == '\n' || c == '\r' {
                    // Consume the second character of a \r\n or \n\r pair
                    if let Some(&(next_pos, next_c)) = self.iter.peek() {
                        if (c == '\r' && next_c == '\n') || (c == '\n' && next_c == '\r') {
                            self.iter.next();
                            self.pos = next_pos + next_c.len_utf8();
                        }
                    }
                    self.linebreaks.push(self.pos);
                }
                Some(c)
            }
            None => None,
        }
    }

    /// Consumes any whitespace characters. Returns whether or not a newline was consumed.
    fn consume_whitespace(&mut self) -> bool {
        let mut ret = false;
        while let Some(c) = self.peek_char() {
            if !c.is_ascii_whitespace() {
                break;
            }
            if c == '\n' || c == '\r' {
                ret = true;
            }
            self.next_char();
        }
        ret
    }

    /// Move a character forward, only if the current character matches
    /// `expected`.
    fn try_next(&mut self, expected: char) -> bool {
        match self.peek_char() {
            Some(c) if c == expected => {
                self.next_char();
                true
            }
            _ => false,
        }
    }

    /// Constructs an error of the given kind at the current position.
    #[must_use]
    fn error(&self, kind: SyntaxError) -> Error {
        let (line_num, column) = self.line_and_col(self.pos);
        Error::new(kind, line_num, column)
    }

    /// The lexer just read a `.`.
    /// Determines whether it is part of a `Dot`, `DotDot`, `DotDotDot` or `Number`.
    fn peek_dot(&mut self, tok_start: usize) -> Result<TokenType> {
        let typ = match self.peek_char() {
            Some('.') => {
                self.next_char();
                if self.try_next('.') {
                    DotDotDot
                } else {
                    DotDot
                }
            }
            Some(c) if c.is_ascii_digit() => {
                self.next_char();
                self.lex_number_after_decimal(tok_start)?;
                LiteralNumber
            }
            _ => Dot,
        };
        Ok(typ)
    }

    /// The lexer just read something which might be part of a two-character
    /// operator, with `=` as the second character.
    ///
    /// Returns `Err` if the first character is `~` and it is not paired with a
    /// `=`.
    fn peek_equals(&mut self, _tok_start: usize, first_char: char) -> Result<TokenType> {
        if self.try_next('=') {
            let typ = match first_char {
                '=' => Equal,
                '~' => NotEqual,
                '<' => LessEqual,
                '>' => GreaterEqual,
                _ => panic!("peek_equals was called with first_char = {first_char}"),
            };
            Ok(typ)
        } else {
            match first_char {
                '=' => Ok(Assign),
                '<' => Ok(Less),
                '>' => Ok(Greater),
                '~' => Err(self.error(SyntaxError::InvalidCharacter(first_char))),
                _ => panic!("peek_equals was called with first_char = {first_char}"),
            }
        }
    }

    /// Tokenizes a 'short' literal string, AKA a string denoted by single or
    /// double quotes and not by two square brackets.
    fn lex_string(&mut self, quote: char, _tok_start: usize) -> Result<TokenType> {
        loop {
            match self.peek_char() {
                None => return Err(self.error(SyntaxError::UnclosedString)),
                Some(c) if c == quote => {
                    self.next_char();
                    return Ok(LiteralString);
                }
                Some('\\') => {
                    // Consume the backslash
                    self.next_char();
                    // Skip the escaped character. For \ddd, we just skip
                    // one digit here; the parser handles full parsing.
                    // For backslash-newline, next_char handles \r\n pairs.
                    self.next_char();
                }
                Some('\n') | Some('\r') => {
                    return Err(self.error(SyntaxError::UnclosedString));
                }
                _ => {
                    self.next_char();
                }
            }
        }
    }

    /// Reads in a number which starts with a digit (as opposed to a decimal point).
    fn lex_full_number(&mut self, tok_start: usize, first_char: char) -> Result<TokenType> {
        // Check for hex values
        if first_char == '0' && (self.try_next('x') || self.try_next('X')) {
            // Has to be at least one digit
            match self.next_char() {
                Some(c) if c.is_ascii_hexdigit() => (),
                _ => return Err(self.error(SyntaxError::BadNumber)),
            }
            // Read the rest of the numbers
            while let Some(c) = self.peek_char() {
                if c.is_ascii_hexdigit() {
                    self.next_char();
                } else {
                    break;
                }
            }

            match self.peek_char() {
                Some(c) if c.is_ascii_hexdigit() => Err(self.error(SyntaxError::BadNumber)),
                _ => Ok(LiteralHexNumber),
            }
        } else {
            // Read in the rest of the base
            self.lex_digits();

            // Handle the fraction and exponent components.
            if self.try_next('.') {
                match self.peek_char() {
                    Some(c) if c.is_ascii_digit() => self.lex_number_after_decimal(tok_start)?,
                    _ => self.lex_exponent(tok_start)?,
                }
            } else {
                self.lex_exponent(tok_start)?;
            }

            Ok(LiteralNumber)
        }
    }

    /// Reads in a literal number which starts with a decimal point.
    fn lex_number_after_decimal(&mut self, tok_start: usize) -> Result<()> {
        self.lex_digits();
        self.lex_exponent(tok_start)
    }

    /// Consumes an unbroken sequence of digits.
    fn lex_digits(&mut self) {
        while let Some(c) = self.peek_char() {
            if c.is_ascii_digit() {
                self.next_char();
            } else {
                break;
            }
        }
    }

    /// Consumes the optional exponent part of a literal number, then checks
    /// for any trailing letters.
    fn lex_exponent(&mut self, _tok_start: usize) -> Result<()> {
        if self.try_next('E') || self.try_next('e') {
            // The exponent might have a sign.
            if let Some(c) = self.peek_char()
                && (c == '+' || c == '-')
            {
                self.next_char();
            }

            self.lex_digits();
        }
        match self.peek_char() {
            Some(c) if c.is_ascii_hexdigit() => Err(self.error(SyntaxError::BadNumber)),
            _ => Ok(()),
        }
    }

    /// Reads a word and returns it as an identifier or keyword.
    fn lex_word(&mut self, first_char: char) -> TokenType {
        let mut word = String::new();
        word.push(first_char);
        while let Some(c) = self.peek_char() {
            if c.is_ascii_alphabetic() || c.is_ascii_digit() || c == '_' {
                word.push(c);
                self.next_char();
            } else {
                break;
            }
        }

        keyword_match(&word)
    }

    /// Returns the current position of the `Lexer`.
    #[must_use]
    fn line_and_col(&self, pos: usize) -> (usize, usize) {
        let iter = self.linebreaks.windows(2).enumerate();
        for (line_num, linebreak_pair) in iter {
            if pos < linebreak_pair[1] {
                let column = pos - linebreak_pair[0];
                // lines and columns start counting at 1
                return (line_num + 1, column + 1);
            }
        }
        let line_num = self.linebreaks.len() - 1;
        let column = pos - self.linebreaks.last().unwrap();
        (line_num + 1, column + 1)
    }

    #[must_use]
    const fn end_of_file(&self) -> Token {
        Token {
            typ: TokenType::EndOfFile,
            start: self.pos,
            len: 0,
        }
    }
}

/// Checks if a word is a keyword, then returns the appropriate `TokenType`.
#[must_use]
fn keyword_match(s: &str) -> TokenType {
    match s {
        "and" => And,
        "break" => Break,
        "do" => Do,
        "else" => Else,
        "elseif" => ElseIf,
        "end" => End,
        "false" => False,
        "for" => For,
        "function" => Function,
        "if" => If,
        "in" => In,
        "local" => Local,
        "nil" => Nil,
        "not" => Not,
        "or" => Or,
        "repeat" => Repeat,
        "return" => Return,
        "then" => Then,
        "true" => True,
        "until" => Until,
        "while" => While,
        _ => Identifier,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn check(input: &str, tokens: &[(TokenType, usize, u32)], lines: &[usize]) {
        let mut lexer = Lexer::new(input);
        let mut tokens = tokens
            .iter()
            .map(|&(typ, start, len)| Token { typ, start, len });
        loop {
            let actual = lexer.next_token().unwrap();
            if actual.typ == TokenType::EndOfFile {
                break;
            }
            let expected = tokens.next().unwrap();
            assert_eq!(expected, actual);
        }
        assert!(tokens.next().is_none());
        assert_eq!(lines, lexer.linebreaks.as_slice());
    }

    fn check_line(input: &str, tokens: &[(TokenType, usize, u32)]) {
        check(input, tokens, &[0]);
    }

    #[test]
    fn test_lexer01() {
        let tokens = &[(LiteralNumber, 0, 2)];
        check_line("50", tokens);
    }

    #[test]
    fn test_lexer02() {
        let input = "hi 4 false";
        let tokens = &[(Identifier, 0, 2), (LiteralNumber, 3, 1), (False, 5, 5)];
        check_line(input, tokens);
    }

    #[test]
    fn test_lexer03() {
        let input = "hi5";
        let tokens = &[(Identifier, 0, 3)];
        check_line(input, tokens);
    }

    #[test]
    fn test_lexer04() {
        let input = "5 + 5";
        let tokens = &[(LiteralNumber, 0, 1), (Plus, 2, 1), (LiteralNumber, 4, 1)];
        check_line(input, tokens);
    }

    #[test]
    fn test_lexer05() {
        let input = "print 5 or 6;";
        let tokens = &[
            (Identifier, 0, 5),
            (LiteralNumber, 6, 1),
            (Or, 8, 2),
            (LiteralNumber, 11, 1),
            (Semi, 12, 1),
        ];
        check_line(input, tokens);
    }

    #[test]
    fn test_lexer06() {
        let input = "t = {x = 3}";
        let tokens = &[
            (Identifier, 0, 1),
            (Assign, 2, 1),
            (LCurly, 4, 1),
            (Identifier, 5, 1),
            (Assign, 7, 1),
            (LiteralNumber, 9, 1),
            (RCurly, 10, 1),
        ];
        check_line(input, tokens);
    }

    #[test]
    fn test_lexer07() {
        let input = "0x5rad";
        let tokens = &[(LiteralHexNumber, 0, 3), (Identifier, 3, 3)];
        check_line(input, tokens);
    }

    #[test]
    fn test_lexer08() {
        let input = "print {x = 5,}";
        let tokens = &[
            (Identifier, 0, 5),
            (LCurly, 6, 1),
            (Identifier, 7, 1),
            (Assign, 9, 1),
            (LiteralNumber, 11, 1),
            (Comma, 12, 1),
            (RCurly, 13, 1),
        ];
        check_line(input, tokens);
    }

    #[test]
    fn test_lexer09() {
        let input = "print()\nsome_other_function(an_argument)\n";
        let tokens = &[
            (Identifier, 0, 5),
            (LParen, 5, 1),
            (RParen, 6, 1),
            (Identifier, 8, 19),
            (LParen, 27, 1),
            (Identifier, 28, 11),
            (RParen, 39, 1),
        ];
        let linebreaks = &[0, 8, 41];
        check(input, tokens, linebreaks);
    }

    #[test]
    fn test_lexer10() {
        let input = "\n\n2\n456\n";
        let tokens = &[(LiteralNumber, 2, 1), (LiteralNumber, 4, 3)];
        let linebreaks = &[0, 1, 2, 4, 8];
        check(input, tokens, linebreaks);
    }

    #[test]
    fn test_lexer11() {
        let input = "-- basic test\nprint('hi' --comment\n )\n";
        let tokens = &[
            (Identifier, 14, 5),
            (LParen, 19, 1),
            (LiteralString, 20, 4),
            (RParen, 36, 1),
        ];
        let linebreaks = &[0, 14, 35, 38];
        check(input, tokens, linebreaks);
    }

    #[test]
    fn test_lexer12() {
        let input = "print()\n(some_other_function)(an_argument)\n";
        let tokens = &[
            (Identifier, 0, 5),
            (LParen, 5, 1),
            (RParen, 6, 1),
            (LParenLineStart, 8, 1),
            (Identifier, 9, 19),
            (RParen, 28, 1),
            (LParen, 29, 1),
            (Identifier, 30, 11),
            (RParen, 41, 1),
        ];
        let linebreaks = &[0, 8, 43];
        check(input, tokens, linebreaks);
    }

    #[test]
    fn hex_uppercase_prefix() {
        let input = "0XFF";
        let tokens = &[(LiteralHexNumber, 0, 4)];
        check_line(input, tokens);
    }

    #[test]
    fn hex_mixed_case() {
        let input = "0XaB";
        let tokens = &[(LiteralHexNumber, 0, 4)];
        check_line(input, tokens);
    }

    // --- Long bracket tests ---

    #[test]
    fn long_comment_basic() {
        // `--[[ comment ]]` should be skipped entirely
        let input = "--[[ comment ]] x = 1";
        let tokens = &[(Identifier, 16, 1), (Assign, 18, 1), (LiteralNumber, 20, 1)];
        check_line(input, tokens);
    }

    #[test]
    fn long_comment_with_level() {
        let input = "--[=[ comment ]=] x = 1";
        let tokens = &[(Identifier, 18, 1), (Assign, 20, 1), (LiteralNumber, 22, 1)];
        check_line(input, tokens);
    }

    #[test]
    fn long_comment_multiline() {
        let input = "--[[\nline1\nline2\n]] x = 1";
        let tokens = &[(Identifier, 20, 1), (Assign, 22, 1), (LiteralNumber, 24, 1)];
        let linebreaks = &[0, 5, 11, 17];
        check(input, tokens, linebreaks);
    }

    #[test]
    fn long_comment_unfinished() {
        let input = "--[[ no end";
        let mut lexer = Lexer::new(input);
        let err = lexer.next_token().unwrap_err();
        assert!(matches!(
            err.kind,
            super::super::error::ErrorKind::SyntaxError(SyntaxError::UnfinishedLongComment)
        ));
    }

    #[test]
    fn long_string_basic() {
        let input = "x = [[hello]]";
        // [[hello]] starts at 4, length = 13 - 4 = 9
        let tokens = &[(Identifier, 0, 1), (Assign, 2, 1), (LiteralString, 4, 9)];
        check_line(input, tokens);
    }

    #[test]
    fn long_string_with_level() {
        let input = "x = [=[hello]=]";
        // [=[hello]=] starts at 4, length = 15 - 4 = 11
        let tokens = &[(Identifier, 0, 1), (Assign, 2, 1), (LiteralString, 4, 11)];
        check_line(input, tokens);
    }

    #[test]
    fn long_string_empty() {
        let input = "x = [[]]";
        // [[]] starts at 4, length = 4
        let tokens = &[(Identifier, 0, 1), (Assign, 2, 1), (LiteralString, 4, 4)];
        check_line(input, tokens);
    }

    #[test]
    fn long_string_unfinished() {
        let input = "x = [[no end";
        let mut lexer = Lexer::new(input);
        // Consume `x`, `=`, then the long string should fail
        let _ = lexer.next_token().unwrap(); // x
        let _ = lexer.next_token().unwrap(); // =
        let err = lexer.next_token().unwrap_err();
        assert!(matches!(
            err.kind,
            super::super::error::ErrorKind::SyntaxError(SyntaxError::UnfinishedLongString)
        ));
    }

    #[test]
    fn bare_lsquare() {
        // `[` by itself should still be LSquare
        let input = "t[1]";
        let tokens = &[
            (Identifier, 0, 1),
            (LSquare, 1, 1),
            (LiteralNumber, 2, 1),
            (RSquare, 3, 1),
        ];
        check_line(input, tokens);
    }

    #[test]
    fn long_comment_then_long_string() {
        let input = "--[[ comment ]] x = [[string]]";
        let tokens = &[
            (Identifier, 16, 1),
            (Assign, 18, 1),
            (LiteralString, 20, 10),
        ];
        check_line(input, tokens);
    }

    #[test]
    fn long_string_mismatched_level() {
        // ]] inside a level-1 string should not close it
        let input = "[=[hello]]world]=]";
        let tokens = &[(LiteralString, 0, 18)];
        check_line(input, tokens);
    }

    #[test]
    fn long_string_leading_newline_skipped() {
        // The newline after [[ should be consumed as part of the delimiter,
        // not the string content. The token includes it in its span.
        let input = "[[\nhello]]";
        let tokens = &[(LiteralString, 0, 10)];
        let linebreaks = &[0, 3];
        check(input, tokens, linebreaks);
    }
}
