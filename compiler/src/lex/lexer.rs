use std::collections::VecDeque;

use crate::diag::{Diagnostic, Severity};
use crate::source::{SourceFile, SourceSpan};

use super::{Token, TokenKind};

#[derive(Debug)]
pub struct LexOutput {
    pub tokens: Vec<Token>,
    pub diagnostics: Vec<Diagnostic>,
}

impl LexOutput {
    pub fn has_errors(&self) -> bool {
        self.diagnostics
            .iter()
            .any(|diagnostic| diagnostic.severity == Severity::Error)
    }
}

#[derive(Debug, Clone, Copy)]
enum LexerMode {
    Normal,
    TemplateString,
    TemplateExpr { brace_depth: usize },
}

pub struct Lexer<'a> {
    file: &'a SourceFile,
    offset: usize,
    modes: Vec<LexerMode>,
    diagnostics: Vec<Diagnostic>,
    pending: VecDeque<Token>,
    leading_newline: bool,
}

impl<'a> Lexer<'a> {
    pub fn new(file: &'a SourceFile) -> Self {
        Self {
            file,
            offset: 0,
            modes: vec![LexerMode::Normal],
            diagnostics: Vec::new(),
            pending: VecDeque::new(),
            leading_newline: false,
        }
    }

    pub fn lex_all(mut self) -> LexOutput {
        self.skip_utf8_bom();
        let mut tokens = Vec::new();
        loop {
            let token = self.next_token();
            let done = token.kind == TokenKind::Eof;
            tokens.push(token);
            if done {
                break;
            }
        }

        LexOutput {
            tokens,
            diagnostics: self.diagnostics,
        }
    }

    fn next_token(&mut self) -> Token {
        loop {
            if let Some(token) = self.pending.pop_front() {
                return token;
            }

            match self.current_mode() {
                LexerMode::Normal => {
                    self.leading_newline = self.skip_trivia();
                    if self.is_eof() {
                        return self.make_token(TokenKind::Eof, self.offset, self.offset);
                    }
                    if let Some(token) = self.lex_normal_token() {
                        return token;
                    }
                }
                LexerMode::TemplateString => {
                    self.leading_newline = false;
                    if let Some(token) = self.lex_template_string_token() {
                        return token;
                    }
                }
                LexerMode::TemplateExpr { .. } => {
                    self.leading_newline = self.skip_trivia();
                    if self.is_eof() {
                        self.report(
                            "LEX003",
                            "unterminated template interpolation",
                            self.offset,
                            self.offset,
                        );
                        return self.make_token(TokenKind::Eof, self.offset, self.offset);
                    }

                    if self.peek_char() == Some('{') {
                        let start = self.offset;
                        self.bump_char();
                        self.bump_template_brace_depth(1);
                        return self.make_token(TokenKind::LBrace, start, self.offset);
                    }

                    if self.peek_char() == Some('}') {
                        let start = self.offset;
                        self.bump_char();
                        if self.current_template_brace_depth() == 0 {
                            self.modes.pop();
                            return self.make_token(TokenKind::TemplateExprEnd, start, self.offset);
                        }
                        self.bump_template_brace_depth(-1);
                        return self.make_token(TokenKind::RBrace, start, self.offset);
                    }

                    if let Some(token) = self.lex_normal_token() {
                        return token;
                    }
                }
            }
        }
    }

    fn current_mode(&self) -> LexerMode {
        self.modes.last().copied().unwrap_or(LexerMode::Normal)
    }

    fn current_template_brace_depth(&self) -> usize {
        match self.current_mode() {
            LexerMode::TemplateExpr { brace_depth } => brace_depth,
            _ => 0,
        }
    }

    fn bump_template_brace_depth(&mut self, delta: isize) {
        if let Some(LexerMode::TemplateExpr { brace_depth }) = self.modes.last_mut() {
            if delta.is_negative() {
                *brace_depth = brace_depth.saturating_sub(delta.unsigned_abs());
            } else {
                *brace_depth = brace_depth.saturating_add(delta as usize);
            }
        }
    }

