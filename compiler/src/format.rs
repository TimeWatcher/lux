use std::fmt::Write as _;

use crate::ast::{
    BinaryOp, BindingMode, Block, CallStyle, ChainExpr, ChainSegmentKind, CompoundAssignOp,
    ConditionalForm, EnumDecl, EnumRepr, EnumVariantPayload, ExportKind, Expr, ExprKind,
    ExprOrBlock, FunctionBody, FunctionDecl, FunctionExpr, FunctionName, Identifier,
    ImportSpecifier, ImportStmt, MatchLiteral, MatchPattern, MatchPatternKind, MatchPatternPayload,
    Module, Param, Pattern, PatternKind, Stmt, StmtKind, TableExpr, TableFieldKind, TemplatePart,
    TemplatePartKind, UnaryOp,
};
use crate::diag::{Diagnostic, Label, Severity};
use crate::lex::{Lexer, Token, TokenKind};
use crate::parse::Parser;
use crate::source::{SourceFile, SourceSpan};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FormatOutput {
    pub text: String,
    pub diagnostics: Vec<Diagnostic>,
}

#[derive(Debug, Clone)]
struct Line {
    start: usize,
    content_end: usize,
    end: usize,
}

pub fn format_source(file: &SourceFile) -> FormatOutput {
    let lex = Lexer::new(file).lex_all();
    let mut diagnostics = lex.diagnostics;
    if diagnostics
        .iter()
        .any(|diagnostic| diagnostic.severity == Severity::Error)
    {
        return FormatOutput {
            text: file.text.clone(),
            diagnostics,
        };
    }

    let parsed = Parser::new(&lex.tokens).parse_module();
    let parse_has_errors = parsed.has_errors();
    diagnostics.extend(parsed.diagnostics);
    if parse_has_errors {
        return FormatOutput {
            text: file.text.clone(),
            diagnostics,
        };
    }

    let text = format_lossless(file, &lex.tokens, &parsed.module);
    let formatted_file = SourceFile::new(file.id.0, file.path.clone(), text.clone());
    let formatted_lex = Lexer::new(&formatted_file).lex_all();

    if formatted_lex.has_errors() {
        diagnostics.push(format_safety_error(
            file,
            "FMT001",
            "formatter produced source that does not lex cleanly",
            "the original source was preserved",
        ));
        return FormatOutput {
            text: file.text.clone(),
            diagnostics,
        };
    }

    let formatted_parse = Parser::new(&formatted_lex.tokens).parse_module();
    if formatted_parse.has_errors() {
        diagnostics.push(format_safety_error(
            file,
            "FMT001",
            "formatter produced source that does not parse cleanly",
            "the original source was preserved",
        ));
        return FormatOutput {
            text: file.text.clone(),
            diagnostics,
        };
    }

    if token_slices(file, &lex.tokens) != token_slices(&formatted_file, &formatted_lex.tokens) {
        diagnostics.push(format_safety_error(
            file,
            "FMT002",
            "formatter safety check failed",
            "token text would change, so the original source was preserved",
        ));
        return FormatOutput {
            text: file.text.clone(),
            diagnostics,
        };
    }

    if comment_slices(file, &lex.tokens) != comment_slices(&formatted_file, &formatted_lex.tokens) {
        diagnostics.push(format_safety_error(
            file,
            "FMT003",
            "formatter safety check failed",
            "comment text would change, so the original source was preserved",
        ));
        return FormatOutput {
            text: file.text.clone(),
            diagnostics,
        };
    }

    if ast_fingerprint_module(&parsed.module) != ast_fingerprint_module(&formatted_parse.module) {
        diagnostics.push(format_safety_error(
            file,
            "FMT004",
            "formatter safety check failed",
            "the parsed Lux syntax tree would change, so the original source was preserved",
        ));
        return FormatOutput {
            text: file.text.clone(),
            diagnostics,
        };
    }

    FormatOutput { text, diagnostics }
}

fn format_safety_error(
    file: &SourceFile,
    code: &'static str,
    message: &'static str,
    help: &'static str,
) -> Diagnostic {
    Diagnostic::error(message)
        .with_code(code)
        .with_label(Label::primary(
            SourceSpan::new(file.id, 0, file.text.len().min(1)),
            "formatting was not written",
        ))
        .with_help(help)
}

fn format_lossless(file: &SourceFile, tokens: &[Token], module: &Module) -> String {
    let lines = split_lines(&file.text);
    let line_starts = lines.iter().map(|line| line.start).collect::<Vec<_>>();
    let protected_lines = protected_lines(file, &lines, tokens);
    let tokens_by_line = tokens_by_line(tokens, &line_starts, lines.len());
    let guide = FormatGuide::new(module, &line_starts, lines.len());

    let mut out = String::new();
    let mut indent = 0usize;
    let mut brace_stack = Vec::<usize>::new();
    let mut grouping_depth = 0usize;
    let mut pending_continuation = false;

    for (index, line) in lines.iter().enumerate() {
        let raw_line = &file.text[line.start..line.content_end];
        let eol = &file.text[line.content_end..line.end];
        let line_tokens = &tokens_by_line[index];

        if protected_lines[index] {
            out.push_str(raw_line);
            out.push_str(eol);
            update_state(
                &mut indent,
                &mut brace_stack,
                &mut grouping_depth,
                line_tokens,
                leading_indent_units(raw_line),
            );
            pending_continuation = line_requests_continuation(line_tokens);
            continue;
        }

        if raw_line.trim().is_empty() {
            out.push_str(eol);
            continue;
        }

        let continuation_indent = usize::from(
            (grouping_depth > 0 || pending_continuation)
                && !starts_with_closer(line_tokens)
                && !guide.starts_sibling(index),
        );
        let line_indent = indent
            .saturating_sub(leading_close_count(line_tokens))
            .saturating_add(continuation_indent);
        let content_start = first_non_whitespace(raw_line)
            .map(|offset| line.start + offset)
            .unwrap_or(line.content_end);
        let content = file.text[content_start..line.content_end].trim_end();

        out.push_str(&"  ".repeat(line_indent));
        out.push_str(content);
        out.push_str(eol);

        update_state(
            &mut indent,
            &mut brace_stack,
            &mut grouping_depth,
            line_tokens,
            line_indent,
        );
        pending_continuation = line_requests_continuation(line_tokens);
    }

    out
}

