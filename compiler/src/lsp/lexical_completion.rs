use std::collections::BTreeMap;
use std::path::Path;

use crate::analysis::{CompletionCandidate, CompletionCandidateKind, ProjectAnalysis};
use crate::lex::{Lexer, Token, TokenKind};
use crate::source::SourceFile;
pub(crate) fn lexical_binding_completions(
    file: &SourceFile,
    offset: usize,
) -> Vec<CompletionCandidate> {
    let tokens = lex_completion_tokens(file);
    let mut collector = LexicalCompletionCollector::new(file, &tokens, offset);
    collector.collect_current_part();
    collector.into_candidates()
}

pub(crate) fn module_part_lexical_completions(
    analysis: &ProjectAnalysis,
    path: &Path,
    current_file: &SourceFile,
    offset: usize,
) -> Vec<CompletionCandidate> {
    let Some(module) = analysis.module_for_path(path) else {
        return lexical_binding_completions(current_file, offset);
    };
    let mut candidates = BTreeMap::<String, CompletionCandidate>::new();
    for part in &module.parts {
        let is_current = same_path(&part.path, path);
        let file = if is_current {
            current_file
        } else {
            &part.source_file
        };
        let part_offset = if is_current { offset } else { file.text.len() };
        let tokens = lex_completion_tokens(file);
        let mut collector = LexicalCompletionCollector::new(file, &tokens, part_offset);
        if is_current {
            collector.collect_current_part();
        } else {
            collector.collect_module_scope_only();
        }
        for candidate in collector.into_candidates() {
            candidates
                .entry(candidate.label.clone())
                .or_insert(candidate);
        }
    }
    candidates.into_values().collect()
}

fn lex_completion_tokens(file: &SourceFile) -> Vec<Token> {
    Lexer::new(file)
        .lex_all()
        .tokens
        .into_iter()
        .filter(|token| !matches!(token.kind, TokenKind::Eof))
        .collect()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LexicalBindingKind {
    Function,
    Variable,
    Constant,
    Parameter,
    Import,
}

struct LexicalCompletionCollector<'a> {
    file: &'a SourceFile,
    tokens: &'a [Token],
    offset: usize,
    candidates: BTreeMap<String, CompletionCandidate>,
}

impl<'a> LexicalCompletionCollector<'a> {
    fn new(file: &'a SourceFile, tokens: &'a [Token], offset: usize) -> Self {
        Self {
            file,
            tokens,
            offset,
            candidates: BTreeMap::new(),
        }
    }

    fn collect_current_part(&mut self) {
        self.collect_module_scope();
        self.collect_part_imports();
        self.collect_visible_locals_and_params();
    }

    fn collect_module_scope_only(&mut self) {
        self.collect_module_scope();
    }

    fn into_candidates(self) -> Vec<CompletionCandidate> {
        self.candidates.into_values().collect()
    }

    fn collect_module_scope(&mut self) {
        let mut index = 0usize;
        while index < self.tokens.len() {
            if !self.is_top_level(index) {
                index += 1;
                continue;
            }
            index = self.collect_top_level_stmt(index);
        }
    }

    fn collect_top_level_stmt(&mut self, index: usize) -> usize {
        match &self.tokens[index].kind {
            TokenKind::KwExport => self.collect_wrapped_top_level_stmt(index + 1),
            TokenKind::Identifier(name) if is_realm_name(name) => {
                self.collect_wrapped_top_level_stmt(index + 1)
            }
            TokenKind::KwFn => {
                self.collect_function_decl(index, true);
                self.next_statement_index(index)
            }
            TokenKind::KwLocal | TokenKind::KwConst => {
                let kind = if matches!(self.tokens[index].kind, TokenKind::KwConst) {
                    LexicalBindingKind::Constant
                } else {
                    LexicalBindingKind::Variable
                };
                if matches!(
                    self.tokens.get(index + 1).map(|token| &token.kind),
                    Some(TokenKind::KwFunction)
                ) {
                    if let Some((name, span_start, span_end)) = self.identifier_name(index + 2) {
                        self.add_candidate(
                            name,
                            LexicalBindingKind::Function,
                            "module function binding",
                            span_start,
                            span_end,
                        );
                    }
                } else {
                    for local_index in self.binding_decl_name_indices(index + 1) {
                        if let Some((name, span_start, span_end)) =
                            self.identifier_name(local_index)
                        {
                            self.add_candidate(name, kind, "module binding", span_start, span_end);
                        }
                    }
                }
                self.next_statement_index(index)
            }
            _ => self.next_statement_index(index),
        }
    }