    fn lex_template_string_token(&mut self) -> Option<Token> {
        if self.is_eof() {
            self.report(
                "LEX002",
                "unterminated template string",
                self.offset,
                self.offset,
            );
            return Some(self.make_token(TokenKind::Eof, self.offset, self.offset));
        }

        if self.starts_with("${") {
            let start = self.offset;
            self.offset += 2;
            self.modes.push(LexerMode::TemplateExpr { brace_depth: 0 });
            return Some(self.make_token(TokenKind::TemplateExprStart, start, self.offset));
        }

        if self.peek_char() == Some('`') {
            let start = self.offset;
            self.bump_char();
            self.modes.pop();
            return Some(self.make_token(TokenKind::TemplateStringEnd, start, self.offset));
        }

        let start = self.offset;
        let mut text = String::new();
        while !self.is_eof() {
            if self.starts_with("${") || self.peek_char() == Some('`') {
                break;
            }
            text.push(self.bump_char().expect("cursor already checked"));
        }

        Some(self.make_token(TokenKind::TemplateStringText(text), start, self.offset))
    }

    fn lex_normal_token(&mut self) -> Option<Token> {
        let start = self.offset;
        let ch = self.peek_char()?;

        let token = match ch {
            '(' => {
                self.bump_char();
                TokenKind::LParen
            }
            ')' => {
                self.bump_char();
                TokenKind::RParen
            }
            '{' => {
                self.bump_char();
                TokenKind::LBrace
            }
            '}' => {
                self.bump_char();
                TokenKind::RBrace
            }
            '[' => {
                self.bump_char();
                TokenKind::LBracket
            }
            ']' => {
                self.bump_char();
                TokenKind::RBracket
            }
            ',' => {
                self.bump_char();
                TokenKind::Comma
            }
            ';' => {
                self.bump_char();
                TokenKind::Semicolon
            }
            '#' => {
                self.bump_char();
                TokenKind::Hash
            }
            '|' => {
                self.bump_char();
                if self.match_char('>') {
                    TokenKind::PipeGt
                } else {
                    TokenKind::Pipe
                }
            }
            '+' => {
                self.bump_char();
                if self.match_char('=') {
                    TokenKind::PlusEq
                } else {
                    TokenKind::Plus
                }
            }
            '*' => {
                self.bump_char();
                if self.match_char('=') {
                    TokenKind::StarEq
                } else {
                    TokenKind::Star
                }
            }
            '/' => {
                self.bump_char();
                if self.match_char('=') {
                    TokenKind::SlashEq
                } else {
                    TokenKind::Slash
                }
            }
            '%' => {
                self.bump_char();
                if self.match_char('=') {
                    TokenKind::PercentEq
                } else {
                    TokenKind::Percent
                }
            }
            '^' => {
                self.bump_char();
                if self.match_char('=') {
                    TokenKind::CaretEq
                } else {
                    TokenKind::Caret
                }
            }
            '<' => {
                self.bump_char();
                if self.match_char('=') {
                    TokenKind::LtEq
                } else {
                    TokenKind::Lt
                }
            }
            '>' => {
                self.bump_char();
                if self.match_char('=') {
                    TokenKind::GtEq
                } else {
                    TokenKind::Gt
                }
            }
            '~' => {
                self.bump_char();
                if self.match_char('=') {
                    TokenKind::NotEq
                } else {
                    self.report("LEX004", "unexpected `~`", start, self.offset);
                    return None;
                }
            }
            '.' => return Some(self.lex_dot_family()),
            ':' => return Some(self.lex_colon_family()),
            '?' => return self.lex_question_family(),
            '-' => return Some(self.lex_minus_family()),
            '=' => return Some(self.lex_equals_family()),
            '"' | '\'' => return Some(self.lex_string(ch)),
            '`' => {
                self.bump_char();
                self.modes.push(LexerMode::TemplateString);
                TokenKind::TemplateStringStart
            }
            c if is_identifier_start(c) => return Some(self.lex_identifier_or_keyword()),
            c if c.is_ascii_digit() => return Some(self.lex_number()),
            _ => {
                self.bump_char();
                self.report(
                    "LEX005",
                    format!("unexpected character `{ch}`"),
                    start,
                    self.offset,
                );
                return None;
            }
        };

        Some(self.make_token(token, start, self.offset))
    }

