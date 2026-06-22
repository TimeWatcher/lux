use std::collections::BTreeMap;
use std::fmt;

use crate::ast::*;
use crate::ir::*;
use crate::module::{ArtifactRealm, RealmSet};
use crate::resolve::ResolveOutput;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LowerError {
    pub message: String,
}

impl LowerError {
    fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

impl fmt::Display for LowerError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for LowerError {}

pub struct Lowerer<'a> {
    module: &'a Module,
    resolved: &'a ResolveOutput,
    artifact_set: Option<RealmSet>,
    enums: EnumCatalog<'a>,
}

impl<'a> Lowerer<'a> {
    pub fn lower(module: &'a Module, resolved: &'a ResolveOutput) -> Result<IrModule, LowerError> {
        Self {
            module,
            resolved,
            artifact_set: None,
            enums: EnumCatalog::collect(module),
        }
        .lower_module()
    }

    pub fn lower_for_artifact(
        module: &'a Module,
        resolved: &'a ResolveOutput,
        artifact_realm: ArtifactRealm,
    ) -> Result<IrModule, LowerError> {
        Self {
            module,
            resolved,
            artifact_set: Some(RealmSet::from_artifact(artifact_realm)),
            enums: EnumCatalog::collect(module),
        }
        .lower_module()
    }

    fn lower_module(&self) -> Result<IrModule, LowerError> {
        Ok(IrModule {
            body: self
                .module
                .body
                .iter()
                .map(|stmt| self.lower_stmt(stmt))
                .collect::<Result<Vec<_>, _>>()?,
            exports: self.resolved.exports.clone(),
            origin: Origin::source(self.module.span),
        })
    }

    fn lower_block(&self, block: &Block) -> Result<IrBlock, LowerError> {
        Ok(IrBlock {
            statements: block
                .statements
                .iter()
                .map(|stmt| self.lower_stmt(stmt))
                .collect::<Result<Vec<_>, _>>()?,
            tail: block
                .tail
                .as_ref()
                .map(|expr| self.lower_tail_expr(expr))
                .transpose()?,
            origin: Origin::source(block.span),
        })
    }