    fn collect_wrapped_top_level_stmt(&mut self, mut index: usize) -> usize {
        while let Some(token) = self.tokens.get(index) {
            match &token.kind {
                TokenKind::Identifier(name) if is_realm_name(name) => index += 1,
                TokenKind::Identifier(name) if name == "macro" => index += 1,
                _ => break,
            }
        }
        if index < self.tokens.len() {
            self.collect_top_level_stmt(index)
        } else {
            index
        }
    }

    fn collect_part_imports(&mut self) {
        let mut index = 0usize;
        while index < self.tokens.len() {
            if !matches!(self.tokens[index].kind, TokenKind::KwImport) {
                index += 1;
                continue;
            }
            if self.tokens[index].span.byte_start > self.offset {
                break;
            }
            let statement_end = self.next_statement_index(index);
            if statement_end.saturating_sub(1) < index {
                index = statement_end.max(index + 1);
                continue;
            }
            if matches!(
                self.tokens.get(index + 1).map(|token| &token.kind),
                Some(TokenKind::Identifier(name)) if name == "macro"
            ) {
                self.collect_import_specifiers(index + 2, statement_end);
            } else {
                self.collect_import_specifiers(index + 1, statement_end);
            }
            index = statement_end;
        }
    }

    fn collect_import_specifiers(&mut self, start: usize, end: usize) {
        match self.tokens.get(start).map(|token| &token.kind) {
            Some(TokenKind::LBrace) => {
                let Some(close) = self.matching_delimiter(start, Delimiter::Brace) else {
                    return;
                };
                let close = close.min(end.saturating_sub(1));
                let mut index = start + 1;
                while index < close {
                    let Some((imported, _, _)) = self.identifier_name(index) else {
                        index += 1;
                        continue;
                    };
                    let mut local_index = index;
                    if matches!(
                        self.tokens.get(index + 1).map(|token| &token.kind),
                        Some(TokenKind::Identifier(name)) if name == "as"
                    ) && self.is_identifier(index + 2)
                    {
                        local_index = index + 2;
                    }
                    if let Some((local, span_start, span_end)) = self.identifier_name(local_index) {
                        self.add_candidate(
                            local,
                            LexicalBindingKind::Import,
                            "part import binding",
                            span_start,
                            span_end,
                        );
                    } else {
                        self.add_candidate(
                            imported,
                            LexicalBindingKind::Import,
                            "part import binding",
                            self.tokens[index].span.byte_start,
                            self.tokens[index].span.byte_end,
                        );
                    }
                    index += 1;
                }
            }
            Some(TokenKind::Star) => {
                if matches!(
                    self.tokens.get(start + 1).map(|token| &token.kind),
                    Some(TokenKind::Identifier(name)) if name == "as"
                ) && let Some((local, span_start, span_end)) = self.identifier_name(start + 2)
                {
                    self.add_candidate(
                        local,
                        LexicalBindingKind::Import,
                        "part namespace import binding",
                        span_start,
                        span_end,
                    );
                }
            }
            _ => {}
        }
    }

    fn collect_visible_locals_and_params(&mut self) {
        for index in 0..self.tokens.len() {
            if self.tokens[index].span.byte_start > self.offset {
                break;
            }
            match self.tokens[index].kind {
                TokenKind::KwFn => self.collect_visible_function_params(index),
                TokenKind::LParen if self.is_arrow_param_list(index) => {
                    self.collect_visible_arrow_params(index)
                }
                TokenKind::KwLocal | TokenKind::KwConst => self.collect_visible_local_decl(index),
                _ => {}
            }
        }
    }

    fn collect_visible_function_params(&mut self, fn_index: usize) {
        let Some(open) =
            self.next_token_index(fn_index + 1, |kind| matches!(kind, TokenKind::LParen))
        else {
            return;
        };
        let Some(close) = self.matching_delimiter(open, Delimiter::Paren) else {
            return;
        };
        let Some(scope_end) = self.function_scope_end(fn_index) else {
            return;
        };
        if self.offset <= self.tokens[close].span.byte_end || self.offset > scope_end {
            return;
        }
        for param_index in self.param_name_indices(open + 1, close) {
            if let Some((name, span_start, span_end)) = self.identifier_name(param_index) {
                self.add_candidate(
                    name,
                    LexicalBindingKind::Parameter,
                    "function parameter",
                    span_start,
                    span_end,
                );
            }
        }
    }