    fn lex_dot_family(&mut self) -> Token {
        let start = self.offset;
        self.bump_char();
        let kind = if self.match_str(".=") {
            TokenKind::DotDotEq
        } else if self.match_str("..") {
            TokenKind::Ellipsis
        } else if self.match_char('.') {
            TokenKind::DotDot
        } else {
            TokenKind::Dot
        };
        self.make_token(kind, start, self.offset)
    }

    fn lex_colon_family(&mut self) -> Token {
        let start = self.offset;
        self.bump_char();
        self.make_token(TokenKind::Colon, start, self.offset)
    }

    fn lex_question_family(&mut self) -> Option<Token> {
        let start = self.offset;
        self.bump_char();
        if self.match_char('.') {
            return Some(self.make_token(TokenKind::QuestionDot, start, self.offset));
        }
        if self.match_char(':') {
            return Some(self.make_token(TokenKind::QuestionColon, start, self.offset));
        }
        if self.match_char('?') {
            return Some(self.make_token(TokenKind::QuestionQuestion, start, self.offset));
        }

        self.report(
            "LEX001",
            "bare `?` is not valid in Lux MVP 0.1",
            start,
            self.offset,
        );
        None
    }

    fn lex_minus_family(&mut self) -> Token {
        let start = self.offset;
        self.bump_char();
        let kind = if self.match_char('>') {
            TokenKind::ArrowImplicitSelf
        } else if self.match_char('=') {
            TokenKind::MinusEq
        } else {
            TokenKind::Minus
        };
        self.make_token(kind, start, self.offset)
    }

    fn lex_equals_family(&mut self) -> Token {
        let start = self.offset;
        self.bump_char();
        let kind = if self.match_char('>') {
            TokenKind::ArrowNormal
        } else if self.match_char('=') {
            TokenKind::EqEq
        } else {
            TokenKind::Eq
        };
        self.make_token(kind, start, self.offset)
    }

    fn lex_identifier_or_keyword(&mut self) -> Token {
        let start = self.offset;
        let mut value = String::new();
        value.push(self.bump_char().expect("identifier start must exist"));
        while let Some(ch) = self.peek_char() {
            if is_identifier_continue(ch) {
                value.push(self.bump_char().expect("peeked identifier continuation"));
            } else {
                break;
            }
        }

        let kind = match value.as_str() {
            "fn" => TokenKind::KwFn,
            "if" => TokenKind::KwIf,
            "then" => TokenKind::KwThen,
            "else" => TokenKind::KwElse,
            "elseif" => TokenKind::KwElseIf,
            "local" => TokenKind::KwLocal,
            "const" => TokenKind::KwConst,
            "nil" => TokenKind::KwNil,
            "true" => TokenKind::KwTrue,
            "false" => TokenKind::KwFalse,
            "and" => TokenKind::KwAnd,
            "or" => TokenKind::KwOr,
            "not" => TokenKind::KwNot,
            "import" => TokenKind::KwImport,
            "export" => TokenKind::KwExport,
            "function" => TokenKind::KwFunction,
            "end" => TokenKind::KwEnd,
            "do" => TokenKind::KwDo,
            "while" => TokenKind::KwWhile,
            "for" => TokenKind::KwFor,
            "repeat" => TokenKind::KwRepeat,
            "until" => TokenKind::KwUntil,
            "break" => TokenKind::KwBreak,
            "return" => TokenKind::KwReturn,
            "in" => TokenKind::KwIn,
            _ => TokenKind::Identifier(value),
        };

        self.make_token(kind, start, self.offset)
    }

