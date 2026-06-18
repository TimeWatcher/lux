use crate::lex::{Lexer, Token, TokenKind};
use crate::source::SourceFile;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum CompletionContext {
    ImportSource,
    ImportSpecifierList { source: Option<String> },
    ExportList,
    ApiMember { prefix: String },
    General,
}

pub(crate) struct PackageMemberCall<'a> {
    pub(crate) path: Vec<&'a str>,
    pub(crate) active_parameter: usize,
}

pub(crate) fn completion_context(prefix: &str, suffix: &str) -> CompletionContext {
    let line = format!("{prefix}{suffix}");
    let cursor = prefix.len();
    if let Some(context) = structured_completion_context(&line, cursor) {
        return context;
    }
    let trimmed = line.trim_start();
    if let Some(prefix) = api_member_prefix(prefix) {
        return CompletionContext::ApiMember { prefix };
    }
    if is_import_specifier_context(&line, cursor) {
        return CompletionContext::ImportSpecifierList {
            source: import_source_for_specifier_list(&line),
        };
    }
    if is_import_source_context(prefix) {
        return CompletionContext::ImportSource;
    }
    if trimmed.starts_with("export") && is_cursor_inside_braces(&line, cursor) {
        return CompletionContext::ExportList;
    }
    CompletionContext::General
}

pub(crate) fn completion_context_at(text: &str, offset: usize) -> CompletionContext {
    let offset = floor_char_boundary(text, offset.min(text.len()));
    if let Some(context) = structured_completion_context(text, offset) {
        return context;
    }
    let line_start = text[..offset].rfind('\n').map(|index| index + 1).unwrap_or(0);
    let line_end = text[offset..]
        .find('\n')
        .map(|index| offset + index)
        .unwrap_or(text.len());
    completion_context(&text[line_start..offset], &text[offset..line_end])
}

pub(crate) fn should_flush_analysis_for_completion(context: &CompletionContext) -> bool {
    !matches!(
        context,
        CompletionContext::ApiMember { .. }
            | CompletionContext::ImportSpecifierList { .. }
            | CompletionContext::ExportList
    )
}

pub(crate) fn previous_non_whitespace_char(text: &str, offset: usize) -> Option<char> {
    let mut index = floor_char_boundary(text, offset.min(text.len()));
    while index > 0 {
        let ch = text[..index].chars().next_back()?;
        index -= ch.len_utf8();
        if !ch.is_whitespace() {
            return Some(ch);
        }
    }
    None
}

pub(crate) fn package_member_call_at_offset(
    text: &str,
    offset: usize,
) -> Option<PackageMemberCall<'_>> {
    let (path_start, path_end, open_paren) = package_member_call_bounds(text, offset)?;
    let path = member_path_from_prefix(&text[path_start..path_end])?;
    let active_parameter = active_parameter_in_text_args(text, open_paren + 1, offset);
    Some(PackageMemberCall {
        path,
        active_parameter,
    })
}

pub(crate) fn package_member_path_at_offset(text: &str, offset: usize) -> Option<Vec<&str>> {
    if let Some(call) = package_member_call_at_offset(text, offset) {
        return Some(call.path);
    }
    let (start, end) = dotted_identifier_bounds_at_offset(text, offset)?;
    member_path_from_prefix(&text[start..end])
}

pub(crate) fn api_member_prefix(prefix: &str) -> Option<String> {
    let token = prefix
        .rsplit(|ch: char| !(ch.is_ascii_alphanumeric() || matches!(ch, '_' | '.' | ':')))
        .next()
        .unwrap_or_default();
    if token.ends_with('.') || token.ends_with(':') {
        return Some(token.to_string());
    }
    token
        .rfind(['.', ':'])
        .map(|index| token[..index].to_string())
        .filter(|prefix| !prefix.is_empty())
}

pub(crate) fn identifier_prefix(prefix: &str) -> &str {
    prefix
        .rsplit(|ch: char| !(ch.is_ascii_alphanumeric() || ch == '_'))
        .next()
        .unwrap_or_default()
}

pub(crate) fn member_path_from_prefix(prefix: &str) -> Option<Vec<&str>> {
    let path = prefix.trim_end_matches(['.', ':']);
    if path.is_empty() || path.contains(':') {
        return None;
    }
    let parts = path.split('.').collect::<Vec<_>>();
    if parts.is_empty() || parts.iter().any(|part| part.is_empty()) {
        return None;
    }
    Some(parts)
}

