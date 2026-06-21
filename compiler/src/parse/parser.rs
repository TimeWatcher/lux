use crate::ast::*;
use crate::diag::{Diagnostic, Label, Severity};
use crate::lex::{Token, TokenKind};
use crate::source::{FileId, SourceSpan};

#[derive(Debug)]
pub struct ParseOutput {
    pub module: Module,
    pub diagnostics: Vec<Diagnostic>,
}

impl ParseOutput {
    pub fn has_errors(&self) -> bool {
        self.diagnostics
            .iter()
            .any(|diagnostic| diagnostic.severity == Severity::Error)
    }
}

pub struct Parser<'a> {
    tokens: &'a [Token],
    index: usize,
    diagnostics: Vec<Diagnostic>,
    file_id: FileId,
}

#[derive(Debug, Clone, Copy)]
struct ExprParseOptions {
    allow_tail_table_call: bool,
    allow_pipeline_placeholder: bool,
    allow_then_else: bool,
}

impl Default for ExprParseOptions {
    fn default() -> Self {
        Self {
            allow_tail_table_call: true,
            allow_pipeline_placeholder: false,
            allow_then_else: true,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ConditionalControlTarget {
    Return,
    Break,
    Continue,
}

impl<'a> Parser<'a> {
    pub fn new(tokens: &'a [Token]) -> Self {
        let file_id = tokens
            .first()
            .map(|token| token.span.file_id)
            .unwrap_or(FileId(0));
        Self {
            tokens,
            index: 0,
            diagnostics: Vec::new(),
            file_id,
        }
    }

    pub fn parse_module(mut self) -> ParseOutput {
        let start = self.current_span().byte_start;
        let mut body = Vec::new();

        while !self.at(&TokenKind::Eof) {
            if self.eat(&TokenKind::Semicolon).is_some() {
                continue;
            }
            let diagnostics_before = self.diagnostics.len();
            body.push(self.parse_stmt());
            if self.diagnostics.len() > diagnostics_before {
                self.recover_after_stmt();
            }
        }

        let end = self.current_span().byte_end;
        ParseOutput {
            module: Module {
                body,
                span: self.span(start, end),
            },
            diagnostics: self.diagnostics,
        }
    }

    fn parse_stmt(&mut self) -> Stmt {
        match &self.current().kind {
            TokenKind::KwLocal => self.parse_local_decl_stmt(),
            TokenKind::KwConst => self.parse_const_decl_stmt(),
            TokenKind::KwReturn => self.parse_return_stmt(),
            TokenKind::KwBreak => self.parse_break_stmt(),
            TokenKind::KwImport => self.parse_import_stmt(),
            TokenKind::KwExport => self.parse_export_stmt(),
            TokenKind::KwFn => self.parse_fn_decl_stmt(),
            TokenKind::KwFunction => self.parse_lua_function_decl_stmt(),
            TokenKind::KwIf => {
                if self.looks_like_if_stmt() {
                    self.parse_if_stmt()
                } else {
                    self.parse_assignment_or_expr_stmt()
                }
            }
            TokenKind::KwWhile => self.parse_while_stmt(),
            TokenKind::KwFor => self.parse_for_stmt(),
            TokenKind::KwRepeat => self.parse_repeat_stmt(),
            TokenKind::KwDo => self.parse_do_stmt(),
            TokenKind::Identifier(name) if name == "continue" => self.parse_continue_stmt(),
            TokenKind::Identifier(name)
                if matches!(
                    name.as_str(),
                    "stopif" | "stopifn" | "breakif" | "breakifn" | "continueif" | "continueifn"
                ) =>
            {
                self.parse_conditional_control_stmt()
            }
            TokenKind::Identifier(name) if name == "enum" => self.parse_enum_decl_stmt(),
            TokenKind::Identifier(name) if name == "part" => self.parse_part_order_stmt(),
            TokenKind::Identifier(name) if name == "extern" => self.parse_extern_stmt(),
            TokenKind::Identifier(name) if name == "init" => self.parse_init_stmt(None, None),
            TokenKind::Identifier(name) if Realm::parse(name).is_some() => {
                self.parse_realm_prefixed_stmt()
            }
            _ => self.parse_assignment_or_expr_stmt(),
        }
    }

    fn parse_local_decl_stmt(&mut self) -> Stmt {
        let start = self.expect(&TokenKind::KwLocal).byte_start;
        if self.at(&TokenKind::KwFunction) {
            return self.parse_local_function_decl_stmt(start);
        }
        self.parse_binding_decl_stmt(start, BindingMode::Local)
    }

    fn parse_const_decl_stmt(&mut self) -> Stmt {
        let start = self.expect(&TokenKind::KwConst).byte_start;
        self.parse_binding_decl_stmt(start, BindingMode::Const)
    }

    fn parse_binding_decl_stmt(&mut self, start: usize, mode: BindingMode) -> Stmt {
        let mut patterns = Vec::new();
        patterns.push(self.parse_pattern());
        while self.eat(&TokenKind::Comma).is_some() {
            patterns.push(self.parse_pattern());
        }

        let mut values = Vec::new();
        if self.eat(&TokenKind::Eq).is_some() {
            values = self.parse_expr_list();
        } else if mode == BindingMode::Const {
            self.error(
                "PARSE016",
                "`const` declarations require an initializer",
                self.span(
                    start,
                    patterns
                        .last()
                        .map(|pattern| pattern.span.byte_end)
                        .unwrap_or(start),
                ),
            );
        }

        let end = values
            .last()
            .map(|expr| expr.span.byte_end)
            .or_else(|| patterns.last().map(|pattern| pattern.span.byte_end))
            .unwrap_or(start);
        let kind = if patterns
            .iter()
            .all(|pattern| matches!(pattern.kind, PatternKind::Identifier(_)))
        {
            StmtKind::LocalDecl {
                mode,
                names: patterns
                    .into_iter()
                    .map(|pattern| match pattern.kind {
                        PatternKind::Identifier(name) => name,
                        _ => unreachable!("checked identifier-only patterns"),
                    })
                    .collect(),
                values,
            }
        } else {
            StmtKind::LocalDestructure {
                mode,
                patterns,
                values,
            }
        };
        Stmt {
            kind,
            span: self.span(start, end),
        }
    }

    fn parse_return_stmt(&mut self) -> Stmt {
        let start = self.expect(&TokenKind::KwReturn).byte_start;
        let values = if self.is_stmt_boundary() {
            Vec::new()
        } else {
            self.parse_expr_list()
        };
        let end = values
            .last()
            .map(|expr| expr.span.byte_end)
            .unwrap_or(start);
        Stmt {
            kind: StmtKind::Return(values),
            span: self.span(start, end),
        }
    }

    fn parse_break_stmt(&mut self) -> Stmt {
        let span = self.expect(&TokenKind::KwBreak);
        Stmt {
            kind: StmtKind::Break,
            span,
        }
    }

    fn parse_continue_stmt(&mut self) -> Stmt {
        let span = self
            .eat_contextual_keyword("continue")
            .expect("parse_continue_stmt called at continue");
        Stmt {
            kind: StmtKind::Continue,
            span,
        }
    }

    fn parse_conditional_control_stmt(&mut self) -> Stmt {
        let token = self.current().clone();
        let TokenKind::Identifier(keyword) = token.kind else {
            unreachable!("parse_conditional_control_stmt called at contextual keyword");
        };
        self.bump();

        let control = match keyword.as_str() {
            "stopif" | "stopifn" => ConditionalControlTarget::Return,
            "breakif" | "breakifn" => ConditionalControlTarget::Break,
            "continueif" | "continueifn" => ConditionalControlTarget::Continue,
            _ => unreachable!("checked by caller"),
        };
        let negated = matches!(keyword.as_str(), "stopifn" | "breakifn" | "continueifn");
        let raw_condition = self.parse_control_condition();
        let condition = if negated {
            Expr {
                span: self.span(token.span.byte_start, raw_condition.span.byte_end),
                kind: ExprKind::Unary {
                    op: UnaryOp::Not,
                    argument: Box::new(raw_condition),
                },
            }
        } else {
            raw_condition
        };

        let mut values = Vec::new();
        if self.eat(&TokenKind::Comma).is_some() {
            if control == ConditionalControlTarget::Return {
                values = self.parse_expr_list();
            } else {
                self.error(
                    "PARSE020",
                    format!("`{keyword}` does not accept return values"),
                    token.span,
                );
            }
        }

        let end = values
            .last()
            .map(|expr| expr.span.byte_end)
            .unwrap_or(condition.span.byte_end);
        let inner = match control {
            ConditionalControlTarget::Return => Stmt {
                kind: StmtKind::Return(values),
                span: self.span(token.span.byte_start, end),
            },
            ConditionalControlTarget::Break => Stmt {
                kind: StmtKind::Break,
                span: token.span,
            },
            ConditionalControlTarget::Continue => Stmt {
                kind: StmtKind::Continue,
                span: token.span,
            },
        };

        Stmt {
            kind: StmtKind::If {
                condition,
                then_block: Block {
                    statements: vec![inner],
                    tail: None,
                    span: self.span(token.span.byte_start, end),
                },
                else_block: None,
            },
            span: self.span(token.span.byte_start, end),
        }
    }

    fn parse_import_stmt(&mut self) -> Stmt {
        let start = self.expect(&TokenKind::KwImport).byte_start;
        let mut specifiers = Vec::new();
        let source;
        let side_effect_only;
        let phase = if self.eat_contextual_keyword("macro").is_some() {
            ImportPhase::Macro
        } else {
            ImportPhase::Runtime
        };

        if self.eat(&TokenKind::LBrace).is_some() {
            loop {
                let imported = self.expect_identifier();
                let local = if self.eat_contextual_keyword("as").is_some() {
                    self.expect_identifier()
                } else {
                    imported.clone()
                };
                specifiers.push(ImportSpecifier::Named { imported, local });
                if self.eat(&TokenKind::Comma).is_none() {
                    break;
                }
                if self.at(&TokenKind::RBrace) {
                    break;
                }
            }
            self.expect(&TokenKind::RBrace);
            self.expect_contextual_keyword("from");
            source = self.expect_string();
            side_effect_only = false;
        } else if self.eat(&TokenKind::Star).is_some() {
            self.expect_contextual_keyword("as");
            let local = self.expect_identifier();
            specifiers.push(ImportSpecifier::Namespace { local });
            self.expect_contextual_keyword("from");
            source = self.expect_string();
            side_effect_only = false;
        } else {
            source = self.expect_string();
            side_effect_only = true;
        }

        let end = self.previous_span().byte_end;
        Stmt {
            kind: StmtKind::Import(ImportStmt {
                source,
                specifiers,
                side_effect_only,
                phase,
            }),
            span: self.span(start, end),
        }
    }

    fn parse_part_order_stmt(&mut self) -> Stmt {
        let start = self
            .eat_contextual_keyword("part")
            .expect("parse_part_order_stmt called at part")
            .byte_start;
        let kind = if self.eat_contextual_keyword("order").is_some() {
            self.expect(&TokenKind::LBrace);
            let mut targets = Vec::new();
            while !self.at(&TokenKind::RBrace) && !self.at(&TokenKind::Eof) {
                targets.push(self.expect_string());
                if self.eat(&TokenKind::Comma).is_none() {
                    break;
                }
                if self.at(&TokenKind::RBrace) {
                    break;
                }
            }
            self.expect(&TokenKind::RBrace);
            PartOrderKind::Order { targets }
        } else if self.eat_contextual_keyword("before").is_some() {
            let target = self.expect_string();
            PartOrderKind::Relative {
                relation: PartOrderRelation::Before,
                target,
            }
        } else if self.eat_contextual_keyword("after").is_some() {
            let target = self.expect_string();
            PartOrderKind::Relative {
                relation: PartOrderRelation::After,
                target,
            }
        } else {
            self.error_with_help(
                "PARSE019",
                "`part` ordering declarations require `order`, `before`, or `after`",
                self.current_span(),
                "write `part order { \"cl_base\", \"cl_install\" }` or `part after \"cl_base\"`",
            );
            PartOrderKind::Order {
                targets: Vec::new(),
            }
        };
        let end = self.previous_span().byte_end;
        Stmt {
            kind: StmtKind::PartOrderDecl(PartOrderDecl { kind }),
            span: self.span(start, end),
        }
    }

    fn parse_export_stmt(&mut self) -> Stmt {
        let start = self.expect(&TokenKind::KwExport).byte_start;
        let runtime_marker = self.eat_contextual_keyword("runtime").is_some();
        let kind = if self.eat_contextual_keyword("macro").is_some() {
            ExportKind::Macro
        } else if self.eat_contextual_keyword("host").is_some() {
            if self.eat_contextual_keyword("package").is_some() {
                return self.parse_host_package_decl_stmt(start);
            }
            self.expect_contextual_keyword("expr");
            ExportKind::HostExpr
        } else {
            ExportKind::Runtime
        };
        let realm = if kind == ExportKind::Runtime {
            self.eat_realm_keyword()
        } else {
            None
        };

        if self.at(&TokenKind::KwFn) {
            let decl = self.parse_fn_decl_stmt();
            let end = decl.span.byte_end;
            return Stmt {
                kind: StmtKind::ExportDecl {
                    kind,
                    realm,
                    stmt: Box::new(decl),
                },
                span: self.span(start, end),
            };
        }

        if self.is_contextual_keyword("enum") {
            if !runtime_marker {
                self.error_with_help(
                    "PARSE024",
                    "`export enum` must be explicit about runtime emission",
                    self.current_span(),
                    "write `export runtime enum Name repr number { ... }` when a runtime table is required",
                );
            }
            let decl = self.parse_enum_decl_stmt_with_runtime(true);
            let end = decl.span.byte_end;
            return Stmt {
                kind: StmtKind::ExportDecl {
                    kind,
                    realm,
                    stmt: Box::new(decl),
                },
                span: self.span(start, end),
            };
        }

        if self.at(&TokenKind::KwConst) {
            if kind != ExportKind::Runtime {
                self.error(
                    "PARSE010",
                    "`export macro` and `export host expr` require a function declaration",
                    self.current_span(),
                );
            }
            let decl = self.parse_const_decl_stmt();
            let end = decl.span.byte_end;
            return Stmt {
                kind: StmtKind::ExportDecl {
                    kind,
                    realm,
                    stmt: Box::new(decl),
                },
                span: self.span(start, end),
            };
        }

        if kind != ExportKind::Runtime {
            self.error(
                "PARSE010",
                "`export macro` and `export host expr` require a function declaration",
                self.current_span(),
            );
        }

        if self.eat_contextual_keyword("all").is_some() {
            let end = self.previous_span().byte_end;
            return Stmt {
                kind: StmtKind::ExportAll { realm },
                span: self.span(start, end),
            };
        }

        self.expect(&TokenKind::LBrace);
        let mut entries = Vec::new();
        while !self.at(&TokenKind::RBrace) && !self.at(&TokenKind::Eof) {
            entries.push(self.parse_export_specifier());
            if self.eat(&TokenKind::Comma).is_none() {
                break;
            }
            if self.at(&TokenKind::RBrace) {
                break;
            }
        }
        let end = self.expect(&TokenKind::RBrace).byte_end;
        Stmt {
            kind: StmtKind::ExportList { realm, entries },
            span: self.span(start, end),
        }
    }

    fn parse_export_specifier(&mut self) -> ExportSpecifier {
        let first = self.expect_identifier();

        if self.eat(&TokenKind::Eq).is_some() {
            let local = self.expect_identifier();
            let span = self.span(first.span.byte_start, local.span.byte_end);
            return ExportSpecifier {
                exported: first,
                local,
                span,
            };
        }

        if self.eat_contextual_keyword("as").is_some() {
            let exported = self.expect_identifier();
            let span = self.span(first.span.byte_start, exported.span.byte_end);
            return ExportSpecifier {
                exported,
                local: first,
                span,
            };
        }

        ExportSpecifier {
            exported: first.clone(),
            local: first.clone(),
            span: first.span,
        }
    }

    fn parse_extern_stmt(&mut self) -> Stmt {
        let start = self
            .eat_contextual_keyword("extern")
            .expect("parse_extern_stmt called at extern")
            .byte_start;
        let realm = self.eat_realm_keyword().unwrap_or_else(|| {
            self.error(
                "PARSE017",
                "`extern` requires a realm: `shared`, `client`, or `server`",
                self.current_span(),
            );
            Realm::Shared
        });
        let mut path = vec![self.expect_identifier()];
        while self.eat(&TokenKind::Dot).is_some() {
            path.push(self.expect_identifier());
        }
        let end = path
            .last()
            .map(|ident| ident.span.byte_end)
            .unwrap_or(start);
        Stmt {
            kind: StmtKind::ExternDecl(ExternDecl { realm, path }),
            span: self.span(start, end),
        }
    }

    fn parse_realm_prefixed_stmt(&mut self) -> Stmt {
        let realm_span = self.current_span();
        let realm = self
            .eat_realm_keyword()
            .expect("parse_realm_prefixed_stmt called at realm");

        if self.at(&TokenKind::KwExport) {
            self.error_with_help(
                "PARSE018",
                "`client export`, `server export`, and `shared export` are not valid Lux syntax",
                realm_span,
                "write the export marker first, for example `export client fn name(...)`",
            );
            return self.parse_export_stmt();
        }

        if self.at(&TokenKind::LBrace) {
            let block = self.parse_block();
            let end = block.span.byte_end;
            return Stmt {
                kind: StmtKind::RealmBlock { realm, block },
                span: self.span(realm_span.byte_start, end),
            };
        }

        if self.is_contextual_keyword("init") {
            return self.parse_init_stmt(Some(realm), Some(realm_span.byte_start));
        }

        let inner = match &self.current().kind {
            TokenKind::KwFn => self.parse_fn_decl_stmt(),
            TokenKind::KwFunction => self.parse_lua_function_decl_stmt(),
            TokenKind::KwLocal => self.parse_local_decl_stmt(),
            TokenKind::KwConst => self.parse_const_decl_stmt(),
            _ => {
                self.error(
                    "PARSE019",
                    "realm markers may only prefix declarations, `init`, or a realm block",
                    self.current_span(),
                );
                self.parse_stmt()
            }
        };
        let end = inner.span.byte_end;
        Stmt {
            kind: StmtKind::RealmDecl {
                realm,
                stmt: Box::new(inner),
            },
            span: self.span(realm_span.byte_start, end),
        }
    }

    fn parse_init_stmt(&mut self, realm: Option<Realm>, start_override: Option<usize>) -> Stmt {
        let start = start_override.unwrap_or_else(|| self.current_span().byte_start);
        self.expect_contextual_keyword("init");
        let block = self.parse_block();
        let end = block.span.byte_end;
        Stmt {
            kind: StmtKind::InitDecl { realm, block },
            span: self.span(start, end),
        }
    }

    fn parse_host_package_decl_stmt(&mut self, start: usize) -> Stmt {
        self.expect(&TokenKind::LBrace);
        let mut target = None;
        let mut runtime = None;

        while !self.at(&TokenKind::RBrace) && !self.at(&TokenKind::Eof) {
            if self.eat(&TokenKind::Semicolon).is_some() || self.eat(&TokenKind::Comma).is_some() {
                continue;
            }

            let key = self.expect_identifier();
            self.expect(&TokenKind::Eq);
            let value = self.expect_string();
            match key.name.as_str() {
                "target" => target = Some(value),
                "runtime" => runtime = Some(value),
                other => self.error(
                    "PARSE011",
                    format!("unknown host package field `{other}`"),
                    key.span,
                ),
            }

            let _ = self
                .eat(&TokenKind::Comma)
                .or_else(|| self.eat(&TokenKind::Semicolon));
        }

        let end = self.expect(&TokenKind::RBrace).byte_end;
        if target.is_none() {
            self.error(
                "PARSE012",
                "host package declaration requires `target`",
                self.span(start, end),
            );
        }
        if runtime.is_none() {
            self.error(
                "PARSE013",
                "host package declaration requires `runtime`",
                self.span(start, end),
            );
        }

        Stmt {
            kind: StmtKind::HostPackageDecl(HostPackageDecl {
                target: target.unwrap_or_default(),
                runtime: runtime.unwrap_or_default(),
            }),
            span: self.span(start, end),
        }
    }

    fn parse_enum_decl_stmt(&mut self) -> Stmt {
        self.parse_enum_decl_stmt_with_runtime(false)
    }

    fn parse_enum_decl_stmt_with_runtime(&mut self, forced_runtime: bool) -> Stmt {
        let start = self
            .eat_contextual_keyword("enum")
            .expect("parse_enum_decl_stmt called at enum")
            .byte_start;
        let name = self.expect_identifier();
        let repr = if self.eat_contextual_keyword("repr").is_some() {
            self.parse_enum_repr()
        } else {
            EnumRepr::String
        };
        let runtime = forced_runtime || self.eat_contextual_keyword("runtime").is_some();

        self.expect(&TokenKind::LBrace);
        let mut variants = Vec::new();
        while !self.at(&TokenKind::RBrace) && !self.at(&TokenKind::Eof) {
            if self.eat(&TokenKind::Semicolon).is_some() || self.eat(&TokenKind::Comma).is_some() {
                continue;
            }
            variants.push(self.parse_enum_variant());
            if self.eat(&TokenKind::Comma).is_none() && self.eat(&TokenKind::Semicolon).is_none() {
                break;
            }
        }
        let end = self.expect(&TokenKind::RBrace).byte_end;
        Stmt {
            kind: StmtKind::EnumDecl(EnumDecl {
                name,
                repr,
                runtime,
                variants,
            }),
            span: self.span(start, end),
        }
    }

    fn parse_enum_repr(&mut self) -> EnumRepr {
        let ident = self.expect_identifier();
        match ident.name.as_str() {
            "string" => EnumRepr::String,
            "number" => EnumRepr::Number,
            "table" => {
                let tag_field = self.parse_enum_repr_tag_options("kind");
                EnumRepr::Table { tag_field }
            }
            "existing" => {
                let tag_field = self.parse_enum_repr_tag_options("kind");
                EnumRepr::Existing { tag_field }
            }
            other => {
                self.error(
                    "PARSE022",
                    format!("unknown enum repr `{other}`"),
                    ident.span,
                );
                EnumRepr::String
            }
        }
    }

    fn parse_enum_repr_tag_options(&mut self, default_tag_field: &str) -> String {
        let mut tag_field = default_tag_field.to_string();
        if self.eat(&TokenKind::LParen).is_some() {
            while !self.at(&TokenKind::RParen) && !self.at(&TokenKind::Eof) {
                let key = self.expect_identifier();
                self.expect(&TokenKind::Eq);
                match key.name.as_str() {
                    "kind" | "tag" | "tag_field" => {
                        tag_field = self.expect_string();
                    }
                    other => {
                        self.error(
                            "PARSE021",
                            format!("unknown enum repr option `{other}`"),
                            key.span,
                        );
                        if !self.is_stmt_boundary() {
                            self.bump();
                        }
                    }
                }
                if self.eat(&TokenKind::Comma).is_none()
                    && self.eat(&TokenKind::Semicolon).is_none()
                {
                    break;
                }
            }
            self.expect(&TokenKind::RParen);
        }
        tag_field
    }

    fn parse_enum_variant(&mut self) -> EnumVariant {
        let start = self.current_span().byte_start;
        let name = self.expect_identifier();
        let mut tag = None;
        let payload = if self.eat(&TokenKind::LParen).is_some() {
            let (fields, payload_tag) = self.parse_enum_payload_fields_until(TokenKind::RParen);
            tag = payload_tag;
            self.expect(&TokenKind::RParen);
            EnumVariantPayload::Tuple(fields)
        } else if self.eat(&TokenKind::LBrace).is_some() {
            let (fields, payload_tag) = self.parse_enum_payload_fields_until(TokenKind::RBrace);
            tag = payload_tag;
            self.expect(&TokenKind::RBrace);
            EnumVariantPayload::Record(fields)
        } else {
            EnumVariantPayload::None
        };
        if self.eat(&TokenKind::Eq).is_some() {
            tag = Some(self.parse_expr(0));
        }
        let end = tag
            .as_ref()
            .map(|expr| expr.span.byte_end)
            .unwrap_or_else(|| self.previous_span().byte_end.max(name.span.byte_end));
        EnumVariant {
            name,
            payload,
            tag,
            span: self.span(start, end),
        }
    }

    fn parse_enum_payload_fields_until(
        &mut self,
        stop: TokenKind,
    ) -> (Vec<Identifier>, Option<Expr>) {
        let mut fields = Vec::new();
        let mut tag = None;
        while !self.at(&stop) && !self.at(&TokenKind::Eof) {
            let name = self.expect_identifier();
            if self.eat(&TokenKind::Eq).is_some() {
                let value = self.parse_expr(0);
                if matches!(name.name.as_str(), "tag" | "kind" | "__tag") {
                    tag = Some(value);
                } else {
                    fields.push(name);
                }
            } else {
                if self.eat(&TokenKind::Colon).is_some() {
                    self.skip_type_annotation(&stop);
                }
                fields.push(name);
            }
            if self.eat(&TokenKind::Comma).is_none() && self.eat(&TokenKind::Semicolon).is_none() {
                break;
            }
        }
        (fields, tag)
    }

    fn skip_type_annotation(&mut self, stop: &TokenKind) {
        let mut depth = 0usize;
        while !self.at(&TokenKind::Eof) {
            if depth == 0
                && (self.at(stop) || self.at(&TokenKind::Comma) || self.at(&TokenKind::Semicolon))
            {
                break;
            }
            match self.current().kind {
                TokenKind::LParen | TokenKind::LBracket | TokenKind::LBrace => depth += 1,
                TokenKind::RParen | TokenKind::RBracket | TokenKind::RBrace => {
                    if depth == 0 {
                        break;
                    }
                    depth -= 1;
                }
                _ => {}
            }
            self.bump();
        }
    }

    fn parse_fn_decl_stmt(&mut self) -> Stmt {
        let start = self.expect(&TokenKind::KwFn).byte_start;
        let name = self.parse_function_name();
        let (params, vararg) = self.parse_param_list();
        let body = self.parse_function_body();
        let end = match &body {
            FunctionBody::Expr(expr) => expr.span.byte_end,
            FunctionBody::Block(block) => block.span.byte_end,
        };
        Stmt {
            kind: StmtKind::FunctionDecl(FunctionDecl {
                name,
                params,
                vararg,
                body,
            }),
            span: self.span(start, end),
        }
    }

    fn parse_lua_function_decl_stmt(&mut self) -> Stmt {
        let start = self.expect(&TokenKind::KwFunction).byte_start;
        let name = self.parse_function_name();
        let (params, vararg) = self.parse_param_list();
        let body = self.parse_lua_function_body(start);
        let end = function_body_end(&body);
        Stmt {
            kind: StmtKind::FunctionDecl(FunctionDecl {
                name,
                params,
                vararg,
                body,
            }),
            span: self.span(start, end),
        }
    }

    fn parse_local_function_decl_stmt(&mut self, local_start: usize) -> Stmt {
        self.expect(&TokenKind::KwFunction);
        let name = self.expect_identifier();
        let (params, vararg) = self.parse_param_list();
        let body = self.parse_lua_function_body(local_start);
        let end = function_body_end(&body);
        Stmt {
            kind: StmtKind::FunctionDecl(FunctionDecl {
                name: FunctionName::Simple(name),
                params,
                vararg,
                body,
            }),
            span: self.span(local_start, end),
        }
    }

    fn parse_if_stmt(&mut self) -> Stmt {
        let start = self.expect(&TokenKind::KwIf).byte_start;
        let condition = self.parse_control_condition();
        self.parse_if_tail(start, condition, false, true)
    }

    fn parse_elseif_stmt(&mut self) -> Stmt {
        let start = self.expect(&TokenKind::KwElseIf).byte_start;
        let condition = self.parse_control_condition();
        self.parse_if_tail(start, condition, true, false)
    }

    fn parse_if_tail(
        &mut self,
        start: usize,
        condition: Expr,
        force_lua_block: bool,
        consume_lua_end: bool,
    ) -> Stmt {
        let lua_block = if force_lua_block {
            self.expect(&TokenKind::KwThen);
            true
        } else {
            self.eat(&TokenKind::KwThen).is_some()
        };
        let then_block = if lua_block {
            self.parse_lua_block_until(&[TokenKind::KwElseIf, TokenKind::KwElse, TokenKind::KwEnd])
        } else {
            self.parse_block()
        };
        let else_block = if self.at(&TokenKind::KwElseIf) {
            let nested = self.parse_elseif_stmt();
            let span = nested.span;
            Some(Block {
                statements: vec![nested],
                tail: None,
                span,
            })
        } else if self.eat(&TokenKind::KwElse).is_some() {
            if !lua_block && self.at(&TokenKind::KwIf) && self.looks_like_if_stmt() {
                let nested = self.parse_if_stmt();
                let span = nested.span;
                Some(Block {
                    statements: vec![nested],
                    tail: None,
                    span,
                })
            } else if lua_block {
                Some(self.parse_lua_block_until(&[TokenKind::KwEnd]))
            } else {
                Some(self.parse_block())
            }
        } else {
            None
        };

        let mut end = else_block
            .as_ref()
            .map(|block| block.span.byte_end)
            .unwrap_or(then_block.span.byte_end);
        if lua_block && consume_lua_end {
            end = self.expect(&TokenKind::KwEnd).byte_end;
        }
        Stmt {
            kind: StmtKind::If {
                condition,
                then_block,
                else_block,
            },
            span: self.span(start, end),
        }
    }

    fn parse_while_stmt(&mut self) -> Stmt {
        let start = self.expect(&TokenKind::KwWhile).byte_start;
        let condition = self.parse_control_condition();
        let (body, end) = if self.eat(&TokenKind::KwDo).is_some() {
            let body = self.parse_lua_block_until(&[TokenKind::KwEnd]);
            let end = self.expect(&TokenKind::KwEnd).byte_end;
            (body, end)
        } else {
            let body = self.parse_block();
            let end = body.span.byte_end;
            (body, end)
        };
        Stmt {
            kind: StmtKind::While { condition, body },
            span: self.span(start, end),
        }
    }

    fn parse_for_stmt(&mut self) -> Stmt {
        let start = self.expect(&TokenKind::KwFor).byte_start;
        let first = self.expect_identifier();

        if self.eat(&TokenKind::Eq).is_some() {
            let start_expr = self.parse_control_condition();
            self.expect(&TokenKind::Comma);
            let end_expr = self.parse_control_condition();
            let step = if self.eat(&TokenKind::Comma).is_some() {
                Some(self.parse_control_condition())
            } else {
                None
            };
            let (body, end) = self.parse_for_body();
            Stmt {
                kind: StmtKind::NumericFor {
                    name: first,
                    start: start_expr,
                    end: end_expr,
                    step,
                    body,
                },
                span: self.span(start, end),
            }
        } else {
            let mut names = vec![first];
            while self.eat(&TokenKind::Comma).is_some() {
                names.push(self.expect_identifier());
            }
            self.expect(&TokenKind::KwIn);
            let iter = self.parse_control_expr_list();
            let (body, end) = self.parse_for_body();
            Stmt {
                kind: StmtKind::GenericFor { names, iter, body },
                span: self.span(start, end),
            }
        }
    }

    fn parse_repeat_stmt(&mut self) -> Stmt {
        let start = self.expect(&TokenKind::KwRepeat).byte_start;
        let body = if self.at(&TokenKind::LBrace) {
            self.parse_block()
        } else {
            self.parse_lua_block_until(&[TokenKind::KwUntil])
        };
        self.expect(&TokenKind::KwUntil);
        let condition = self.parse_expr(0);
        let end = condition.span.byte_end;
        Stmt {
            kind: StmtKind::RepeatUntil { body, condition },
            span: self.span(start, end),
        }
    }

    fn parse_do_stmt(&mut self) -> Stmt {
        let start = self.expect(&TokenKind::KwDo).byte_start;
        let (block, end) = if self.at(&TokenKind::LBrace) {
            let block = self.parse_block();
            let end = block.span.byte_end;
            (block, end)
        } else {
            let block = self.parse_lua_block_until(&[TokenKind::KwEnd]);
            let end = self.expect(&TokenKind::KwEnd).byte_end;
            (block, end)
        };
        Stmt {
            kind: StmtKind::Do(block),
            span: self.span(start, end),
        }
    }

    fn parse_assignment_or_expr_stmt(&mut self) -> Stmt {
        let start = self.current_span().byte_start;
        let first = self.parse_expr(0);

        let mut targets = vec![first];
        let mut has_target_list = false;
        while self.eat(&TokenKind::Comma).is_some() {
            has_target_list = true;
            targets.push(self.parse_expr(0));
        }

        if has_target_list {
            self.expect(&TokenKind::Eq);
            let values = self.parse_expr_list();
            let end = values
                .last()
                .map(|expr| expr.span.byte_end)
                .or_else(|| targets.last().map(|expr| expr.span.byte_end))
                .unwrap_or(start);
            return Stmt {
                kind: StmtKind::Assign { targets, values },
                span: self.span(start, end),
            };
        }

        let first = targets.remove(0);
        if let Some(op) = self.eat_compound_assign() {
            let value = self.parse_expr(0);
            let end = value.span.byte_end;
            return Stmt {
                kind: StmtKind::CompoundAssign {
                    target: first,
                    op,
                    value,
                },
                span: self.span(start, end),
            };
        }

        if self.eat(&TokenKind::Eq).is_some() {
            let values = self.parse_expr_list();
            let end = values
                .last()
                .map(|expr| expr.span.byte_end)
                .unwrap_or(first.span.byte_end);
            return Stmt {
                kind: StmtKind::Assign {
                    targets: vec![first],
                    values,
                },
                span: self.span(start, end),
            };
        }

        let end = first.span.byte_end;
        Stmt {
            kind: StmtKind::Expr(first),
            span: self.span(start, end),
        }
    }

    fn parse_block(&mut self) -> Block {
        let start = self.expect(&TokenKind::LBrace).byte_start;
        let mut statements = Vec::new();
        let mut tail = None;

        while !self.at(&TokenKind::RBrace) && !self.at(&TokenKind::Eof) {
            if self.eat(&TokenKind::Semicolon).is_some() {
                continue;
            }

            let diagnostics_before = self.diagnostics.len();
            let stmt = self.parse_stmt();
            if self.diagnostics.len() > diagnostics_before {
                self.recover_after_stmt();
                statements.push(stmt);
                continue;
            }
            if self.eat(&TokenKind::Semicolon).is_some() {
                statements.push(stmt);
                continue;
            }

            if self.at(&TokenKind::RBrace) {
                match stmt_into_tail_expr(stmt) {
                    Ok(expr) => tail = Some(expr),
                    Err(stmt) => statements.push(stmt),
                }
                break;
            }
            statements.push(stmt);
        }

        let end = self.expect(&TokenKind::RBrace).byte_end;
        Block {
            statements,
            tail,
            span: self.span(start, end),
        }
    }

    fn parse_lua_block_until(&mut self, stops: &[TokenKind]) -> Block {
        let start = self.current_span().byte_start;
        let mut statements = Vec::new();

        while !self.at_any(stops) && !self.at(&TokenKind::Eof) {
            if self.eat(&TokenKind::Semicolon).is_some() {
                continue;
            }

            let diagnostics_before = self.diagnostics.len();
            let stmt = self.parse_stmt();
            if self.diagnostics.len() > diagnostics_before {
                self.recover_after_stmt();
            }
            statements.push(stmt);
            let _ = self.eat(&TokenKind::Semicolon);
        }

        let end = statements
            .last()
            .map(|stmt| stmt.span.byte_end)
            .unwrap_or(start);
        Block {
            statements,
            tail: None,
            span: self.span(start, end),
        }
    }

    fn parse_for_body(&mut self) -> (Block, usize) {
        if self.eat(&TokenKind::KwDo).is_some() {
            let body = self.parse_lua_block_until(&[TokenKind::KwEnd]);
            let end = self.expect(&TokenKind::KwEnd).byte_end;
            (body, end)
        } else {
            let body = self.parse_block();
            let end = body.span.byte_end;
            (body, end)
        }
    }

    fn parse_function_body(&mut self) -> FunctionBody {
        if self.eat(&TokenKind::Eq).is_some() {
            FunctionBody::Expr(Box::new(self.parse_expr(0)))
        } else {
            FunctionBody::Block(Box::new(self.parse_block()))
        }
    }

    fn parse_lua_function_body(&mut self, start: usize) -> FunctionBody {
        let block = self.parse_lua_block_until(&[TokenKind::KwEnd]);
        let end = self.expect(&TokenKind::KwEnd).byte_end;
        FunctionBody::Block(Box::new(Block {
            statements: block.statements,
            tail: None,
            span: self.span(start, end),
        }))
    }

    fn parse_function_name(&mut self) -> FunctionName {
        let first = self.expect_identifier();
        let mut path = vec![first];
        while self.eat(&TokenKind::Dot).is_some() {
            path.push(self.expect_identifier());
        }
        if self.eat(&TokenKind::Colon).is_some() {
            let method = self.expect_identifier();
            FunctionName::Method {
                receiver: path,
                method,
            }
        } else if path.len() == 1 {
            FunctionName::Simple(path.remove(0))
        } else {
            FunctionName::Dotted(path)
        }
    }

    fn parse_pattern(&mut self) -> Pattern {
        match self.current().kind {
            TokenKind::LBrace => self.parse_object_pattern(),
            TokenKind::LBracket => self.parse_array_pattern(),
            _ => {
                let ident = self.expect_identifier();
                let span = ident.span;
                Pattern {
                    kind: PatternKind::Identifier(ident),
                    span,
                }
            }
        }
    }

    fn parse_object_pattern(&mut self) -> Pattern {
        let start = self.expect(&TokenKind::LBrace).byte_start;
        let mut fields = Vec::new();

        while !self.at(&TokenKind::RBrace) && !self.at(&TokenKind::Eof) {
            let field_start = self.current_span().byte_start;
            let key = self.expect_identifier();
            let pattern = if self.eat(&TokenKind::Colon).is_some() {
                self.parse_pattern()
            } else {
                Pattern {
                    kind: PatternKind::Identifier(key.clone()),
                    span: key.span,
                }
            };
            let default = if self.eat(&TokenKind::Eq).is_some() {
                Some(self.parse_expr(0))
            } else {
                None
            };
            let end = default
                .as_ref()
                .map(|expr| expr.span.byte_end)
                .unwrap_or(pattern.span.byte_end);
            fields.push(ObjectPatternField {
                key,
                pattern,
                default,
                span: self.span(field_start, end),
            });

            if self.eat(&TokenKind::Comma).is_none() && self.eat(&TokenKind::Semicolon).is_none() {
                break;
            }
        }

        let end = self.expect(&TokenKind::RBrace).byte_end;
        Pattern {
            kind: PatternKind::Object(fields),
            span: self.span(start, end),
        }
    }

    fn parse_array_pattern(&mut self) -> Pattern {
        let start = self.expect(&TokenKind::LBracket).byte_start;
        let mut items = Vec::new();

        while !self.at(&TokenKind::RBracket) && !self.at(&TokenKind::Eof) {
            let item_start = self.current_span().byte_start;
            let pattern = self.parse_pattern();
            let default = if self.eat(&TokenKind::Eq).is_some() {
                Some(self.parse_expr(0))
            } else {
                None
            };
            let end = default
                .as_ref()
                .map(|expr| expr.span.byte_end)
                .unwrap_or(pattern.span.byte_end);
            items.push(ArrayPatternItem {
                pattern,
                default,
                span: self.span(item_start, end),
            });

            if self.eat(&TokenKind::Comma).is_none() && self.eat(&TokenKind::Semicolon).is_none() {
                break;
            }
        }

        let end = self.expect(&TokenKind::RBracket).byte_end;
        Pattern {
            kind: PatternKind::Array(items),
            span: self.span(start, end),
        }
    }

    fn parse_param_list(&mut self) -> (Vec<Param>, bool) {
        self.expect(&TokenKind::LParen);
        let mut params = Vec::new();
        let mut vararg = false;

        while !self.at(&TokenKind::RParen) && !self.at(&TokenKind::Eof) {
            if self.eat(&TokenKind::Ellipsis).is_some() {
                vararg = true;
                break;
            }
            params.push(self.parse_param());
            if self.eat(&TokenKind::Comma).is_none() {
                break;
            }
        }

        self.expect(&TokenKind::RParen);
        (params, vararg)
    }

    fn parse_param(&mut self) -> Param {
        let name = self.expect_identifier();
        let default = if self.eat(&TokenKind::Eq).is_some() {
            Some(self.parse_expr(0))
        } else {
            None
        };
        let end = default
            .as_ref()
            .map(|expr| expr.span.byte_end)
            .unwrap_or(name.span.byte_end);
        Param {
            span: self.span(name.span.byte_start, end),
            name,
            default,
        }
    }

    fn parse_expr_list(&mut self) -> Vec<Expr> {
        self.parse_expr_list_with_options(ExprParseOptions::default())
    }

    fn parse_expr_list_with_options(&mut self, options: ExprParseOptions) -> Vec<Expr> {
        let mut exprs = vec![self.parse_expr_with_options(0, options)];
        while self.eat(&TokenKind::Comma).is_some() {
            exprs.push(self.parse_expr_with_options(0, options));
        }
        exprs
    }

    fn parse_control_expr_list(&mut self) -> Vec<Expr> {
        let mut exprs = vec![self.parse_control_condition()];
        while self.eat(&TokenKind::Comma).is_some() {
            exprs.push(self.parse_control_condition());
        }
        exprs
    }

    fn parse_expr(&mut self, min_bp: u8) -> Expr {
        self.parse_expr_with_options(min_bp, ExprParseOptions::default())
    }

    fn parse_control_condition(&mut self) -> Expr {
        self.parse_expr_with_options(
            0,
            ExprParseOptions {
                allow_tail_table_call: false,
                allow_pipeline_placeholder: false,
                allow_then_else: false,
            },
        )
    }

    fn parse_expr_with_options(&mut self, min_bp: u8, options: ExprParseOptions) -> Expr {
        let mut lhs = self.parse_prefix_expr(options);

        lhs = self.parse_postfix_chain(lhs, options);

        loop {
            let Some((op, left_bp, right_bp)) = self.current_binary_op() else {
                break;
            };
            if left_bp < min_bp {
                break;
            }

            self.bump();
            let rhs_options = if op == BinaryOp::Pipe {
                ExprParseOptions {
                    allow_pipeline_placeholder: true,
                    ..options
                }
            } else {
                options
            };
            let rhs = self.parse_expr_with_options(right_bp, rhs_options);
            let start = lhs.span.byte_start;
            let end = rhs.span.byte_end;
            self.validate_coalesce_comparison_mix(op, &lhs, &rhs, self.span(start, end));
            if op == BinaryOp::Pipe && !contains_pipeline_placeholder(&rhs) {
                self.error_with_help(
                    "PARSE014",
                    "pipeline RHS must contain `%`",
                    rhs.span,
                    "write the insertion point explicitly, for example `value |> f(%)` or `value |> clamp(0, %, 100)`",
                );
            }
            lhs = Expr {
                kind: ExprKind::Binary {
                    op,
                    left: Box::new(lhs),
                    right: Box::new(rhs),
                },
                span: self.span(start, end),
            };
        }

        if min_bp <= 1 && options.allow_then_else && self.eat(&TokenKind::KwThen).is_some() {
            let then_branch = ExprOrBlock::Expr(Box::new(self.parse_expr(0)));
            self.expect(&TokenKind::KwElse);
            let else_branch = ExprOrBlock::Expr(Box::new(self.parse_expr(1)));
            let start = lhs.span.byte_start;
            let end = branch_end(&else_branch);
            return Expr {
                kind: ExprKind::Conditional {
                    condition: Box::new(lhs),
                    then_branch,
                    else_branch,
                    form: ConditionalForm::ThenElse,
                },
                span: self.span(start, end),
            };
        }

        lhs
    }

    fn parse_prefix_expr(&mut self, options: ExprParseOptions) -> Expr {
        let token = self.current().clone();
        match token.kind {
            TokenKind::KwNil => {
                self.bump();
                Expr {
                    kind: ExprKind::Nil,
                    span: token.span,
                }
            }
            TokenKind::KwTrue => {
                self.bump();
                Expr {
                    kind: ExprKind::Boolean(true),
                    span: token.span,
                }
            }
            TokenKind::KwFalse => {
                self.bump();
                Expr {
                    kind: ExprKind::Boolean(false),
                    span: token.span,
                }
            }
            TokenKind::Identifier(name) if name == "match" => self.parse_match_expr(),
            TokenKind::Identifier(name) => {
                self.bump();
                Expr {
                    kind: ExprKind::Identifier(Identifier {
                        name,
                        span: token.span,
                    }),
                    span: token.span,
                }
            }
            TokenKind::Number(value) => {
                self.bump();
                Expr {
                    kind: ExprKind::Number(value),
                    span: token.span,
                }
            }
            TokenKind::String(value) => {
                self.bump();
                Expr {
                    kind: ExprKind::String(value),
                    span: token.span,
                }
            }
            TokenKind::Ellipsis => {
                self.bump();
                Expr {
                    kind: ExprKind::Vararg,
                    span: token.span,
                }
            }
            TokenKind::Percent if options.allow_pipeline_placeholder => {
                self.bump();
                Expr {
                    kind: ExprKind::PipelinePlaceholder,
                    span: token.span,
                }
            }
            TokenKind::Percent => {
                self.error(
                    "PARSE015",
                    "`%` pipeline placeholder is only valid on the right side of `|>`",
                    token.span,
                );
                self.bump();
                Expr {
                    kind: ExprKind::Nil,
                    span: token.span,
                }
            }
            TokenKind::KwNot | TokenKind::Hash | TokenKind::Minus => {
                self.bump();
                let op = match token.kind {
                    TokenKind::KwNot => UnaryOp::Not,
                    TokenKind::Hash => UnaryOp::Len,
                    TokenKind::Minus => UnaryOp::Neg,
                    _ => unreachable!(),
                };
                let argument = self.parse_expr_with_options(9, options);
                let span = self.span(token.span.byte_start, argument.span.byte_end);
                Expr {
                    kind: ExprKind::Unary {
                        op,
                        argument: Box::new(argument),
                    },
                    span,
                }
            }
            TokenKind::LParen => self.parse_paren_or_arrow_expr(options),
            TokenKind::LBrace => self.parse_table_expr(options),
            TokenKind::KwIf => self.parse_if_expr(),
            TokenKind::KwDo => self.parse_do_expr(),
            TokenKind::KwFunction => self.parse_lua_function_expr(),
            TokenKind::TemplateStringStart => self.parse_template_string(),
            _ => {
                let span = token.span;
                self.error("PARSE001", "expected expression", span);
                self.bump();
                Expr {
                    kind: ExprKind::Nil,
                    span,
                }
            }
        }
    }

    fn parse_match_expr(&mut self) -> Expr {
        let start = self
            .eat_contextual_keyword("match")
            .expect("parse_match_expr called at match")
            .byte_start;
        let subject = self.parse_control_condition();
        self.expect(&TokenKind::LBrace);
        let mut arms = Vec::new();

        while !self.at(&TokenKind::RBrace) && !self.at(&TokenKind::Eof) {
            if self.eat(&TokenKind::Semicolon).is_some() || self.eat(&TokenKind::Comma).is_some() {
                continue;
            }
            arms.push(self.parse_match_arm());
            if self.eat(&TokenKind::Comma).is_none() && self.eat(&TokenKind::Semicolon).is_none() {
                if self.at(&TokenKind::RBrace) {
                    break;
                }
            }
        }

        let end = self.expect(&TokenKind::RBrace).byte_end;
        Expr {
            kind: ExprKind::Match(MatchExpr {
                subject: Box::new(subject),
                arms,
            }),
            span: self.span(start, end),
        }
    }

    fn parse_match_arm(&mut self) -> MatchArm {
        let start = self.current_span().byte_start;
        let pattern = self.parse_match_pattern();
        self.expect(&TokenKind::ArrowNormal);
        let body = if self.at(&TokenKind::LBrace) {
            ExprOrBlock::Block(Box::new(self.parse_block()))
        } else {
            ExprOrBlock::Expr(Box::new(self.parse_expr(0)))
        };
        let end = branch_end(&body);
        MatchArm {
            pattern,
            body,
            span: self.span(start, end),
        }
    }

    fn parse_match_pattern(&mut self) -> MatchPattern {
        let first = self.parse_match_pattern_atom();
        if self.eat(&TokenKind::Pipe).is_none() {
            return first;
        }
        let start = first.span.byte_start;
        let mut patterns = vec![first];
        loop {
            patterns.push(self.parse_match_pattern_atom());
            if self.eat(&TokenKind::Pipe).is_none() {
                break;
            }
        }
        let end = patterns
            .last()
            .map(|pattern| pattern.span.byte_end)
            .unwrap_or(start);
        MatchPattern {
            kind: MatchPatternKind::Or(patterns),
            span: self.span(start, end),
        }
    }

    fn parse_match_pattern_atom(&mut self) -> MatchPattern {
        let token = self.current().clone();
        match token.kind {
            TokenKind::Identifier(name) if name == "_" => {
                self.bump();
                MatchPattern {
                    kind: MatchPatternKind::Wildcard,
                    span: token.span,
                }
            }
            TokenKind::Identifier(_) => self.parse_identifier_match_pattern(),
            TokenKind::KwNil => {
                self.bump();
                MatchPattern {
                    kind: MatchPatternKind::Literal(MatchLiteral::Nil),
                    span: token.span,
                }
            }
            TokenKind::KwTrue => {
                self.bump();
                MatchPattern {
                    kind: MatchPatternKind::Literal(MatchLiteral::Boolean(true)),
                    span: token.span,
                }
            }
            TokenKind::KwFalse => {
                self.bump();
                MatchPattern {
                    kind: MatchPatternKind::Literal(MatchLiteral::Boolean(false)),
                    span: token.span,
                }
            }
            TokenKind::Number(value) => {
                self.bump();
                MatchPattern {
                    kind: MatchPatternKind::Literal(MatchLiteral::Number(value)),
                    span: token.span,
                }
            }
            TokenKind::String(value) => {
                self.bump();
                MatchPattern {
                    kind: MatchPatternKind::Literal(MatchLiteral::String(value)),
                    span: token.span,
                }
            }
            TokenKind::LBrace => self.parse_object_match_pattern(),
            TokenKind::LBracket => self.parse_array_match_pattern(),
            _ => {
                self.error("PARSE023", "expected match pattern", token.span);
                self.bump();
                MatchPattern {
                    kind: MatchPatternKind::Wildcard,
                    span: token.span,
                }
            }
        }
    }

    fn parse_identifier_match_pattern(&mut self) -> MatchPattern {
        let start = self.current_span().byte_start;
        let mut path = vec![self.expect_identifier()];
        while self.eat(&TokenKind::Dot).is_some() {
            path.push(self.expect_identifier());
        }
        let payload = if self.eat(&TokenKind::LParen).is_some() {
            let patterns = self.parse_match_pattern_list_until(TokenKind::RParen);
            self.expect(&TokenKind::RParen);
            Some(MatchPatternPayload::Tuple(patterns))
        } else if self.eat(&TokenKind::LBrace).is_some() {
            let fields = self.parse_match_object_fields(TokenKind::RBrace);
            self.expect(&TokenKind::RBrace);
            Some(MatchPatternPayload::Record(fields))
        } else {
            None
        };
        let end = self.previous_span().byte_end.max(
            path.last()
                .map(|ident| ident.span.byte_end)
                .unwrap_or(start),
        );
        let variant_like = path.len() > 1
            || payload.is_some()
            || path
                .first()
                .is_some_and(|ident| starts_with_uppercase(&ident.name));
        let kind = if variant_like {
            MatchPatternKind::Variant { path, payload }
        } else {
            MatchPatternKind::Binding(path.remove(0))
        };
        MatchPattern {
            kind,
            span: self.span(start, end),
        }
    }

    fn parse_object_match_pattern(&mut self) -> MatchPattern {
        let start = self.expect(&TokenKind::LBrace).byte_start;
        let fields = self.parse_match_object_fields(TokenKind::RBrace);
        let end = self.expect(&TokenKind::RBrace).byte_end;
        MatchPattern {
            kind: MatchPatternKind::Object(fields),
            span: self.span(start, end),
        }
    }

    fn parse_array_match_pattern(&mut self) -> MatchPattern {
        let start = self.expect(&TokenKind::LBracket).byte_start;
        let mut items = Vec::new();
        while !self.at(&TokenKind::RBracket) && !self.at(&TokenKind::Eof) {
            let item_start = self.current_span().byte_start;
            let pattern = self.parse_match_pattern();
            items.push(MatchArrayPatternItem {
                span: self.span(item_start, pattern.span.byte_end),
                pattern,
            });
            if self.eat(&TokenKind::Comma).is_none() && self.eat(&TokenKind::Semicolon).is_none() {
                break;
            }
        }
        let end = self.expect(&TokenKind::RBracket).byte_end;
        MatchPattern {
            kind: MatchPatternKind::Array(items),
            span: self.span(start, end),
        }
    }

    fn parse_match_object_fields(&mut self, stop: TokenKind) -> Vec<MatchObjectPatternField> {
        let mut fields = Vec::new();
        while !self.at(&stop) && !self.at(&TokenKind::Eof) {
            let field_start = self.current_span().byte_start;
            let key = self.expect_identifier();
            let pattern =
                if self.eat(&TokenKind::Colon).is_some() || self.eat(&TokenKind::Eq).is_some() {
                    self.parse_match_pattern()
                } else {
                    MatchPattern {
                        kind: MatchPatternKind::Binding(key.clone()),
                        span: key.span,
                    }
                };
            fields.push(MatchObjectPatternField {
                span: self.span(field_start, pattern.span.byte_end),
                key,
                pattern,
            });
            if self.eat(&TokenKind::Comma).is_none() && self.eat(&TokenKind::Semicolon).is_none() {
                break;
            }
        }
        fields
    }

    fn parse_match_pattern_list_until(&mut self, stop: TokenKind) -> Vec<MatchPattern> {
        let mut patterns = Vec::new();
        while !self.at(&stop) && !self.at(&TokenKind::Eof) {
            patterns.push(self.parse_match_pattern());
            if self.eat(&TokenKind::Comma).is_none() && self.eat(&TokenKind::Semicolon).is_none() {
                break;
            }
        }
        patterns
    }

    fn parse_lua_function_expr(&mut self) -> Expr {
        let start = self.expect(&TokenKind::KwFunction).byte_start;
        let (params, vararg) = self.parse_param_list();
        let body = self.parse_lua_function_body(start);
        let end = function_body_end(&body);
        Expr {
            kind: ExprKind::Function(FunctionExpr {
                params,
                vararg,
                body,
                arrow_kind: ArrowKind::Normal,
            }),
            span: self.span(start, end),
        }
    }

    fn parse_paren_or_arrow_expr(&mut self, options: ExprParseOptions) -> Expr {
        let start = self.expect(&TokenKind::LParen).byte_start;
        let checkpoint = self.index;
        let params_result = self.try_parse_arrow_params();

        if let Some((params, vararg)) = params_result {
            if self.eat(&TokenKind::ArrowNormal).is_some() {
                let body = self.parse_arrow_body();
                let end = function_body_end(&body);
                return Expr {
                    kind: ExprKind::Function(FunctionExpr {
                        params,
                        vararg,
                        body,
                        arrow_kind: ArrowKind::Normal,
                    }),
                    span: self.span(start, end),
                };
            }
            if self.eat(&TokenKind::ArrowImplicitSelf).is_some() {
                let body = self.parse_arrow_body();
                let end = function_body_end(&body);
                return Expr {
                    kind: ExprKind::Function(FunctionExpr {
                        params,
                        vararg,
                        body,
                        arrow_kind: ArrowKind::ImplicitSelf,
                    }),
                    span: self.span(start, end),
                };
            }
        }

        self.index = checkpoint;
        let expr = self.parse_expr_with_options(0, options);
        let end = self.expect(&TokenKind::RParen).byte_end;
        Expr {
            kind: ExprKind::Paren(Box::new(expr)),
            span: self.span(start, end),
        }
    }

    fn try_parse_arrow_params(&mut self) -> Option<(Vec<Param>, bool)> {
        let mut params = Vec::new();
        let mut vararg = false;

        if self.eat(&TokenKind::RParen).is_some() {
            return Some((params, vararg));
        }

        loop {
            if self.eat(&TokenKind::Ellipsis).is_some() {
                vararg = true;
                break;
            }

            let TokenKind::Identifier(_) = self.current().kind else {
                return None;
            };
            params.push(self.parse_param());

            if self.eat(&TokenKind::Comma).is_none() {
                break;
            }
        }

        if self.eat(&TokenKind::RParen).is_none() {
            return None;
        }

        Some((params, vararg))
    }

    fn parse_arrow_body(&mut self) -> FunctionBody {
        if self.at(&TokenKind::LBrace) {
            FunctionBody::Block(Box::new(self.parse_block()))
        } else {
            FunctionBody::Expr(Box::new(self.parse_expr(0)))
        }
    }

    fn parse_if_expr(&mut self) -> Expr {
        let start = self.expect(&TokenKind::KwIf).byte_start;
        let condition = self.parse_control_condition();
        let then_branch = ExprOrBlock::Block(Box::new(self.parse_block()));
        self.expect(&TokenKind::KwElse);
        let else_branch = if self.at(&TokenKind::LBrace) {
            ExprOrBlock::Block(Box::new(self.parse_block()))
        } else {
            ExprOrBlock::Expr(Box::new(self.parse_expr(0)))
        };
        let end = branch_end(&else_branch);
        Expr {
            kind: ExprKind::Conditional {
                condition: Box::new(condition),
                then_branch,
                else_branch,
                form: ConditionalForm::IfExpr,
            },
            span: self.span(start, end),
        }
    }

    fn parse_do_expr(&mut self) -> Expr {
        let start = self.expect(&TokenKind::KwDo).byte_start;
        let block = self.parse_block();
        let end = block.span.byte_end;
        Expr {
            kind: ExprKind::Do(Box::new(block)),
            span: self.span(start, end),
        }
    }

    fn parse_table_expr(&mut self, options: ExprParseOptions) -> Expr {
        let start = self.expect(&TokenKind::LBrace).byte_start;
        let mut fields = Vec::new();

        while !self.at(&TokenKind::RBrace) && !self.at(&TokenKind::Eof) {
            let field_start = self.current_span().byte_start;
            let field = if self.at(&TokenKind::Ellipsis) && self.looks_like_table_spread() {
                self.expect(&TokenKind::Ellipsis);
                let value = self.parse_expr_with_options(0, options);
                let span = self.span(field_start, value.span.byte_end);
                TableField {
                    kind: TableFieldKind::Spread(value),
                    span,
                }
            } else if self.eat(&TokenKind::LBracket).is_some() {
                let key = self.parse_expr_with_options(0, options);
                self.expect(&TokenKind::RBracket);
                self.expect(&TokenKind::Eq);
                let value = self.parse_expr_with_options(0, options);
                let span = self.span(field_start, value.span.byte_end);
                TableField {
                    kind: TableFieldKind::ExprKey { key, value },
                    span,
                }
            } else if self.looks_like_named_table_field() {
                let name = self.expect_identifier();
                self.expect(&TokenKind::Eq);
                let value = self.parse_expr_with_options(0, options);
                let span = self.span(field_start, value.span.byte_end);
                TableField {
                    kind: TableFieldKind::Named { name, value },
                    span,
                }
            } else {
                let value = self.parse_expr_with_options(0, options);
                let span = self.span(field_start, value.span.byte_end);
                TableField {
                    kind: TableFieldKind::Array(value),
                    span,
                }
            };
            fields.push(field);
            if self.eat(&TokenKind::Comma).is_none() && self.eat(&TokenKind::Semicolon).is_none() {
                break;
            }
        }

        let end = self.expect(&TokenKind::RBrace).byte_end;
        Expr {
            kind: ExprKind::Table(TableExpr { fields }),
            span: self.span(start, end),
        }
    }

    fn parse_template_string(&mut self) -> Expr {
        let start = self.expect(&TokenKind::TemplateStringStart).byte_start;
        let mut parts = Vec::new();

        while !self.at(&TokenKind::TemplateStringEnd) && !self.at(&TokenKind::Eof) {
            let token = self.current().clone();
            match token.kind {
                TokenKind::TemplateStringText(text) => {
                    self.bump();
                    parts.push(TemplatePart {
                        kind: TemplatePartKind::Text(text),
                        span: token.span,
                    });
                }
                TokenKind::TemplateExprStart => {
                    let expr_start = self.expect(&TokenKind::TemplateExprStart).byte_start;
                    let expr = self.parse_expr(0);
                    let end = self.expect(&TokenKind::TemplateExprEnd).byte_end;
                    parts.push(TemplatePart {
                        kind: TemplatePartKind::Expr(expr),
                        span: self.span(expr_start, end),
                    });
                }
                _ => {
                    self.error(
                        "PARSE002",
                        "expected template text or interpolation",
                        token.span,
                    );
                    self.bump();
                }
            }
        }

        let end = self.expect(&TokenKind::TemplateStringEnd).byte_end;
        Expr {
            kind: ExprKind::TemplateString(parts),
            span: self.span(start, end),
        }
    }

    fn parse_postfix_chain(&mut self, base: Expr, options: ExprParseOptions) -> Expr {
        let mut segments = Vec::new();

        loop {
            let start = self.current_span().byte_start;
            let segment = match self.current().kind.clone() {
                TokenKind::Dot => {
                    self.bump();
                    let name = self.expect_identifier();
                    Some(ChainSegment {
                        span: self.span(start, name.span.byte_end),
                        kind: ChainSegmentKind::Member {
                            name,
                            optional: false,
                        },
                    })
                }
                TokenKind::QuestionDot => {
                    self.bump();
                    if self.eat(&TokenKind::LBracket).is_some() {
                        let index = self.parse_expr(0);
                        let end = self.expect(&TokenKind::RBracket).byte_end;
                        Some(ChainSegment {
                            span: self.span(start, end),
                            kind: ChainSegmentKind::Index {
                                index,
                                optional: true,
                            },
                        })
                    } else {
                        let name = self.expect_identifier();
                        if self.at(&TokenKind::LParen) {
                            let (args, end, style) = self.parse_call_args(options);
                            Some(ChainSegment {
                                span: self.span(start, end),
                                kind: ChainSegmentKind::SafeDotCall { name, args, style },
                            })
                        } else {
                            Some(ChainSegment {
                                span: self.span(start, name.span.byte_end),
                                kind: ChainSegmentKind::Member {
                                    name,
                                    optional: true,
                                },
                            })
                        }
                    }
                }
                TokenKind::QuestionColon => {
                    self.bump();
                    let name = self.expect_identifier();
                    let (args, end, style) = if self.at(&TokenKind::LParen) {
                        self.parse_call_args(options)
                    } else {
                        self.error(
                            "PARSE003",
                            "safe method call requires argument parentheses",
                            name.span,
                        );
                        (Vec::new(), name.span.byte_end, CallStyle::Paren)
                    };
                    Some(ChainSegment {
                        span: self.span(start, end),
                        kind: ChainSegmentKind::MethodCall {
                            name,
                            args,
                            optional: true,
                            style,
                        },
                    })
                }
                TokenKind::Colon => {
                    self.bump();
                    let name = self.expect_identifier();
                    let (args, end, style) = if self.at(&TokenKind::LParen) {
                        self.parse_call_args(options)
                    } else {
                        self.error(
                            "PARSE004",
                            "method call requires argument parentheses",
                            name.span,
                        );
                        (Vec::new(), name.span.byte_end, CallStyle::Paren)
                    };
                    Some(ChainSegment {
                        span: self.span(start, end),
                        kind: ChainSegmentKind::MethodCall {
                            name,
                            args,
                            optional: false,
                            style,
                        },
                    })
                }
                TokenKind::LBracket => {
                    self.bump();
                    let index = self.parse_expr(0);
                    let end = self.expect(&TokenKind::RBracket).byte_end;
                    Some(ChainSegment {
                        span: self.span(start, end),
                        kind: ChainSegmentKind::Index {
                            index,
                            optional: false,
                        },
                    })
                }
                TokenKind::LParen => {
                    let (args, end, style) = self.parse_call_args(options);
                    Some(ChainSegment {
                        span: self.span(start, end),
                        kind: ChainSegmentKind::Call { args, style },
                    })
                }
                TokenKind::LBrace => {
                    if !options.allow_tail_table_call || self.current().leading_newline {
                        None
                    } else {
                        let table = self.parse_table_expr(options);
                        let end = table.span.byte_end;
                        Some(ChainSegment {
                            span: self.span(start, end),
                            kind: ChainSegmentKind::Call {
                                args: vec![table],
                                style: CallStyle::TailTable,
                            },
                        })
                    }
                }
                TokenKind::String(value) => {
                    if self.current().leading_newline {
                        None
                    } else {
                        self.bump();
                        Some(ChainSegment {
                            span: self.span(start, self.previous_span().byte_end),
                            kind: ChainSegmentKind::Call {
                                args: vec![Expr {
                                    kind: ExprKind::String(value),
                                    span: self.previous_span(),
                                }],
                                style: CallStyle::TailString,
                            },
                        })
                    }
                }
                _ => None,
            };

            if let Some(segment) = segment {
                segments.push(segment);
            } else {
                break;
            }
        }

        if segments.is_empty() {
            base
        } else {
            let start = base.span.byte_start;
            let end = segments
                .last()
                .map(|segment| segment.span.byte_end)
                .unwrap_or(base.span.byte_end);
            Expr {
                kind: ExprKind::Chain(ChainExpr {
                    base: Box::new(base),
                    segments,
                }),
                span: self.span(start, end),
            }
        }
    }

    fn parse_call_args(&mut self, options: ExprParseOptions) -> (Vec<Expr>, usize, CallStyle) {
        self.expect(&TokenKind::LParen);
        let mut args = Vec::new();
        if !self.at(&TokenKind::RParen) {
            args = self.parse_expr_list_with_options(options);
        }
        let end = self.expect(&TokenKind::RParen).byte_end;
        (args, end, CallStyle::Paren)
    }

    fn current_binary_op(&self) -> Option<(BinaryOp, u8, u8)> {
        match self.current().kind {
            TokenKind::PipeGt => Some((BinaryOp::Pipe, 1, 2)),
            TokenKind::KwOr => Some((BinaryOp::Or, 2, 3)),
            TokenKind::KwAnd => Some((BinaryOp::And, 3, 4)),
            TokenKind::EqEq => Some((BinaryOp::Eq, 4, 5)),
            TokenKind::NotEq => Some((BinaryOp::NotEq, 4, 5)),
            TokenKind::Lt => Some((BinaryOp::Lt, 4, 5)),
            TokenKind::LtEq => Some((BinaryOp::LtEq, 4, 5)),
            TokenKind::Gt => Some((BinaryOp::Gt, 4, 5)),
            TokenKind::GtEq => Some((BinaryOp::GtEq, 4, 5)),
            TokenKind::QuestionQuestion => Some((BinaryOp::Coalesce, 5, 5)),
            TokenKind::DotDot => Some((BinaryOp::Concat, 6, 6)),
            TokenKind::Plus => Some((BinaryOp::Add, 7, 8)),
            TokenKind::Minus => Some((BinaryOp::Sub, 7, 8)),
            TokenKind::Star => Some((BinaryOp::Mul, 8, 9)),
            TokenKind::Slash => Some((BinaryOp::Div, 8, 9)),
            TokenKind::Percent => Some((BinaryOp::Mod, 8, 9)),
            TokenKind::Caret => Some((BinaryOp::Pow, 10, 10)),
            _ => None,
        }
    }

    fn eat_compound_assign(&mut self) -> Option<CompoundAssignOp> {
        let op = match self.current().kind {
            TokenKind::PlusEq => CompoundAssignOp::Add,
            TokenKind::MinusEq => CompoundAssignOp::Sub,
            TokenKind::StarEq => CompoundAssignOp::Mul,
            TokenKind::SlashEq => CompoundAssignOp::Div,
            TokenKind::PercentEq => CompoundAssignOp::Mod,
            TokenKind::CaretEq => CompoundAssignOp::Pow,
            TokenKind::DotDotEq => CompoundAssignOp::Concat,
            _ => return None,
        };
        self.bump();
        Some(op)
    }

    fn looks_like_if_stmt(&self) -> bool {
        let mut depth = 0usize;
        let mut index = self.index + 1;
        while let Some(token) = self.tokens.get(index) {
            match token.kind {
                TokenKind::LParen | TokenKind::LBracket => depth += 1,
                TokenKind::RParen | TokenKind::RBracket => depth = depth.saturating_sub(1),
                TokenKind::LBrace | TokenKind::KwThen if depth == 0 => return true,
                TokenKind::Eof if depth == 0 => return false,
                _ => {}
            }
            index += 1;
        }
        false
    }

    fn looks_like_named_table_field(&self) -> bool {
        matches!(self.current().kind, TokenKind::Identifier(_))
            && matches!(self.peek_kind(1), Some(TokenKind::Eq))
    }

    fn looks_like_table_spread(&self) -> bool {
        !matches!(
            self.peek_kind(1),
            Some(TokenKind::Comma | TokenKind::Semicolon | TokenKind::RBrace)
        )
    }

    fn is_stmt_boundary(&self) -> bool {
        matches!(
            self.current().kind,
            TokenKind::Semicolon
                | TokenKind::RBrace
                | TokenKind::KwElse
                | TokenKind::KwElseIf
                | TokenKind::KwEnd
                | TokenKind::KwUntil
                | TokenKind::Eof
        )
    }

    fn at_stmt_start(&self) -> bool {
        matches!(
            self.current().kind,
            TokenKind::KwLocal
                | TokenKind::KwConst
                | TokenKind::KwReturn
                | TokenKind::KwBreak
                | TokenKind::KwImport
                | TokenKind::KwExport
                | TokenKind::KwFn
                | TokenKind::KwFunction
                | TokenKind::KwIf
                | TokenKind::KwWhile
                | TokenKind::KwFor
                | TokenKind::KwRepeat
                | TokenKind::KwDo
        ) || matches!(
            &self.current().kind,
            TokenKind::Identifier(name)
                if matches!(
                    name.as_str(),
                    "extern"
                        | "init"
                        | "enum"
                        | "continue"
                        | "stopif"
                        | "stopifn"
                        | "breakif"
                        | "breakifn"
                        | "continueif"
                        | "continueifn"
                ) || Realm::parse(name).is_some()
        )
    }

    fn at_any(&self, kinds: &[TokenKind]) -> bool {
        kinds.iter().any(|kind| self.at(kind))
    }

    fn expect_identifier(&mut self) -> Identifier {
        let token = self.current().clone();
        match token.kind {
            TokenKind::Identifier(name) => {
                self.bump();
                Identifier {
                    name,
                    span: token.span,
                }
            }
            _ => {
                self.error("PARSE005", "expected identifier", token.span);
                self.bump();
                Identifier {
                    name: "<error>".into(),
                    span: token.span,
                }
            }
        }
    }

    fn eat_contextual_keyword(&mut self, expected: &str) -> Option<SourceSpan> {
        let token = self.current().clone();
        match token.kind {
            TokenKind::Identifier(name) if name == expected => {
                self.bump();
                Some(token.span)
            }
            _ => None,
        }
    }

    fn is_contextual_keyword(&self, expected: &str) -> bool {
        matches!(&self.current().kind, TokenKind::Identifier(name) if name == expected)
    }

    fn eat_realm_keyword(&mut self) -> Option<Realm> {
        let token = self.current().clone();
        match token.kind {
            TokenKind::Identifier(name) => {
                let realm = Realm::parse(&name)?;
                self.bump();
                Some(realm)
            }
            _ => None,
        }
    }

    fn expect_contextual_keyword(&mut self, expected: &str) {
        let token = self.current().clone();
        match token.kind {
            TokenKind::Identifier(name) if name == expected => {
                self.bump();
            }
            _ => {
                self.error("PARSE006", format!("expected `{expected}`"), token.span);
                self.bump();
            }
        }
    }

    fn expect_string(&mut self) -> String {
        let token = self.current().clone();
        match token.kind {
            TokenKind::String(value) => {
                self.bump();
                value
            }
            _ => {
                self.error("PARSE007", "expected string literal", token.span);
                self.bump();
                String::new()
            }
        }
    }

    fn eat(&mut self, kind: &TokenKind) -> Option<SourceSpan> {
        if self.at(kind) {
            Some(self.bump().span)
        } else {
            None
        }
    }

    fn expect(&mut self, kind: &TokenKind) -> SourceSpan {
        if self.at(kind) {
            return self.bump().span;
        }
        let span = self.current_span();
        self.error("PARSE008", format!("expected {}", kind.name()), span);
        span
    }

    fn at(&self, kind: &TokenKind) -> bool {
        same_token_variant(&self.current().kind, kind)
    }

    fn current(&self) -> &Token {
        self.tokens
            .get(self.index)
            .or_else(|| self.tokens.last())
            .expect("parser requires at least EOF token")
    }

    fn current_span(&self) -> SourceSpan {
        self.current().span
    }

    fn previous_span(&self) -> SourceSpan {
        self.tokens
            .get(self.index.saturating_sub(1))
            .map(|token| token.span)
            .unwrap_or_else(|| self.current_span())
    }

    fn peek_kind(&self, offset: usize) -> Option<&TokenKind> {
        self.tokens
            .get(self.index + offset)
            .map(|token| &token.kind)
    }

    fn bump(&mut self) -> Token {
        let token = self.current().clone();
        if !same_token_variant(&token.kind, &TokenKind::Eof) {
            self.index += 1;
        }
        token
    }

    fn recover_after_stmt(&mut self) {
        if self.is_stmt_boundary() || self.at_stmt_start() {
            return;
        }

        while !self.at(&TokenKind::Eof)
            && !self.at(&TokenKind::Semicolon)
            && !self.at(&TokenKind::RBrace)
            && !self.at_stmt_start()
        {
            self.bump();
        }

        let _ = self.eat(&TokenKind::Semicolon);
    }

    fn error(&mut self, code: &str, message: impl Into<String>, span: SourceSpan) {
        self.diagnostics.push(
            Diagnostic::error(message)
                .with_code(code)
                .with_label(Label::primary(span, "here")),
        );
    }

    fn error_with_help(
        &mut self,
        code: &str,
        message: impl Into<String>,
        span: SourceSpan,
        help: impl Into<String>,
    ) {
        self.diagnostics.push(
            Diagnostic::error(message)
                .with_code(code)
                .with_label(Label::primary(span, "ambiguous expression"))
                .with_help(help),
        );
    }

    fn validate_coalesce_comparison_mix(
        &mut self,
        op: BinaryOp,
        lhs: &Expr,
        rhs: &Expr,
        span: SourceSpan,
    ) {
        let invalid = if op == BinaryOp::Coalesce {
            contains_unparenthesized_comparison(lhs) || contains_unparenthesized_comparison(rhs)
        } else if is_comparison_op(op) {
            contains_unparenthesized_coalesce(lhs) || contains_unparenthesized_coalesce(rhs)
        } else {
            false
        };

        if invalid {
            self.error_with_help(
                "PARSE009",
                "ambiguous use of `??` with a comparison operator",
                span,
                "add parentheses around the nil-fallback or the comparison, for example `(player?:GetExp() ?? 0) > 5`",
            );
        }
    }

    fn span(&self, start: usize, end: usize) -> SourceSpan {
        SourceSpan::new(self.file_id, start, end)
    }
}

fn same_token_variant(left: &TokenKind, right: &TokenKind) -> bool {
    std::mem::discriminant(left) == std::mem::discriminant(right)
}

fn function_body_end(body: &FunctionBody) -> usize {
    match body {
        FunctionBody::Expr(expr) => expr.span.byte_end,
        FunctionBody::Block(block) => block.span.byte_end,
    }
}

fn branch_end(branch: &ExprOrBlock) -> usize {
    match branch {
        ExprOrBlock::Expr(expr) => expr.span.byte_end,
        ExprOrBlock::Block(block) => block.span.byte_end,
    }
}

fn stmt_into_tail_expr(stmt: Stmt) -> Result<Expr, Stmt> {
    match stmt.kind {
        StmtKind::Expr(expr) => Ok(expr),
        StmtKind::If {
            condition,
            then_block,
            else_block: Some(else_block),
        } => {
            let span = stmt.span;
            Ok(Expr {
                kind: ExprKind::Conditional {
                    condition: Box::new(condition),
                    then_branch: ExprOrBlock::Block(Box::new(then_block)),
                    else_branch: block_into_conditional_branch(else_block),
                    form: ConditionalForm::IfExpr,
                },
                span,
            })
        }
        kind => Err(Stmt {
            kind,
            span: stmt.span,
        }),
    }
}

fn block_into_conditional_branch(block: Block) -> ExprOrBlock {
    if block.tail.is_none() && block.statements.len() == 1 {
        let span = block.span;
        let mut statements = block.statements;
        let stmt = statements
            .pop()
            .expect("block statement length was checked above");
        return match stmt_into_tail_expr(stmt) {
            Ok(expr) => ExprOrBlock::Expr(Box::new(expr)),
            Err(stmt) => ExprOrBlock::Block(Box::new(Block {
                statements: vec![stmt],
                tail: None,
                span,
            })),
        };
    }

    ExprOrBlock::Block(Box::new(block))
}

fn contains_unparenthesized_coalesce(expr: &Expr) -> bool {
    match &expr.kind {
        ExprKind::Paren(_) => false,
        ExprKind::Binary { op, left, right } => {
            *op == BinaryOp::Coalesce
                || contains_unparenthesized_coalesce(left)
                || contains_unparenthesized_coalesce(right)
        }
        _ => false,
    }
}

fn contains_pipeline_placeholder(expr: &Expr) -> bool {
    match &expr.kind {
        ExprKind::PipelinePlaceholder => true,
        ExprKind::Nil
        | ExprKind::Boolean(_)
        | ExprKind::Number(_)
        | ExprKind::String(_)
        | ExprKind::Vararg
        | ExprKind::Identifier(_) => false,
        ExprKind::TemplateString(parts) => parts.iter().any(|part| match &part.kind {
            TemplatePartKind::Text(_) => false,
            TemplatePartKind::Expr(expr) => contains_pipeline_placeholder(expr),
        }),
        ExprKind::Table(table) => table.fields.iter().any(|field| match &field.kind {
            TableFieldKind::Array(expr) | TableFieldKind::Spread(expr) => {
                contains_pipeline_placeholder(expr)
            }
            TableFieldKind::Named { value, .. } => contains_pipeline_placeholder(value),
            TableFieldKind::ExprKey { key, value } => {
                contains_pipeline_placeholder(key) || contains_pipeline_placeholder(value)
            }
        }),
        ExprKind::Paren(expr) => contains_pipeline_placeholder(expr),
        ExprKind::Unary { argument, .. } => contains_pipeline_placeholder(argument),
        ExprKind::Binary { left, right, .. } => {
            contains_pipeline_placeholder(left) || contains_pipeline_placeholder(right)
        }
        ExprKind::Conditional {
            condition,
            then_branch,
            else_branch,
            ..
        } => {
            contains_pipeline_placeholder(condition)
                || expr_or_block_contains_pipeline_placeholder(then_branch)
                || expr_or_block_contains_pipeline_placeholder(else_branch)
        }
        ExprKind::Match(match_expr) => {
            contains_pipeline_placeholder(&match_expr.subject)
                || match_expr
                    .arms
                    .iter()
                    .any(|arm| expr_or_block_contains_pipeline_placeholder(&arm.body))
        }
        ExprKind::Do(block) => block_contains_pipeline_placeholder(block),
        ExprKind::Function(_) => false,
        ExprKind::Chain(chain) => {
            contains_pipeline_placeholder(&chain.base)
                || chain.segments.iter().any(|segment| match &segment.kind {
                    ChainSegmentKind::Member { .. } => false,
                    ChainSegmentKind::Index { index, .. } => contains_pipeline_placeholder(index),
                    ChainSegmentKind::Call { args, .. }
                    | ChainSegmentKind::SafeDotCall { args, .. }
                    | ChainSegmentKind::MethodCall { args, .. } => {
                        args.iter().any(contains_pipeline_placeholder)
                    }
                })
        }
    }
}

fn expr_or_block_contains_pipeline_placeholder(item: &ExprOrBlock) -> bool {
    match item {
        ExprOrBlock::Expr(expr) => contains_pipeline_placeholder(expr),
        ExprOrBlock::Block(block) => block_contains_pipeline_placeholder(block),
    }
}

fn block_contains_pipeline_placeholder(block: &Block) -> bool {
    block
        .tail
        .as_ref()
        .is_some_and(contains_pipeline_placeholder)
        || block.statements.iter().any(|stmt| match &stmt.kind {
            StmtKind::Expr(expr) | StmtKind::CompoundAssign { value: expr, .. } => {
                contains_pipeline_placeholder(expr)
            }
            StmtKind::LocalDecl { values, .. }
            | StmtKind::LocalDestructure { values, .. }
            | StmtKind::Assign { values, .. }
            | StmtKind::Return(values) => values.iter().any(contains_pipeline_placeholder),
            _ => false,
        })
}

fn contains_unparenthesized_comparison(expr: &Expr) -> bool {
    match &expr.kind {
        ExprKind::Paren(_) => false,
        ExprKind::Binary { op, left, right } => {
            is_comparison_op(*op)
                || contains_unparenthesized_comparison(left)
                || contains_unparenthesized_comparison(right)
        }
        _ => false,
    }
}

fn is_comparison_op(op: BinaryOp) -> bool {
    matches!(
        op,
        BinaryOp::Eq
            | BinaryOp::NotEq
            | BinaryOp::Lt
            | BinaryOp::LtEq
            | BinaryOp::Gt
            | BinaryOp::GtEq
    )
}

fn starts_with_uppercase(value: &str) -> bool {
    value
        .chars()
        .next()
        .is_some_and(|ch| ch.is_ascii_uppercase())
}

#[cfg(test)]
#[path = "parser/tests.rs"]
mod tests;