#[derive(Debug, Clone)]
struct FormatGuide {
    sibling_lines: Vec<bool>,
}

impl FormatGuide {
    fn new(module: &Module, line_starts: &[usize], line_count: usize) -> Self {
        let mut guide = Self {
            sibling_lines: vec![false; line_count],
        };
        guide.collect_module(module, line_starts);
        guide
    }

    fn starts_sibling(&self, line: usize) -> bool {
        self.sibling_lines.get(line).copied().unwrap_or(false)
    }

    fn mark_sibling(&mut self, span: SourceSpan, line_starts: &[usize]) {
        if self.sibling_lines.is_empty() {
            return;
        }
        let line = line_index_for_offset(line_starts, span.byte_start);
        if let Some(item) = self.sibling_lines.get_mut(line) {
            *item = true;
        }
    }

    fn collect_module(&mut self, module: &Module, line_starts: &[usize]) {
        for stmt in &module.body {
            self.collect_stmt(stmt, line_starts);
        }
    }

    fn collect_block(&mut self, block: &Block, line_starts: &[usize]) {
        for stmt in &block.statements {
            self.collect_stmt(stmt, line_starts);
        }
        if let Some(tail) = &block.tail {
            self.collect_expr(tail, line_starts);
        }
    }

    fn collect_stmt(&mut self, stmt: &Stmt, line_starts: &[usize]) {
        match &stmt.kind {
            StmtKind::LocalDecl { values, .. } => {
                for expr in values {
                    self.collect_expr(expr, line_starts);
                }
            }
            StmtKind::LocalDestructure {
                patterns, values, ..
            } => {
                for pattern in patterns {
                    self.collect_pattern(pattern, line_starts);
                }
                for expr in values {
                    self.collect_expr(expr, line_starts);
                }
            }
            StmtKind::Assign { targets, values } => {
                for expr in targets.iter().chain(values) {
                    self.collect_expr(expr, line_starts);
                }
            }
            StmtKind::CompoundAssign { target, value, .. } => {
                self.collect_expr(target, line_starts);
                self.collect_expr(value, line_starts);
            }
            StmtKind::Expr(expr) => self.collect_expr(expr, line_starts),
            StmtKind::Return(values) => {
                for expr in values {
                    self.collect_expr(expr, line_starts);
                }
            }
            StmtKind::ExportDecl { stmt, .. } | StmtKind::RealmDecl { stmt, .. } => {
                self.collect_stmt(stmt, line_starts);
            }
            StmtKind::RealmBlock { block, .. }
            | StmtKind::InitDecl { block, .. }
            | StmtKind::Do(block) => self.collect_block(block, line_starts),
            StmtKind::FunctionDecl(decl) => self.collect_function_decl(decl, line_starts),
            StmtKind::EnumDecl(decl) => {
                for variant in &decl.variants {
                    self.mark_sibling(variant.name.span, line_starts);
                    if let Some(tag) = &variant.tag {
                        self.collect_expr(tag, line_starts);
                    }
                }
            }
            StmtKind::If {
                condition,
                then_block,
                else_block,
            } => {
                self.collect_expr(condition, line_starts);
                self.collect_block(then_block, line_starts);
                if let Some(block) = else_block {
                    self.collect_block(block, line_starts);
                }
            }
            StmtKind::While { condition, body } => {
                self.collect_expr(condition, line_starts);
                self.collect_block(body, line_starts);
            }
            StmtKind::NumericFor {
                start,
                end,
                step,
                body,
                ..
            } => {
                self.collect_expr(start, line_starts);
                self.collect_expr(end, line_starts);
                if let Some(step) = step {
                    self.collect_expr(step, line_starts);
                }
                self.collect_block(body, line_starts);
            }
            StmtKind::GenericFor { iter, body, .. } => {
                for expr in iter {
                    self.collect_expr(expr, line_starts);
                }
                self.collect_block(body, line_starts);
            }
            StmtKind::RepeatUntil { body, condition } => {
                self.collect_block(body, line_starts);
                self.collect_expr(condition, line_starts);
            }
            StmtKind::Break
            | StmtKind::Continue
            | StmtKind::Import(_)
            | StmtKind::PartOrderDecl(_)
            | StmtKind::ExternDecl(_)
            | StmtKind::HostPackageDecl(_)
            | StmtKind::ExportList { .. }
            | StmtKind::ExportAll { .. } => {}
        }
    }

    fn collect_function_decl(&mut self, decl: &FunctionDecl, line_starts: &[usize]) {
        for param in &decl.params {
            if let Some(default) = &param.default {
                self.collect_expr(default, line_starts);
            }
        }
        self.collect_function_body(&decl.body, line_starts);
    }

    fn collect_function_body(&mut self, body: &FunctionBody, line_starts: &[usize]) {
        match body {
            FunctionBody::Expr(expr) => self.collect_expr(expr, line_starts),
            FunctionBody::Block(block) => self.collect_block(block, line_starts),
        }
    }

    fn collect_pattern(&mut self, pattern: &Pattern, line_starts: &[usize]) {
        match &pattern.kind {
            PatternKind::Identifier(_) => {}
            PatternKind::Object(fields) => {
                for field in fields {
                    self.mark_sibling(field.span, line_starts);
                    self.collect_pattern(&field.pattern, line_starts);
                    if let Some(default) = &field.default {
                        self.collect_expr(default, line_starts);
                    }
                }
            }
            PatternKind::Array(items) => {
                for item in items {
                    self.mark_sibling(item.span, line_starts);
                    self.collect_pattern(&item.pattern, line_starts);
                    if let Some(default) = &item.default {
                        self.collect_expr(default, line_starts);
                    }
                }
            }
        }
    }