    fn collect_visible_arrow_params(&mut self, open: usize) {
        let Some(close) = self.matching_delimiter(open, Delimiter::Paren) else {
            return;
        };
        let Some(after) = self.tokens.get(close + 1) else {
            return;
        };
        if !matches!(
            after.kind,
            TokenKind::ArrowNormal | TokenKind::ArrowImplicitSelf
        ) {
            return;
        }
        let scope_end = self.arrow_scope_end(close + 1);
        if self.offset <= self.tokens[close].span.byte_end || self.offset > scope_end {
            return;
        }
        for param_index in self.param_name_indices(open + 1, close) {
            if let Some((name, span_start, span_end)) = self.identifier_name(param_index) {
                self.add_candidate(
                    name,
                    LexicalBindingKind::Parameter,
                    "arrow function parameter",
                    span_start,
                    span_end,
                );
            }
        }
    }

    fn collect_visible_local_decl(&mut self, local_index: usize) {
        if self.tokens[local_index].span.byte_start > self.offset {
            return;
        }
        if self.scope_depth_at(local_index) == 0 {
            return;
        }
        let kind = if matches!(self.tokens[local_index].kind, TokenKind::KwConst) {
            LexicalBindingKind::Constant
        } else {
            LexicalBindingKind::Variable
        };
        if matches!(
            self.tokens.get(local_index + 1).map(|token| &token.kind),
            Some(TokenKind::KwFunction)
        ) {
            if let Some((name, span_start, span_end)) = self.identifier_name(local_index + 2)
                && span_end <= self.offset
                && self.local_binding_visible(local_index + 2)
            {
                self.add_candidate(
                    name,
                    LexicalBindingKind::Function,
                    "local function binding",
                    span_start,
                    span_end,
                );
            }
            return;
        }
        for name_index in self.binding_decl_name_indices(local_index + 1) {
            let Some((name, span_start, span_end)) = self.identifier_name(name_index) else {
                continue;
            };
            if span_end > self.offset || !self.local_binding_visible(name_index) {
                continue;
            }
            self.add_candidate(name, kind, "local binding", span_start, span_end);
        }
    }

    fn collect_function_decl(&mut self, fn_index: usize, module_scope: bool) {
        if let Some((name, span_start, span_end)) = self.function_decl_name(fn_index) {
            self.add_candidate(
                name,
                LexicalBindingKind::Function,
                if module_scope {
                    "module function binding"
                } else {
                    "function binding"
                },
                span_start,
                span_end,
            );
        }
    }

    fn binding_decl_name_indices(&self, start: usize) -> Vec<usize> {
        let end = self.next_statement_index(start).min(self.tokens.len());
        let mut names = Vec::new();
        let mut index = start;
        let mut brace_depth = 0usize;
        let mut bracket_depth = 0usize;
        let mut paren_depth = 0usize;
        while index < end {
            match self.tokens[index].kind {
                TokenKind::Eq if brace_depth == 0 && bracket_depth == 0 && paren_depth == 0 => {
                    break;
                }
                TokenKind::Identifier(_)
                    if brace_depth == 0 && bracket_depth == 0 && paren_depth == 0 =>
                {
                    names.push(index);
                }
                TokenKind::Identifier(_) if self.is_destructure_binding_name(index) => {
                    names.push(index);
                }
                TokenKind::LBrace => brace_depth += 1,
                TokenKind::RBrace => brace_depth = brace_depth.saturating_sub(1),
                TokenKind::LBracket => bracket_depth += 1,
                TokenKind::RBracket => bracket_depth = bracket_depth.saturating_sub(1),
                TokenKind::LParen => paren_depth += 1,
                TokenKind::RParen => paren_depth = paren_depth.saturating_sub(1),
                _ => {}
            }
            index += 1;
        }
        names
    }

    fn is_destructure_binding_name(&self, index: usize) -> bool {
        let Some(prev) = index.checked_sub(1).and_then(|prev| self.tokens.get(prev)) else {
            return false;
        };
        if matches!(prev.kind, TokenKind::Dot) {
            return false;
        }
        let Some(container) = self.innermost_open_delimiter_before(index) else {
            return false;
        };
        match self.tokens[container].kind {
            TokenKind::LBracket => true,
            TokenKind::LBrace => {
                matches!(prev.kind, TokenKind::Colon)
                    || !matches!(
                        self.tokens.get(index + 1).map(|token| &token.kind),
                        Some(TokenKind::Colon)
                    )
            }
            _ => false,
        }
    }