    fn lower_stmt(&self, stmt: &Stmt) -> Result<IrStmt, LowerError> {
        let kind = match &stmt.kind {
            StmtKind::LocalDecl {
                mode,
                names,
                values,
            } => IrStmtKind::LocalDecl {
                mode: *mode,
                names: names.iter().map(|name| name.name.clone()).collect(),
                values: self.lower_exprs_with_tail(values)?,
            },
            StmtKind::LocalDestructure {
                mode,
                patterns,
                values,
            } => IrStmtKind::LocalDestructure {
                mode: *mode,
                patterns: patterns
                    .iter()
                    .map(|pattern| self.lower_pattern(pattern))
                    .collect::<Result<Vec<_>, _>>()?,
                values: values
                    .iter()
                    .map(|expr| self.lower_expr(expr))
                    .collect::<Result<Vec<_>, _>>()?,
            },
            StmtKind::Assign { targets, values } => IrStmtKind::Assign {
                targets: targets
                    .iter()
                    .map(|target| self.lower_place(target))
                    .collect::<Result<Vec<_>, _>>()?,
                values: self.lower_exprs_with_tail(values)?,
            },
            StmtKind::CompoundAssign { target, op, value } => IrStmtKind::CompoundAssign {
                target: self.lower_place(target)?,
                op: *op,
                value: self.lower_expr(value)?,
            },
            StmtKind::Expr(expr) => IrStmtKind::Expr(self.lower_expr(expr)?),
            StmtKind::Return(values) => IrStmtKind::Return(self.lower_exprs_with_tail(values)?),
            StmtKind::Break => IrStmtKind::Break,
            StmtKind::Continue => IrStmtKind::Continue,
            StmtKind::Import(import) if import.phase == ImportPhase::Macro => IrStmtKind::Noop,
            StmtKind::PartOrderDecl(_) => IrStmtKind::Noop,
            StmtKind::ExternDecl(_) => IrStmtKind::Noop,
            StmtKind::HostPackageDecl(_) => IrStmtKind::Noop,
            StmtKind::Import(import) => IrStmtKind::Import {
                source: import.source.clone(),
                specifiers: import
                    .specifiers
                    .iter()
                    .filter_map(lower_runtime_import_specifier)
                    .collect(),
                side_effect_only: import.side_effect_only,
            },
            StmtKind::ExportDecl {
                realm, stmt: inner, ..
            } => {
                if !self.realm_allowed(*realm) {
                    IrStmtKind::Noop
                } else {
                    return self.lower_stmt(inner);
                }
            }
            StmtKind::ExportList { realm, entries } => {
                if !self.realm_allowed(*realm) {
                    IrStmtKind::Noop
                } else {
                    IrStmtKind::ExportList(
                        entries
                            .iter()
                            .map(|entry| entry.exported.name.clone())
                            .collect(),
                    )
                }
            }
            StmtKind::ExportAll { .. } => IrStmtKind::Noop,
            StmtKind::RealmDecl { realm, stmt: inner } => {
                if !self.realm_allowed(Some(*realm)) {
                    IrStmtKind::Noop
                } else {
                    return self.lower_stmt(inner);
                }
            }
            StmtKind::RealmBlock { realm, block } => {
                if !self.realm_allowed(Some(*realm)) {
                    IrStmtKind::Noop
                } else {
                    IrStmtKind::Do(self.lower_block(block)?)
                }
            }
            StmtKind::InitDecl { realm, block } => {
                if !self.realm_allowed(*realm) {
                    IrStmtKind::Noop
                } else {
                    IrStmtKind::Do(self.lower_block(block)?)
                }
            }
            StmtKind::FunctionDecl(decl) => {
                IrStmtKind::FunctionDecl(self.lower_function_decl(decl)?)
            }
            StmtKind::EnumDecl(decl) => IrStmtKind::EnumDecl(self.lower_enum_decl(decl)?),
            StmtKind::If {
                condition,
                then_block,
                else_block,
            } => IrStmtKind::If {
                condition: self.lower_expr(condition)?,
                then_block: self.lower_block(then_block)?,
                else_block: else_block
                    .as_ref()
                    .map(|block| self.lower_block(block))
                    .transpose()?,
            },
            StmtKind::While { condition, body } => IrStmtKind::While {
                condition: self.lower_expr(condition)?,
                body: self.lower_block(body)?,
            },
            StmtKind::NumericFor {
                name,
                start,
                end,
                step,
                body,
            } => IrStmtKind::NumericFor {
                name: name.name.clone(),
                start: self.lower_expr(start)?,
                end: self.lower_expr(end)?,
                step: step
                    .as_ref()
                    .map(|expr| self.lower_expr(expr))
                    .transpose()?,
                body: self.lower_block(body)?,
            },
            StmtKind::GenericFor { names, iter, body } => IrStmtKind::GenericFor {
                names: names.iter().map(|name| name.name.clone()).collect(),
                iter: self.lower_exprs_with_tail(iter)?,
                body: self.lower_block(body)?,
            },
            StmtKind::RepeatUntil { body, condition } => IrStmtKind::RepeatUntil {
                body: self.lower_block(body)?,
                condition: self.lower_expr(condition)?,
            },
            StmtKind::Do(block) => IrStmtKind::Do(self.lower_block(block)?),
        };

        Ok(IrStmt {
            kind,
            origin: Origin::source(stmt.span),
        })
    }

    fn realm_allowed(&self, realm: Option<Realm>) -> bool {
        let Some(artifact_set) = self.artifact_set else {
            return true;
        };
        realm
            .map(RealmSet::from_realm)
            .unwrap_or(RealmSet::SHARED)
            .intersects(artifact_set)
    }

    fn lower_function_decl(&self, decl: &FunctionDecl) -> Result<IrFunctionDecl, LowerError> {
        Ok(IrFunctionDecl {
            name: decl.name.clone(),
            params: decl
                .params
                .iter()
                .map(|param| self.lower_param(param))
                .collect::<Result<Vec<_>, _>>()?,
            vararg: decl.vararg,
            body: self.lower_function_body(&decl.body)?,
        })
    }

    fn lower_param(&self, param: &Param) -> Result<IrParam, LowerError> {
        Ok(IrParam {
            name: param.name.name.clone(),
            default: param
                .default
                .as_ref()
                .map(|expr| self.lower_expr(expr))
                .transpose()?,
            origin: Origin::source(param.span),
        })
    }

