use std::collections::HashSet;

use crate::ast::{
    Block, ChainExpr, ChainSegmentKind, Expr, ExprKind, FunctionBody, FunctionDecl, FunctionExpr,
    FunctionName, Module, Pattern, PatternKind, Stmt, StmtKind,
};
use crate::diag::{Diagnostic, Label, Severity};
use crate::source::SourceFile;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LintOptions {
    pub warn_tail_table_newline: bool,
    pub warn_semicolon_suppressed_return: bool,
    pub warn_gmod_callback_implicit_return: bool,
}

impl Default for LintOptions {
    fn default() -> Self {
        Self {
            warn_tail_table_newline: true,
            warn_semicolon_suppressed_return: true,
            warn_gmod_callback_implicit_return: true,
        }
    }
}

pub fn lint_module(module: &Module, file: &SourceFile, options: LintOptions) -> Vec<Diagnostic> {
    let mut linter = Linter {
        file,
        options,
        diagnostics: Vec::new(),
        gmod_callback_blocks: HashSet::new(),
    };
    linter.module(module);
    linter.diagnostics
}

struct Linter<'a> {
    file: &'a SourceFile,
    options: LintOptions,
    diagnostics: Vec<Diagnostic>,
    gmod_callback_blocks: HashSet<crate::source::SourceSpan>,
}