    fn lex_number(&mut self) -> Token {
        let start = self.offset;
        let mut value = String::new();

        if self.starts_with("0x") || self.starts_with("0X") {
            value.push(self.bump_char().unwrap());
            value.push(self.bump_char().unwrap());
            while let Some(ch) = self.peek_char() {
                if ch.is_ascii_hexdigit() {
                    value.push(self.bump_char().unwrap());
                } else {
                    break;
                }
            }
            return self.make_token(TokenKind::Number(value), start, self.offset);
        }

        value.push(self.bump_char().unwrap());
        while let Some(ch) = self.peek_char() {
            if ch.is_ascii_digit() {
                value.push(self.bump_char().unwrap());
            } else {
                break;
            }
        }

        if self.peek_char() == Some('.') && !matches!(self.peek_n(1), Some('.')) {
            value.push(self.bump_char().unwrap());
            while let Some(ch) = self.peek_char() {
                if ch.is_ascii_digit() {
                    value.push(self.bump_char().unwrap());
                } else {
                    break;
                }
            }
        }

        if matches!(self.peek_char(), Some('e' | 'E')) {
            let checkpoint = self.offset;
            let mut exponent = String::new();
            exponent.push(self.bump_char().unwrap());
            if matches!(self.peek_char(), Some('+' | '-')) {
                exponent.push(self.bump_char().unwrap());
            }

            let mut saw_digit = false;
            while let Some(ch) = self.peek_char() {
                if ch.is_ascii_digit() {
                    saw_digit = true;
                    exponent.push(self.bump_char().unwrap());
                } else {
                    break;
                }
            }

            if saw_digit {
                value.push_str(&exponent);
            } else {
                self.offset = checkpoint;
            }
        }

        self.make_token(TokenKind::Number(value), start, self.offset)
    }

    fn lex_string(&mut self, quote: char) -> Token {
        let start = self.offset;
        self.bump_char();
        let mut value = String::new();

        loop {
            match self.peek_char() {
                Some(ch) if ch == quote => {
                    self.bump_char();
                    break;
                }
                Some('\\') => {
                    self.bump_char();
                    match self.peek_char() {
                        Some('n') => {
                            self.bump_char();
                            value.push('\n');
                        }
                        Some('r') => {
                            self.bump_char();
                            value.push('\r');
                        }
                        Some('t') => {
                            self.bump_char();
                            value.push('\t');
                        }
                        Some('\\') => {
                            self.bump_char();
                            value.push('\\');
                        }
                        Some('\'') => {
                            self.bump_char();
                            value.push('\'');
                        }
                        Some('"') => {
                            self.bump_char();
                            value.push('"');
                        }
                        Some('`') => {
                            self.bump_char();
                            value.push('`');
                        }
                        Some(other) => {
                            self.bump_char();
                            value.push(other);
                        }
                        None => {
                            self.report(
                                "LEX006",
                                "unterminated string literal",
                                start,
                                self.offset,
                            );
                            break;
                        }
                    }
                }
                Some('\n') | Some('\r') | None => {
                    self.report("LEX006", "unterminated string literal", start, self.offset);
                    break;
                }
                Some(ch) => {
                    self.bump_char();
                    value.push(ch);
                }
            }
        }

        self.make_token(TokenKind::String(value), start, self.offset)
    }