    fn lower_pattern(&self, pattern: &Pattern) -> Result<IrPattern, LowerError> {
        let kind = match &pattern.kind {
            PatternKind::Identifier(name) => IrPatternKind::Identifier(name.name.clone()),
            PatternKind::Object(fields) => IrPatternKind::Object(
                fields
                    .iter()
                    .map(|field| {
                        Ok(IrObjectPatternField {
                            key: field.key.name.clone(),
                            pattern: self.lower_pattern(&field.pattern)?,
                            default: field
                                .default
                                .as_ref()
                                .map(|expr| self.lower_expr(expr))
                                .transpose()?,
                            origin: Origin::source(field.span),
                        })
                    })
                    .collect::<Result<Vec<_>, LowerError>>()?,
            ),
            PatternKind::Array(items) => IrPatternKind::Array(
                items
                    .iter()
                    .map(|item| {
                        Ok(IrArrayPatternItem {
                            pattern: self.lower_pattern(&item.pattern)?,
                            default: item
                                .default
                                .as_ref()
                                .map(|expr| self.lower_expr(expr))
                                .transpose()?,
                            origin: Origin::source(item.span),
                        })
                    })
                    .collect::<Result<Vec<_>, LowerError>>()?,
            ),
        };
        Ok(IrPattern {
            kind,
            origin: Origin::source(pattern.span),
        })
    }

    fn lower_function_body(&self, body: &FunctionBody) -> Result<IrFunctionBody, LowerError> {
        match body {
            FunctionBody::Expr(expr) => Ok(IrFunctionBody::Expr(Box::new(self.lower_expr(expr)?))),
            FunctionBody::Block(block) => {
                Ok(IrFunctionBody::Block(Box::new(self.lower_block(block)?)))
            }
        }
    }

    fn lower_exprs_with_tail(&self, exprs: &[Expr]) -> Result<Vec<IrExpr>, LowerError> {
        let last_index = exprs.len().saturating_sub(1);
        exprs
            .iter()
            .enumerate()
            .map(|(index, expr)| {
                let lowered = if index == last_index {
                    self.lower_tail_expr(expr)?
                } else {
                    self.lower_expr(expr)?
                };
                Ok(lowered)
            })
            .collect()
    }

    fn lower_tail_expr(&self, expr: &Expr) -> Result<IrExpr, LowerError> {
        let mut lowered = self.lower_expr(expr)?;
        mark_tail_multivalue(&mut lowered);
        Ok(lowered)
    }

    fn lower_expr(&self, expr: &Expr) -> Result<IrExpr, LowerError> {
        let kind = match &expr.kind {
            ExprKind::Identifier(ident) => IrExprKind::Identifier(ident.name.clone()),
            ExprKind::Nil => IrExprKind::Nil,
            ExprKind::Boolean(value) => IrExprKind::Boolean(*value),
            ExprKind::Number(value) => IrExprKind::Number(value.clone()),
            ExprKind::String(value) => IrExprKind::String(value.clone()),
            ExprKind::Vararg => IrExprKind::Vararg,
            ExprKind::PipelinePlaceholder => IrExprKind::PipelinePlaceholder,
            ExprKind::TemplateString(parts) => IrExprKind::Template(
                parts
                    .iter()
                    .map(|part| self.lower_template_part(part))
                    .collect::<Result<Vec<_>, _>>()?,
            ),
            ExprKind::Table(table) => IrExprKind::Table(self.lower_table_fields(&table.fields)?),
            ExprKind::Paren(expr) => return self.lower_expr(expr),
            ExprKind::Unary { op, argument } => IrExprKind::Unary {
                op: *op,
                argument: Box::new(self.lower_expr(argument)?),
            },
            ExprKind::Binary { op, left, right } => IrExprKind::Binary {
                op: *op,
                left: Box::new(self.lower_expr(left)?),
                right: Box::new(self.lower_expr(right)?),
            },
            ExprKind::Conditional {
                condition,
                then_branch,
                else_branch,
                ..
            } => IrExprKind::Conditional {
                condition: Box::new(self.lower_expr(condition)?),
                then_branch: self.lower_expr_or_block(then_branch)?,
                else_branch: self.lower_expr_or_block(else_branch)?,
            },
            ExprKind::Match(match_expr) => IrExprKind::Match(IrMatchExpr {
                subject: Box::new(self.lower_expr(&match_expr.subject)?),
                arms: match_expr
                    .arms
                    .iter()
                    .map(|arm| self.lower_match_arm(arm))
                    .collect::<Result<Vec<_>, _>>()?,
            }),
            ExprKind::Do(block) => IrExprKind::Do(Box::new(self.lower_block(block)?)),
            ExprKind::Function(function) => IrExprKind::Function(IrFunctionExpr {
                params: function
                    .params
                    .iter()
                    .map(|param| self.lower_param(param))
                    .collect::<Result<Vec<_>, _>>()?,
                vararg: function.vararg,
                implicit_self: function.arrow_kind == ArrowKind::ImplicitSelf,
                body: self.lower_function_body(&function.body)?,
            }),
            ExprKind::Chain(chain) => {
                if let Some(expr) = self.lower_enum_chain(chain, expr.span)? {
                    return Ok(expr);
                }
                IrExprKind::Chain(self.lower_chain(chain)?)
            }
        };

        Ok(IrExpr {
            kind,
            origin: Origin::source(expr.span),
            value_mode: ValueMode::Single,
            symbol: resolved_symbol_for_expr(self.resolved, expr),
        })
    }