impl Linter<'_> {
    fn module(&mut self, module: &Module) {
        for stmt in &module.body {
            self.stmt(stmt);
        }
    }

    fn stmt(&mut self, stmt: &Stmt) {
        match &stmt.kind {
            StmtKind::LocalDecl { values, .. } | StmtKind::Return(values) => {
                for expr in values {
                    self.expr(expr);
                }
            }
            StmtKind::Assign { targets, values } => {
                for (index, target) in targets.iter().enumerate() {
                    if let Some(value) = values.get(index) {
                        self.warn_self_assignment(target, value);
                        if self.options.warn_gmod_callback_implicit_return
                            && is_gmod_callback_target(target)
                        {
                            self.warn_gmod_callback_value_return(value, target.span);
                        }
                    }
                    self.expr(target);
                }
                for expr in values {
                    self.expr(expr);
                }
            }
            StmtKind::LocalDestructure {
                patterns, values, ..
            } => {
                for pattern in patterns {
                    self.pattern(pattern);
                }
                for expr in values {
                    self.expr(expr);
                }
            }
            StmtKind::CompoundAssign { target, value, .. } => {
                self.expr(target);
                self.expr(value);
            }
            StmtKind::Expr(expr) => self.expr(expr),
            StmtKind::Import(_)
            | StmtKind::PartOrderDecl(_)
            | StmtKind::ExternDecl(_)
            | StmtKind::HostPackageDecl(_)
            | StmtKind::ExportList { .. }
            | StmtKind::ExportAll { .. }
            | StmtKind::Break
            | StmtKind::Continue => {}
            StmtKind::EnumDecl(decl) => {
                for variant in &decl.variants {
                    if let Some(tag) = &variant.tag {
                        self.expr(tag);
                    }
                }
            }
            StmtKind::ExportDecl { stmt, .. } | StmtKind::RealmDecl { stmt, .. } => self.stmt(stmt),
            StmtKind::RealmBlock { block, .. } | StmtKind::InitDecl { block, .. } => {
                self.block(block)
            }
            StmtKind::FunctionDecl(decl) => {
                for param in &decl.params {
                    if let Some(default) = &param.default {
                        self.expr(default);
                    }
                }
                if self.options.warn_gmod_callback_implicit_return {
                    self.warn_gmod_callback_function_decl(decl);
                }
                self.function_body(&decl.body);
            }
            StmtKind::If {
                condition,
                then_block,
                else_block,
            } => {
                self.expr(condition);
                self.block(then_block);
                if let Some(block) = else_block {
                    self.block(block);
                }
            }
            StmtKind::While { condition, body } => {
                self.expr(condition);
                self.block(body);
            }
            StmtKind::NumericFor {
                start,
                end,
                step,
                body,
                ..
            } => {
                self.expr(start);
                self.expr(end);
                if let Some(step) = step {
                    self.expr(step);
                }
                self.block(body);
            }
            StmtKind::GenericFor { iter, body, .. } => {
                for expr in iter {
                    self.expr(expr);
                }
                self.block(body);
            }
            StmtKind::RepeatUntil { body, condition } => {
                self.block(body);
                self.expr(condition);
            }
            StmtKind::Do(block) => self.block(block),
        }
    }

    fn function_body(&mut self, body: &FunctionBody) {
        match body {
            FunctionBody::Expr(expr) => self.expr(expr),
            FunctionBody::Block(block) => {
                if self.options.warn_semicolon_suppressed_return {
                    self.warn_suppressed_tail(block);
                }
                self.block(block);
            }
        }
    }

    fn warn_suppressed_tail(&mut self, block: &Block) {
        if block.tail.is_some() {
            return;
        }
        if self.gmod_callback_blocks.contains(&block.span) {
            return;
        }
        let Some(stmt) = block.statements.last() else {
            return;
        };
        let StmtKind::Expr(_) = &stmt.kind else {
            return;
        };
        if !block_contains_trailing_semicolon(self.file, stmt.span.byte_end, block.span.byte_end) {
            return;
        }
        if lint_suppressed(self.file, stmt.span.byte_start, "LINT002") {
            return;
        }

        self.diagnostics.push(
            Diagnostic::new(Severity::Warning, "final expression is terminated by `;`")
                .with_code("LINT002")
                .with_label(Label::primary(
                    stmt.span,
                    "this expression is a statement, not an implicit return",
                ))
                .with_help(
                    "remove the trailing semicolon to make it the function's implicit return",
                ),
        );
    }

    fn block(&mut self, block: &Block) {
        for stmt in &block.statements {
            self.stmt(stmt);
        }
        if let Some(tail) = &block.tail {
            self.expr(tail);
        }
    }

    fn expr(&mut self, expr: &Expr) {
        match &expr.kind {
            ExprKind::Identifier(_)
            | ExprKind::Nil
            | ExprKind::Boolean(_)
            | ExprKind::Number(_)
            | ExprKind::String(_)
            | ExprKind::Vararg
            | ExprKind::PipelinePlaceholder => {}
            ExprKind::TemplateString(parts) => {
                for part in parts {
                    if let crate::ast::TemplatePartKind::Expr(expr) = &part.kind {
                        self.expr(expr);
                    }
                }
            }
            ExprKind::Table(table) => {
                for field in &table.fields {
                    match &field.kind {
                        crate::ast::TableFieldKind::Array(expr) => self.expr(expr),
                        crate::ast::TableFieldKind::Named { value, .. } => self.expr(value),
                        crate::ast::TableFieldKind::ExprKey { key, value } => {
                            self.expr(key);
                            self.expr(value);
                        }
                        crate::ast::TableFieldKind::Spread(value) => self.expr(value),
                    }
                }
            }
            ExprKind::Paren(expr) => self.expr(expr),
            ExprKind::Unary { argument, .. } => self.expr(argument),
            ExprKind::Binary { left, right, .. } => {
                self.expr(left);
                self.expr(right);
            }
            ExprKind::Conditional {
                condition,
                then_branch,
                else_branch,
                ..
            } => {
                self.expr(condition);
                self.expr_or_block(then_branch);
                self.expr_or_block(else_branch);
            }
            ExprKind::Match(match_expr) => {
                self.expr(&match_expr.subject);
                for arm in &match_expr.arms {
                    self.expr_or_block(&arm.body);
                }
            }
            ExprKind::Do(block) => self.block(block),
            ExprKind::Function(function) => {
                for param in &function.params {
                    if let Some(default) = &param.default {
                        self.expr(default);
                    }
                }
                self.function_body(&function.body);
            }
            ExprKind::Chain(chain) => {
                self.expr(&chain.base);
                if self.options.warn_gmod_callback_implicit_return {
                    self.warn_gmod_hook_callback_arg(chain);
                }
                for segment in &chain.segments {
                    match &segment.kind {
                        ChainSegmentKind::Member { .. } => {}
                        ChainSegmentKind::Index { index, .. } => self.expr(index),
                        ChainSegmentKind::Call { args, style } => {
                            if self.options.warn_tail_table_newline
                                && matches!(style, crate::ast::CallStyle::TailTable)
                            {
                                self.warn_tail_table_newline(&chain.base, segment.span);
                            }
                            for arg in args {
                                self.expr(arg);
                            }
                        }
                        ChainSegmentKind::SafeDotCall { args, .. }
                        | ChainSegmentKind::MethodCall { args, .. } => {
                            for arg in args {
                                self.expr(arg);
                            }
                        }
                    }
                }
            }
        }
    }

    fn pattern(&mut self, pattern: &Pattern) {
        match &pattern.kind {
            PatternKind::Identifier(_) => {}
            PatternKind::Object(fields) => {
                for field in fields {
                    self.pattern(&field.pattern);
                    if let Some(default) = &field.default {
                        self.expr(default);
                    }
                }
            }
            PatternKind::Array(items) => {
                for item in items {
                    self.pattern(&item.pattern);
                    if let Some(default) = &item.default {
                        self.expr(default);
                    }
                }
            }
        }
    }

    fn expr_or_block(&mut self, item: &crate::ast::ExprOrBlock) {
        match item {
            crate::ast::ExprOrBlock::Expr(expr) => self.expr(expr),
            crate::ast::ExprOrBlock::Block(block) => self.block(block),
        }
    }

    fn warn_tail_table_newline(&mut self, receiver: &Expr, table_span: crate::source::SourceSpan) {
        let (receiver_line, _) = self.file.line_col(receiver.span.byte_end);
        let (table_line, _) = self.file.line_col(table_span.byte_start);
        if receiver_line == table_line {
            return;
        }
        if lint_suppressed(self.file, table_span.byte_start, "LINT001") {
            return;
        }

        self.diagnostics.push(
            Diagnostic::new(
                Severity::Warning,
                "tail table call continues across a newline",
            )
            .with_code("LINT001")
            .with_label(Label::primary(
                table_span,
                "this `{ ... }` is passed as a call argument",
            ))
            .with_label(Label::secondary(
                receiver.span,
                "call receiver continues here",
            ))
            .with_help("add `;` before the table if you intended to start a new statement"),
        );
    }

    fn warn_self_assignment(&mut self, target: &Expr, value: &Expr) {
        if !expr_equivalent(target, value) {
            return;
        }
        if lint_suppressed(self.file, target.span.byte_start, "LINT003") {
            return;
        }

        self.diagnostics.push(
            Diagnostic::new(Severity::Warning, "self-assignment has no effect")
                .with_code("LINT003")
                .with_label(Label::primary(
                    target.span,
                    "this target is assigned to itself",
                ))
                .with_help("remove the assignment or assign a different value"),
        );
    }

    fn warn_gmod_callback_function_decl(&mut self, decl: &FunctionDecl) {
        let FunctionName::Method { method, .. } = &decl.name else {
            return;
        };
        if !is_gmod_callback_name(&method.name) {
            return;
        }
        self.warn_gmod_callback_body_return(&decl.body, method.span);
    }

    fn warn_gmod_callback_value_return(
        &mut self,
        value: &Expr,
        target_span: crate::source::SourceSpan,
    ) {
        let ExprKind::Function(function) = &value.kind else {
            return;
        };
        self.warn_gmod_callback_function_return(function, target_span);
    }

    fn warn_gmod_callback_function_return(
        &mut self,
        function: &FunctionExpr,
        target_span: crate::source::SourceSpan,
    ) {
        self.warn_gmod_callback_body_return(&function.body, target_span);
    }

    fn warn_gmod_hook_callback_arg(&mut self, chain: &ChainExpr) {
        let Some((args, call_span)) = hook_registration_call(chain) else {
            return;
        };
        let Some(callback) = args.last() else {
            return;
        };
        let ExprKind::Function(function) = &callback.kind else {
            return;
        };
        self.warn_gmod_callback_function_return(function, call_span);
    }

    fn warn_gmod_callback_body_return(
        &mut self,
        body: &FunctionBody,
        label_span: crate::source::SourceSpan,
    ) {
        let return_span = match body {
            FunctionBody::Expr(expr) => expr.span,
            FunctionBody::Block(block) => {
                self.gmod_callback_blocks.insert(block.span);
                let Some(tail) = &block.tail else {
                    return;
                };
                tail.span
            }
        };
        if lint_suppressed(self.file, return_span.byte_start, "LINT004") {
            return;
        }

        self.diagnostics.push(
            Diagnostic::new(
                Severity::Warning,
                "GMod callback implicitly returns its final expression",
            )
            .with_code("LINT004")
            .with_label(Label::primary(
                return_span,
                "this expression becomes the callback return value",
            ))
            .with_label(Label::secondary(label_span, "callback is registered here"))
            .with_help(
                "add `;` after the final expression when the callback should not return a value",
            ),
        );
    }
}

