use std::collections::BTreeMap;

use crate::lex::{Lexer, Token, TokenKind};
use crate::source::{SourceFile, SourceSpan};

use lsp_types::{Color, ColorInformation, ColorPresentation, Range, TextEdit};

use super::protocol::source_range;

#[derive(Debug, Clone)]
struct LuxColorCall {
    span: SourceSpan,
    constructor: String,
    r: u8,
    g: u8,
    b: u8,
    a: u8,
}

pub(crate) fn document_colors(
    file: &SourceFile,
    mut semantic_color_constructor: impl FnMut(usize) -> bool,
) -> Vec<ColorInformation> {
    color_calls(file, &mut semantic_color_constructor)
        .into_iter()
        .map(|call| ColorInformation {
            range: source_range(file, call.span),
            color: lsp_color(call.r, call.g, call.b, call.a),
        })
        .collect()
}

pub(crate) fn color_presentations(
    file: &SourceFile,
    color: Color,
    range: Range,
    mut semantic_color_constructor: impl FnMut(usize) -> bool,
) -> Vec<ColorPresentation> {
    let r = color_channel_to_byte(color.red);
    let g = color_channel_to_byte(color.green);
    let b = color_channel_to_byte(color.blue);
    let a = color_channel_to_byte(color.alpha);
    let request_span = span_for_range(file, range);
    let call = request_span.and_then(|span| {
        color_calls(file, &mut semantic_color_constructor)
            .into_iter()
            .filter(|call| spans_touch(call.span, span))
            .min_by_key(|call| {
                (
                    call.span.byte_start.abs_diff(span.byte_start),
                    call.span.len(),
                )
            })
    });
    let constructor = call
        .as_ref()
        .map(|call| call.constructor.as_str())
        .unwrap_or("Color");
    let edit_range = call
        .as_ref()
        .map(|call| source_range(file, call.span))
        .unwrap_or(range);
    let label = color_call_label(constructor, r, g, b, a);
    vec![ColorPresentation {
        text_edit: Some(TextEdit {
            range: edit_range,
            new_text: label.clone(),
        }),
        label,
        additional_text_edits: None,
    }]
}

fn color_calls(
    file: &SourceFile,
    semantic_color_constructor: &mut impl FnMut(usize) -> bool,
) -> Vec<LuxColorCall> {
    let lex = Lexer::new(file).lex_all();
    let tokens = lex
        .tokens
        .into_iter()
        .filter(|token| !matches!(token.kind, TokenKind::Eof))
        .collect::<Vec<_>>();
    let constructors = color_constructor_names(&tokens);
    let mut calls = Vec::new();
    let mut index = 0usize;
    while index < tokens.len() {
        let Some(call) = parse_color_call(
            file,
            &tokens,
            &constructors,
            semantic_color_constructor,
            index,
        ) else {
            index += 1;
            continue;
        };
        calls.push(call.0);
        index = call.1;
    }
    calls
}

fn parse_color_call(
    file: &SourceFile,
    tokens: &[Token],
    constructors: &BTreeMap<String, usize>,
    semantic_color_constructor: &mut impl FnMut(usize) -> bool,
    index: usize,
) -> Option<(LuxColorCall, usize)> {
    let TokenKind::Identifier(name) = &tokens.get(index)?.kind else {
        return None;
    };
    if !matches!(tokens.get(index + 1)?.kind, TokenKind::LParen) {
        return None;
    }
    let close = matching_paren(tokens, index + 1)?;
    let args = parse_color_args(file, tokens, index + 2, close)?;
    if !is_color_constructor_at(constructors, name, index)
        && !semantic_color_constructor(tokens[index].span.byte_start)
    {
        return None;
    }
    Some((
        LuxColorCall {
            span: SourceSpan::new(
                file.id,
                tokens[index].span.byte_start,
                tokens[close].span.byte_end,
            ),
            constructor: name.clone(),
            r: args[0],
            g: args[1],
            b: args[2],
            a: args.get(3).copied().unwrap_or(255),
        },
        close + 1,
    ))
}

fn parse_color_args(
    file: &SourceFile,
    tokens: &[Token],
    start: usize,
    end: usize,
) -> Option<Vec<u8>> {
    let mut args = Vec::new();
    let mut index = start;
    while index < end {
        args.push(parse_byte_literal(file, tokens.get(index)?)?);
        index += 1;
        if index == end {
            break;
        }
        if !matches!(tokens.get(index)?.kind, TokenKind::Comma) {
            return None;
        }
        index += 1;
    }
    (args.len() == 3 || args.len() == 4).then_some(args)
}

fn parse_byte_literal(file: &SourceFile, token: &Token) -> Option<u8> {
    let TokenKind::Number(_) = &token.kind else {
        return None;
    };
    let text = &file.text[token.span.byte_start..token.span.byte_end];
    if text.contains(['.', 'e', 'E']) {
        return None;
    }
    text.parse::<u16>()
        .ok()
        .filter(|value| *value <= 255)
        .map(|value| value as u8)
}

fn color_constructor_names(tokens: &[Token]) -> BTreeMap<String, usize> {
    let mut constructors = BTreeMap::from([("Color".to_string(), 0usize)]);
    let mut changed = true;
    while changed {
        changed = false;
        let mut index = 0usize;
        while index < tokens.len() {
            if !matches!(tokens[index].kind, TokenKind::KwLocal | TokenKind::KwConst) {
                index += 1;
                continue;
            }
            let statement_end = simple_statement_end(tokens, index + 1);
            if collect_color_aliases(tokens, index + 1, statement_end, &mut constructors) {
                changed = true;
            }
            index = statement_end.max(index + 1);
        }
    }
    constructors
}