    fn collect_expr(&mut self, expr: &Expr, line_starts: &[usize]) {
        match &expr.kind {
            ExprKind::Table(table) => {
                for field in &table.fields {
                    self.mark_sibling(field.span, line_starts);
                    match &field.kind {
                        TableFieldKind::Array(expr) | TableFieldKind::Spread(expr) => {
                            self.collect_expr(expr, line_starts);
                        }
                        TableFieldKind::Named { value, .. } => {
                            self.collect_expr(value, line_starts);
                        }
                        TableFieldKind::ExprKey { key, value } => {
                            self.collect_expr(key, line_starts);
                            self.collect_expr(value, line_starts);
                        }
                    }
                }
            }
            ExprKind::Paren(inner) => self.collect_expr(inner, line_starts),
            ExprKind::Unary { argument, .. } => self.collect_expr(argument, line_starts),
            ExprKind::Binary { left, right, .. } => {
                self.collect_expr(left, line_starts);
                self.collect_expr(right, line_starts);
            }
            ExprKind::Conditional {
                condition,
                then_branch,
                else_branch,
                ..
            } => {
                self.collect_expr(condition, line_starts);
                self.collect_expr_or_block(then_branch, line_starts);
                self.collect_expr_or_block(else_branch, line_starts);
            }
            ExprKind::Match(match_expr) => {
                self.collect_expr(&match_expr.subject, line_starts);
                for arm in &match_expr.arms {
                    self.mark_sibling(arm.pattern.span, line_starts);
                    self.collect_match_pattern(&arm.pattern, line_starts);
                    self.collect_expr_or_block(&arm.body, line_starts);
                }
            }
            ExprKind::Do(block) => self.collect_block(block, line_starts),
            ExprKind::Function(function) => {
                for param in &function.params {
                    if let Some(default) = &param.default {
                        self.collect_expr(default, line_starts);
                    }
                }
                self.collect_function_body(&function.body, line_starts);
            }
            ExprKind::Chain(chain) => {
                self.collect_expr(&chain.base, line_starts);
                for segment in &chain.segments {
                    match &segment.kind {
                        ChainSegmentKind::Index { index, .. } => {
                            self.collect_expr(index, line_starts);
                        }
                        ChainSegmentKind::Call { args, .. }
                        | ChainSegmentKind::SafeDotCall { args, .. }
                        | ChainSegmentKind::MethodCall { args, .. } => {
                            for arg in args {
                                self.collect_expr(arg, line_starts);
                            }
                        }
                        ChainSegmentKind::Member { .. } => {}
                    }
                }
            }
            ExprKind::TemplateString(parts) => {
                for part in parts {
                    if let TemplatePartKind::Expr(expr) = &part.kind {
                        self.collect_expr(expr, line_starts);
                    }
                }
            }
            ExprKind::Identifier(_)
            | ExprKind::Nil
            | ExprKind::Boolean(_)
            | ExprKind::Number(_)
            | ExprKind::String(_)
            | ExprKind::Vararg
            | ExprKind::PipelinePlaceholder => {}
        }
    }

    fn collect_expr_or_block(&mut self, item: &ExprOrBlock, line_starts: &[usize]) {
        match item {
            ExprOrBlock::Expr(expr) => self.collect_expr(expr, line_starts),
            ExprOrBlock::Block(block) => self.collect_block(block, line_starts),
        }
    }

    fn collect_match_pattern(&mut self, pattern: &MatchPattern, line_starts: &[usize]) {
        match &pattern.kind {
            MatchPatternKind::Or(patterns) => {
                for pattern in patterns {
                    self.collect_match_pattern(pattern, line_starts);
                }
            }
            MatchPatternKind::Variant { payload, .. } => {
                if let Some(payload) = payload {
                    self.collect_match_payload(payload, line_starts);
                }
            }
            MatchPatternKind::Object(fields) => {
                for field in fields {
                    self.mark_sibling(field.span, line_starts);
                    self.collect_match_pattern(&field.pattern, line_starts);
                }
            }
            MatchPatternKind::Array(items) => {
                for item in items {
                    self.mark_sibling(item.span, line_starts);
                    self.collect_match_pattern(&item.pattern, line_starts);
                }
            }
            MatchPatternKind::Wildcard
            | MatchPatternKind::Binding(_)
            | MatchPatternKind::Literal(_) => {}
        }
    }

    fn collect_match_payload(&mut self, payload: &MatchPatternPayload, line_starts: &[usize]) {
        match payload {
            MatchPatternPayload::Tuple(patterns) => {
                for pattern in patterns {
                    self.collect_match_pattern(pattern, line_starts);
                }
            }
            MatchPatternPayload::Record(fields) => {
                for field in fields {
                    self.mark_sibling(field.span, line_starts);
                    self.collect_match_pattern(&field.pattern, line_starts);
                }
            }
        }
    }
}

fn split_lines(text: &str) -> Vec<Line> {
    let mut lines = Vec::new();
    let mut start = 0usize;

    for (index, byte) in text.bytes().enumerate() {
        if byte == b'\n' {
            let content_end = if index > start && text.as_bytes()[index - 1] == b'\r' {
                index - 1
            } else {
                index
            };
            lines.push(Line {
                start,
                content_end,
                end: index + 1,
            });
            start = index + 1;
        }
    }

    if start < text.len() {
        lines.push(Line {
            start,
            content_end: text.len(),
            end: text.len(),
        });
    }

    lines
}

fn tokens_by_line(
    tokens: &[Token],
    line_starts: &[usize],
    line_count: usize,
) -> Vec<Vec<TokenKind>> {
    let mut out = vec![Vec::new(); line_count];
    for token in tokens {
        if token.kind == TokenKind::Eof {
            continue;
        }
        let line = line_index_for_offset(line_starts, token.span.byte_start);
        if let Some(items) = out.get_mut(line) {
            items.push(token.kind.clone());
        }
    }
    out
}

fn protected_lines(file: &SourceFile, lines: &[Line], tokens: &[Token]) -> Vec<bool> {
    let line_starts = lines.iter().map(|line| line.start).collect::<Vec<_>>();
    let mut protected = vec![false; lines.len()];

    for token in tokens {
        if matches!(token.kind, TokenKind::TemplateStringText(_))
            && file.slice(token.span).contains('\n')
        {
            protect_range(
                &mut protected,
                &line_starts,
                token.span.byte_start,
                token.span.byte_end,
            );
        }
    }

    for (start, end) in block_comment_ranges(&file.text) {
        protect_range(&mut protected, &line_starts, start, end);
    }

    protected
}

fn block_comment_ranges(text: &str) -> Vec<(usize, usize)> {
    let mut ranges = Vec::new();
    let mut index = 0usize;

    while index < text.len() {
        if text[index..].starts_with("--[[") {
            let start = index;
            index += 4;
            while index < text.len() && !text[index..].starts_with("]]") {
                index += next_char_len(text, index);
            }
            if index < text.len() {
                index += 2;
            }
            ranges.push((start, index));
            continue;
        }
        index += next_char_len(text, index);
    }

    ranges
}