    fn lower_expr_or_block(&self, item: &ExprOrBlock) -> Result<IrExprOrBlock, LowerError> {
        match item {
            ExprOrBlock::Expr(expr) => Ok(IrExprOrBlock::Expr(Box::new(self.lower_expr(expr)?))),
            ExprOrBlock::Block(block) => {
                Ok(IrExprOrBlock::Block(Box::new(self.lower_block(block)?)))
            }
        }
    }

    fn lower_enum_decl(&self, decl: &EnumDecl) -> Result<IrEnumDecl, LowerError> {
        Ok(IrEnumDecl {
            name: decl.name.name.clone(),
            repr: lower_enum_repr(&decl.repr),
            runtime: decl.runtime,
            variants: decl
                .variants
                .iter()
                .enumerate()
                .map(|(index, variant)| {
                    Ok(IrEnumVariant {
                        name: variant.name.name.clone(),
                        payload: lower_enum_payload(&variant.payload),
                        tag: self.lower_enum_variant_tag(decl, variant, index)?,
                        origin: Origin::source(variant.span),
                    })
                })
                .collect::<Result<Vec<_>, LowerError>>()?,
        })
    }

    fn lower_enum_variant_tag(
        &self,
        decl: &EnumDecl,
        variant: &EnumVariant,
        index: usize,
    ) -> Result<IrExpr, LowerError> {
        if let Some(tag) = &variant.tag {
            return self.lower_expr(tag);
        }
        let kind = match &decl.repr {
            EnumRepr::Number => IrExprKind::Number(index.to_string()),
            EnumRepr::String | EnumRepr::Table { .. } | EnumRepr::Existing { .. } => {
                IrExprKind::String(variant.name.name.clone())
            }
        };
        Ok(IrExpr {
            kind,
            origin: Origin::Synthetic {
                source: variant.span,
                reason: "default enum tag".into(),
            },
            value_mode: ValueMode::Single,
            symbol: None,
        })
    }

    fn lower_match_arm(&self, arm: &MatchArm) -> Result<IrMatchArm, LowerError> {
        Ok(IrMatchArm {
            pattern: self.lower_match_pattern(&arm.pattern)?,
            body: self.lower_expr_or_block(&arm.body)?,
            origin: Origin::source(arm.span),
        })
    }

    fn lower_match_pattern(&self, pattern: &MatchPattern) -> Result<IrMatchPattern, LowerError> {
        let kind = match &pattern.kind {
            MatchPatternKind::Or(patterns) => IrMatchPatternKind::Or(
                patterns
                    .iter()
                    .map(|pattern| self.lower_match_pattern(pattern))
                    .collect::<Result<Vec<_>, _>>()?,
            ),
            MatchPatternKind::Wildcard => IrMatchPatternKind::Wildcard,
            MatchPatternKind::Binding(name) => IrMatchPatternKind::Binding(name.name.clone()),
            MatchPatternKind::Literal(literal) => IrMatchPatternKind::Literal(match literal {
                MatchLiteral::Nil => IrMatchLiteral::Nil,
                MatchLiteral::Boolean(value) => IrMatchLiteral::Boolean(*value),
                MatchLiteral::Number(value) => IrMatchLiteral::Number(value.clone()),
                MatchLiteral::String(value) => IrMatchLiteral::String(value.clone()),
            }),
            MatchPatternKind::Variant { path, payload } => IrMatchPatternKind::Variant {
                path: path.iter().map(|part| part.name.clone()).collect(),
                payload: payload
                    .as_ref()
                    .map(|payload| self.lower_match_pattern_payload(payload))
                    .transpose()?,
            },
            MatchPatternKind::Object(fields) => IrMatchPatternKind::Object(
                fields
                    .iter()
                    .map(|field| {
                        Ok(IrMatchObjectPatternField {
                            key: field.key.name.clone(),
                            pattern: self.lower_match_pattern(&field.pattern)?,
                            origin: Origin::source(field.span),
                        })
                    })
                    .collect::<Result<Vec<_>, LowerError>>()?,
            ),
            MatchPatternKind::Array(items) => IrMatchPatternKind::Array(
                items
                    .iter()
                    .map(|item| {
                        Ok(IrMatchArrayPatternItem {
                            pattern: self.lower_match_pattern(&item.pattern)?,
                            origin: Origin::source(item.span),
                        })
                    })
                    .collect::<Result<Vec<_>, LowerError>>()?,
            ),
        };
        Ok(IrMatchPattern {
            kind,
            origin: Origin::source(pattern.span),
        })
    }