fn package_member_call_bounds(text: &str, offset: usize) -> Option<(usize, usize, usize)> {
    let search_end = offset.min(text.len());
    let open_paren = text[..search_end].rfind('(')?;
    if text[open_paren..search_end].contains([')', '\n', '\r']) {
        return None;
    }
    let prefix_end = trim_ascii_whitespace_end(text, open_paren);
    let (path_start, path_end) = dotted_identifier_bounds_ending_at(text, prefix_end)?;
    Some((path_start, path_end, open_paren))
}

fn dotted_identifier_bounds_at_offset(text: &str, offset: usize) -> Option<(usize, usize)> {
    let mut start = offset.min(text.len());
    while start > 0 {
        let Some(ch) = text[..start].chars().next_back() else {
            break;
        };
        if !is_member_path_char(ch) {
            break;
        }
        start -= ch.len_utf8();
    }
    let mut end = offset.min(text.len());
    while end < text.len() {
        let Some(ch) = text[end..].chars().next() else {
            break;
        };
        if !is_member_path_char(ch) {
            break;
        }
        end += ch.len_utf8();
    }
    (start < end).then_some((start, end))
}

fn dotted_identifier_bounds_ending_at(text: &str, end: usize) -> Option<(usize, usize)> {
    if end == 0 {
        return None;
    }
    let mut start = end.min(text.len());
    while start > 0 {
        let Some(ch) = text[..start].chars().next_back() else {
            break;
        };
        if !is_member_path_char(ch) {
            break;
        }
        start -= ch.len_utf8();
    }
    (start < end).then_some((start, end))
}

fn trim_ascii_whitespace_end(text: &str, mut end: usize) -> usize {
    while end > 0 {
        let Some(ch) = text[..end].chars().next_back() else {
            break;
        };
        if !ch.is_ascii_whitespace() {
            break;
        }
        end -= ch.len_utf8();
    }
    end
}

fn is_member_path_char(ch: char) -> bool {
    ch.is_ascii_alphanumeric() || matches!(ch, '_' | '.')
}

fn structured_completion_context(text: &str, offset: usize) -> Option<CompletionContext> {
    let file = SourceFile::new(0, None, text.to_string());
    let tokens = Lexer::new(&file)
        .lex_all()
        .tokens
        .into_iter()
        .filter(|token| !matches!(token.kind, TokenKind::Eof))
        .collect::<Vec<_>>();
    import_specifier_context(&tokens, text, offset)
        .or_else(|| export_specifier_context(&tokens, text.len(), offset))
}

fn import_specifier_context(
    tokens: &[Token],
    text: &str,
    offset: usize,
) -> Option<CompletionContext> {
    for (index, token) in tokens.iter().enumerate() {
        if !matches!(token.kind, TokenKind::KwImport) {
            continue;
        }
        let mut list_start = index + 1;
        if matches!(
            tokens.get(list_start).map(|token| &token.kind),
            Some(TokenKind::Identifier(name)) if name == "macro"
        ) {
            list_start += 1;
        }
        if !matches!(
            tokens.get(list_start).map(|token| &token.kind),
            Some(TokenKind::LBrace)
        ) {
            continue;
        }
        let close = matching_delimiter(tokens, list_start, TokenKind::LBrace, TokenKind::RBrace);
        let cursor_inside = close
            .map(|list_end| cursor_inside_token_range(tokens, list_start, list_end, offset))
            .unwrap_or_else(|| {
                tokens[list_start].span.byte_end <= offset && offset <= text.len()
            });
        if !cursor_inside {
            continue;
        }
        let source = close.and_then(|list_end| import_source_after_brace(tokens, text, list_end));
        return Some(CompletionContext::ImportSpecifierList { source });
    }
    None
}

fn export_specifier_context(
    tokens: &[Token],
    text_len: usize,
    offset: usize,
) -> Option<CompletionContext> {
    for (index, token) in tokens.iter().enumerate() {
        if !matches!(token.kind, TokenKind::KwExport) {
            continue;
        }
        let Some(open) = export_list_open(tokens, index + 1) else {
            continue;
        };
        let close = matching_delimiter(tokens, open, TokenKind::LBrace, TokenKind::RBrace);
        let cursor_inside = close
            .map(|list_end| cursor_inside_token_range(tokens, open, list_end, offset))
            .unwrap_or_else(|| tokens[open].span.byte_end <= offset && offset <= text_len);
        if cursor_inside {
            return Some(CompletionContext::ExportList);
        }
    }
    None
}

