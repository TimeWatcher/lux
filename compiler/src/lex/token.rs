use std::fmt;

use crate::source::SourceSpan;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Token {
    pub kind: TokenKind,
    pub span: SourceSpan,
    pub leading_newline: bool,
}

impl Token {
    pub const fn new(kind: TokenKind, span: SourceSpan) -> Self {
        Self {
            kind,
            span,
            leading_newline: false,
        }
    }

    pub const fn new_with_leading_newline(
        kind: TokenKind,
        span: SourceSpan,
        leading_newline: bool,
    ) -> Self {
        Self {
            kind,
            span,
            leading_newline,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TokenKind {
    Identifier(String),

    KwFn,
    KwIf,
    KwThen,
    KwElse,
    KwElseIf,
    KwLocal,
    KwConst,
    KwNil,
    KwTrue,
    KwFalse,
    KwAnd,
    KwOr,
    KwNot,
    KwImport,
    KwExport,
    KwFunction,
    KwEnd,
    KwDo,
    KwWhile,
    KwFor,
    KwRepeat,
    KwUntil,
    KwBreak,
    KwReturn,
    KwIn,

    Number(String),
    String(String),

    TemplateStringStart,
    TemplateStringText(String),
    TemplateExprStart,
    TemplateExprEnd,
    TemplateStringEnd,

    LParen,
    RParen,
    LBrace,
    RBrace,
    LBracket,
    RBracket,
    Comma,
    Semicolon,

    Dot,
    DotDot,
    Ellipsis,
    Colon,
    Question,
    QuestionDot,
    QuestionColon,
    QuestionQuestion,

    Eq,
    PlusEq,
    MinusEq,
    StarEq,
    SlashEq,
    PercentEq,
    CaretEq,
    DotDotEq,

    Plus,
    Minus,
    Star,
    Slash,
    Percent,
    Caret,
    Hash,
    Pipe,
    PipeGt,

    EqEq,
    NotEq,
    Lt,
    LtEq,
    Gt,
    GtEq,

    ArrowNormal,
    ArrowImplicitSelf,

    Eof,
}

impl fmt::Display for TokenKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            TokenKind::Identifier(value) => write!(f, "Identifier({value})"),
            TokenKind::Number(value) => write!(f, "Number({value})"),
            TokenKind::String(value) => write!(f, "String({value:?})"),
            TokenKind::TemplateStringText(value) => write!(f, "TemplateStringText({value:?})"),
            other => f.write_str(other.name()),
        }
    }
}

impl TokenKind {
    pub const fn name(&self) -> &'static str {
        match self {
            TokenKind::Identifier(_) => "Identifier",
            TokenKind::KwFn => "KwFn",
            TokenKind::KwIf => "KwIf",
            TokenKind::KwThen => "KwThen",
            TokenKind::KwElse => "KwElse",
            TokenKind::KwElseIf => "KwElseIf",
            TokenKind::KwLocal => "KwLocal",
            TokenKind::KwConst => "KwConst",
            TokenKind::KwNil => "KwNil",
            TokenKind::KwTrue => "KwTrue",
            TokenKind::KwFalse => "KwFalse",
            TokenKind::KwAnd => "KwAnd",
            TokenKind::KwOr => "KwOr",
            TokenKind::KwNot => "KwNot",
            TokenKind::KwImport => "KwImport",
            TokenKind::KwExport => "KwExport",
            TokenKind::KwFunction => "KwFunction",
            TokenKind::KwEnd => "KwEnd",
            TokenKind::KwDo => "KwDo",
            TokenKind::KwWhile => "KwWhile",
            TokenKind::KwFor => "KwFor",
            TokenKind::KwRepeat => "KwRepeat",
            TokenKind::KwUntil => "KwUntil",
            TokenKind::KwBreak => "KwBreak",
            TokenKind::KwReturn => "KwReturn",
            TokenKind::KwIn => "KwIn",
            TokenKind::Number(_) => "Number",
            TokenKind::String(_) => "String",
            TokenKind::TemplateStringStart => "TemplateStringStart",
            TokenKind::TemplateStringText(_) => "TemplateStringText",
            TokenKind::TemplateExprStart => "TemplateExprStart",
            TokenKind::TemplateExprEnd => "TemplateExprEnd",
            TokenKind::TemplateStringEnd => "TemplateStringEnd",
            TokenKind::LParen => "LParen",
            TokenKind::RParen => "RParen",
            TokenKind::LBrace => "LBrace",
            TokenKind::RBrace => "RBrace",
            TokenKind::LBracket => "LBracket",
            TokenKind::RBracket => "RBracket",
            TokenKind::Comma => "Comma",
            TokenKind::Semicolon => "Semicolon",
            TokenKind::Dot => "Dot",
            TokenKind::DotDot => "DotDot",
            TokenKind::Ellipsis => "Ellipsis",
            TokenKind::Colon => "Colon",
            TokenKind::Question => "Question",
            TokenKind::QuestionDot => "QuestionDot",
            TokenKind::QuestionColon => "QuestionColon",
            TokenKind::QuestionQuestion => "QuestionQuestion",
            TokenKind::Eq => "Eq",
            TokenKind::PlusEq => "PlusEq",
            TokenKind::MinusEq => "MinusEq",
            TokenKind::StarEq => "StarEq",
            TokenKind::SlashEq => "SlashEq",
            TokenKind::PercentEq => "PercentEq",
            TokenKind::CaretEq => "CaretEq",
            TokenKind::DotDotEq => "DotDotEq",
            TokenKind::Plus => "Plus",
            TokenKind::Minus => "Minus",
            TokenKind::Star => "Star",
            TokenKind::Slash => "Slash",
            TokenKind::Percent => "Percent",
            TokenKind::Caret => "Caret",
            TokenKind::Hash => "Hash",
            TokenKind::Pipe => "Pipe",
            TokenKind::PipeGt => "PipeGt",
            TokenKind::EqEq => "EqEq",
            TokenKind::NotEq => "NotEq",
            TokenKind::Lt => "Lt",
            TokenKind::LtEq => "LtEq",
            TokenKind::Gt => "Gt",
            TokenKind::GtEq => "GtEq",
            TokenKind::ArrowNormal => "ArrowNormal",
            TokenKind::ArrowImplicitSelf => "ArrowImplicitSelf",
            TokenKind::Eof => "Eof",
        }
    }
}