fn is_gmod_callback_target(target: &Expr) -> bool {
    let ExprKind::Chain(chain) = &target.kind else {
        return false;
    };
    chain_last_callback_name(chain).is_some_and(is_gmod_callback_name)
}

fn chain_last_callback_name(chain: &ChainExpr) -> Option<&str> {
    let last = chain.segments.last()?;
    match &last.kind {
        ChainSegmentKind::Member { name, .. } => Some(name.name.as_str()),
        ChainSegmentKind::Index { .. }
        | ChainSegmentKind::Call { .. }
        | ChainSegmentKind::SafeDotCall { .. }
        | ChainSegmentKind::MethodCall { .. } => None,
    }
}

fn is_gmod_callback_name(name: &str) -> bool {
    matches!(
        name,
        "Paint"
            | "PaintOver"
            | "Think"
            | "PerformLayout"
            | "LayoutEntity"
            | "OnMousePressed"
            | "OnMouseReleased"
            | "OnCursorEntered"
            | "OnCursorExited"
            | "OnCursorMoved"
            | "OnKeyCodePressed"
            | "OnKeyCodeReleased"
            | "OnMouseWheeled"
            | "OnRemove"
            | "Init"
    )
}

fn hook_registration_call(chain: &ChainExpr) -> Option<(&[Expr], crate::source::SourceSpan)> {
    let (last, prefix) = chain.segments.split_last()?;
    let ChainSegmentKind::Call { args, .. } = &last.kind else {
        return None;
    };
    if is_hook_add_callee(&chain.base, prefix) {
        Some((args, last.span))
    } else {
        None
    }
}