fn protect_range(protected: &mut [bool], line_starts: &[usize], start: usize, end: usize) {
    if protected.is_empty() {
        return;
    }
    let first = line_index_for_offset(line_starts, start);
    let last = line_index_for_offset(line_starts, end.saturating_sub(1).max(start));
    for index in first..=last {
        if let Some(item) = protected.get_mut(index) {
            *item = true;
        }
    }
}

fn line_index_for_offset(line_starts: &[usize], offset: usize) -> usize {
    match line_starts.binary_search(&offset) {
        Ok(index) => index,
        Err(next) => next.saturating_sub(1),
    }
}

fn first_non_whitespace(line: &str) -> Option<usize> {
    line.char_indices()
        .find(|(_, ch)| !matches!(ch, ' ' | '\t'))
        .map(|(index, _)| index)
}

fn leading_indent_units(line: &str) -> usize {
    let columns = line
        .chars()
        .take_while(|ch| matches!(ch, ' ' | '\t'))
        .map(|ch| if ch == '\t' { 2 } else { 1 })
        .sum::<usize>();
    columns / 2
}

fn leading_close_count(tokens: &[TokenKind]) -> usize {
    tokens
        .iter()
        .take_while(|kind| matches!(kind, TokenKind::RBrace))
        .count()
}

fn starts_with_closer(tokens: &[TokenKind]) -> bool {
    matches!(
        tokens.first(),
        Some(TokenKind::RBrace | TokenKind::RParen | TokenKind::RBracket)
    )
}

fn update_state(
    indent: &mut usize,
    brace_stack: &mut Vec<usize>,
    grouping_depth: &mut usize,
    tokens: &[TokenKind],
    line_indent: usize,
) {
    for token in tokens {
        match token {
            TokenKind::LBrace => {
                brace_stack.push(line_indent);
                *indent = line_indent + 1;
            }
            TokenKind::RBrace => {
                brace_stack.pop();
                *indent = brace_stack.last().map(|base| base + 1).unwrap_or(0);
            }
            TokenKind::LParen | TokenKind::LBracket => *grouping_depth += 1,
            TokenKind::RParen | TokenKind::RBracket => {
                *grouping_depth = grouping_depth.saturating_sub(1)
            }
            _ => {}
        }
    }
}

fn line_requests_continuation(tokens: &[TokenKind]) -> bool {
    matches!(
        tokens.last(),
        Some(
            TokenKind::Eq
                | TokenKind::ArrowNormal
                | TokenKind::ArrowImplicitSelf
                | TokenKind::Plus
                | TokenKind::Minus
                | TokenKind::Star
                | TokenKind::Slash
                | TokenKind::Percent
                | TokenKind::Caret
                | TokenKind::DotDot
                | TokenKind::QuestionQuestion
                | TokenKind::PipeGt
                | TokenKind::Comma
                | TokenKind::KwAnd
                | TokenKind::KwOr
                | TokenKind::KwThen
                | TokenKind::KwElse
        )
    )
}

fn next_char_len(text: &str, index: usize) -> usize {
    text[index..]
        .chars()
        .next()
        .map(char::len_utf8)
        .unwrap_or(1)
}

fn token_slices(file: &SourceFile, tokens: &[Token]) -> Vec<String> {
    tokens
        .iter()
        .filter(|token| token.kind != TokenKind::Eof)
        .map(|token| file.slice(token.span).to_string())
        .collect()
}

fn comment_slices(file: &SourceFile, tokens: &[Token]) -> Vec<String> {
    let mut comments = Vec::new();
    let mut cursor = 0usize;

    for token in tokens.iter().filter(|token| token.kind != TokenKind::Eof) {
        collect_comments_in_gap(&file.text, cursor, token.span.byte_start, &mut comments);
        cursor = token.span.byte_end;
    }
    collect_comments_in_gap(&file.text, cursor, file.text.len(), &mut comments);

    comments
}

fn collect_comments_in_gap(text: &str, start: usize, end: usize, comments: &mut Vec<String>) {
    let mut index = start;
    while index < end {
        if text[index..end].starts_with("--[[") {
            let comment_start = index;
            index += 4;
            while index < end && !text[index..end].starts_with("]]") {
                index += next_char_len(text, index);
            }
            if index < end {
                index += 2;
            }
            comments.push(text[comment_start..index].to_string());
            continue;
        }

        if text[index..end].starts_with("--") {
            let comment_start = index;
            index += 2;
            while index < end {
                let Some(ch) = text[index..end].chars().next() else {
                    break;
                };
                if matches!(ch, '\r' | '\n') {
                    break;
                }
                index += ch.len_utf8();
            }
            comments.push(text[comment_start..index].to_string());
            continue;
        }

        index += next_char_len(text, index);
    }
}

fn ast_fingerprint_module(module: &Module) -> String {
    let mut out = String::new();
    out.push_str("module[");
    for stmt in &module.body {
        fp_stmt(&mut out, stmt);
    }
    out.push(']');
    out
}

fn fp_block(out: &mut String, block: &Block) {
    out.push_str("block[");
    for stmt in &block.statements {
        fp_stmt(out, stmt);
    }
    out.push_str("tail=");
    fp_opt_expr(out, block.tail.as_ref());
    out.push(']');
}