    fn lower_match_pattern_payload(
        &self,
        payload: &MatchPatternPayload,
    ) -> Result<IrMatchPatternPayload, LowerError> {
        match payload {
            MatchPatternPayload::Tuple(patterns) => Ok(IrMatchPatternPayload::Tuple(
                patterns
                    .iter()
                    .map(|pattern| self.lower_match_pattern(pattern))
                    .collect::<Result<Vec<_>, _>>()?,
            )),
            MatchPatternPayload::Record(fields) => Ok(IrMatchPatternPayload::Record(
                fields
                    .iter()
                    .map(|field| {
                        Ok(IrMatchObjectPatternField {
                            key: field.key.name.clone(),
                            pattern: self.lower_match_pattern(&field.pattern)?,
                            origin: Origin::source(field.span),
                        })
                    })
                    .collect::<Result<Vec<_>, LowerError>>()?,
            )),
        }
    }

    fn lower_template_part(&self, part: &TemplatePart) -> Result<IrTemplatePart, LowerError> {
        let kind = match &part.kind {
            TemplatePartKind::Text(text) => IrTemplatePartKind::Text(text.clone()),
            TemplatePartKind::Expr(expr) => IrTemplatePartKind::Expr(self.lower_expr(expr)?),
        };
        Ok(IrTemplatePart {
            kind,
            origin: Origin::source(part.span),
        })
    }

    fn lower_table_field(&self, field: &TableField) -> Result<IrTableField, LowerError> {
        let kind = match &field.kind {
            TableFieldKind::Array(expr) => IrTableFieldKind::Array(self.lower_expr(expr)?),
            TableFieldKind::Named { name, value } => IrTableFieldKind::Named {
                name: name.name.clone(),
                value: self.lower_expr(value)?,
            },
            TableFieldKind::ExprKey { key, value } => IrTableFieldKind::ExprKey {
                key: self.lower_expr(key)?,
                value: self.lower_expr(value)?,
            },
            TableFieldKind::Spread(value) => IrTableFieldKind::Spread(self.lower_expr(value)?),
        };
        Ok(IrTableField {
            kind,
            origin: Origin::source(field.span),
        })
    }

    fn lower_table_fields(&self, fields: &[TableField]) -> Result<Vec<IrTableField>, LowerError> {
        let last_array_index = fields
            .iter()
            .rposition(|field| matches!(field.kind, TableFieldKind::Array(_)));

        fields
            .iter()
            .enumerate()
            .map(|(index, field)| {
                let mut lowered = self.lower_table_field(field)?;
                if Some(index) == last_array_index {
                    if let IrTableFieldKind::Array(expr) = &mut lowered.kind {
                        if can_produce_multivalue(expr) {
                            expr.value_mode = ValueMode::MultiTail;
                        }
                    }
                }
                Ok(lowered)
            })
            .collect()
    }

    fn lower_chain(&self, chain: &ChainExpr) -> Result<IrChain, LowerError> {
        Ok(IrChain {
            base: Box::new(self.lower_expr(&chain.base)?),
            segments: chain
                .segments
                .iter()
                .map(|segment| self.lower_chain_segment(segment))
                .collect::<Result<Vec<_>, _>>()?,
        })
    }