fn is_hook_add_callee(base: &Expr, segments: &[crate::ast::ChainSegment]) -> bool {
    let ExprKind::Identifier(base) = &base.kind else {
        return false;
    };
    let path = std::iter::once(base.name.as_str())
        .chain(segments.iter().filter_map(|segment| match &segment.kind {
            ChainSegmentKind::Member {
                name,
                optional: false,
            } => Some(name.name.as_str()),
            _ => None,
        }))
        .collect::<Vec<_>>();

    matches!(
        path.as_slice(),
        ["hookAdd"] | ["hook", "Add"] | ["hookx", "add"]
    )
}

fn expr_equivalent(left: &Expr, right: &Expr) -> bool {
    match (&left.kind, &right.kind) {
        (ExprKind::Paren(left), _) => expr_equivalent(left, right),
        (_, ExprKind::Paren(right)) => expr_equivalent(left, right),
        (ExprKind::Identifier(left), ExprKind::Identifier(right)) => left.name == right.name,
        (ExprKind::Nil, ExprKind::Nil) => true,
        (ExprKind::Boolean(left), ExprKind::Boolean(right)) => left == right,
        (ExprKind::Number(left), ExprKind::Number(right)) => left == right,
        (ExprKind::String(left), ExprKind::String(right)) => left == right,
        (ExprKind::Vararg, ExprKind::Vararg) => true,
        (ExprKind::PipelinePlaceholder, ExprKind::PipelinePlaceholder) => true,
        (
            ExprKind::Unary {
                op: left_op,
                argument: left_arg,
            },
            ExprKind::Unary {
                op: right_op,
                argument: right_arg,
            },
        ) => left_op == right_op && expr_equivalent(left_arg, right_arg),
        (
            ExprKind::Binary {
                op: left_op,
                left: left_left,
                right: left_right,
            },
            ExprKind::Binary {
                op: right_op,
                left: right_left,
                right: right_right,
            },
        ) => {
            left_op == right_op
                && expr_equivalent(left_left, right_left)
                && expr_equivalent(left_right, right_right)
        }
        (ExprKind::Chain(left), ExprKind::Chain(right)) => {
            expr_equivalent(&left.base, &right.base)
                && left.segments.len() == right.segments.len()
                && left
                    .segments
                    .iter()
                    .zip(&right.segments)
                    .all(|(left, right)| chain_segment_equivalent(&left.kind, &right.kind))
        }
        _ => false,
    }
}