fn fp_stmt(out: &mut String, stmt: &Stmt) {
    match &stmt.kind {
        StmtKind::LocalDecl {
            mode,
            names,
            values,
        } => {
            fp_binding_mode(out, *mode);
            out.push('(');
            fp_ident_list(out, names);
            out.push_str(")=");
            fp_expr_list(out, values);
        }
        StmtKind::LocalDestructure {
            mode,
            patterns,
            values,
        } => {
            fp_binding_mode(out, *mode);
            out.push_str("_destructure(");
            fp_pattern_list(out, patterns);
            out.push_str(")=");
            fp_expr_list(out, values);
        }
        StmtKind::Assign { targets, values } => {
            out.push_str("assign(");
            fp_expr_list(out, targets);
            out.push_str(")=");
            fp_expr_list(out, values);
        }
        StmtKind::CompoundAssign { target, op, value } => {
            out.push_str("compound(");
            fp_compound_op(out, *op);
            out.push(',');
            fp_expr(out, target);
            out.push(',');
            fp_expr(out, value);
            out.push(')');
        }
        StmtKind::Expr(expr) => {
            out.push_str("expr(");
            fp_expr(out, expr);
            out.push(')');
        }
        StmtKind::Return(values) => {
            out.push_str("return");
            fp_expr_list(out, values);
        }
        StmtKind::Break => out.push_str("break"),
        StmtKind::Continue => out.push_str("continue"),
        StmtKind::Import(import) => fp_import(out, import),
        StmtKind::PartOrderDecl(decl) => {
            out.push_str("part_order(");
            match &decl.kind {
                crate::ast::PartOrderKind::Relative { relation, target } => {
                    match relation {
                        crate::ast::PartOrderRelation::Before => out.push_str("before"),
                        crate::ast::PartOrderRelation::After => out.push_str("after"),
                    }
                    out.push(',');
                    fp_quoted(out, "target", target);
                }
                crate::ast::PartOrderKind::Order { targets } => {
                    out.push_str("order");
                    for target in targets {
                        out.push(',');
                        fp_quoted(out, "target", target);
                    }
                }
            }
            out.push(')');
        }
        StmtKind::ExternDecl(decl) => {
            out.push_str("extern(");
            out.push_str(decl.realm.as_str());
            out.push(',');
            fp_ident_list(out, &decl.path);
            out.push(')');
        }
        StmtKind::HostPackageDecl(decl) => {
            out.push_str("host_package(");
            fp_quoted(out, "target", &decl.target);
            out.push(',');
            fp_quoted(out, "runtime", &decl.runtime);
            out.push(')');
        }
        StmtKind::ExportDecl { kind, realm, stmt } => {
            out.push_str("export_decl(");
            fp_export_kind(out, *kind);
            out.push(',');
            if let Some(realm) = realm {
                out.push_str(realm.as_str());
            } else {
                out.push_str("default");
            }
            out.push(',');
            fp_stmt(out, stmt);
            out.push(')');
        }
        StmtKind::ExportList { realm, entries } => {
            out.push_str("export_list(");
            if let Some(realm) = realm {
                out.push_str(realm.as_str());
            } else {
                out.push_str("default");
            }
            out.push(',');
            for (index, entry) in entries.iter().enumerate() {
                if index > 0 {
                    out.push(',');
                }
                out.push_str(&entry.exported.name);
                out.push('=');
                out.push_str(&entry.local.name);
            }
            out.push(')');
        }
        StmtKind::ExportAll { realm } => {
            out.push_str("export_all(");
            if let Some(realm) = realm {
                out.push_str(realm.as_str());
            } else {
                out.push_str("default");
            }
            out.push(')');
        }
        StmtKind::RealmDecl { realm, stmt } => {
            out.push_str("realm_decl(");
            out.push_str(realm.as_str());
            out.push(',');
            fp_stmt(out, stmt);
            out.push(')');
        }
        StmtKind::RealmBlock { realm, block } => {
            out.push_str("realm_block(");
            out.push_str(realm.as_str());
            out.push(',');
            fp_block(out, block);
            out.push(')');
        }
        StmtKind::InitDecl { realm, block } => {
            out.push_str("init(");
            if let Some(realm) = realm {
                out.push_str(realm.as_str());
            } else {
                out.push_str("default");
            }
            out.push(',');
            fp_block(out, block);
            out.push(')');
        }
        StmtKind::FunctionDecl(decl) => fp_function_decl(out, decl),
        StmtKind::EnumDecl(decl) => fp_enum_decl(out, decl),
        StmtKind::If {
            condition,
            then_block,
            else_block,
        } => {
            out.push_str("if(");
            fp_expr(out, condition);
            out.push(',');
            fp_block(out, then_block);
            out.push(',');
            if let Some(block) = else_block {
                fp_block(out, block);
            } else {
                out.push_str("none");
            }
            out.push(')');
        }
        StmtKind::While { condition, body } => {
            out.push_str("while(");
            fp_expr(out, condition);
            out.push(',');
            fp_block(out, body);
            out.push(')');
        }
        StmtKind::NumericFor {
            name,
            start,
            end,
            step,
            body,
        } => {
            out.push_str("fornum(");
            fp_ident(out, name);
            out.push(',');
            fp_expr(out, start);
            out.push(',');
            fp_expr(out, end);
            out.push(',');
            fp_opt_expr(out, step.as_ref());
            out.push(',');
            fp_block(out, body);
            out.push(')');
        }
        StmtKind::GenericFor { names, iter, body } => {
            out.push_str("forin(");
            fp_ident_list(out, names);
            out.push(',');
            fp_expr_list(out, iter);
            out.push(',');
            fp_block(out, body);
            out.push(')');
        }
        StmtKind::RepeatUntil { body, condition } => {
            out.push_str("repeat(");
            fp_block(out, body);
            out.push(',');
            fp_expr(out, condition);
            out.push(')');
        }
        StmtKind::Do(block) => {
            out.push_str("do(");
            fp_block(out, block);
            out.push(')');
        }
    }
    out.push(';');
}

fn fp_expr(out: &mut String, expr: &Expr) {
    match &expr.kind {
        ExprKind::Identifier(identifier) => fp_ident(out, identifier),
        ExprKind::Nil => out.push_str("nil"),
        ExprKind::Boolean(value) => {
            let _ = write!(out, "bool({value})");
        }
        ExprKind::Number(value) => fp_quoted(out, "num", value),
        ExprKind::String(value) => fp_quoted(out, "str", value),
        ExprKind::Vararg => out.push_str("vararg"),
        ExprKind::PipelinePlaceholder => out.push_str("pipe_placeholder"),
        ExprKind::TemplateString(parts) => fp_template_parts(out, parts),
        ExprKind::Table(table) => fp_table(out, table),
        ExprKind::Paren(expr) => {
            out.push_str("paren(");
            fp_expr(out, expr);
            out.push(')');
        }
        ExprKind::Unary { op, argument } => {
            out.push_str("unary(");
            fp_unary_op(out, *op);
            out.push(',');
            fp_expr(out, argument);
            out.push(')');
        }
        ExprKind::Binary { op, left, right } => {
            out.push_str("binary(");
            fp_binary_op(out, *op);
            out.push(',');
            fp_expr(out, left);
            out.push(',');
            fp_expr(out, right);
            out.push(')');
        }
        ExprKind::Conditional {
            condition,
            then_branch,
            else_branch,
            form,
        } => {
            out.push_str("cond(");
            fp_conditional_form(out, *form);
            out.push(',');
            fp_expr(out, condition);
            out.push(',');
            fp_expr_or_block(out, then_branch);
            out.push(',');
            fp_expr_or_block(out, else_branch);
            out.push(')');
        }
        ExprKind::Match(match_expr) => {
            out.push_str("match(");
            fp_expr(out, &match_expr.subject);
            for arm in &match_expr.arms {
                out.push(',');
                fp_match_pattern(out, &arm.pattern);
                out.push_str("=>");
                fp_expr_or_block(out, &arm.body);
            }
            out.push(')');
        }
        ExprKind::Do(block) => {
            out.push_str("do_expr(");
            fp_block(out, block);
            out.push(')');
        }
        ExprKind::Function(function) => fp_function_expr(out, function),
        ExprKind::Chain(chain) => fp_chain(out, chain),
    }
}