    fn lower_enum_chain(
        &self,
        chain: &ChainExpr,
        span: crate::source::SourceSpan,
    ) -> Result<Option<IrExpr>, LowerError> {
        let ExprKind::Identifier(base) = &chain.base.kind else {
            return Ok(None);
        };
        let Some(enum_decl) = self.enums.by_name.get(base.name.as_str()).copied() else {
            return Ok(None);
        };
        let Some(first_segment) = chain.segments.first() else {
            return Ok(None);
        };
        let ChainSegmentKind::Member {
            name: variant_name,
            optional: false,
        } = &first_segment.kind
        else {
            return Ok(None);
        };
        let Some((variant_index, variant)) = enum_decl
            .variants
            .iter()
            .enumerate()
            .find(|(_, variant)| variant.name.name == variant_name.name)
        else {
            return Ok(None);
        };

        if chain.segments.len() == 1 {
            return Ok(Some(self.lower_enum_variant_tag(
                enum_decl,
                variant,
                variant_index,
            )?));
        }

        let [_, call_segment] = chain.segments.as_slice() else {
            return Ok(None);
        };
        let ChainSegmentKind::Call { args, .. } = &call_segment.kind else {
            return Ok(None);
        };

        match &enum_decl.repr {
            EnumRepr::Existing { .. } => {
                return Err(LowerError::new(format!(
                    "`{}.{}` is a repr existing enum view and cannot be constructed implicitly",
                    enum_decl.name.name, variant.name.name
                )));
            }
            EnumRepr::Number | EnumRepr::String => {
                if !matches!(variant.payload, EnumVariantPayload::None) || !args.is_empty() {
                    return Err(LowerError::new(format!(
                        "`{}.{}` has scalar repr and cannot carry payload",
                        enum_decl.name.name, variant.name.name
                    )));
                }
                return Ok(Some(self.lower_enum_variant_tag(
                    enum_decl,
                    variant,
                    variant_index,
                )?));
            }
            EnumRepr::Table { tag_field } => {
                let fields = enum_payload_fields(&variant.payload);
                if args.len() > fields.len() {
                    return Err(LowerError::new(format!(
                        "`{}.{}` constructor got {} arguments but only {} payload fields exist",
                        enum_decl.name.name,
                        variant.name.name,
                        args.len(),
                        fields.len()
                    )));
                }
                let mut table_fields = vec![IrTableField {
                    kind: IrTableFieldKind::Named {
                        name: tag_field.clone(),
                        value: self.lower_enum_variant_tag(enum_decl, variant, variant_index)?,
                    },
                    origin: Origin::source(first_segment.span),
                }];
                for (field, arg) in fields.iter().zip(args.iter()) {
                    table_fields.push(IrTableField {
                        kind: IrTableFieldKind::Named {
                            name: field.clone(),
                            value: self.lower_expr(arg)?,
                        },
                        origin: Origin::source(arg.span),
                    });
                }
                return Ok(Some(IrExpr {
                    kind: IrExprKind::Table(table_fields),
                    origin: Origin::source(span),
                    value_mode: ValueMode::Single,
                    symbol: None,
                }));
            }
        }
    }

    fn lower_chain_segment(&self, segment: &ChainSegment) -> Result<IrChainSegment, LowerError> {
        let kind = match &segment.kind {
            ChainSegmentKind::Member { name, optional } => IrChainSegmentKind::Member {
                name: name.name.clone(),
                optional: *optional,
            },
            ChainSegmentKind::Index { index, optional } => IrChainSegmentKind::Index {
                index: self.lower_expr(index)?,
                optional: *optional,
            },
            ChainSegmentKind::Call { args, style } => IrChainSegmentKind::Call {
                args: self.lower_exprs_with_tail(args)?,
                style: lower_call_style(*style),
            },
            ChainSegmentKind::SafeCall { args, style } => IrChainSegmentKind::SafeCall {
                args: self.lower_exprs_with_tail(args)?,
                style: lower_call_style(*style),
            },
            ChainSegmentKind::SafeDotCall { name, args, style } => {
                IrChainSegmentKind::SafeDotCall {
                    name: name.name.clone(),
                    args: self.lower_exprs_with_tail(args)?,
                    style: lower_call_style(*style),
                }
            }
            ChainSegmentKind::MethodCall {
                name,
                args,
                optional,
                style,
            } => IrChainSegmentKind::MethodCall {
                name: name.name.clone(),
                args: self.lower_exprs_with_tail(args)?,
                optional: *optional,
                style: lower_call_style(*style),
            },
        };

        Ok(IrChainSegment {
            kind,
            origin: Origin::source(segment.span),
        })
    }

    fn lower_place(&self, expr: &Expr) -> Result<IrPlace, LowerError> {
        match &expr.kind {
            ExprKind::Identifier(ident) => Ok(IrPlace::Identifier(ident.name.clone())),
            ExprKind::Chain(chain) => lower_chain_place(self, chain),
            _ => Err(LowerError::new("invalid assignment target")),
        }
    }
}

fn can_produce_multivalue(expr: &IrExpr) -> bool {
    matches!(expr.kind, IrExprKind::Vararg | IrExprKind::Chain(_))
}

fn mark_tail_multivalue(expr: &mut IrExpr) {
    if can_produce_multivalue(expr) {
        expr.value_mode = ValueMode::MultiTail;
    }
    match &mut expr.kind {
        IrExprKind::Conditional {
            then_branch,
            else_branch,
            ..
        } => {
            mark_expr_or_block_tail_multivalue(then_branch);
            mark_expr_or_block_tail_multivalue(else_branch);
        }
        IrExprKind::Match(match_expr) => {
            for arm in &mut match_expr.arms {
                mark_expr_or_block_tail_multivalue(&mut arm.body);
            }
        }
        IrExprKind::Do(block) => mark_block_tail_multivalue(block),
        _ => {}
    }
}