fn chain_segment_equivalent(left: &ChainSegmentKind, right: &ChainSegmentKind) -> bool {
    match (left, right) {
        (
            ChainSegmentKind::Member {
                name: left_name,
                optional: left_optional,
            },
            ChainSegmentKind::Member {
                name: right_name,
                optional: right_optional,
            },
        ) => left_name.name == right_name.name && left_optional == right_optional,
        (
            ChainSegmentKind::Index {
                index: left_index,
                optional: left_optional,
            },
            ChainSegmentKind::Index {
                index: right_index,
                optional: right_optional,
            },
        ) => left_optional == right_optional && expr_equivalent(left_index, right_index),
        (
            ChainSegmentKind::Call {
                args: left_args,
                style: left_style,
            },
            ChainSegmentKind::Call {
                args: right_args,
                style: right_style,
            },
        ) => {
            left_style == right_style
                && left_args.len() == right_args.len()
                && left_args
                    .iter()
                    .zip(right_args)
                    .all(|(left, right)| expr_equivalent(left, right))
        }
        (
            ChainSegmentKind::SafeDotCall {
                name: left_name,
                args: left_args,
                style: left_style,
            },
            ChainSegmentKind::SafeDotCall {
                name: right_name,
                args: right_args,
                style: right_style,
            },
        ) => {
            left_name.name == right_name.name
                && left_style == right_style
                && left_args.len() == right_args.len()
                && left_args
                    .iter()
                    .zip(right_args)
                    .all(|(left, right)| expr_equivalent(left, right))
        }
        (
            ChainSegmentKind::MethodCall {
                name: left_name,
                args: left_args,
                optional: left_optional,
                style: left_style,
            },
            ChainSegmentKind::MethodCall {
                name: right_name,
                args: right_args,
                optional: right_optional,
                style: right_style,
            },
        ) => {
            left_name.name == right_name.name
                && left_optional == right_optional
                && left_style == right_style
                && left_args.len() == right_args.len()
                && left_args
                    .iter()
                    .zip(right_args)
                    .all(|(left, right)| expr_equivalent(left, right))
        }
        _ => false,
    }
}

fn block_contains_trailing_semicolon(file: &SourceFile, expr_end: usize, block_end: usize) -> bool {
    file.text
        .get(expr_end..block_end)
        .is_some_and(|text| text.contains(';'))
}

fn lint_suppressed(file: &SourceFile, offset: usize, code: &str) -> bool {
    let (line, _) = file.line_col(offset);
    line_has_lint_allow(file, line, code)
        || line
            .checked_sub(1)
            .is_some_and(|previous| line_has_lint_allow(file, previous, code))
}

fn line_has_lint_allow(file: &SourceFile, line: usize, code: &str) -> bool {
    let Some(text) = file.line_text(line) else {
        return false;
    };
    text.contains("lux-lint: allow") && (text.contains(code) || text.contains("all"))
}

#[cfg(test)]
mod tests {
    use crate::lex::Lexer;
    use crate::parse::Parser;
    use crate::source::SourceFile;

    use super::{LintOptions, lint_module};

    fn lint(input: &str) -> Vec<crate::diag::Diagnostic> {
        let file = SourceFile::new(0, None, input);
        let lex = Lexer::new(&file).lex_all();
        assert!(lex.diagnostics.is_empty(), "{:#?}", lex.diagnostics);
        let parsed = Parser::new(&lex.tokens).parse_module();
        assert!(parsed.diagnostics.is_empty(), "{:#?}", parsed.diagnostics);
        lint_module(&parsed.module, &file, LintOptions::default())
    }

    #[test]
    fn newline_table_after_expression_is_no_longer_tail_table_call() {
        let diagnostics = lint("fn demo() { values(...)\n{ x = 1 } }");
        assert!(
            diagnostics
                .iter()
                .all(|diag| diag.code.as_deref() != Some("LINT001"))
        );
    }