    fn param_name_indices(&self, start: usize, end: usize) -> Vec<usize> {
        let mut names = Vec::new();
        let mut index = start;
        let mut paren = 0usize;
        let mut brace = 0usize;
        let mut bracket = 0usize;
        let mut in_default = false;
        while index < end {
            match self.tokens[index].kind {
                TokenKind::Comma if paren == 0 && brace == 0 && bracket == 0 => {
                    in_default = false;
                }
                TokenKind::Eq if paren == 0 && brace == 0 && bracket == 0 => {
                    in_default = true;
                }
                TokenKind::Identifier(_)
                    if !in_default && paren == 0 && brace == 0 && bracket == 0 =>
                {
                    if !matches!(
                        self.tokens
                            .get(index.saturating_sub(1))
                            .map(|token| &token.kind),
                        Some(TokenKind::Dot) | Some(TokenKind::Colon)
                    ) {
                        names.push(index);
                    }
                }
                TokenKind::LParen => paren += 1,
                TokenKind::RParen => paren = paren.saturating_sub(1),
                TokenKind::LBrace => brace += 1,
                TokenKind::RBrace => brace = brace.saturating_sub(1),
                TokenKind::LBracket => bracket += 1,
                TokenKind::RBracket => bracket = bracket.saturating_sub(1),
                _ => {}
            }
            index += 1;
        }
        names
    }

    fn function_decl_name(&self, fn_index: usize) -> Option<(String, usize, usize)> {
        let mut index = fn_index + 1;
        let mut name = None;
        while index < self.tokens.len() {
            match self.tokens[index].kind {
                TokenKind::Identifier(_) => {
                    name = self.identifier_name(index);
                    index += 1;
                }
                TokenKind::Dot => index += 1,
                TokenKind::Colon => {
                    if self.is_identifier(index + 1) {
                        name = self.identifier_name(index + 1);
                    }
                    break;
                }
                TokenKind::LParen => break,
                _ => break,
            }
        }
        name
    }

    fn local_binding_visible(&self, binding_index: usize) -> bool {
        let binding_depth = self.scope_depth_at(binding_index);
        let cursor_depth = self.scope_depth_at_offset(self.offset);
        if cursor_depth < binding_depth {
            return false;
        }
        let mut depth = binding_depth;
        for token in self.tokens.iter().skip(binding_index + 1) {
            if token.span.byte_start >= self.offset {
                return true;
            }
            match token.kind {
                TokenKind::LBrace | TokenKind::KwDo | TokenKind::KwThen | TokenKind::KwRepeat => {
                    depth += 1;
                }
                TokenKind::RBrace | TokenKind::KwEnd | TokenKind::KwUntil => {
                    depth = depth.saturating_sub(1);
                    if depth < binding_depth {
                        return false;
                    }
                }
                _ => {}
            }
        }
        true
    }

    fn function_scope_end(&self, fn_index: usize) -> Option<usize> {
        let open = self.next_token_index(fn_index + 1, |kind| matches!(kind, TokenKind::LParen))?;
        let close = self.matching_delimiter(open, Delimiter::Paren)?;
        if matches!(
            self.tokens.get(close + 1).map(|token| &token.kind),
            Some(TokenKind::LBrace)
        ) {
            return Some(
                self.matching_delimiter(close + 1, Delimiter::Brace)
                    .map(|index| self.tokens[index].span.byte_end)
                    .unwrap_or(self.file.text.len()),
            );
        }
        if matches!(
            self.tokens.get(close + 1).map(|token| &token.kind),
            Some(TokenKind::Eq | TokenKind::ArrowNormal | TokenKind::ArrowImplicitSelf)
        ) {
            return Some(self.expression_scope_end(close + 1));
        }
        Some(self.block_keyword_scope_end(fn_index))
    }

    fn arrow_scope_end(&self, arrow_index: usize) -> usize {
        if matches!(
            self.tokens.get(arrow_index + 1).map(|token| &token.kind),
            Some(TokenKind::LBrace)
        ) {
            return self
                .matching_delimiter(arrow_index + 1, Delimiter::Brace)
                .map(|index| self.tokens[index].span.byte_end)
                .unwrap_or(self.file.text.len());
        }
        self.expression_scope_end(arrow_index)
    }