fn mark_expr_or_block_tail_multivalue(item: &mut IrExprOrBlock) {
    match item {
        IrExprOrBlock::Expr(expr) => mark_tail_multivalue(expr),
        IrExprOrBlock::Block(block) => mark_block_tail_multivalue(block),
    }
}

fn mark_block_tail_multivalue(block: &mut IrBlock) {
    if let Some(tail) = &mut block.tail {
        mark_tail_multivalue(tail);
    }
}

fn resolved_symbol_for_expr(
    resolved: &ResolveOutput,
    expr: &Expr,
) -> Option<crate::resolve::ResolvedSymbol> {
    let ExprKind::Identifier(_) = &expr.kind else {
        return None;
    };
    resolved.symbols_by_span.get(&expr.span).cloned()
}

fn lower_chain_place(lowerer: &Lowerer<'_>, chain: &ChainExpr) -> Result<IrPlace, LowerError> {
    if chain.segments.is_empty() {
        return Err(LowerError::new("chain assignment target has no segment"));
    }

    let mut object = lowerer.lower_expr(&chain.base)?;
    for segment in &chain.segments[..chain.segments.len() - 1] {
        object = match &segment.kind {
            ChainSegmentKind::Member {
                name,
                optional: false,
            } => IrExpr {
                kind: IrExprKind::Chain(IrChain {
                    base: Box::new(object),
                    segments: vec![IrChainSegment {
                        kind: IrChainSegmentKind::Member {
                            name: name.name.clone(),
                            optional: false,
                        },
                        origin: Origin::source(segment.span),
                    }],
                }),
                origin: Origin::source(segment.span),
                value_mode: ValueMode::Single,
                symbol: None,
            },
            ChainSegmentKind::Index {
                index,
                optional: false,
            } => IrExpr {
                kind: IrExprKind::Chain(IrChain {
                    base: Box::new(object),
                    segments: vec![IrChainSegment {
                        kind: IrChainSegmentKind::Index {
                            index: lowerer.lower_expr(index)?,
                            optional: false,
                        },
                        origin: Origin::source(segment.span),
                    }],
                }),
                origin: Origin::source(segment.span),
                value_mode: ValueMode::Single,
                symbol: None,
            },
            ChainSegmentKind::Call { args, style } => IrExpr {
                kind: IrExprKind::Chain(IrChain {
                    base: Box::new(object),
                    segments: vec![IrChainSegment {
                        kind: IrChainSegmentKind::Call {
                            args: lowerer.lower_exprs_with_tail(args)?,
                            style: lower_call_style(*style),
                        },
                        origin: Origin::source(segment.span),
                    }],
                }),
                origin: Origin::source(segment.span),
                value_mode: ValueMode::Single,
                symbol: None,
            },
            ChainSegmentKind::MethodCall {
                name,
                args,
                optional: false,
                style,
            } => IrExpr {
                kind: IrExprKind::Chain(IrChain {
                    base: Box::new(object),
                    segments: vec![IrChainSegment {
                        kind: IrChainSegmentKind::MethodCall {
                            name: name.name.clone(),
                            args: lowerer.lower_exprs_with_tail(args)?,
                            optional: false,
                            style: lower_call_style(*style),
                        },
                        origin: Origin::source(segment.span),
                    }],
                }),
                origin: Origin::source(segment.span),
                value_mode: ValueMode::Single,
                symbol: None,
            },
            _ => {
                return Err(LowerError::new(
                    "assignment target cannot contain calls or optional segments",
                ));
            }
        };
    }

    let last = chain.segments.last().expect("checked non-empty");
    match &last.kind {
        ChainSegmentKind::Member {
            name,
            optional: false,
        } => Ok(IrPlace::Member {
            object,
            name: name.name.clone(),
        }),
        ChainSegmentKind::Index {
            index,
            optional: false,
        } => Ok(IrPlace::Index {
            object,
            index: lowerer.lower_expr(index)?,
        }),
        _ => Err(LowerError::new("invalid assignment target")),
    }
}

fn lower_call_style(style: CallStyle) -> IrCallStyle {
    match style {
        CallStyle::Paren => IrCallStyle::Paren,
        CallStyle::TailTable => IrCallStyle::TailTable,
        CallStyle::TailString => IrCallStyle::TailString,
    }
}