fn fp_expr_or_block(out: &mut String, item: &ExprOrBlock) {
    match item {
        ExprOrBlock::Expr(expr) => {
            out.push_str("expr:");
            fp_expr(out, expr);
        }
        ExprOrBlock::Block(block) => {
            out.push_str("block:");
            fp_block(out, block);
        }
    }
}

fn fp_opt_expr(out: &mut String, expr: Option<&Expr>) {
    if let Some(expr) = expr {
        fp_expr(out, expr);
    } else {
        out.push_str("none");
    }
}

fn fp_expr_list(out: &mut String, exprs: &[Expr]) {
    out.push('[');
    for expr in exprs {
        fp_expr(out, expr);
        out.push(',');
    }
    out.push(']');
}

fn fp_ident(out: &mut String, ident: &Identifier) {
    fp_quoted(out, "id", &ident.name);
}

fn fp_ident_list(out: &mut String, names: &[Identifier]) {
    out.push('[');
    for name in names {
        fp_ident(out, name);
        out.push(',');
    }
    out.push(']');
}

fn fp_param(out: &mut String, param: &Param) {
    out.push_str("param(");
    fp_ident(out, &param.name);
    out.push_str(",default=");
    fp_opt_expr(out, param.default.as_ref());
    out.push(')');
}

fn fp_param_list(out: &mut String, params: &[Param]) {
    out.push('[');
    for param in params {
        fp_param(out, param);
        out.push(',');
    }
    out.push(']');
}

fn fp_pattern(out: &mut String, pattern: &Pattern) {
    match &pattern.kind {
        PatternKind::Identifier(name) => {
            out.push_str("pat_id(");
            fp_ident(out, name);
            out.push(')');
        }
        PatternKind::Object(fields) => {
            out.push_str("pat_object[");
            for field in fields {
                out.push_str("field(");
                fp_ident(out, &field.key);
                out.push(',');
                fp_pattern(out, &field.pattern);
                out.push_str(",default=");
                fp_opt_expr(out, field.default.as_ref());
                out.push(')');
            }
            out.push(']');
        }
        PatternKind::Array(items) => {
            out.push_str("pat_array[");
            for item in items {
                out.push_str("item(");
                fp_pattern(out, &item.pattern);
                out.push_str(",default=");
                fp_opt_expr(out, item.default.as_ref());
                out.push(')');
            }
            out.push(']');
        }
    }
}

fn fp_pattern_list(out: &mut String, patterns: &[Pattern]) {
    out.push('[');
    for pattern in patterns {
        fp_pattern(out, pattern);
        out.push(',');
    }
    out.push(']');
}

fn fp_import(out: &mut String, import: &ImportStmt) {
    out.push_str("import(");
    fp_quoted(out, "source", &import.source);
    let _ = write!(
        out,
        ",side_effect={},phase={:?},specs=[",
        import.side_effect_only, import.phase
    );
    for specifier in &import.specifiers {
        fp_import_specifier(out, specifier);
        out.push(',');
    }
    out.push_str("])");
}

fn fp_import_specifier(out: &mut String, specifier: &ImportSpecifier) {
    match specifier {
        ImportSpecifier::Named { imported, local } => {
            out.push_str("spec_named(");
            fp_ident(out, imported);
            out.push(',');
            fp_ident(out, local);
            out.push(')');
        }
        ImportSpecifier::Namespace { local } => {
            out.push_str("spec_namespace(");
            fp_ident(out, local);
            out.push(')');
        }
    }
}

fn fp_export_kind(out: &mut String, kind: ExportKind) {
    out.push_str(match kind {
        ExportKind::Runtime => "runtime",
        ExportKind::Macro => "macro",
        ExportKind::HostExpr => "host_expr",
    });
}

fn fp_enum_decl(out: &mut String, decl: &EnumDecl) {
    out.push_str("enum(");
    fp_ident(out, &decl.name);
    out.push(',');
    fp_enum_repr(out, &decl.repr);
    let _ = write!(out, ",runtime={},variants=[", decl.runtime);
    for variant in &decl.variants {
        fp_ident(out, &variant.name);
        out.push(':');
        match &variant.payload {
            EnumVariantPayload::None => out.push_str("none"),
            EnumVariantPayload::Tuple(fields) => {
                out.push_str("tuple");
                fp_ident_list(out, fields);
            }
            EnumVariantPayload::Record(fields) => {
                out.push_str("record");
                fp_ident_list(out, fields);
            }
        }
        out.push('=');
        fp_opt_expr(out, variant.tag.as_ref());
        out.push(',');
    }
    out.push_str("])");
}

fn fp_enum_repr(out: &mut String, repr: &EnumRepr) {
    match repr {
        EnumRepr::String => out.push_str("repr_string"),
        EnumRepr::Number => out.push_str("repr_number"),
        EnumRepr::Table { tag_field } => fp_quoted(out, "repr_table", tag_field),
        EnumRepr::Existing { tag_field } => fp_quoted(out, "repr_existing", tag_field),
    }
}