fn collect_color_aliases(
    tokens: &[Token],
    start: usize,
    end: usize,
    constructors: &mut BTreeMap<String, usize>,
) -> bool {
    let Some(eq) = find_top_level_eq(tokens, start, end) else {
        return false;
    };
    let names = simple_identifier_list(tokens, start, eq);
    let values = simple_identifier_list(tokens, eq + 1, end);
    if names.is_empty() || values.is_empty() {
        return false;
    }
    let mut changed = false;
    for (name, value) in names.into_iter().zip(values) {
        if is_color_constructor_at(constructors, &value, eq + 1) {
            let visible_from = end;
            match constructors.get_mut(&name) {
                Some(existing) if visible_from < *existing => {
                    *existing = visible_from;
                    changed = true;
                }
                None => {
                    constructors.insert(name, visible_from);
                    changed = true;
                }
                _ => {}
            }
        }
    }
    changed
}

fn is_color_constructor_at(
    constructors: &BTreeMap<String, usize>,
    name: &str,
    token_index: usize,
) -> bool {
    constructors
        .get(name)
        .is_some_and(|visible_from| *visible_from <= token_index)
}

fn simple_identifier_list(tokens: &[Token], start: usize, end: usize) -> Vec<String> {
    let mut out = Vec::new();
    let mut expect_identifier = true;
    for token in &tokens[start..end] {
        if expect_identifier {
            let TokenKind::Identifier(name) = &token.kind else {
                return Vec::new();
            };
            out.push(name.clone());
            expect_identifier = false;
        } else if matches!(token.kind, TokenKind::Comma) {
            expect_identifier = true;
        } else {
            return Vec::new();
        }
    }
    if expect_identifier { Vec::new() } else { out }
}

fn find_top_level_eq(tokens: &[Token], start: usize, end: usize) -> Option<usize> {
    let mut paren = 0usize;
    let mut brace = 0usize;
    let mut bracket = 0usize;
    for (index, token) in tokens.iter().enumerate().take(end).skip(start) {
        match token.kind {
            TokenKind::LParen => paren += 1,
            TokenKind::RParen => paren = paren.saturating_sub(1),
            TokenKind::LBrace => brace += 1,
            TokenKind::RBrace => brace = brace.saturating_sub(1),
            TokenKind::LBracket => bracket += 1,
            TokenKind::RBracket => bracket = bracket.saturating_sub(1),
            TokenKind::Eq if paren == 0 && brace == 0 && bracket == 0 => return Some(index),
            _ => {}
        }
    }
    None
}

fn simple_statement_end(tokens: &[Token], start: usize) -> usize {
    let mut paren = 0usize;
    let mut brace = 0usize;
    let mut bracket = 0usize;
    for (index, token) in tokens.iter().enumerate().skip(start) {
        if token.leading_newline && paren == 0 && brace == 0 && bracket == 0 {
            return index;
        }
        match token.kind {
            TokenKind::LParen => paren += 1,
            TokenKind::RParen => paren = paren.saturating_sub(1),
            TokenKind::LBrace => brace += 1,
            TokenKind::RBrace => brace = brace.saturating_sub(1),
            TokenKind::LBracket => bracket += 1,
            TokenKind::RBracket => bracket = bracket.saturating_sub(1),
            TokenKind::Semicolon if paren == 0 && brace == 0 && bracket == 0 => return index + 1,
            _ => {}
        }
    }
    tokens.len()
}

fn matching_paren(tokens: &[Token], open_index: usize) -> Option<usize> {
    let mut depth = 0usize;
    for (index, token) in tokens.iter().enumerate().skip(open_index) {
        match token.kind {
            TokenKind::LParen => depth += 1,
            TokenKind::RParen => {
                depth = depth.saturating_sub(1);
                if depth == 0 {
                    return Some(index);
                }
            }
            _ => {}
        }
    }
    None
}

fn span_for_range(file: &SourceFile, range: Range) -> Option<SourceSpan> {
    let start =
        file.offset_at_line_col_utf16(range.start.line as usize, range.start.character as usize);
    let end = file.offset_at_line_col_utf16(range.end.line as usize, range.end.character as usize);
    (start <= end).then(|| SourceSpan::new(file.id, start, end))
}

fn spans_touch(left: SourceSpan, right: SourceSpan) -> bool {
    left.byte_start <= right.byte_end && right.byte_start <= left.byte_end
}

fn lsp_color(r: u8, g: u8, b: u8, a: u8) -> Color {
    Color {
        red: f32::from(r) / 255.0,
        green: f32::from(g) / 255.0,
        blue: f32::from(b) / 255.0,
        alpha: f32::from(a) / 255.0,
    }
}

fn color_channel_to_byte(value: f32) -> u8 {
    (value.clamp(0.0, 1.0) * 255.0).round() as u8
}

fn color_call_label(constructor: &str, r: u8, g: u8, b: u8, a: u8) -> String {
    if a == 255 {
        format!("{constructor}({r}, {g}, {b})")
    } else {
        format!("{constructor}({r}, {g}, {b}, {a})")
    }
}