#[derive(Debug, Clone)]
struct EnumCatalog<'a> {
    by_name: BTreeMap<&'a str, &'a EnumDecl>,
}

impl<'a> EnumCatalog<'a> {
    fn collect(module: &'a Module) -> Self {
        let mut catalog = Self {
            by_name: BTreeMap::new(),
        };
        for stmt in &module.body {
            catalog.collect_stmt(stmt);
        }
        catalog
    }

    fn collect_stmt(&mut self, stmt: &'a Stmt) {
        match &stmt.kind {
            StmtKind::EnumDecl(decl) => {
                self.by_name.insert(decl.name.name.as_str(), decl);
            }
            StmtKind::ExportDecl { stmt, .. } | StmtKind::RealmDecl { stmt, .. } => {
                self.collect_stmt(stmt);
            }
            StmtKind::RealmBlock { block, .. } | StmtKind::InitDecl { block, .. } => {
                for stmt in &block.statements {
                    self.collect_stmt(stmt);
                }
            }
            _ => {}
        }
    }
}

fn lower_enum_repr(repr: &EnumRepr) -> IrEnumRepr {
    match repr {
        EnumRepr::String => IrEnumRepr::String,
        EnumRepr::Number => IrEnumRepr::Number,
        EnumRepr::Table { tag_field } => IrEnumRepr::Table {
            tag_field: tag_field.clone(),
        },
        EnumRepr::Existing { tag_field } => IrEnumRepr::Existing {
            tag_field: tag_field.clone(),
        },
    }
}

fn lower_enum_payload(payload: &EnumVariantPayload) -> IrEnumVariantPayload {
    match payload {
        EnumVariantPayload::None => IrEnumVariantPayload::None,
        EnumVariantPayload::Tuple(fields) => {
            IrEnumVariantPayload::Tuple(fields.iter().map(|field| field.name.clone()).collect())
        }
        EnumVariantPayload::Record(fields) => {
            IrEnumVariantPayload::Record(fields.iter().map(|field| field.name.clone()).collect())
        }
    }
}

fn enum_payload_fields(payload: &EnumVariantPayload) -> Vec<String> {
    match payload {
        EnumVariantPayload::None => Vec::new(),
        EnumVariantPayload::Tuple(fields) | EnumVariantPayload::Record(fields) => {
            fields.iter().map(|field| field.name.clone()).collect()
        }
    }
}

fn lower_runtime_import_specifier(specifier: &ImportSpecifier) -> Option<IrImportSpecifier> {
    match specifier {
        ImportSpecifier::Named { imported, local } => Some(IrImportSpecifier {
            imported: imported.name.clone(),
            local: local.name.clone(),
            namespace: false,
        }),
        ImportSpecifier::Namespace { local } => Some(IrImportSpecifier {
            imported: "*".into(),
            local: local.name.clone(),
            namespace: true,
        }),
    }
}

#[cfg(test)]
mod tests {
    use crate::ir::{IrChainSegmentKind, IrExprKind, IrStmtKind};
    use crate::lex::Lexer;
    use crate::parse::Parser;
    use crate::resolve::Resolver;
    use crate::source::SourceFile;

    use super::Lowerer;

    fn lower(input: &str) -> crate::ir::IrModule {
        let file = SourceFile::new(0, None, input);
        let lex = Lexer::new(&file).lex_all();
        assert!(lex.diagnostics.is_empty(), "{:#?}", lex.diagnostics);
        let parsed = Parser::new(&lex.tokens).parse_module();
        assert!(parsed.diagnostics.is_empty(), "{:#?}", parsed.diagnostics);
        let resolved = Resolver::resolve(&parsed.module);
        assert!(
            resolved.diagnostics.is_empty(),
            "{:#?}",
            resolved.diagnostics
        );
        Lowerer::lower(&parsed.module, &resolved).expect("lower")
    }

    #[test]
    fn lowers_exports_and_function_body() {
        let module = lower("export fn foo(x) = x + 1");
        assert_eq!(module.exports.len(), 1);
        assert!(matches!(module.body[0].kind, IrStmtKind::FunctionDecl(_)));
    }

    #[test]
    fn preserves_safe_dot_call_segment() {
        let module = lower("obj?.name(args)");
        let IrStmtKind::Expr(expr) = &module.body[0].kind else {
            panic!("expected expr stmt");
        };
        let IrExprKind::Chain(chain) = &expr.kind else {
            panic!("expected chain");
        };
        assert!(matches!(
            chain.segments.first().map(|segment| &segment.kind),
            Some(IrChainSegmentKind::SafeDotCall { .. })
        ));
    }
}