    fn skip_trivia(&mut self) -> bool {
        let mut saw_newline = false;
        loop {
            let Some(ch) = self.peek_char() else {
                return saw_newline;
            };

            if ch.is_whitespace() {
                if ch == '\n' || ch == '\r' {
                    saw_newline = true;
                }
                self.bump_char();
                continue;
            }

            if self.starts_with("--[[") {
                let start = self.offset;
                self.offset += 4;
                while !self.is_eof() && !self.starts_with("]]") {
                    if matches!(self.peek_char(), Some('\n' | '\r')) {
                        saw_newline = true;
                    }
                    self.bump_char();
                }
                if self.starts_with("]]") {
                    self.offset += 2;
                } else {
                    self.report("LEX007", "unterminated block comment", start, self.offset);
                }
                continue;
            }

            if self.starts_with("--") {
                self.offset += 2;
                while let Some(next) = self.peek_char() {
                    self.bump_char();
                    if next == '\n' {
                        saw_newline = true;
                        break;
                    }
                }
                continue;
            }

            return saw_newline;
        }
    }

    fn skip_utf8_bom(&mut self) {
        if self.offset == 0 && self.starts_with("\u{feff}") {
            self.offset += "\u{feff}".len();
        }
    }

    fn report(&mut self, code: &str, message: impl Into<String>, start: usize, end: usize) {
        let diagnostic = Diagnostic::error(message)
            .with_code(code)
            .with_label(crate::diag::Label::primary(self.span(start, end), "here"));
        self.diagnostics.push(diagnostic);
    }

    fn make_token(&self, kind: TokenKind, start: usize, end: usize) -> Token {
        Token::new_with_leading_newline(kind, self.span(start, end), self.leading_newline)
    }

    fn span(&self, start: usize, end: usize) -> SourceSpan {
        SourceSpan::new(self.file.id, start, end)
    }

    fn is_eof(&self) -> bool {
        self.offset >= self.file.text.len()
    }

    fn starts_with(&self, value: &str) -> bool {
        self.file.text[self.offset..].starts_with(value)
    }

    fn peek_char(&self) -> Option<char> {
        self.file.text[self.offset..].chars().next()
    }

    fn peek_n(&self, n: usize) -> Option<char> {
        self.file.text[self.offset..].chars().nth(n)
    }

    fn bump_char(&mut self) -> Option<char> {
        let ch = self.peek_char()?;
        self.offset += ch.len_utf8();
        Some(ch)
    }

    fn match_char(&mut self, expected: char) -> bool {
        if self.peek_char() == Some(expected) {
            self.bump_char();
            true
        } else {
            false
        }
    }

    fn match_str(&mut self, expected: &str) -> bool {
        if self.starts_with(expected) {
            self.offset += expected.len();
            true
        } else {
            false
        }
    }
}

fn is_identifier_start(ch: char) -> bool {
    ch == '_' || ch.is_ascii_alphabetic()
}

fn is_identifier_continue(ch: char) -> bool {
    is_identifier_start(ch) || ch.is_ascii_digit()
}

#[cfg(test)]
mod tests {
    use crate::source::SourceFile;

    use super::{Lexer, TokenKind};

    fn lex_kinds(input: &str) -> Vec<TokenKind> {
        let file = SourceFile::new(0, None, input);
        let output = Lexer::new(&file).lex_all();
        assert!(
            output.diagnostics.is_empty(),
            "expected no diagnostics, got: {:#?}",
            output.diagnostics
        );
        output.tokens.into_iter().map(|token| token.kind).collect()
    }

    fn lex_diagnostics(input: &str) -> Vec<crate::diag::Diagnostic> {
        let file = SourceFile::new(0, None, input);
        Lexer::new(&file).lex_all().diagnostics
    }

    #[test]
    fn lexes_longest_match_families() {
        let kinds = lex_kinds("..= ... .. . ?: : ?. ?? |> -> -= - => == =");
        assert_eq!(
            kinds,
            vec![
                TokenKind::DotDotEq,
                TokenKind::Ellipsis,
                TokenKind::DotDot,
                TokenKind::Dot,
                TokenKind::QuestionColon,
                TokenKind::Colon,
                TokenKind::QuestionDot,
                TokenKind::QuestionQuestion,
                TokenKind::PipeGt,
                TokenKind::ArrowImplicitSelf,
                TokenKind::MinusEq,
                TokenKind::Minus,
                TokenKind::ArrowNormal,
                TokenKind::EqEq,
                TokenKind::Eq,
                TokenKind::Eof,
            ]
        );
    }