    fn block_keyword_scope_end(&self, start: usize) -> usize {
        let mut depth = 0usize;
        for (index, token) in self.tokens.iter().enumerate().skip(start) {
            match token.kind {
                TokenKind::KwFn
                | TokenKind::KwIf
                | TokenKind::KwDo
                | TokenKind::KwWhile
                | TokenKind::KwFor
                | TokenKind::KwRepeat => depth += 1,
                TokenKind::KwEnd | TokenKind::KwUntil => {
                    depth = depth.saturating_sub(1);
                    if depth == 0 {
                        return self.tokens[index].span.byte_end;
                    }
                }
                _ => {}
            }
        }
        self.file.text.len()
    }

    fn expression_scope_end(&self, start: usize) -> usize {
        let Some(start_token) = self.tokens.get(start) else {
            return self.file.text.len();
        };
        let line = self.file.line_col(start_token.span.byte_start).0;
        self.tokens
            .iter()
            .skip(start + 1)
            .find(|token| {
                self.file.line_col(token.span.byte_start).0 > line
                    && matches!(
                        token.kind,
                        TokenKind::KwFn
                            | TokenKind::KwLocal
                            | TokenKind::KwConst
                            | TokenKind::KwImport
                            | TokenKind::KwExport
                    )
            })
            .map(|token| token.span.byte_start)
            .unwrap_or(self.file.text.len())
    }

    fn next_statement_index(&self, start: usize) -> usize {
        let mut index = start;
        let mut paren = 0usize;
        let mut brace = 0usize;
        let mut bracket = 0usize;
        while index < self.tokens.len() {
            if index > start && paren == 0 && brace == 0 && bracket == 0 {
                if self.tokens[index].leading_newline
                    || matches!(self.tokens[index].kind, TokenKind::Semicolon)
                {
                    break;
                }
                if self.is_top_level(index)
                    && matches!(
                        self.tokens[index].kind,
                        TokenKind::KwImport
                            | TokenKind::KwExport
                            | TokenKind::KwFn
                            | TokenKind::KwLocal
                            | TokenKind::KwConst
                    )
                {
                    break;
                }
            }
            match self.tokens[index].kind {
                TokenKind::LParen => paren += 1,
                TokenKind::RParen => paren = paren.saturating_sub(1),
                TokenKind::LBrace => brace += 1,
                TokenKind::RBrace => brace = brace.saturating_sub(1),
                TokenKind::LBracket => bracket += 1,
                TokenKind::RBracket => bracket = bracket.saturating_sub(1),
                _ => {}
            }
            index += 1;
        }
        index.max(start + 1)
    }

    fn is_top_level(&self, index: usize) -> bool {
        self.scope_depth_at(index) == 0
    }

    fn scope_depth_at(&self, index: usize) -> usize {
        self.tokens
            .iter()
            .take(index)
            .fold(0usize, |depth, token| match token.kind {
                TokenKind::LBrace | TokenKind::KwDo | TokenKind::KwThen | TokenKind::KwRepeat => {
                    depth + 1
                }
                TokenKind::RBrace | TokenKind::KwEnd | TokenKind::KwUntil => {
                    depth.saturating_sub(1)
                }
                _ => depth,
            })
    }

    fn scope_depth_at_offset(&self, offset: usize) -> usize {
        self.tokens
            .iter()
            .take_while(|token| token.span.byte_start < offset)
            .fold(0usize, |depth, token| match token.kind {
                TokenKind::LBrace | TokenKind::KwDo | TokenKind::KwThen | TokenKind::KwRepeat => {
                    depth + 1
                }
                TokenKind::RBrace | TokenKind::KwEnd | TokenKind::KwUntil => {
                    depth.saturating_sub(1)
                }
                _ => depth,
            })
    }

    fn is_arrow_param_list(&self, open: usize) -> bool {
        self.matching_delimiter(open, Delimiter::Paren)
            .and_then(|close| self.tokens.get(close + 1))
            .is_some_and(|token| {
                matches!(
                    token.kind,
                    TokenKind::ArrowNormal | TokenKind::ArrowImplicitSelf
                )
            })
    }