fn export_list_open(tokens: &[Token], mut index: usize) -> Option<usize> {
    while let Some(token) = tokens.get(index) {
        match &token.kind {
            TokenKind::Identifier(name)
                if matches!(
                    name.as_str(),
                    "runtime" | "macro" | "host" | "client" | "server" | "shared"
                ) =>
            {
                index += 1;
            }
            TokenKind::LBrace => return Some(index),
            _ => return None,
        }
    }
    None
}

fn cursor_inside_token_range(
    tokens: &[Token],
    open_index: usize,
    close_or_end_index: usize,
    offset: usize,
) -> bool {
    let Some(open) = tokens.get(open_index) else {
        return false;
    };
    let end = tokens
        .get(close_or_end_index)
        .map(|token| token.span.byte_start)
        .unwrap_or_else(|| tokens.last().map(|token| token.span.byte_end).unwrap_or(0));
    open.span.byte_end <= offset && offset <= end
}

fn import_source_after_brace(tokens: &[Token], text: &str, brace_end_index: usize) -> Option<String> {
    let mut index = brace_end_index + 1;
    if !matches!(
        tokens.get(index).map(|token| &token.kind),
        Some(TokenKind::Identifier(name)) if name == "from"
    ) {
        return None;
    }
    index += 1;
    let token = tokens.get(index)?;
    match &token.kind {
        TokenKind::String(source) => Some(source.clone()),
        _ => source_literal_at(text, token.span.byte_start),
    }
}

fn source_literal_at(text: &str, start: usize) -> Option<String> {
    let text = &text[start.min(text.len())..];
    let mut chars = text.chars();
    let quote = chars.next()?;
    if !matches!(quote, '"' | '\'') {
        return None;
    }
    let mut value = String::new();
    let mut escape = false;
    for ch in chars {
        if escape {
            value.push(ch);
            escape = false;
        } else if ch == '\\' {
            escape = true;
        } else if ch == quote {
            break;
        } else {
            value.push(ch);
        }
    }
    Some(value)
}

fn matching_delimiter(
    tokens: &[Token],
    open: usize,
    open_kind: TokenKind,
    close_kind: TokenKind,
) -> Option<usize> {
    let mut depth = 0usize;
    for (index, token) in tokens.iter().enumerate().skip(open) {
        if same_token_kind(&token.kind, &open_kind) {
            depth += 1;
        } else if same_token_kind(&token.kind, &close_kind) {
            depth = depth.saturating_sub(1);
            if depth == 0 {
                return Some(index);
            }
        }
    }
    None
}

fn same_token_kind(a: &TokenKind, b: &TokenKind) -> bool {
    std::mem::discriminant(a) == std::mem::discriminant(b)
}

fn floor_char_boundary(text: &str, mut offset: usize) -> usize {
    while offset > 0 && !text.is_char_boundary(offset) {
        offset -= 1;
    }
    offset
}

fn active_parameter_in_text_args(text: &str, args_start: usize, offset: usize) -> usize {
    let mut active = 0usize;
    let mut depth = 0usize;
    let mut string_quote = None::<char>;
    let mut escape = false;
    let end = offset.min(text.len());
    for ch in text[args_start.min(text.len())..end].chars() {
        if let Some(quote) = string_quote {
            if escape {
                escape = false;
            } else if ch == '\\' {
                escape = true;
            } else if ch == quote {
                string_quote = None;
            }
            continue;
        }
        match ch {
            '"' | '\'' => string_quote = Some(ch),
            '(' | '[' | '{' => depth = depth.saturating_add(1),
            ')' | ']' | '}' => depth = depth.saturating_sub(1),
            ',' if depth == 0 => active = active.saturating_add(1),
            _ => {}
        }
    }
    active
}

fn is_import_source_context(prefix: &str) -> bool {
    let trimmed = prefix.trim_start();
    if !trimmed.starts_with("import") {
        return false;
    }
    let Some(from_index) = trimmed.rfind("from") else {
        return false;
    };
    let after_from = trimmed[from_index + "from".len()..].trim_start();
    after_from.starts_with('"') || after_from.starts_with('\'') || after_from.is_empty()
}