    #[test]
    fn lexes_keywords_and_identifiers() {
        let kinds = lex_kinds("fn local const import export PANEL foo return in");
        assert_eq!(
            kinds,
            vec![
                TokenKind::KwFn,
                TokenKind::KwLocal,
                TokenKind::KwConst,
                TokenKind::KwImport,
                TokenKind::KwExport,
                TokenKind::Identifier("PANEL".into()),
                TokenKind::Identifier("foo".into()),
                TokenKind::KwReturn,
                TokenKind::KwIn,
                TokenKind::Eof,
            ]
        );
    }

    #[test]
    fn skips_line_and_block_comments() {
        let kinds = lex_kinds("fn foo -- line\n--[[ block ]] local bar");
        assert_eq!(
            kinds,
            vec![
                TokenKind::KwFn,
                TokenKind::Identifier("foo".into()),
                TokenKind::KwLocal,
                TokenKind::Identifier("bar".into()),
                TokenKind::Eof,
            ]
        );
    }

    #[test]
    fn lexes_template_strings_and_interpolation() {
        let kinds = lex_kinds("`Count: ${count()}`");
        assert_eq!(
            kinds,
            vec![
                TokenKind::TemplateStringStart,
                TokenKind::TemplateStringText("Count: ".into()),
                TokenKind::TemplateExprStart,
                TokenKind::Identifier("count".into()),
                TokenKind::LParen,
                TokenKind::RParen,
                TokenKind::TemplateExprEnd,
                TokenKind::TemplateStringEnd,
                TokenKind::Eof,
            ]
        );
    }

    #[test]
    fn lexes_safe_index_and_optional_calls() {
        let kinds = lex_kinds("tbl?.[key] obj?.name(args) obj?:call()");
        assert_eq!(
            kinds,
            vec![
                TokenKind::Identifier("tbl".into()),
                TokenKind::QuestionDot,
                TokenKind::LBracket,
                TokenKind::Identifier("key".into()),
                TokenKind::RBracket,
                TokenKind::Identifier("obj".into()),
                TokenKind::QuestionDot,
                TokenKind::Identifier("name".into()),
                TokenKind::LParen,
                TokenKind::Identifier("args".into()),
                TokenKind::RParen,
                TokenKind::Identifier("obj".into()),
                TokenKind::QuestionColon,
                TokenKind::Identifier("call".into()),
                TokenKind::LParen,
                TokenKind::RParen,
                TokenKind::Eof,
            ]
        );
    }

    #[test]
    fn rejects_old_safe_method_spelling() {
        let diagnostics = lex_diagnostics("obj:?call()");
        assert!(diagnostics.iter().any(|diagnostic| {
            diagnostic.code.as_deref() == Some("LEX001")
                && diagnostic.message.contains("bare `?` is not valid")
        }));
    }

    #[test]
    fn lexes_strings_and_numbers() {
        let kinds = lex_kinds("'hi' 12.5e-2 0xFF");
        assert_eq!(
            kinds,
            vec![
                TokenKind::String("hi".into()),
                TokenKind::Number("12.5e-2".into()),
                TokenKind::Number("0xFF".into()),
                TokenKind::Eof,
            ]
        );
    }

    #[test]
    fn skips_utf8_bom() {
        let kinds = lex_kinds("\u{feff}import \"setup\"");
        assert_eq!(
            kinds,
            vec![
                TokenKind::KwImport,
                TokenKind::String("setup".into()),
                TokenKind::Eof,
            ]
        );
    }
}