    fn innermost_open_delimiter_before(&self, index: usize) -> Option<usize> {
        let mut stack = Vec::<usize>::new();
        for candidate in 0..index {
            match self.tokens[candidate].kind {
                TokenKind::LBrace | TokenKind::LBracket | TokenKind::LParen => {
                    stack.push(candidate);
                }
                TokenKind::RBrace => self.pop_matching_open(&mut stack, TokenKind::LBrace),
                TokenKind::RBracket => self.pop_matching_open(&mut stack, TokenKind::LBracket),
                TokenKind::RParen => self.pop_matching_open(&mut stack, TokenKind::LParen),
                _ => {}
            }
        }
        stack.pop()
    }

    fn pop_matching_open(&self, stack: &mut Vec<usize>, open_kind: TokenKind) {
        if let Some(position) = stack.iter().rposition(|index| {
            std::mem::discriminant(&self.tokens[*index].kind) == std::mem::discriminant(&open_kind)
        }) {
            stack.truncate(position);
        }
    }

    fn matching_delimiter(&self, open: usize, delimiter: Delimiter) -> Option<usize> {
        let mut depth = 0usize;
        for (index, token) in self.tokens.iter().enumerate().skip(open) {
            if delimiter.is_open(&token.kind) {
                depth += 1;
            } else if delimiter.is_close(&token.kind) {
                depth = depth.saturating_sub(1);
                if depth == 0 {
                    return Some(index);
                }
            }
        }
        None
    }

    fn next_token_index(
        &self,
        start: usize,
        predicate: impl Fn(&TokenKind) -> bool,
    ) -> Option<usize> {
        self.tokens
            .iter()
            .enumerate()
            .skip(start)
            .find(|(_, token)| predicate(&token.kind))
            .map(|(index, _)| index)
    }

    fn identifier_name(&self, index: usize) -> Option<(String, usize, usize)> {
        let token = self.tokens.get(index)?;
        match &token.kind {
            TokenKind::Identifier(name) => {
                Some((name.clone(), token.span.byte_start, token.span.byte_end))
            }
            TokenKind::Ellipsis => Some(("...".into(), token.span.byte_start, token.span.byte_end)),
            _ => None,
        }
    }

    fn is_identifier(&self, index: usize) -> bool {
        matches!(
            self.tokens.get(index).map(|token| &token.kind),
            Some(TokenKind::Identifier(_))
        )
    }

    fn add_candidate(
        &mut self,
        name: String,
        kind: LexicalBindingKind,
        detail: &'static str,
        span_start: usize,
        _span_end: usize,
    ) {
        if name.is_empty() || name == "_" || name == "from" || name == "as" {
            return;
        }
        let candidate = CompletionCandidate {
            label: name.clone(),
            kind: lexical_completion_kind(kind),
            detail: Some(detail.into()),
            documentation: Some(format!(
                "`{name}` is available from the current Lux lexical scope."
            )),
            source: None,
        };
        if span_start <= self.offset {
            self.candidates.insert(name, candidate);
        } else {
            self.candidates.entry(name).or_insert(candidate);
        }
    }
}

#[derive(Debug, Clone, Copy)]
enum Delimiter {
    Paren,
    Brace,
}

impl Delimiter {
    fn is_open(self, kind: &TokenKind) -> bool {
        matches!(
            (self, kind),
            (Self::Paren, TokenKind::LParen) | (Self::Brace, TokenKind::LBrace)
        )
    }

    fn is_close(self, kind: &TokenKind) -> bool {
        matches!(
            (self, kind),
            (Self::Paren, TokenKind::RParen) | (Self::Brace, TokenKind::RBrace)
        )
    }
}

fn lexical_completion_kind(kind: LexicalBindingKind) -> CompletionCandidateKind {
    match kind {
        LexicalBindingKind::Function => CompletionCandidateKind::Function,
        LexicalBindingKind::Variable => CompletionCandidateKind::Variable,
        LexicalBindingKind::Constant => CompletionCandidateKind::Constant,
        LexicalBindingKind::Parameter => CompletionCandidateKind::Parameter,
        LexicalBindingKind::Import => CompletionCandidateKind::Reference,
    }
}

fn is_realm_name(name: &str) -> bool {
    matches!(name, "shared" | "client" | "server")
}

fn same_path(a: &Path, b: &Path) -> bool {
    normalized_path(a) == normalized_path(b)
}

fn normalized_path(path: &Path) -> String {
    let value = path.to_string_lossy().replace('\\', "/");
    if cfg!(windows) {
        value.to_ascii_lowercase()
    } else {
        value
    }
}