fn is_import_specifier_context(line: &str, cursor: usize) -> bool {
    line.trim_start().starts_with("import") && is_cursor_inside_braces(line, cursor)
}

fn is_cursor_inside_braces(line: &str, cursor: usize) -> bool {
    let Some(open) = line.find('{') else {
        return false;
    };
    let close = line[open + 1..]
        .find('}')
        .map(|offset| open + 1 + offset)
        .unwrap_or(line.len());
    open < cursor && cursor <= close
}

fn import_source_for_specifier_list(prefix: &str) -> Option<String> {
    let from_index = prefix.rfind("from")?;
    let after_from = prefix[from_index + "from".len()..].trim_start();
    let quote = after_from.chars().next()?;
    if quote != '"' && quote != '\'' {
        return None;
    }
    let rest = &after_from[quote.len_utf8()..];
    let value = rest.split(quote).next().unwrap_or(rest).to_string();
    Some(value)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn package_member_call_tracks_active_parameter_after_comma_space() {
        let text = "MPaint.chamferBoxEx(0, 0, ";
        let call = package_member_call_at_offset(text, text.len()).expect("call");
        assert_eq!(call.path, vec!["MPaint", "chamferBoxEx"]);
        assert_eq!(call.active_parameter, 2);
    }

    #[test]
    fn package_member_call_ignores_nested_commas_and_strings() {
        let text = "MPaint.chamferBoxEx({ x = 1, y = 2 }, \"a,b\", ";
        let call = package_member_call_at_offset(text, text.len()).expect("call");
        assert_eq!(call.path, vec!["MPaint", "chamferBoxEx"]);
        assert_eq!(call.active_parameter, 2);
    }

    #[test]
    fn package_member_path_resolves_from_call_target_or_member_name() {
        let text = "mgfx.paint.chamferBoxEx(0, 0, ";
        let offset = "mgfx.paint.chamferBoxEx".len();
        assert_eq!(
            package_member_path_at_offset(text, offset),
            Some(vec!["mgfx", "paint", "chamferBoxEx"])
        );
        assert_eq!(
            package_member_path_at_offset(text, text.len()),
            Some(vec!["mgfx", "paint", "chamferBoxEx"])
        );
    }

    #[test]
    fn completion_context_at_handles_import_list_slots_after_commas() {
        assert_eq!(
            completion_context_at(
                "import { Button,  } from \"@vendor/ui\"",
                "import { Button, ".len()
            ),
            CompletionContext::ImportSpecifierList {
                source: Some("@vendor/ui".into())
            }
        );
        assert_eq!(
            completion_context_at(
                "import { Button as B,  } from \"@vendor/ui\"",
                "import { Button as B, ".len()
            ),
            CompletionContext::ImportSpecifierList {
                source: Some("@vendor/ui".into())
            }
        );
        assert_eq!(
            completion_context_at(
                "import { paint as MPaint, widgets as MWidgets,  } from '@lux/mgfx'",
                "import { paint as MPaint, widgets as MWidgets, ".len()
            ),
            CompletionContext::ImportSpecifierList {
                source: Some("@lux/mgfx".into())
            }
        );
    }

    #[test]
    fn completion_context_at_handles_multiline_import_list_slots() {
        let text = "import {\n  Button,\n  \n} from \"@vendor/ui\"";
        let offset = text.find("  \n}").expect("slot") + 2;
        assert_eq!(
            completion_context_at(text, offset),
            CompletionContext::ImportSpecifierList {
                source: Some("@vendor/ui".into())
            }
        );
    }

    #[test]
    fn completion_context_at_handles_open_import_list_without_source() {
        let text = "import {\n  Button,\n  ";
        assert_eq!(
            completion_context_at(text, text.len()),
            CompletionContext::ImportSpecifierList { source: None }
        );
    }

    #[test]
    fn completion_context_at_handles_export_list_slots_after_commas() {
        assert_eq!(
            completion_context_at("export { mount,  }", "export { mount, ".len()),
            CompletionContext::ExportList
        );
        assert_eq!(
            completion_context_at("export client { mount,  }", "export client { mount, ".len()),
            CompletionContext::ExportList
        );
    }

    #[test]
    fn completion_context_at_does_not_treat_table_literals_as_export_or_import_lists() {
        let text = "local t = { Button,  }";
        assert_eq!(completion_context_at(text, "local t = { Button, ".len()), CompletionContext::General);
    }
}