    #[test]
    fn warns_for_semicolon_suppressed_return() {
        let diagnostics = lint("fn demo() { 1; }");
        assert!(
            diagnostics
                .iter()
                .any(|diag| diag.code.as_deref() == Some("LINT002"))
        );
    }

    #[test]
    fn warns_for_self_assignment() {
        let diagnostics = lint("fn demo(out) { out.radius = out.radius }");
        assert!(
            diagnostics
                .iter()
                .any(|diag| diag.code.as_deref() == Some("LINT003")),
            "{diagnostics:#?}"
        );

        let diagnostics = lint("fn demo(out) { out.radius = (out.radius) }");
        assert!(
            diagnostics
                .iter()
                .any(|diag| diag.code.as_deref() == Some("LINT003")),
            "{diagnostics:#?}"
        );
    }

    #[test]
    fn warns_for_gmod_callback_implicit_return() {
        let diagnostics = lint("fn PANEL:Paint(w, h) { drawBody(self, w, h) }");
        assert!(
            diagnostics
                .iter()
                .any(|diag| diag.code.as_deref() == Some("LINT004")),
            "{diagnostics:#?}"
        );

        let diagnostics =
            lint("fn setup() { hookAdd(\"Initialize\", \"id\", () => { loadWorthCarts() }) }");
        assert!(
            diagnostics
                .iter()
                .any(|diag| diag.code.as_deref() == Some("LINT004")),
            "{diagnostics:#?}"
        );

        let diagnostics =
            lint("fn setup() { hook.Add(\"Initialize\", \"id\", () => { loadWorthCarts() }) }");
        assert!(
            diagnostics
                .iter()
                .any(|diag| diag.code.as_deref() == Some("LINT004")),
            "{diagnostics:#?}"
        );

        let diagnostics =
            lint("fn setup() { hookx.add(\"Initialize\", \"id\", () => { loadWorthCarts() }) }");
        assert!(
            diagnostics
                .iter()
                .any(|diag| diag.code.as_deref() == Some("LINT004")),
            "{diagnostics:#?}"
        );

        let diagnostics =
            lint("fn setup(panel) { panel.Paint = (w, h) -> { drawBody(self, w, h) } }");
        assert!(
            diagnostics
                .iter()
                .any(|diag| diag.code.as_deref() == Some("LINT004")),
            "{diagnostics:#?}"
        );
    }

    #[test]
    fn gmod_callback_semicolon_does_not_warn_as_suppressed_return() {
        let diagnostics = lint("fn PANEL:Paint(w, h) { drawBody(self, w, h); }");
        assert!(
            diagnostics
                .iter()
                .all(|diag| diag.code.as_deref() != Some("LINT002")),
            "{diagnostics:#?}"
        );
        assert!(
            diagnostics
                .iter()
                .all(|diag| diag.code.as_deref() != Some("LINT004")),
            "{diagnostics:#?}"
        );

        let diagnostics =
            lint("fn setup() { hookAdd(\"Initialize\", \"id\", () => { loadWorthCarts(); }) }");
        assert!(
            diagnostics
                .iter()
                .all(|diag| diag.code.as_deref() != Some("LINT002")),
            "{diagnostics:#?}"
        );
        assert!(
            diagnostics
                .iter()
                .all(|diag| diag.code.as_deref() != Some("LINT004")),
            "{diagnostics:#?}"
        );
    }

    #[test]
    fn supports_lint_suppression_comments() {
        let diagnostics = lint("fn demo() {\n  -- lux-lint: allow LINT002\n  1;\n}");
        assert!(diagnostics.is_empty(), "{diagnostics:#?}");

        let diagnostics =
            lint("fn demo(out) {\n  -- lux-lint: allow LINT003\n  out.radius = out.radius\n}");
        assert!(diagnostics.is_empty(), "{diagnostics:#?}");

        let diagnostics =
            lint("fn PANEL:Paint(w, h) {\n  -- lux-lint: allow LINT004\n  drawBody(self, w, h)\n}");
        assert!(diagnostics.is_empty(), "{diagnostics:#?}");
    }
}