fn fp_match_pattern(out: &mut String, pattern: &MatchPattern) {
    match &pattern.kind {
        MatchPatternKind::Or(patterns) => {
            out.push_str("or(");
            for pattern in patterns {
                fp_match_pattern(out, pattern);
                out.push(',');
            }
            out.push(')');
        }
        MatchPatternKind::Wildcard => out.push('_'),
        MatchPatternKind::Binding(name) => {
            out.push_str("bind(");
            fp_ident(out, name);
            out.push(')');
        }
        MatchPatternKind::Literal(literal) => match literal {
            MatchLiteral::Nil => out.push_str("lit_nil"),
            MatchLiteral::Boolean(value) => {
                let _ = write!(out, "lit_bool({value})");
            }
            MatchLiteral::Number(value) => fp_quoted(out, "lit_num", value),
            MatchLiteral::String(value) => fp_quoted(out, "lit_str", value),
        },
        MatchPatternKind::Variant { path, payload } => {
            out.push_str("variant(");
            fp_ident_list(out, path);
            out.push(',');
            if let Some(payload) = payload {
                fp_match_payload(out, payload);
            } else {
                out.push_str("none");
            }
            out.push(')');
        }
        MatchPatternKind::Object(fields) => {
            out.push_str("object(");
            for field in fields {
                fp_ident(out, &field.key);
                out.push(':');
                fp_match_pattern(out, &field.pattern);
                out.push(',');
            }
            out.push(')');
        }
        MatchPatternKind::Array(items) => {
            out.push_str("array(");
            for item in items {
                fp_match_pattern(out, &item.pattern);
                out.push(',');
            }
            out.push(')');
        }
    }
}

fn fp_match_payload(out: &mut String, payload: &MatchPatternPayload) {
    match payload {
        MatchPatternPayload::Tuple(patterns) => {
            out.push_str("tuple(");
            for pattern in patterns {
                fp_match_pattern(out, pattern);
                out.push(',');
            }
            out.push(')');
        }
        MatchPatternPayload::Record(fields) => {
            out.push_str("record(");
            for field in fields {
                fp_ident(out, &field.key);
                out.push(':');
                fp_match_pattern(out, &field.pattern);
                out.push(',');
            }
            out.push(')');
        }
    }
}

fn fp_function_decl(out: &mut String, decl: &FunctionDecl) {
    out.push_str("fn_decl(");
    fp_function_name(out, &decl.name);
    out.push(',');
    fp_param_list(out, &decl.params);
    let _ = write!(out, ",vararg={},", decl.vararg);
    fp_function_body(out, &decl.body);
    out.push(')');
}

fn fp_function_name(out: &mut String, name: &FunctionName) {
    match name {
        FunctionName::Simple(name) => {
            out.push_str("simple:");
            fp_ident(out, name);
        }
        FunctionName::Dotted(path) => {
            out.push_str("dotted:");
            fp_ident_list(out, path);
        }
        FunctionName::Method { receiver, method } => {
            out.push_str("method:");
            fp_ident_list(out, receiver);
            out.push(':');
            fp_ident(out, method);
        }
    }
}

fn fp_function_expr(out: &mut String, function: &FunctionExpr) {
    out.push_str("fn_expr(");
    fp_param_list(out, &function.params);
    let _ = write!(
        out,
        ",vararg={},arrow={:?},",
        function.vararg, function.arrow_kind
    );
    fp_function_body(out, &function.body);
    out.push(')');
}

fn fp_function_body(out: &mut String, body: &FunctionBody) {
    match body {
        FunctionBody::Expr(expr) => {
            out.push_str("body_expr(");
            fp_expr(out, expr);
            out.push(')');
        }
        FunctionBody::Block(block) => {
            out.push_str("body_block(");
            fp_block(out, block);
            out.push(')');
        }
    }
}

fn fp_template_parts(out: &mut String, parts: &[TemplatePart]) {
    out.push_str("template[");
    for part in parts {
        match &part.kind {
            TemplatePartKind::Text(text) => fp_quoted(out, "text", text),
            TemplatePartKind::Expr(expr) => {
                out.push_str("interp(");
                fp_expr(out, expr);
                out.push(')');
            }
        }
        out.push(',');
    }
    out.push(']');
}

fn fp_table(out: &mut String, table: &TableExpr) {
    out.push_str("table[");
    for field in &table.fields {
        match &field.kind {
            TableFieldKind::Array(expr) => {
                out.push_str("array(");
                fp_expr(out, expr);
                out.push(')');
            }
            TableFieldKind::Named { name, value } => {
                out.push_str("named(");
                fp_ident(out, name);
                out.push(',');
                fp_expr(out, value);
                out.push(')');
            }
            TableFieldKind::ExprKey { key, value } => {
                out.push_str("key(");
                fp_expr(out, key);
                out.push(',');
                fp_expr(out, value);
                out.push(')');
            }
            TableFieldKind::Spread(expr) => {
                out.push_str("spread(");
                fp_expr(out, expr);
                out.push(')');
            }
        }
        out.push(',');
    }
    out.push(']');
}

fn fp_chain(out: &mut String, chain: &ChainExpr) {
    out.push_str("chain(");
    fp_expr(out, &chain.base);
    out.push_str(",segments=[");
    for segment in &chain.segments {
        match &segment.kind {
            ChainSegmentKind::Member { name, optional } => {
                let _ = write!(out, "member(optional={optional},");
                fp_ident(out, name);
                out.push(')');
            }
            ChainSegmentKind::Index { index, optional } => {
                let _ = write!(out, "index(optional={optional},");
                fp_expr(out, index);
                out.push(')');
            }
            ChainSegmentKind::Call { args, style } => {
                out.push_str("call(");
                fp_call_style(out, *style);
                out.push(',');
                fp_expr_list(out, args);
                out.push(')');
            }
            ChainSegmentKind::SafeDotCall { name, args, style } => {
                out.push_str("safe_dot_call(");
                fp_ident(out, name);
                out.push(',');
                fp_call_style(out, *style);
                out.push(',');
                fp_expr_list(out, args);
                out.push(')');
            }
            ChainSegmentKind::MethodCall {
                name,
                args,
                optional,
                style,
            } => {
                let _ = write!(out, "method_call(optional={optional},");
                fp_ident(out, name);
                out.push(',');
                fp_call_style(out, *style);
                out.push(',');
                fp_expr_list(out, args);
                out.push(')');
            }
        }
        out.push(',');
    }
    out.push_str("])");
}

fn fp_unary_op(out: &mut String, op: UnaryOp) {
    let _ = write!(out, "{op:?}");
}

fn fp_binary_op(out: &mut String, op: BinaryOp) {
    let _ = write!(out, "{op:?}");
}

fn fp_compound_op(out: &mut String, op: CompoundAssignOp) {
    let _ = write!(out, "{op:?}");
}

fn fp_binding_mode(out: &mut String, mode: BindingMode) {
    match mode {
        BindingMode::Local => out.push_str("local"),
        BindingMode::Const => out.push_str("const"),
    }
}

fn fp_conditional_form(out: &mut String, form: ConditionalForm) {
    let _ = write!(out, "{form:?}");
}

fn fp_call_style(out: &mut String, style: CallStyle) {
    let _ = write!(out, "{style:?}");
}

fn fp_quoted(out: &mut String, tag: &str, value: &str) {
    let _ = write!(out, "{tag}({}:{});", value.len(), value);
}

#[cfg(test)]
mod tests {
    use crate::source::SourceFile;

    use super::format_source;

    #[test]
    fn formats_indentation_without_rebuilding_tokens() {
        let file = SourceFile::new(0, None, "fn demo(){\nlocal s = 'x'  \n}\n");
        let output = format_source(&file);
        assert!(output.diagnostics.is_empty(), "{:#?}", output.diagnostics);
        assert_eq!(output.text, "fn demo(){\n  local s = 'x'\n}\n");
    }

    #[test]
    fn preserves_comments_and_template_text() {
        let input = "fn demo(){\n-- comment\nlocal s = `  ${x}`\n--[[\n  keep me\n]]\n}\n";
        let file = SourceFile::new(0, None, input);
        let output = format_source(&file);
        assert!(output.diagnostics.is_empty(), "{:#?}", output.diagnostics);
        assert!(output.text.contains("-- comment"));
        assert!(output.text.contains("`  ${x}`"));
        assert!(output.text.contains("--[[\n  keep me\n]]"));
    }

    #[test]
    fn preserves_newline_tail_table_call_shape() {
        let input = "fn demo(){\nvalues(...)\n{ x = 1 }\n}\n";
        let file = SourceFile::new(0, None, input);
        let output = format_source(&file);
        assert!(output.diagnostics.is_empty(), "{:#?}", output.diagnostics);
        assert!(output.text.contains("values(...)\n  { x = 1 }"));
    }

    #[test]
    fn format_is_idempotent() {
        let file = SourceFile::new(0, None, "fn demo(){\nlocal x = 1  \n}\n");
        let once = format_source(&file);
        let twice_file = SourceFile::new(0, None, once.text.clone());
        let twice = format_source(&twice_file);
        assert_eq!(once.text, twice.text);
    }

    #[test]
    fn indents_expression_body_continuation() {
        let file = SourceFile::new(0, None, "fn demo() =\nvalue\n");
        let output = format_source(&file);
        assert!(output.diagnostics.is_empty(), "{:#?}", output.diagnostics);
        assert_eq!(output.text, "fn demo() =\n  value\n");
    }

    #[test]
    fn indents_grouped_continuation_without_changing_lines() {
        let file = SourceFile::new(0, None, "fn demo(){\ncall(\na,\nb\n)\n}\n");
        let output = format_source(&file);
        assert!(output.diagnostics.is_empty(), "{:#?}", output.diagnostics);
        assert_eq!(output.text, "fn demo(){\n  call(\n    a,\n    b\n  )\n}\n");
    }

    #[test]
    fn formats_modern_syntax_without_ast_drift() {
        let input = "fn demo(base, xs, fallback = 0){\nconst { name, hp = fallback } = base\nconst next = do {\nlocal current = base.count ?? 0\ncurrent + 1\n}\n{ ...base, name = name, hp = hp, next = xs |> arr.map(%, (x) => x + 1) }\n}\n";
        let file = SourceFile::new(0, None, input);
        let output = format_source(&file);
        assert!(output.diagnostics.is_empty(), "{:#?}", output.diagnostics);
        assert_eq!(
            output.text,
            "fn demo(base, xs, fallback = 0){\n  const { name, hp = fallback } = base\n  const next = do {\n    local current = base.count ?? 0\n    current + 1\n  }\n  { ...base, name = name, hp = hp, next = xs |> arr.map(%, (x) => x + 1) }\n}\n"
        );
    }

    #[test]
    fn formats_enum_variants_as_siblings() {
        let input = "enum PlayerTier repr number {\nGuest = 0,\nRegular = 1,\nVeteran = 2\n}\n";
        let file = SourceFile::new(0, None, input);
        let output = format_source(&file);
        assert!(output.diagnostics.is_empty(), "{:#?}", output.diagnostics);
        assert_eq!(
            output.text,
            "enum PlayerTier repr number {\n  Guest = 0,\n  Regular = 1,\n  Veteran = 2\n}\n"
        );
    }

    #[test]
    fn formats_match_arms_inside_expression_body() {
        let input = "fn tierLabel(tier) =\nmatch tier {\nPlayerTier.Guest => \"guest\"\nPlayerTier.Regular => \"regular\"\nPlayerTier.Veteran => \"veteran\"\n}\n";
        let file = SourceFile::new(0, None, input);
        let output = format_source(&file);
        assert!(output.diagnostics.is_empty(), "{:#?}", output.diagnostics);
        assert_eq!(
            output.text,
            "fn tierLabel(tier) =\n  match tier {\n    PlayerTier.Guest => \"guest\"\n    PlayerTier.Regular => \"regular\"\n    PlayerTier.Veteran => \"veteran\"\n  }\n"
        );
    }

    #[test]
    fn formatter_rejects_invalid_const_without_rewrite() {
        let input = "const missing\n";
        let file = SourceFile::new(0, None, input);
        let output = format_source(&file);
        assert_eq!(output.text, input);
        assert!(
            output
                .diagnostics
                .iter()
                .any(|diagnostic| diagnostic.code.as_deref() == Some("PARSE016"))
        );
    }

    #[test]
    fn formatter_preserves_crlf_line_endings() {
        let input = "fn demo(){\r\nconst x = 1  \r\nx\r\n}\r\n";
        let file = SourceFile::new(0, None, input);
        let output = format_source(&file);
        assert!(output.diagnostics.is_empty(), "{:#?}", output.diagnostics);
        assert_eq!(output.text, "fn demo(){\r\n  const x = 1\r\n  x\r\n}\r\n");
    }
}
