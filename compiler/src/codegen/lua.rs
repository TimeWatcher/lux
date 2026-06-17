use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

use crate::ast::{BinaryOp, CompoundAssignOp, FunctionName, UnaryOp};
use crate::ir::*;
use crate::sourcemap::{LuaWriter, SourceMap};

use super::lua_budget::{LuaLocalBudget, analyze_lua_local_budget};

mod analysis;
mod match_analysis;
mod scope;
mod syntax;

use self::analysis::*;
use self::match_analysis::*;
use self::scope::*;
use self::syntax::*;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CodegenError {
    pub message: String,
}

impl fmt::Display for CodegenError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for CodegenError {}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LuaOutput {
    pub lua: String,
    pub source_map: SourceMap,
    pub local_budget: LuaLocalBudget,
}

#[derive(Debug)]
struct Gensym {
    next_id: usize,
    reserved: BTreeSet<String>,
}

impl Gensym {
    fn new(reserved: BTreeSet<String>) -> Self {
        Self {
            next_id: 0,
            reserved,
        }
    }

    fn next(&mut self, hint: &str) -> String {
        loop {
            self.next_id += 1;
            let name = format!("__lux_{hint}_{}", self.next_id);
            if self.reserved.insert(name.clone()) {
                return name;
            }
        }
    }

    fn next_hinted(&mut self, role: &str, hint: Option<&str>) -> String {
        let Some(hint) = hint.and_then(sanitize_temp_hint) else {
            return self.next(role);
        };
        self.next(&format!("{role}_{hint}"))
    }
}

pub struct LuaCodegen<'a> {
    module: &'a IrModule,
    writer: LuaWriter,
    gensym: Gensym,
    import_scopes: Vec<BTreeMap<String, String>>,
    name_scopes: Vec<BTreeSet<String>>,
    pipeline_placeholders: Vec<String>,
    module_lift: ModuleLift,
    loop_stack: Vec<LoopControlContext>,
    enums: IrEnumCatalog<'a>,
}

impl<'a> LuaCodegen<'a> {
    pub fn generate(module: &'a IrModule) -> Result<LuaOutput, CodegenError> {
        let mut codegen = Self {
            module,
            writer: LuaWriter::new(),
            gensym: Gensym::new(collect_reserved_names(module)),
            import_scopes: vec![BTreeMap::new()],
            name_scopes: vec![BTreeSet::new()],
            pipeline_placeholders: Vec::new(),
            module_lift: ModuleLift::default(),
            loop_stack: Vec::new(),
            enums: IrEnumCatalog::collect(module),
        };
        codegen.configure_module_lift();
        codegen.emit_module()?;
        let (lua, source_map) = codegen.writer.finish();
        let local_budget = analyze_lua_local_budget(&lua);
        local_budget.validate().map_err(|source| CodegenError {
            message: source.to_string(),
        })?;
        Ok(LuaOutput {
            lua,
            source_map,
            local_budget,
        })
    }

    fn emit_module(&mut self) -> Result<(), CodegenError> {
        self.line("local __lux_exports = {}", &self.module.origin);
        if let Some(table) = &self.module_lift.table {
            self.line(format!("local {table} = {{}}"), &self.module.origin);
        }

        let hoisted = collect_simple_function_hoists(&self.module.body);
        for hoist in &hoisted {
            if self.is_module_lifted(&hoist.name) {
                continue;
            }
            self.line(format!("local {}", hoist.name), &hoist.origin);
            self.declare_name(hoist.name.clone());
        }
        if !hoisted.is_empty() {
            self.blank();
        }

        for stmt in &self.module.body {
            self.emit_stmt(stmt)?;
        }

        if !self.module.exports.is_empty() {
            self.blank();
            for export in &self.module.exports {
                let local_name = self.emit_identifier(&export.local_name);
                self.line(
                    format!("__lux_exports.{} = {local_name}", export.name),
                    &Origin::source(export.span),
                );
            }
        }

        self.blank();
        self.line("return __lux_exports", &self.module.origin);
        Ok(())
    }

    fn configure_module_lift(&mut self) {
        let lift = collect_module_lift_candidates(&self.module.body);
        if lift.local_pressure() <= MODULE_LIFT_THRESHOLD {
            return;
        }

        let table = self.gensym.next("module");
        self.module_lift = ModuleLift {
            table: Some(table),
            names: lift.names,
        };
    }

    fn is_module_lifted(&self, name: &str) -> bool {
        self.module_lift.names.contains(name)
    }

    fn is_module_scope(&self) -> bool {
        self.name_scopes.len() == 1
    }

    fn module_lift_table(&self) -> Option<&str> {
        self.module_lift.table.as_deref()
    }

    fn emit_identifier(&self, name: &str) -> String {
        if self.is_shadowed(name) {
            return name.to_string();
        }
        if self.is_module_lifted(name) {
            if let Some(table) = &self.module_lift.table {
                return format!("{table}.{name}");
            }
        }
        name.to_string()
    }

    fn is_shadowed(&self, name: &str) -> bool {
        self.name_scopes
            .iter()
            .rev()
            .any(|scope| scope.contains(name))
    }

    fn declare_name(&mut self, name: String) {
        if let Some(scope) = self.name_scopes.last_mut() {
            scope.insert(name);
        }
    }

    fn declare_names(&mut self, names: impl IntoIterator<Item = String>) {
        if let Some(scope) = self.name_scopes.last_mut() {
            scope.extend(names);
        }
    }

    fn with_name_scope(
        &mut self,
        names: impl IntoIterator<Item = String>,
        f: impl FnOnce(&mut Self) -> Result<(), CodegenError>,
    ) -> Result<(), CodegenError> {
        let mut scope = BTreeSet::new();
        scope.extend(names);
        self.name_scopes.push(scope);
        let result = f(self);
        self.name_scopes.pop();
        result
    }

    fn emit_stmt(&mut self, stmt: &IrStmt) -> Result<(), CodegenError> {
        match &stmt.kind {
            IrStmtKind::Noop => {}
            IrStmtKind::LocalDecl { names, values, .. } => {
                if self.is_module_scope()
                    && !names.is_empty()
                    && names.iter().all(|name| self.is_module_lifted(name))
                {
                    self.emit_lifted_local_decl(names, values, &stmt.origin)?;
                    return Ok(());
                }

                let lhs = names.join(", ");
                let references_declared_names = exprs_reference_any_names(values, names);
                if values.is_empty() {
                    self.line(format!("local {lhs}"), &stmt.origin);
                    self.declare_names(names.iter().cloned());
                } else if names.len() == 1
                    && values.len() == 1
                    && !references_declared_names
                    && self.emit_direct_local_decl(&names[0], &values[0], &stmt.origin)?
                {
                } else if names.len() == 1
                    && values.len() == 1
                    && prefers_direct_assignment_expr(&values[0])
                    && !references_declared_names
                {
                    self.line(format!("local {lhs}"), &stmt.origin);
                    self.declare_names(names.iter().cloned());
                    self.emit_expr_into(&values[0], &lhs)?;
                } else {
                    let emitted = self.emit_expr_list(values)?;
                    if emitted.setup.is_empty() {
                        self.line(format!("local {lhs} = {}", emitted.values), &stmt.origin);
                        self.declare_names(names.iter().cloned());
                    } else if !references_declared_names {
                        self.line(format!("local {lhs}"), &stmt.origin);
                        self.declare_names(names.iter().cloned());
                        self.emit_scoped_setup(emitted.setup, &stmt.origin, |this| {
                            this.line(format!("{lhs} = {}", emitted.values), &stmt.origin);
                            Ok(())
                        })?;
                    } else {
                        self.emit_setup(emitted.setup);
                        self.line(format!("local {lhs} = {}", emitted.values), &stmt.origin);
                        self.declare_names(names.iter().cloned());
                    }
                }
            }
            IrStmtKind::LocalDestructure {
                patterns, values, ..
            } => {
                self.emit_local_destructure(patterns, values, &stmt.origin)?;
            }
            IrStmtKind::Assign { targets, values } => {
                if self.emit_direct_coalesce_assign(targets, values, &stmt.origin)? {
                    return Ok(());
                }
                let places = targets
                    .iter()
                    .map(|target| self.emit_place_ref(target, PlaceMode::Direct))
                    .collect::<Result<Vec<_>, _>>()?;
                let values = self.emit_expr_list(values)?;
                let mut setup = Vec::new();
                for place in &places {
                    setup.extend(place.setup.clone());
                }
                setup.extend(values.setup);
                let lhs = places
                    .iter()
                    .map(|place| place.write_target.as_str())
                    .collect::<Vec<_>>()
                    .join(", ");
                self.emit_scoped_setup(setup, &stmt.origin, |this| {
                    this.line(format!("{lhs} = {}", values.values), &stmt.origin);
                    Ok(())
                })?;
            }
            IrStmtKind::CompoundAssign { target, op, value } => {
                self.emit_compound_assign(target, *op, value, &stmt.origin)?;
            }
            IrStmtKind::Expr(expr) => self.emit_expr_as_stmt(expr)?,
            IrStmtKind::Return(values) => {
                if values.is_empty() {
                    self.line("return", &stmt.origin);
                } else if values.len() == 1 {
                    self.emit_return_expr(&values[0])?;
                } else {
                    let values = self.emit_expr_list(values)?;
                    self.emit_setup(values.setup);
                    self.line(format!("return {}", values.values), &stmt.origin);
                }
            }
            IrStmtKind::Break => {
                if let Some(flag) = self
                    .loop_stack
                    .last()
                    .and_then(|context| context.break_flag.as_ref())
                    .cloned()
                {
                    self.line(format!("{flag} = true"), &stmt.origin);
                }
                self.line("break", &stmt.origin)
            }
            IrStmtKind::Continue => self.line("break", &stmt.origin),
            IrStmtKind::FunctionDecl(decl) => self.emit_function_decl(decl, &stmt.origin)?,
            IrStmtKind::EnumDecl(decl) => self.emit_enum_decl(decl, &stmt.origin)?,
            IrStmtKind::If {
                condition,
                then_block,
                else_block,
            } => self.emit_if_stmt(condition, then_block, else_block.as_ref(), &stmt.origin)?,
            IrStmtKind::While { condition, body } => {
                let condition = self.emit_condition_setup(condition)?;
                if condition.setup.is_empty() {
                    self.line(format!("while {} do", condition.value), &stmt.origin);
                    self.indented(|this| this.emit_loop_body(body, &stmt.origin))?;
                    self.line("end", &stmt.origin);
                } else {
                    self.line("while true do", &stmt.origin);
                    self.indented(|this| {
                        this.emit_scoped_setup(condition.setup, &stmt.origin, |this| {
                            this.line(format!("if not ({}) then", condition.value), &stmt.origin);
                            this.indented(|this| {
                                this.line("break", &stmt.origin);
                                Ok(())
                            })?;
                            this.line("end", &stmt.origin);
                            Ok(())
                        })?;
                        this.emit_loop_body(body, &stmt.origin)
                    })?;
                    self.line("end", &stmt.origin);
                }
            }
            IrStmtKind::NumericFor {
                name,
                start,
                end,
                step,
                body,
            } => {
                let start = self.emit_expr_setup(start)?;
                let end = self.emit_expr_setup(end)?;
                let step = step
                    .as_ref()
                    .map(|expr| self.emit_expr_setup(expr))
                    .transpose()?;
                let mut setup = start.setup;
                setup.extend(end.setup);
                if let Some(step) = &step {
                    setup.extend(step.setup.clone());
                }
                let mut header = format!("for {name} = {}, {}", start.value, end.value);
                if let Some(step) = step {
                    header.push_str(&format!(", {}", step.value));
                }
                header.push_str(" do");
                self.emit_scoped_setup(setup, &stmt.origin, |this| {
                    this.line(header, &stmt.origin);
                    this.indented(|this| {
                        this.with_name_scope(std::iter::once(name.clone()), |this| {
                            this.emit_loop_body(body, &stmt.origin)
                        })
                    })?;
                    this.line("end", &stmt.origin);
                    Ok(())
                })?;
            }
            IrStmtKind::GenericFor { names, iter, body } => {
                let iter = self.emit_expr_list(iter)?;
                self.emit_scoped_setup(iter.setup, &stmt.origin, |this| {
                    this.line(
                        format!("for {} in {} do", names.join(", "), iter.values),
                        &stmt.origin,
                    );
                    this.indented(|this| {
                        this.with_name_scope(names.iter().cloned(), |this| {
                            this.emit_loop_body(body, &stmt.origin)
                        })
                    })?;
                    this.line("end", &stmt.origin);
                    Ok(())
                })?;
            }
            IrStmtKind::RepeatUntil { body, condition } => {
                self.line("repeat", &stmt.origin);
                self.indented(|this| this.emit_loop_body(body, &stmt.origin))?;
                let condition = self.emit_condition_setup(condition)?;
                self.emit_setup(condition.setup);
                self.line(format!("until {}", condition.value), &stmt.origin);
            }
            IrStmtKind::Do(block) => {
                self.line("do", &stmt.origin);
                self.indented(|this| this.emit_block_as_statements(block))?;
                self.line("end", &stmt.origin);
            }
            IrStmtKind::Import {
                source,
                specifiers,
                side_effect_only,
            } => self.emit_import(source, specifiers, *side_effect_only, &stmt.origin),
            IrStmtKind::ExportList(_) => {}
        }
        Ok(())
    }

    fn emit_direct_coalesce_assign(
        &mut self,
        targets: &[IrPlace],
        values: &[IrExpr],
        origin: &Origin,
    ) -> Result<bool, CodegenError> {
        let [IrPlace::Identifier(target_name)] = targets else {
            return Ok(false);
        };
        let [value] = values else {
            return Ok(false);
        };
        let IrExprKind::Binary {
            op: BinaryOp::Coalesce,
            left,
            right,
        } = &value.kind
        else {
            return Ok(false);
        };
        let IrExprKind::Identifier(left_name) = &left.kind else {
            return Ok(false);
        };
        if left_name != target_name {
            return Ok(false);
        }

        let target = self.emit_identifier(target_name);
        self.line(format!("if {target} == nil then"), origin);
        self.indented(|this| this.emit_expr_into(right, &target))?;
        self.line("end", origin);
        Ok(true)
    }

    fn emit_direct_local_decl(
        &mut self,
        name: &str,
        expr: &IrExpr,
        origin: &Origin,
    ) -> Result<bool, CodegenError> {
        let IrExprKind::Binary {
            op: BinaryOp::Coalesce,
            left,
            right,
        } = &expr.kind
        else {
            return Ok(false);
        };

        let left = self.emit_expr_setup(left)?;
        if left.setup.is_empty() {
            self.line(format!("local {name} = {}", left.value), origin);
            self.declare_name(name.to_string());
            self.line(format!("if {name} == nil then"), origin);
            self.indented(|this| this.emit_expr_into(right, name))?;
            self.line("end", origin);
        } else {
            self.line(format!("local {name}"), origin);
            self.declare_name(name.to_string());
            self.emit_scoped_setup(left.setup, origin, |this| {
                this.line(format!("{name} = {}", left.value), origin);
                this.line(format!("if {name} == nil then"), origin);
                this.indented(|this| this.emit_expr_into(right, name))?;
                this.line("end", origin);
                Ok(())
            })?;
        }
        Ok(true)
    }

    fn emit_import(
        &mut self,
        source: &str,
        specifiers: &[IrImportSpecifier],
        side_effect_only: bool,
        origin: &Origin,
    ) {
        if let Some(namespace) = single_namespace_import(specifiers)
            && !side_effect_only
            && self.lookup_import_tmp(source).is_none()
        {
            let lifted = self.is_module_scope() && self.is_module_lifted(&namespace.local);
            let target = if lifted {
                self.emit_identifier(&namespace.local)
            } else {
                namespace.local.clone()
            };
            if lifted {
                self.line(
                    format!("{target} = __lux_import({})", lua_string(source)),
                    origin,
                );
            } else {
                self.line(
                    format!("local {target} = __lux_import({})", lua_string(source)),
                    origin,
                );
                self.declare_name(namespace.local.clone());
            }
            self.bind_import_tmp(source, &target);
            return;
        }

        let module_tmp = if let Some(module_tmp) = self.lookup_import_tmp(source) {
            module_tmp
        } else {
            let module_tmp = self.gensym.next("import");
            let module_ref = if self.is_module_scope() && self.module_lift_table().is_some() {
                let module_ref = format!(
                    "{}.{module_tmp}",
                    self.module_lift_table().expect("module lift table")
                );
                self.line(
                    format!("{module_ref} = __lux_import({})", lua_string(source)),
                    origin,
                );
                module_ref
            } else {
                self.line(
                    format!("local {module_tmp} = __lux_import({})", lua_string(source)),
                    origin,
                );
                module_tmp
            };
            self.bind_import_tmp(source, &module_ref);
            module_ref
        };

        if side_effect_only || specifiers.is_empty() {
            return;
        }

        for specifier in specifiers {
            let lifted = self.is_module_scope() && self.is_module_lifted(&specifier.local);
            let target = if lifted {
                self.emit_identifier(&specifier.local)
            } else {
                specifier.local.clone()
            };
            if specifier.namespace {
                if lifted {
                    self.line(format!("{target} = {module_tmp}"), origin);
                } else {
                    self.line(format!("local {target} = {module_tmp}"), origin);
                }
            } else {
                if lifted {
                    self.line(
                        format!("{target} = {module_tmp}.{}", specifier.imported),
                        origin,
                    );
                } else {
                    self.line(
                        format!("local {target} = {module_tmp}.{}", specifier.imported),
                        origin,
                    );
                }
            }
            if !lifted {
                self.declare_name(specifier.local.clone());
            }
        }
    }

    fn emit_lifted_local_decl(
        &mut self,
        names: &[String],
        values: &[IrExpr],
        origin: &Origin,
    ) -> Result<(), CodegenError> {
        if values.is_empty() {
            return Ok(());
        }

        let targets = names
            .iter()
            .map(|name| self.emit_identifier(name))
            .collect::<Vec<_>>();
        let lhs = targets.join(", ");
        if names.len() == 1
            && values.len() == 1
            && prefers_direct_assignment_expr(&values[0])
            && !exprs_reference_any_names(values, names)
        {
            self.emit_expr_into(&values[0], &lhs)?;
            return Ok(());
        }

        let emitted = self.emit_expr_list(values)?;
        if emitted.setup.is_empty() {
            self.line(format!("{lhs} = {}", emitted.values), origin);
        } else {
            self.emit_scoped_setup(emitted.setup, origin, |this| {
                this.line(format!("{lhs} = {}", emitted.values), origin);
                Ok(())
            })?;
        }
        Ok(())
    }

    fn emit_enum_decl(&mut self, decl: &IrEnumDecl, origin: &Origin) -> Result<(), CodegenError> {
        if !decl.runtime {
            return Ok(());
        }

        let target = self.emit_identifier(&decl.name);
        let mut fields = Vec::new();
        let mut setup = Vec::new();
        for variant in &decl.variants {
            let tag = self.emit_expr_setup(&variant.tag)?;
            setup.extend(tag.setup);
            fields.push(format!("{} = {}", variant.name, tag.value));
        }
        self.emit_scoped_setup(setup, origin, |this| {
            if this.is_module_scope() && this.is_module_lifted(&decl.name) {
                this.line(format!("{target} = {{ {} }}", fields.join(", ")), origin);
            } else {
                this.line(
                    format!("local {target} = {{ {} }}", fields.join(", ")),
                    origin,
                );
                this.declare_name(decl.name.clone());
            }
            Ok(())
        })
    }

    fn lookup_import_tmp(&self, source: &str) -> Option<String> {
        self.import_scopes
            .iter()
            .rev()
            .find_map(|scope| scope.get(source).cloned())
    }

    fn bind_import_tmp(&mut self, source: &str, module_tmp: &str) {
        if let Some(scope) = self.import_scopes.last_mut() {
            scope.insert(source.to_string(), module_tmp.to_string());
        }
    }

    fn emit_block_as_statements(&mut self, block: &IrBlock) -> Result<(), CodegenError> {
        self.import_scopes.push(BTreeMap::new());
        self.name_scopes.push(BTreeSet::new());
        let result = (|| {
            self.emit_block_hoists(block);
            self.emit_block_contents(block, BlockTailAction::Statement)
        })();
        self.name_scopes.pop();
        self.import_scopes.pop();
        result
    }

    fn emit_loop_body(&mut self, block: &IrBlock, origin: &Origin) -> Result<(), CodegenError> {
        if !block_has_continue_for_current_loop(block) {
            self.loop_stack
                .push(LoopControlContext { break_flag: None });
            let result = self.emit_block_as_statements(block);
            self.loop_stack.pop();
            return result;
        }

        let break_flag = if block_has_break_for_current_loop(block) {
            let flag = self.gensym.next("break");
            self.line(format!("local {flag} = false"), origin);
            Some(flag)
        } else {
            None
        };

        self.line("repeat", origin);
        self.loop_stack.push(LoopControlContext {
            break_flag: break_flag.clone(),
        });
        let result = self.indented(|this| this.emit_block_as_statements(block));
        self.loop_stack.pop();
        result?;
        self.line("until true", origin);

        if let Some(flag) = break_flag {
            self.line(format!("if {flag} then"), origin);
            self.indented(|this| {
                this.line("break", origin);
                Ok(())
            })?;
            self.line("end", origin);
        }

        Ok(())
    }

    fn emit_block_as_return(&mut self, block: &IrBlock) -> Result<(), CodegenError> {
        self.import_scopes.push(BTreeMap::new());
        self.name_scopes.push(BTreeSet::new());
        let result = (|| {
            self.emit_block_hoists(block);
            self.emit_block_contents(block, BlockTailAction::Return)
        })();
        self.name_scopes.pop();
        self.import_scopes.pop();
        result
    }

    fn emit_block_value_as_return(&mut self, block: &IrBlock) -> Result<(), CodegenError> {
        self.import_scopes.push(BTreeMap::new());
        self.name_scopes.push(BTreeSet::new());
        let result = (|| {
            self.emit_block_hoists(block);
            self.emit_block_contents(block, BlockTailAction::ReturnNil)
        })();
        self.name_scopes.pop();
        self.import_scopes.pop();
        result
    }

    fn emit_block_into(&mut self, block: &IrBlock, target: &str) -> Result<(), CodegenError> {
        self.import_scopes.push(BTreeMap::new());
        self.name_scopes.push(BTreeSet::new());
        let result = (|| {
            self.emit_block_hoists(block);
            self.emit_block_contents(block, BlockTailAction::AssignInto(target))
        })();
        self.name_scopes.pop();
        self.import_scopes.pop();
        result
    }

    fn emit_block_contents(
        &mut self,
        block: &IrBlock,
        tail_action: BlockTailAction<'_>,
    ) -> Result<(), CodegenError> {
        if let Some(plan) = plan_block_scope_narrowing(block) {
            let mut tail_emitted = false;
            for segment in plan {
                if segment.scoped {
                    self.line("do", &block.statements[segment.start].origin);
                    self.indented(|this| {
                        this.import_scopes.push(BTreeMap::new());
                        this.name_scopes.push(BTreeSet::new());
                        let result = this.emit_block_segment(block, &segment, tail_action);
                        this.name_scopes.pop();
                        this.import_scopes.pop();
                        result
                    })?;
                    self.line("end", &block.statements[segment.start].origin);
                } else {
                    self.emit_block_segment(block, &segment, tail_action)?;
                }
                tail_emitted |= segment.include_tail;
            }
            if !tail_emitted {
                self.emit_block_tail(block, tail_action)?;
            }
            return Ok(());
        }

        for stmt in &block.statements {
            self.emit_stmt(stmt)?;
        }
        self.emit_block_tail(block, tail_action)
    }

    fn emit_block_segment(
        &mut self,
        block: &IrBlock,
        segment: &BlockScopeSegment,
        tail_action: BlockTailAction<'_>,
    ) -> Result<(), CodegenError> {
        for stmt in &block.statements[segment.start..segment.end] {
            self.emit_stmt(stmt)?;
        }
        if segment.include_tail {
            self.emit_block_tail(block, tail_action)?;
        }
        Ok(())
    }

    fn emit_block_tail(
        &mut self,
        block: &IrBlock,
        action: BlockTailAction<'_>,
    ) -> Result<(), CodegenError> {
        match (action, block.tail.as_ref()) {
            (BlockTailAction::Statement, Some(tail)) => self.emit_expr_as_stmt(tail),
            (BlockTailAction::Statement, None) => Ok(()),
            (BlockTailAction::Return, Some(tail)) | (BlockTailAction::ReturnNil, Some(tail)) => {
                self.emit_return_expr(tail)
            }
            (BlockTailAction::Return, None) => Ok(()),
            (BlockTailAction::ReturnNil, None) if block_ends_with_terminal_stmt(block) => Ok(()),
            (BlockTailAction::ReturnNil, None) => {
                self.line("return nil", &block.origin);
                Ok(())
            }
            (BlockTailAction::AssignInto(target), Some(tail)) => self.emit_expr_into(tail, target),
            (BlockTailAction::AssignInto(target), None) => {
                self.line(format!("{target} = nil"), &block.origin);
                Ok(())
            }
        }
    }

    fn emit_block_hoists(&mut self, block: &IrBlock) {
        let hoisted = collect_simple_function_hoists(&block.statements);
        for hoist in &hoisted {
            self.line(format!("local {}", hoist.name), &hoist.origin);
            self.declare_name(hoist.name.clone());
        }
        if !hoisted.is_empty() {
            self.blank();
        }
    }

    fn emit_function_decl(
        &mut self,
        decl: &IrFunctionDecl,
        origin: &Origin,
    ) -> Result<(), CodegenError> {
        match &decl.name {
            FunctionName::Simple(name) => {
                let target = self.emit_identifier(&name.name);
                self.line(
                    format!(
                        "{target} = function({})",
                        param_list(&decl.params, decl.vararg)
                    ),
                    origin,
                );
                self.indented(|this| {
                    this.with_name_scope(
                        decl.params.iter().map(|param| param.name.clone()),
                        |this| {
                            this.emit_param_defaults(&decl.params)?;
                            this.emit_function_body(&decl.body)
                        },
                    )
                })?;
                self.line("end", origin);
            }
            FunctionName::Dotted(path) => {
                let name = dotted_name(path);
                self.line(
                    format!(
                        "function {}({})",
                        name,
                        param_list(&decl.params, decl.vararg)
                    ),
                    origin,
                );
                self.indented(|this| {
                    this.with_name_scope(
                        decl.params.iter().map(|param| param.name.clone()),
                        |this| {
                            this.emit_param_defaults(&decl.params)?;
                            this.emit_function_body(&decl.body)
                        },
                    )
                })?;
                self.line("end", origin);
            }
            FunctionName::Method { receiver, method } => {
                let receiver = dotted_name(receiver);
                self.line(
                    format!(
                        "function {}:{}({})",
                        receiver,
                        method.name,
                        param_list(&decl.params, decl.vararg)
                    ),
                    origin,
                );
                self.indented(|this| {
                    let params = std::iter::once("self".to_string())
                        .chain(decl.params.iter().map(|param| param.name.clone()));
                    this.with_name_scope(params, |this| {
                        this.emit_param_defaults(&decl.params)?;
                        this.emit_function_body(&decl.body)
                    })
                })?;
                self.line("end", origin);
            }
        }
        Ok(())
    }

    fn emit_function_body(&mut self, body: &IrFunctionBody) -> Result<(), CodegenError> {
        match body {
            IrFunctionBody::Expr(expr) => self.emit_return_expr(expr),
            IrFunctionBody::Block(block) => self.emit_block_as_return(block),
        }
    }

    fn emit_param_defaults(&mut self, params: &[IrParam]) -> Result<(), CodegenError> {
        for param in params {
            let Some(default) = &param.default else {
                continue;
            };
            if matches!(default.kind, IrExprKind::Nil) {
                continue;
            }
            self.line(format!("if {} == nil then", param.name), &param.origin);
            self.indented(|this| {
                let default = this.emit_expr_setup(default)?;
                this.emit_setup(default.setup);
                this.line(format!("{} = {}", param.name, default.value), &param.origin);
                Ok(())
            })?;
            self.line("end", &param.origin);
        }
        Ok(())
    }

    fn emit_local_destructure(
        &mut self,
        patterns: &[IrPattern],
        values: &[IrExpr],
        origin: &Origin,
    ) -> Result<(), CodegenError> {
        let names = collect_pattern_names(patterns);
        if !names.is_empty() {
            self.line(format!("local {}", names.join(", ")), origin);
            self.declare_names(names.iter().cloned());
        }

        for (index, pattern) in patterns.iter().enumerate() {
            let value = values.get(index).cloned().unwrap_or_else(|| IrExpr {
                kind: IrExprKind::Nil,
                origin: pattern.origin.clone(),
                value_mode: ValueMode::Single,
                symbol: None,
            });
            let value = self.emit_expr_setup(&value)?;
            let temp = self.gensym.next("destructure");
            self.line("do", &pattern.origin);
            self.indented(|this| {
                this.emit_setup(value.setup);
                this.line(format!("local {temp} = {}", value.value), &pattern.origin);
                this.emit_pattern_assign(pattern, &temp)
            })?;
            self.line("end", &pattern.origin);
        }
        Ok(())
    }

    fn emit_pattern_assign(
        &mut self,
        pattern: &IrPattern,
        source: &str,
    ) -> Result<(), CodegenError> {
        match &pattern.kind {
            IrPatternKind::Identifier(name) => {
                self.line(format!("{name} = {source}"), &pattern.origin);
            }
            IrPatternKind::Object(fields) => {
                for field in fields {
                    let field_temp = self.gensym.next("field");
                    self.line(
                        format!("local {field_temp} = {source}.{}", field.key),
                        &field.origin,
                    );
                    if let Some(default) = &field.default {
                        self.emit_default_into(&field_temp, default, &field.origin)?;
                    }
                    self.emit_pattern_assign(&field.pattern, &field_temp)?;
                }
            }
            IrPatternKind::Array(items) => {
                for (index, item) in items.iter().enumerate() {
                    let item_temp = self.gensym.next("item");
                    let lua_index = index + 1;
                    self.line(
                        format!("local {item_temp} = {source}[{lua_index}]"),
                        &item.origin,
                    );
                    if let Some(default) = &item.default {
                        self.emit_default_into(&item_temp, default, &item.origin)?;
                    }
                    self.emit_pattern_assign(&item.pattern, &item_temp)?;
                }
            }
        }
        Ok(())
    }

    fn emit_default_into(
        &mut self,
        target: &str,
        default: &IrExpr,
        origin: &Origin,
    ) -> Result<(), CodegenError> {
        self.line(format!("if {target} == nil then"), origin);
        self.indented(|this| {
            let default = this.emit_expr_setup(default)?;
            this.emit_setup(default.setup);
            this.line(format!("{target} = {}", default.value), origin);
            Ok(())
        })?;
        self.line("end", origin);
        Ok(())
    }

    fn emit_if_stmt(
        &mut self,
        condition: &IrExpr,
        then_block: &IrBlock,
        else_block: Option<&IrBlock>,
        origin: &Origin,
    ) -> Result<(), CodegenError> {
        let condition = self.emit_condition_setup(condition)?;
        self.emit_setup(condition.setup);
        self.line(format!("if {} then", condition.value), origin);
        self.indented(|this| this.emit_block_as_statements(then_block))?;
        if let Some(block) = else_block {
            self.line("else", origin);
            self.indented(|this| this.emit_block_as_statements(block))?;
        }
        self.line("end", origin);
        Ok(())
    }

    fn emit_condition_setup(&mut self, expr: &IrExpr) -> Result<ConditionSetup, CodegenError> {
        let Some(condition) = self.try_emit_inline_condition(expr)? else {
            let setup = self.emit_expr_setup(expr)?;
            return Ok(ConditionSetup {
                setup: setup.setup,
                value: setup.value,
                precedence: setup.precedence,
            });
        };
        Ok(condition)
    }

    fn try_emit_inline_condition(
        &mut self,
        expr: &IrExpr,
    ) -> Result<Option<ConditionSetup>, CodegenError> {
        match &expr.kind {
            IrExprKind::Unary {
                op: UnaryOp::Not,
                argument,
            } => {
                let Some(argument) = self.try_emit_inline_condition(argument)? else {
                    return Ok(None);
                };
                Ok(Some(ConditionSetup {
                    setup: argument.setup,
                    value: format!(
                        "not {}",
                        parenthesize_unary_operand(
                            UnaryOp::Not,
                            argument.value,
                            argument.precedence
                        )
                    ),
                    precedence: LuaPrecedence::Unary,
                }))
            }
            IrExprKind::Binary { op, left, right }
                if matches!(op, BinaryOp::And | BinaryOp::Or) =>
            {
                let Some(left) = self.try_emit_inline_condition(left)? else {
                    return Ok(None);
                };
                let Some(right) = self.try_emit_inline_condition(right)? else {
                    return Ok(None);
                };
                if !right.setup.is_empty() {
                    return Ok(None);
                }

                let precedence = condition_precedence(*op);
                let setup = left.setup;
                let left = parenthesize_condition_operand(left.value, left.precedence, *op);
                let right = parenthesize_condition_operand(right.value, right.precedence, *op);
                Ok(Some(ConditionSetup {
                    setup,
                    value: format_lua_binary(*op, &left, &right),
                    precedence,
                }))
            }
            _ => {
                let setup = self.emit_expr_setup(expr)?;
                Ok(Some(ConditionSetup {
                    setup: setup.setup,
                    value: setup.value,
                    precedence: setup.precedence,
                }))
            }
        }
    }

    fn emit_expr_as_stmt(&mut self, expr: &IrExpr) -> Result<(), CodegenError> {
        if self.emit_conditional_with_mode(expr, ExprEmitMode::Statement)? {
            return Ok(());
        }
        if self.emit_match_with_mode(expr, ExprEmitMode::Statement)? {
            return Ok(());
        }
        if let IrExprKind::Do(block) = &expr.kind {
            self.line("do", &expr.origin);
            self.indented(|this| this.emit_block_as_statements(block))?;
            self.line("end", &expr.origin);
            return Ok(());
        }

        let setup = self.emit_expr_setup(expr)?;
        let final_call = final_call_kind(expr);

        self.emit_scoped_setup(setup.setup, &expr.origin, |this| {
            match final_call {
                Some(FinalCallKind::Inline) => this.line(setup.value, &expr.origin),
                Some(FinalCallKind::AlreadyEmittedInSetup) => {}
                None => {
                    let unused = this.gensym.next("unused");
                    this.line(format!("local {unused} = {}", setup.value), &expr.origin);
                }
            }
            Ok(())
        })?;

        Ok(())
    }

    fn emit_branch_as_stmt(&mut self, branch: &IrExprOrBlock) -> Result<(), CodegenError> {
        match branch {
            IrExprOrBlock::Expr(expr) => self.emit_expr_as_stmt(expr),
            IrExprOrBlock::Block(block) => self.emit_block_as_statements(block),
        }
    }

    fn emit_return_expr(&mut self, expr: &IrExpr) -> Result<(), CodegenError> {
        if self.emit_conditional_with_mode(expr, ExprEmitMode::Return)? {
            return Ok(());
        }
        if self.emit_match_with_mode(expr, ExprEmitMode::Return)? {
            return Ok(());
        }
        if let IrExprKind::Do(block) = &expr.kind {
            self.line("do", &expr.origin);
            self.indented(|this| this.emit_block_value_as_return(block))?;
            self.line("end", &expr.origin);
            return Ok(());
        }

        let setup = self.emit_expr_setup(expr)?;
        self.emit_setup(setup.setup);
        self.line(format!("return {}", setup.value), &expr.origin);
        Ok(())
    }

    fn emit_branch_as_return(&mut self, branch: &IrExprOrBlock) -> Result<(), CodegenError> {
        match branch {
            IrExprOrBlock::Expr(expr) => self.emit_return_expr(expr),
            IrExprOrBlock::Block(block) => self.emit_block_value_as_return(block),
        }
    }

    fn emit_expr_into(&mut self, expr: &IrExpr, target: &str) -> Result<(), CodegenError> {
        if self.emit_coalesce_into(expr, target)? {
            return Ok(());
        }
        if self.emit_conditional_with_mode(expr, ExprEmitMode::AssignInto(target))? {
            return Ok(());
        }
        if self.emit_match_with_mode(expr, ExprEmitMode::AssignInto(target))? {
            return Ok(());
        }
        if let IrExprKind::Do(block) = &expr.kind {
            self.line("do", &expr.origin);
            self.indented(|this| this.emit_block_into(block, target))?;
            self.line("end", &expr.origin);
            return Ok(());
        }

        let setup = self.emit_expr_setup(expr)?;
        self.emit_scoped_setup(setup.setup, &expr.origin, |this| {
            this.line(format!("{target} = {}", setup.value), &expr.origin);
            Ok(())
        })
    }

    fn emit_coalesce_into(&mut self, expr: &IrExpr, target: &str) -> Result<bool, CodegenError> {
        let IrExprKind::Binary {
            op: BinaryOp::Coalesce,
            left,
            right,
        } = &expr.kind
        else {
            return Ok(false);
        };
        let Some(target_name) = simple_lua_identifier(target) else {
            return Ok(false);
        };
        let mut target_names = BTreeSet::new();
        target_names.insert(target_name);
        if expr_references_any_name(right, &target_names) {
            return Ok(false);
        }

        let left = self.emit_expr_setup(left)?;
        self.emit_scoped_setup(left.setup, &expr.origin, |this| {
            this.line(format!("{target} = {}", left.value), &expr.origin);
            this.line(format!("if {target} == nil then"), &expr.origin);
            this.indented(|this| this.emit_expr_into(right, target))?;
            this.line("end", &expr.origin);
            Ok(())
        })?;
        Ok(true)
    }

    fn emit_conditional_with_mode(
        &mut self,
        expr: &IrExpr,
        mode: ExprEmitMode<'_>,
    ) -> Result<bool, CodegenError> {
        if let IrExprKind::Conditional {
            condition,
            then_branch,
            else_branch,
        } = &expr.kind
        {
            let condition = self.emit_condition_setup(condition)?;
            self.emit_scoped_setup(condition.setup, &expr.origin, |this| {
                this.line(format!("if {} then", condition.value), &expr.origin);
                this.indented(|this| this.emit_branch_with_mode(then_branch, mode))?;
                this.line("else", &expr.origin);
                this.indented(|this| this.emit_branch_with_mode(else_branch, mode))?;
                this.line("end", &expr.origin);
                Ok(())
            })?;
            return Ok(true);
        }

        Ok(false)
    }

    fn emit_match_with_mode(
        &mut self,
        expr: &IrExpr,
        mode: ExprEmitMode<'_>,
    ) -> Result<bool, CodegenError> {
        let IrExprKind::Match(match_expr) = &expr.kind else {
            return Ok(false);
        };

        let subject = self.emit_expr_setup(&match_expr.subject)?;
        self.emit_scoped_setup(subject.setup, &expr.origin, |this| {
            let subject_temp = this.gensym.next("match");
            this.line(
                format!("local {subject_temp} = {}", subject.value),
                &expr.origin,
            );

            let reachable_arms = reachable_match_arms(&this.enums, &match_expr.arms);
            let tag_field = this.match_tag_field(&reachable_arms);
            let nil_safe_tag_read = tag_field.is_some()
                && reachable_arms
                    .iter()
                    .any(|arm| ir_match_pattern_is_unconditional(&arm.pattern));
            let tag_temp = if let Some(field) = tag_field.as_ref() {
                let tag = this.gensym.next("tag");
                if nil_safe_tag_read {
                    this.line(format!("local {tag}"), &expr.origin);
                    this.line(
                        format!("if {subject_temp} ~= nil then"),
                        &match_expr.subject.origin,
                    );
                    this.indented(|this| {
                        this.line(format!("{tag} = {subject_temp}.{field}"), &expr.origin);
                        Ok(())
                    })?;
                    this.line("end", &match_expr.subject.origin);
                } else {
                    this.line(
                        format!("local {tag} = {subject_temp}.{field}"),
                        &expr.origin,
                    );
                }
                Some(tag)
            } else {
                None
            };

            let compiled = reachable_arms
                .iter()
                .map(|arm| this.compile_match_arm(arm, &subject_temp, tag_temp.as_deref()))
                .collect::<Result<Vec<_>, _>>()?;

            let mut emitted_unconditional = false;
            for (index, arm) in compiled.iter().enumerate() {
                if let Some(condition) = arm.condition.as_deref() {
                    if index == 0 {
                        this.line(format!("if {condition} then"), &arm.origin);
                    } else {
                        this.line(format!("elseif {condition} then"), &arm.origin);
                    }
                } else {
                    if index == 0 {
                        this.line("if true then", &arm.origin);
                    } else {
                        this.line("else", &arm.origin);
                    }
                    emitted_unconditional = true;
                }
                this.indented(|this| {
                    this.emit_setup(arm.bindings.clone());
                    this.emit_branch_with_mode(&arm.body, mode)
                })?;
                if emitted_unconditional {
                    break;
                }
            }
            if !compiled.is_empty() {
                this.line("end", &expr.origin);
            }
            Ok(())
        })?;

        Ok(true)
    }

    fn match_tag_field(&self, arms: &[&IrMatchArm]) -> Option<String> {
        let mut tag_field = None;
        for arm in arms {
            collect_match_tag_field(&self.enums, &arm.pattern, &mut tag_field);
        }
        tag_field
    }

    fn compile_match_arm(
        &mut self,
        arm: &IrMatchArm,
        subject: &str,
        tag_temp: Option<&str>,
    ) -> Result<CompiledMatchArm, CodegenError> {
        let compiled = self.compile_match_pattern(&arm.pattern, subject, tag_temp)?;
        Ok(CompiledMatchArm {
            condition: join_conditions(compiled.conditions),
            bindings: compiled.bindings,
            body: arm.body.clone(),
            origin: arm.origin.clone(),
        })
    }

    fn compile_match_pattern(
        &mut self,
        pattern: &IrMatchPattern,
        source: &str,
        tag_temp: Option<&str>,
    ) -> Result<CompiledMatchPattern, CodegenError> {
        match &pattern.kind {
            IrMatchPatternKind::Or(patterns) => {
                let mut alternatives = Vec::new();
                let mut seen = BTreeSet::new();
                for pattern in patterns {
                    let compiled = self.compile_match_pattern(pattern, source, tag_temp)?;
                    if !compiled.bindings.is_empty() {
                        return Err(CodegenError {
                            message: "or-pattern alternatives with bindings are not supported yet"
                                .into(),
                        });
                    }
                    let condition = join_conditions(compiled.conditions).unwrap_or("true".into());
                    if condition == "true" {
                        return Ok(CompiledMatchPattern {
                            conditions: Vec::new(),
                            bindings: Vec::new(),
                        });
                    }
                    if seen.insert(condition.clone()) {
                        alternatives.push(condition);
                    }
                }
                Ok(CompiledMatchPattern {
                    conditions: vec![alternatives.join(" or ")],
                    bindings: Vec::new(),
                })
            }
            IrMatchPatternKind::Wildcard => Ok(CompiledMatchPattern::default()),
            IrMatchPatternKind::Binding(name) => {
                if name == "_" {
                    Ok(CompiledMatchPattern::default())
                } else {
                    Ok(CompiledMatchPattern {
                        conditions: Vec::new(),
                        bindings: vec![PendingLine::new(
                            format!("local {name} = {source}"),
                            &pattern.origin,
                        )],
                    })
                }
            }
            IrMatchPatternKind::Literal(literal) => Ok(CompiledMatchPattern {
                conditions: vec![format!("{source} == {}", self.emit_match_literal(literal))],
                bindings: Vec::new(),
            }),
            IrMatchPatternKind::Variant { path, payload } => self.compile_variant_match_pattern(
                path,
                payload.as_ref(),
                source,
                tag_temp,
                pattern,
            ),
            IrMatchPatternKind::Object(fields) => {
                let mut out = CompiledMatchPattern {
                    conditions: vec![format!("type({source}) == \"table\"")],
                    bindings: Vec::new(),
                };
                for field in fields {
                    let field_source = format!("{source}.{}", field.key);
                    let compiled =
                        self.compile_match_pattern(&field.pattern, &field_source, tag_temp)?;
                    out.conditions.extend(compiled.conditions);
                    out.bindings.extend(compiled.bindings);
                }
                Ok(out)
            }
            IrMatchPatternKind::Array(items) => {
                let mut out = CompiledMatchPattern {
                    conditions: vec![format!("type({source}) == \"table\"")],
                    bindings: Vec::new(),
                };
                for (index, item) in items.iter().enumerate() {
                    let field_source = format!("{source}[{}]", index + 1);
                    let compiled =
                        self.compile_match_pattern(&item.pattern, &field_source, tag_temp)?;
                    out.conditions.extend(compiled.conditions);
                    out.bindings.extend(compiled.bindings);
                }
                Ok(out)
            }
        }
    }

    fn compile_variant_match_pattern(
        &mut self,
        path: &[String],
        payload: Option<&IrMatchPatternPayload>,
        source: &str,
        tag_temp: Option<&str>,
        pattern: &IrMatchPattern,
    ) -> Result<CompiledMatchPattern, CodegenError> {
        let Some((enum_decl, variant)) = self.enums.lookup_variant(path) else {
            return Ok(CompiledMatchPattern {
                conditions: vec![format!("{source} == {}", path.join("."))],
                bindings: Vec::new(),
            });
        };
        let repr = enum_decl.repr.clone();
        let variant_tag = variant.tag.clone();
        let variant_payload = variant.payload.clone();
        let tag = self.emit_simple_match_tag(&variant_tag)?;
        let mut out = match &repr {
            IrEnumRepr::String | IrEnumRepr::Number => CompiledMatchPattern {
                conditions: vec![format!("{source} == {tag}")],
                bindings: Vec::new(),
            },
            IrEnumRepr::Table { tag_field } | IrEnumRepr::Existing { tag_field } => {
                let tag_source = tag_temp
                    .map(str::to_string)
                    .unwrap_or_else(|| format!("{source}.{tag_field}"));
                CompiledMatchPattern {
                    conditions: vec![format!("{tag_source} == {tag}")],
                    bindings: Vec::new(),
                }
            }
        };

        if let Some(payload) = payload {
            match payload {
                IrMatchPatternPayload::Tuple(patterns) => {
                    let fields = ir_enum_payload_fields(&variant_payload);
                    for (index, item_pattern) in patterns.iter().enumerate() {
                        let Some(field) = fields.get(index) else {
                            continue;
                        };
                        let field_source = format!("{source}.{field}");
                        let compiled =
                            self.compile_match_pattern(item_pattern, &field_source, tag_temp)?;
                        out.conditions.extend(compiled.conditions);
                        out.bindings.extend(compiled.bindings);
                    }
                }
                IrMatchPatternPayload::Record(fields) => {
                    for field in fields {
                        let field_source = format!("{source}.{}", field.key);
                        let compiled =
                            self.compile_match_pattern(&field.pattern, &field_source, tag_temp)?;
                        out.conditions.extend(compiled.conditions);
                        out.bindings.extend(compiled.bindings);
                    }
                }
            }
        } else if matches!(pattern.kind, IrMatchPatternKind::Variant { .. }) {
            let _ = pattern;
        }

        Ok(out)
    }

    fn emit_match_literal(&self, literal: &IrMatchLiteral) -> String {
        match literal {
            IrMatchLiteral::Nil => "nil".into(),
            IrMatchLiteral::Boolean(value) => {
                if *value {
                    "true".into()
                } else {
                    "false".into()
                }
            }
            IrMatchLiteral::Number(value) => value.clone(),
            IrMatchLiteral::String(value) => lua_string(value),
        }
    }

    fn emit_simple_match_tag(&mut self, tag: &IrExpr) -> Result<String, CodegenError> {
        let emitted = self.emit_expr_setup(tag)?;
        if !emitted.setup.is_empty() {
            return Err(CodegenError {
                message: "enum tags used in match patterns must lower without setup".into(),
            });
        }
        Ok(emitted.value)
    }

    fn emit_branch_into(
        &mut self,
        branch: &IrExprOrBlock,
        target: &str,
    ) -> Result<(), CodegenError> {
        match branch {
            IrExprOrBlock::Expr(expr) => self.emit_expr_into(expr, target),
            IrExprOrBlock::Block(block) => self.emit_block_into(block, target),
        }
    }

    fn emit_branch_with_mode(
        &mut self,
        branch: &IrExprOrBlock,
        mode: ExprEmitMode<'_>,
    ) -> Result<(), CodegenError> {
        match mode {
            ExprEmitMode::Statement => self.emit_branch_as_stmt(branch),
            ExprEmitMode::Return => self.emit_branch_as_return(branch),
            ExprEmitMode::AssignInto(target) => self.emit_branch_into(branch, target),
        }
    }

    fn emit_expr_list(&mut self, exprs: &[IrExpr]) -> Result<ExprListSetup, CodegenError> {
        let mut setup = Vec::new();
        let mut values = Vec::new();
        let last_index = exprs.len().saturating_sub(1);
        for (index, expr) in exprs.iter().enumerate() {
            let item = self.emit_expr_setup(expr)?;
            setup.extend(item.setup);
            if index == last_index && expr.value_mode == ValueMode::MultiTail {
                values.push(item.value);
            } else if index == last_index {
                values.push(force_tail_single_value_expr(expr, item.value));
            } else {
                values.push(item.value);
            }
        }
        Ok(ExprListSetup {
            setup,
            items: values.clone(),
            values: values.join(", "),
        })
    }

    fn emit_expr_setup(&mut self, expr: &IrExpr) -> Result<ExprSetup, CodegenError> {
        match &expr.kind {
            IrExprKind::Identifier(name) => Ok(ExprSetup::value(self.emit_identifier(name))),
            IrExprKind::Nil => Ok(ExprSetup::value("nil").with_may_be_nil(true)),
            IrExprKind::Boolean(value) => {
                Ok(ExprSetup::value(if *value { "true" } else { "false" })
                    .with_statically_non_nil())
            }
            IrExprKind::Number(value) => {
                Ok(ExprSetup::value(value.clone()).with_statically_non_nil())
            }
            IrExprKind::String(value) => {
                Ok(ExprSetup::value(lua_string(value)).with_statically_non_nil())
            }
            IrExprKind::Vararg => Ok(ExprSetup::value("...").with_may_be_nil(true)),
            IrExprKind::PipelinePlaceholder => {
                let Some(value) = self.pipeline_placeholders.last() else {
                    return Err(CodegenError {
                        message: "pipeline placeholder `%` used outside `|>`".into(),
                    });
                };
                Ok(ExprSetup::value(value.clone()))
            }
            IrExprKind::Template(parts) => self.emit_template(parts),
            IrExprKind::Table(fields) => self.emit_table(fields),
            IrExprKind::Unary { op, argument } => {
                let argument = self.emit_expr_setup(argument)?;
                let op_kind = *op;
                let op_text = match op_kind {
                    UnaryOp::Not => "not ",
                    UnaryOp::Len => "#",
                    UnaryOp::Neg => "-",
                };
                Ok(ExprSetup {
                    setup: argument.setup,
                    value: format!(
                        "{op_text}{}",
                        parenthesize_unary_operand(op_kind, argument.value, argument.precedence)
                    ),
                    precedence: LuaPrecedence::Unary,
                    may_be_nil: false,
                    statically_non_nil: false,
                })
            }
            IrExprKind::Binary { op, left, right } => {
                self.emit_binary(*op, left, right, &expr.origin)
            }
            IrExprKind::Conditional { .. } => {
                let target = self.gensym.next("tmp");
                let mut setup = vec![PendingLine::new(format!("local {target}"), &expr.origin)];
                let mut nested = self.fork();
                nested.emit_expr_into(expr, &target)?;
                self.gensym.next_id = nested.gensym.next_id;
                setup.extend(nested.into_lines());
                Ok(ExprSetup {
                    setup,
                    value: target,
                    precedence: LuaPrecedence::Primary,
                    may_be_nil: true,
                    statically_non_nil: false,
                })
            }
            IrExprKind::Match(_) => {
                let target = self.gensym.next("tmp");
                let mut setup = vec![PendingLine::new(format!("local {target}"), &expr.origin)];
                let mut nested = self.fork();
                nested.emit_expr_into(expr, &target)?;
                self.gensym.next_id = nested.gensym.next_id;
                setup.extend(nested.into_lines());
                Ok(ExprSetup {
                    setup,
                    value: target,
                    precedence: LuaPrecedence::Primary,
                    may_be_nil: true,
                    statically_non_nil: false,
                })
            }
            IrExprKind::Do(block) => {
                let target = self.gensym.next("tmp");
                let mut setup = vec![PendingLine::new(format!("local {target}"), &expr.origin)];
                let mut nested = self.fork();
                nested.emit_block_into(block, &target)?;
                self.gensym.next_id = nested.gensym.next_id;
                setup.extend(nested.into_lines());
                Ok(ExprSetup {
                    setup,
                    value: target,
                    precedence: LuaPrecedence::Primary,
                    may_be_nil: true,
                    statically_non_nil: false,
                })
            }
            IrExprKind::Function(function) => self.emit_function_expr(function, &expr.origin),
            IrExprKind::Chain(chain) => self.emit_chain(chain, &expr.origin),
        }
    }

    fn emit_binary(
        &mut self,
        op: BinaryOp,
        left: &IrExpr,
        right: &IrExpr,
        origin: &Origin,
    ) -> Result<ExprSetup, CodegenError> {
        if op == BinaryOp::Coalesce {
            return self.emit_coalesce(left, right, origin);
        }
        if op == BinaryOp::Pipe {
            return self.emit_pipe(left, right, origin);
        }
        if matches!(op, BinaryOp::And | BinaryOp::Or) {
            return self.emit_short_circuit(op, left, right, origin);
        }

        let left = self.emit_expr_setup(left)?;
        let right = self.emit_expr_setup(right)?;
        let precedence = binary_precedence(op);
        let left_value = parenthesize_binary_operand(
            left.value.clone(),
            left.precedence,
            op,
            BinaryOperandSide::Left,
        );
        let right_value = parenthesize_binary_operand(
            right.value.clone(),
            right.precedence,
            op,
            BinaryOperandSide::Right,
        );
        let needs_ordering_nil_guard =
            is_ordering_comparison(op) && (left.needs_nil_guard() || right.needs_nil_guard());
        let ordering_nil_guard_condition = needs_ordering_nil_guard
            .then(|| comparison_nil_guard_condition(&left, &left_value, &right, &right_value));
        let mut setup = left.setup;
        setup.extend(right.setup);

        if needs_ordering_nil_guard {
            let result = self.gensym.next("cmp");
            setup.push(PendingLine::new(format!("local {result} = false"), origin));
            let condition = ordering_nil_guard_condition.expect("guard condition");
            setup.push(PendingLine::new(format!("if {condition} then"), origin));
            setup.push(PendingLine::new(
                format!(
                    "  {result} = {} {} {}",
                    left_value,
                    lua_binary_op(op),
                    right_value
                ),
                origin,
            ));
            setup.push(PendingLine::new("end", origin));
            return Ok(ExprSetup {
                setup,
                value: result,
                precedence: LuaPrecedence::Primary,
                may_be_nil: false,
                statically_non_nil: false,
            });
        }

        Ok(ExprSetup {
            setup,
            value: format_lua_binary(op, &left_value, &right_value),
            precedence,
            may_be_nil: false,
            statically_non_nil: false,
        })
    }

    fn emit_template(&mut self, parts: &[IrTemplatePart]) -> Result<ExprSetup, CodegenError> {
        if parts.is_empty() {
            return Ok(ExprSetup::value(lua_string("")));
        }

        let mut setup = Vec::new();
        let mut emitted = Vec::new();
        for part in parts {
            match &part.kind {
                IrTemplatePartKind::Text(text) => emitted.push(lua_string(text)),
                IrTemplatePartKind::Expr(expr) => {
                    let expr = self.emit_expr_setup(expr)?;
                    setup.extend(expr.setup);
                    emitted.push(format!("tostring({})", expr.value));
                }
            }
        }

        Ok(ExprSetup {
            setup,
            value: format_lua_concat_parts(&emitted),
            precedence: LuaPrecedence::Concat,
            may_be_nil: false,
            statically_non_nil: true,
        })
    }

    fn emit_table(&mut self, fields: &[IrTableField]) -> Result<ExprSetup, CodegenError> {
        if fields
            .iter()
            .any(|field| matches!(field.kind, IrTableFieldKind::Spread(_)))
        {
            return self.emit_spread_table(fields);
        }

        if fields.is_empty() {
            return Ok(ExprSetup {
                setup: Vec::new(),
                value: "{}".into(),
                precedence: LuaPrecedence::Primary,
                may_be_nil: false,
                statically_non_nil: true,
            });
        }

        let mut setup = Vec::new();
        let mut emitted = Vec::new();

        let last_array_index = fields
            .iter()
            .rposition(|field| matches!(field.kind, IrTableFieldKind::Array(_)));

        for (index, field) in fields.iter().enumerate() {
            match &field.kind {
                IrTableFieldKind::Array(expr) => {
                    let original_expr = expr;
                    let multi_tail = original_expr.value_mode == ValueMode::MultiTail;
                    let expr = self.emit_expr_setup(original_expr)?;
                    let value = if multi_tail {
                        expr.value
                    } else if Some(index) == last_array_index {
                        force_tail_single_value_expr(original_expr, expr.value)
                    } else {
                        expr.value
                    };
                    setup.extend(expr.setup);
                    emitted.push(value);
                }
                IrTableFieldKind::Named { name, value } => {
                    let value = self.emit_expr_setup(value)?;
                    setup.extend(value.setup);
                    emitted.push(format!("{name} = {}", value.value));
                }
                IrTableFieldKind::ExprKey { key, value } => {
                    let key = self.emit_expr_setup(key)?;
                    let value = self.emit_expr_setup(value)?;
                    setup.extend(key.setup);
                    setup.extend(value.setup);
                    emitted.push(format!("[{}] = {}", key.value, value.value));
                }
                IrTableFieldKind::Spread(_) => unreachable!("handled by emit_spread_table"),
            }
        }

        Ok(ExprSetup {
            setup,
            value: format_lua_table(&emitted),
            precedence: LuaPrecedence::Primary,
            may_be_nil: false,
            statically_non_nil: true,
        })
    }

    fn emit_spread_table(&mut self, fields: &[IrTableField]) -> Result<ExprSetup, CodegenError> {
        let result = self.gensym.next("table");
        let mut setup = vec![PendingLine::new(
            format!("local {result} = {{}}"),
            &fields
                .first()
                .map(|field| field.origin.clone())
                .unwrap_or_else(|| Origin::Synthetic {
                    source: self.module.origin.span(),
                    reason: "empty spread table".into(),
                }),
        )];

        for field in fields {
            match &field.kind {
                IrTableFieldKind::Array(expr) => {
                    let item = self.emit_expr_setup(expr)?;
                    setup.extend(item.setup);
                    setup.push(PendingLine::new(
                        format!("{result}[#{result} + 1] = {}", item.value),
                        &field.origin,
                    ));
                }
                IrTableFieldKind::Named { name, value } => {
                    let value = self.emit_expr_setup(value)?;
                    setup.extend(value.setup);
                    setup.push(PendingLine::new(
                        format!("{result}.{name} = {}", value.value),
                        &field.origin,
                    ));
                }
                IrTableFieldKind::ExprKey { key, value } => {
                    let key = self.emit_expr_setup(key)?;
                    let value = self.emit_expr_setup(value)?;
                    setup.extend(key.setup);
                    setup.extend(value.setup);
                    setup.push(PendingLine::new(
                        format!("{result}[{}] = {}", key.value, value.value),
                        &field.origin,
                    ));
                }
                IrTableFieldKind::Spread(value) => {
                    let spread = self.emit_expr_setup(value)?;
                    let table = self.gensym.next("spread");
                    let key = self.gensym.next("k");
                    let item = self.gensym.next("v");
                    setup.extend(spread.setup);
                    setup.push(PendingLine::new(
                        format!("local {table} = {}", spread.value),
                        &field.origin,
                    ));
                    setup.push(PendingLine::new(
                        format!("if {table} ~= nil then"),
                        &field.origin,
                    ));
                    setup.push(PendingLine::new(
                        format!("  for {key}, {item} in pairs({table}) do"),
                        &field.origin,
                    ));
                    setup.push(PendingLine::new(
                        format!("    {result}[{key}] = {item}"),
                        &field.origin,
                    ));
                    setup.push(PendingLine::new("  end", &field.origin));
                    setup.push(PendingLine::new("end", &field.origin));
                }
            }
        }

        Ok(ExprSetup {
            setup,
            value: result,
            precedence: LuaPrecedence::Primary,
            may_be_nil: false,
            statically_non_nil: false,
        })
    }

    fn emit_coalesce(
        &mut self,
        left: &IrExpr,
        right: &IrExpr,
        origin: &Origin,
    ) -> Result<ExprSetup, CodegenError> {
        let mut terms = Vec::new();
        collect_coalesce_terms(left, &mut terms);
        collect_coalesce_terms(right, &mut terms);

        if terms.len() == 2
            && let Some(emitted) = self.emit_optional_member_coalesce(terms[0], terms[1], origin)?
        {
            return Ok(emitted);
        }

        let result_hint = terms.first().and_then(|term| expr_temp_hint(term));
        let Some((first, rest)) = terms.split_first() else {
            unreachable!("coalesce expression always has at least two terms")
        };
        let first = self.emit_expr_setup(first)?;
        let result = self.gensym.next_hinted("tmp", result_hint.as_deref());
        let mut setup = first.setup;
        let mut may_be_nil = first.may_be_nil;
        let mut statically_non_nil = first.statically_non_nil;

        setup.push(PendingLine::new(
            format!("local {result} = {}", first.value),
            origin,
        ));

        for term in rest {
            let term = self.emit_expr_setup(term)?;
            setup.push(PendingLine::new(format!("if {result} == nil then"), origin));
            setup.extend(indent_pending(term.setup, 1));
            setup.push(PendingLine::new(
                format!("  {result} = {}", term.value),
                origin,
            ));
            setup.push(PendingLine::new("end", origin));
            may_be_nil = if statically_non_nil {
                false
            } else {
                term.may_be_nil
            };
            statically_non_nil |= term.statically_non_nil;
        }

        Ok(ExprSetup {
            setup,
            value: result,
            precedence: LuaPrecedence::Primary,
            may_be_nil,
            statically_non_nil,
        })
    }

    fn emit_optional_member_coalesce(
        &mut self,
        left: &IrExpr,
        right: &IrExpr,
        origin: &Origin,
    ) -> Result<Option<ExprSetup>, CodegenError> {
        let IrExprKind::Chain(chain) = &left.kind else {
            return Ok(None);
        };
        let Some((last, receiver_segments)) = chain.segments.split_last() else {
            return Ok(None);
        };
        let IrChainSegmentKind::Member {
            name,
            optional: true,
        } = &last.kind
        else {
            return Ok(None);
        };

        let receiver = IrExpr {
            kind: IrExprKind::Chain(IrChain {
                base: chain.base.clone(),
                segments: receiver_segments.to_vec(),
            }),
            origin: chain.base.origin.clone(),
            value_mode: ValueMode::Single,
            symbol: None,
        };
        let receiver = self.emit_expr_setup(&receiver)?;
        let obj = self
            .gensym
            .next_hinted("obj", expr_temp_hint(&chain.base).as_deref());
        let result = self.gensym.next_hinted("tmp", Some(name));
        let mut setup = receiver.setup;
        setup.push(PendingLine::new(
            format!("local {obj} = {}", receiver.value),
            &last.origin,
        ));
        setup.push(PendingLine::new(format!("local {result} = nil"), origin));
        setup.push(PendingLine::new(
            format!("if {obj} ~= nil then"),
            &last.origin,
        ));
        setup.push(PendingLine::new(
            format!("  {result} = {obj}.{name}"),
            &last.origin,
        ));
        setup.push(PendingLine::new("end", &last.origin));
        setup.push(PendingLine::new(format!("if {result} == nil then"), origin));

        let right = self.emit_expr_setup(right)?;
        setup.extend(indent_pending(right.setup, 1));
        setup.push(PendingLine::new(
            format!("  {result} = {}", right.value),
            origin,
        ));
        setup.push(PendingLine::new("end", origin));

        Ok(Some(ExprSetup {
            setup,
            value: result,
            precedence: LuaPrecedence::Primary,
            may_be_nil: right.may_be_nil,
            statically_non_nil: right.statically_non_nil,
        }))
    }

    fn emit_short_circuit(
        &mut self,
        op: BinaryOp,
        left: &IrExpr,
        right: &IrExpr,
        origin: &Origin,
    ) -> Result<ExprSetup, CodegenError> {
        let left = self.emit_expr_setup(left)?;
        if left.setup.is_empty() {
            let right = self.emit_expr_setup(right)?;
            if right.setup.is_empty() {
                let precedence = binary_precedence(op);
                let left_value = parenthesize_binary_operand(
                    left.value,
                    left.precedence,
                    op,
                    BinaryOperandSide::Left,
                );
                let right_value = parenthesize_binary_operand(
                    right.value,
                    right.precedence,
                    op,
                    BinaryOperandSide::Right,
                );
                return Ok(ExprSetup {
                    setup: Vec::new(),
                    value: format_lua_binary(op, &left_value, &right_value),
                    precedence,
                    may_be_nil: left.may_be_nil || right.may_be_nil,
                    statically_non_nil: false,
                });
            }
            return self.emit_guarded_short_circuit(op, left, right, origin);
        }

        let right = self.emit_expr_setup(right)?;
        self.emit_guarded_short_circuit(op, left, right, origin)
    }

    fn emit_guarded_short_circuit(
        &mut self,
        op: BinaryOp,
        left: ExprSetup,
        right: ExprSetup,
        origin: &Origin,
    ) -> Result<ExprSetup, CodegenError> {
        let result = self.gensym.next("tmp");
        let mut setup = left.setup;

        setup.push(PendingLine::new(
            format!("local {result} = {}", left.value),
            origin,
        ));
        let condition = match op {
            BinaryOp::And => result.clone(),
            BinaryOp::Or => format!("not {result}"),
            _ => unreachable!("only logical operators use short-circuit lowering"),
        };
        setup.push(PendingLine::new(format!("if {condition} then"), origin));

        setup.extend(indent_pending(right.setup, 1));
        setup.push(PendingLine::new(
            format!("  {result} = {}", right.value),
            origin,
        ));
        setup.push(PendingLine::new("end", origin));

        Ok(ExprSetup {
            setup,
            value: result,
            precedence: LuaPrecedence::Primary,
            may_be_nil: left.may_be_nil || right.may_be_nil,
            statically_non_nil: false,
        })
    }

    fn emit_function_expr(
        &mut self,
        function: &IrFunctionExpr,
        origin: &Origin,
    ) -> Result<ExprSetup, CodegenError> {
        let mut params = function.params.clone();
        if function.implicit_self {
            params.insert(
                0,
                IrParam {
                    name: "self".into(),
                    default: None,
                    origin: origin.clone(),
                },
            );
        }

        let mut nested = self.fork();
        nested.line(
            format!("function({})", param_list(&params, function.vararg)),
            origin,
        );
        nested.indented(|this| {
            this.with_name_scope(params.iter().map(|param| param.name.clone()), |this| {
                this.emit_param_defaults(&params)?;
                this.emit_function_body(&function.body)
            })
        })?;
        nested.line("end", origin);
        self.gensym.next_id = nested.gensym.next_id;

        Ok(ExprSetup {
            setup: Vec::new(),
            value: nested.into_lines_text(),
            precedence: LuaPrecedence::Primary,
            may_be_nil: false,
            statically_non_nil: true,
        })
    }

    fn emit_chain(&mut self, chain: &IrChain, origin: &Origin) -> Result<ExprSetup, CodegenError> {
        if let Some(emitted) = self.emit_scalar_enum_variant_expr(chain)? {
            return Ok(emitted);
        }

        let mut current = self.emit_expr_setup(&chain.base)?;
        let mut current_hint = expr_temp_hint(&chain.base);

        for segment in &chain.segments {
            match &segment.kind {
                IrChainSegmentKind::Member { name, optional } => {
                    if *optional {
                        let obj = self.gensym.next_hinted("obj", current_hint.as_deref());
                        let result = self.gensym.next_hinted("val", Some(name));
                        current.setup.push(PendingLine::new(
                            format!("local {obj} = {}", current.value),
                            &segment.origin,
                        ));
                        current.setup.push(PendingLine::new(
                            format!("local {result} = nil"),
                            &segment.origin,
                        ));
                        current.setup.push(PendingLine::new(
                            format!("if {obj} ~= nil then"),
                            &segment.origin,
                        ));
                        current.setup.push(PendingLine::new(
                            format!("  {result} = {obj}.{name}"),
                            &segment.origin,
                        ));
                        current.setup.push(PendingLine::new("end", &segment.origin));
                        current.value = result;
                        current.precedence = LuaPrecedence::Primary;
                        current.may_be_nil = true;
                        current_hint = Some(name.clone());
                    } else {
                        current.value = format!(
                            "{}.{}",
                            chain_prefix_expr(&current.value, current.precedence),
                            name
                        );
                        current.precedence = LuaPrecedence::Primary;
                        current.may_be_nil = false;
                        current_hint = Some(name.clone());
                    }
                }
                IrChainSegmentKind::Index { index, optional } => {
                    if *optional {
                        let obj = self.gensym.next_hinted("obj", current_hint.as_deref());
                        let key = self
                            .gensym
                            .next_hinted("key", expr_temp_hint(index).as_deref());
                        let result = self.gensym.next_hinted("val", current_hint.as_deref());
                        current.setup.push(PendingLine::new(
                            format!("local {obj} = {}", current.value),
                            &segment.origin,
                        ));
                        current.setup.push(PendingLine::new(
                            format!("local {result} = nil"),
                            &segment.origin,
                        ));
                        current.setup.push(PendingLine::new(
                            format!("if {obj} ~= nil then"),
                            &segment.origin,
                        ));
                        let index_setup = self.emit_expr_setup(index)?;
                        current.setup.extend(indent_pending(index_setup.setup, 1));
                        current.setup.push(PendingLine::new(
                            format!("  local {key} = {}", index_setup.value),
                            &segment.origin,
                        ));
                        current.setup.push(PendingLine::new(
                            format!("  {result} = {obj}[{key}]"),
                            &segment.origin,
                        ));
                        current.setup.push(PendingLine::new("end", &segment.origin));
                        current.value = result;
                        current.precedence = LuaPrecedence::Primary;
                        current.may_be_nil = true;
                        current_hint = None;
                    } else {
                        let index = self.emit_expr_setup(index)?;
                        current.setup.extend(index.setup);
                        current.value = format!(
                            "{}[{}]",
                            chain_prefix_expr(&current.value, current.precedence),
                            index.value
                        );
                        current.precedence = LuaPrecedence::Primary;
                        current.may_be_nil = false;
                        current_hint = None;
                    }
                }
                IrChainSegmentKind::Call { args, .. } => {
                    let args = self.emit_expr_list(args)?;
                    current.setup.extend(args.setup);
                    current.value = format_lua_call(
                        &callable_expr(&chain_prefix_expr(&current.value, current.precedence)),
                        &args.items,
                    );
                    current.precedence = LuaPrecedence::Primary;
                    current.may_be_nil = true;
                    current_hint = None;
                }
                IrChainSegmentKind::SafeDotCall { name, args, .. } => {
                    let obj = self.gensym.next_hinted("obj", current_hint.as_deref());
                    let func = self.gensym.next_hinted("fn", Some(name));
                    let result = self.gensym.next_hinted("val", Some(name));

                    current.setup.push(PendingLine::new(
                        format!("local {obj} = {}", current.value),
                        &segment.origin,
                    ));
                    current.setup.push(PendingLine::new(
                        format!("local {result} = nil"),
                        &segment.origin,
                    ));
                    current.setup.push(PendingLine::new(
                        format!("if {obj} ~= nil then"),
                        &segment.origin,
                    ));
                    current.setup.push(PendingLine::new(
                        format!("  local {func} = {obj}.{name}"),
                        &segment.origin,
                    ));
                    current.setup.push(PendingLine::new(
                        format!("  if {func} ~= nil then"),
                        &segment.origin,
                    ));
                    let args = self.emit_expr_list(args)?;
                    current.setup.extend(indent_pending(args.setup, 2));
                    current.setup.push(PendingLine::new(
                        format!("    {result} = {}", format_lua_call(&func, &args.items)),
                        &segment.origin,
                    ));
                    current
                        .setup
                        .push(PendingLine::new("  end", &segment.origin));
                    current.setup.push(PendingLine::new("end", &segment.origin));
                    current.value = result;
                    current.precedence = LuaPrecedence::Primary;
                    current.may_be_nil = true;
                    current_hint = Some(name.clone());
                }
                IrChainSegmentKind::MethodCall {
                    name,
                    args,
                    optional,
                    ..
                } => {
                    if *optional {
                        let obj = self.gensym.next_hinted("obj", current_hint.as_deref());
                        let method = self.gensym.next_hinted("method", Some(name));
                        let result = self.gensym.next_hinted("val", Some(name));
                        current.setup.push(PendingLine::new(
                            format!("local {obj} = {}", current.value),
                            &segment.origin,
                        ));
                        current.setup.push(PendingLine::new(
                            format!("local {result} = nil"),
                            &segment.origin,
                        ));
                        current.setup.push(PendingLine::new(
                            format!("if {obj} ~= nil then"),
                            &segment.origin,
                        ));
                        current.setup.push(PendingLine::new(
                            format!("  local {method} = {obj}.{name}"),
                            &segment.origin,
                        ));
                        current.setup.push(PendingLine::new(
                            format!("  if {method} ~= nil then"),
                            &segment.origin,
                        ));
                        let args = self.emit_expr_list(args)?;
                        current.setup.extend(indent_pending(args.setup, 2));
                        let mut method_args = vec![obj.clone()];
                        method_args.extend(args.items);
                        current.setup.push(PendingLine::new(
                            format!("    {result} = {}", format_lua_call(&method, &method_args)),
                            &segment.origin,
                        ));
                        current
                            .setup
                            .push(PendingLine::new("  end", &segment.origin));
                        current.setup.push(PendingLine::new("end", &segment.origin));
                        current.value = result;
                        current.precedence = LuaPrecedence::Primary;
                        current.may_be_nil = true;
                        current_hint = Some(name.clone());
                    } else {
                        let args = self.emit_expr_list(args)?;
                        current.setup.extend(args.setup);
                        current.value = format_lua_call(
                            &format!(
                                "{}:{name}",
                                chain_prefix_expr(&current.value, current.precedence)
                            ),
                            &args.items,
                        );
                        current.precedence = LuaPrecedence::Primary;
                        current.may_be_nil = true;
                        current_hint = Some(name.clone());
                    }
                }
            }
        }

        let _ = origin;
        Ok(current)
    }

    fn emit_scalar_enum_variant_expr(
        &mut self,
        chain: &IrChain,
    ) -> Result<Option<ExprSetup>, CodegenError> {
        let Some(path) = scalar_enum_variant_chain_path(chain) else {
            return Ok(None);
        };
        let Some((enum_decl, variant)) = self.enums.lookup_variant(&path) else {
            return Ok(None);
        };
        if !matches!(enum_decl.repr, IrEnumRepr::String | IrEnumRepr::Number) {
            return Ok(None);
        }

        let tag = variant.tag.clone();
        let value = self.emit_simple_match_tag(&tag)?;
        Ok(Some(ExprSetup {
            setup: Vec::new(),
            value,
            precedence: LuaPrecedence::Primary,
            may_be_nil: false,
            statically_non_nil: true,
        }))
    }

    fn emit_compound_assign(
        &mut self,
        target: &IrPlace,
        op: CompoundAssignOp,
        value: &IrExpr,
        origin: &Origin,
    ) -> Result<(), CodegenError> {
        let place = self.emit_place_ref(target, PlaceMode::Stable)?;
        let value = self.emit_expr_setup(value)?;
        let op = lua_compound_op(op);
        let mut setup = place.setup;
        setup.extend(value.setup);
        self.emit_scoped_setup(setup, origin, |this| {
            this.line(
                format!(
                    "{} = {} {op} {}",
                    place.write_target, place.read_expr, value.value
                ),
                origin,
            );
            Ok(())
        })
    }

    fn emit_place_ref(
        &mut self,
        target: &IrPlace,
        mode: PlaceMode,
    ) -> Result<PlaceSetup, CodegenError> {
        match target {
            IrPlace::Identifier(name) => Ok(PlaceSetup {
                setup: Vec::new(),
                read_expr: self.emit_identifier(name),
                write_target: self.emit_identifier(name),
            }),
            IrPlace::Member { object, name } => {
                let object_origin = object.origin.clone();
                let object_is_stable = is_stable_place_component(object);
                let object = self.emit_expr_setup(object)?;
                let mut setup = object.setup;
                let object_value = if mode == PlaceMode::Stable && !object_is_stable {
                    let obj = self.gensym.next("obj");
                    setup.push(PendingLine::new(
                        format!("local {obj} = {}", object.value),
                        &object_origin,
                    ));
                    obj
                } else {
                    object.value
                };
                Ok(PlaceSetup {
                    setup,
                    read_expr: format!("{object_value}.{name}"),
                    write_target: format!("{object_value}.{name}"),
                })
            }
            IrPlace::Index { object, index } => {
                let object_origin = object.origin.clone();
                let index_origin = index.origin.clone();
                let object_is_stable = is_stable_place_component(object);
                let index_is_stable = is_stable_place_component(index);
                let object = self.emit_expr_setup(object)?;
                let index = self.emit_expr_setup(index)?;
                let mut setup = object.setup;
                setup.extend(index.setup);
                let object_value = if mode == PlaceMode::Stable && !object_is_stable {
                    let obj = self.gensym.next("tbl");
                    setup.push(PendingLine::new(
                        format!("local {obj} = {}", object.value),
                        &object_origin,
                    ));
                    obj
                } else {
                    object.value
                };
                let index_value = if mode == PlaceMode::Stable && !index_is_stable {
                    let key = self.gensym.next("key");
                    setup.push(PendingLine::new(
                        format!("local {key} = {}", index.value),
                        &index_origin,
                    ));
                    key
                } else {
                    index.value
                };
                Ok(PlaceSetup {
                    setup,
                    read_expr: format!("{object_value}[{index_value}]"),
                    write_target: format!("{object_value}[{index_value}]"),
                })
            }
        }
    }

    fn emit_pipe(
        &mut self,
        left: &IrExpr,
        right: &IrExpr,
        origin: &Origin,
    ) -> Result<ExprSetup, CodegenError> {
        let left_setup = self.emit_expr_setup(left)?;
        let pipe = self.gensym.next("pipe");
        let mut setup = left_setup.setup;
        setup.push(PendingLine::new(
            format!("local {pipe} = {}", left_setup.value),
            origin,
        ));

        self.pipeline_placeholders.push(pipe);
        let right_setup = self.emit_expr_setup(right);
        self.pipeline_placeholders.pop();
        let right_setup = right_setup?;

        setup.extend(right_setup.setup);
        Ok(ExprSetup {
            setup,
            value: right_setup.value,
            precedence: right_setup.precedence,
            may_be_nil: right_setup.may_be_nil,
            statically_non_nil: right_setup.statically_non_nil,
        })
    }

    fn emit_setup(&mut self, setup: Vec<PendingLine>) {
        for line in setup {
            self.writer.line(line.text, line.origin.as_ref());
        }
    }

    fn emit_scoped_setup(
        &mut self,
        setup: Vec<PendingLine>,
        origin: &Origin,
        body: impl FnOnce(&mut Self) -> Result<(), CodegenError>,
    ) -> Result<(), CodegenError> {
        if setup.is_empty() {
            return body(self);
        }

        self.line("do", origin);
        self.indented(|this| {
            this.emit_setup(setup);
            body(this)
        })?;
        self.line("end", origin);
        Ok(())
    }

    fn line(&mut self, line: impl AsRef<str>, origin: &Origin) {
        self.writer.line(line, Some(origin));
    }

    fn blank(&mut self) {
        self.writer.line("", None);
    }

    fn indented(
        &mut self,
        f: impl FnOnce(&mut Self) -> Result<(), CodegenError>,
    ) -> Result<(), CodegenError> {
        self.writer.indent();
        let result = f(self);
        self.writer.dedent();
        result
    }

    fn fork(&self) -> Self {
        Self {
            module: self.module,
            writer: LuaWriter::new(),
            gensym: Gensym {
                next_id: self.gensym.next_id,
                reserved: self.gensym.reserved.clone(),
            },
            import_scopes: self.import_scopes.clone(),
            name_scopes: self.name_scopes.clone(),
            pipeline_placeholders: self.pipeline_placeholders.clone(),
            module_lift: self.module_lift.clone(),
            loop_stack: self.loop_stack.clone(),
            enums: self.enums.clone(),
        }
    }

    fn into_lines(self) -> Vec<PendingLine> {
        let (lua, map) = self.writer.finish();
        let mut origins_by_line = BTreeMap::new();
        for mapping in map.mappings() {
            origins_by_line
                .entry(mapping.generated_line)
                .or_insert(mapping.source);
        }

        lua.lines()
            .enumerate()
            .map(|(index, line)| PendingLine {
                text: line.to_string(),
                origin: origins_by_line
                    .get(&(index + 1))
                    .copied()
                    .map(Origin::source),
            })
            .collect()
    }

    fn into_lines_text(self) -> String {
        let (lua, _map) = self.writer.finish();
        lua.trim_end().to_string()
    }
}

#[derive(Debug, Clone)]
struct PendingLine {
    text: String,
    origin: Option<Origin>,
}

impl PendingLine {
    fn new(text: impl Into<String>, origin: &Origin) -> Self {
        Self {
            text: text.into(),
            origin: Some(origin.clone()),
        }
    }
}

#[derive(Debug)]
struct ExprSetup {
    setup: Vec<PendingLine>,
    value: String,
    precedence: LuaPrecedence,
    may_be_nil: bool,
    statically_non_nil: bool,
}

impl ExprSetup {
    fn value(value: impl Into<String>) -> Self {
        Self {
            setup: Vec::new(),
            value: value.into(),
            precedence: LuaPrecedence::Primary,
            may_be_nil: false,
            statically_non_nil: false,
        }
    }

    fn with_may_be_nil(mut self, may_be_nil: bool) -> Self {
        self.may_be_nil = may_be_nil;
        self
    }

    fn with_statically_non_nil(mut self) -> Self {
        self.statically_non_nil = true;
        self
    }

    fn needs_nil_guard(&self) -> bool {
        self.may_be_nil && !self.statically_non_nil
    }
}

#[derive(Debug)]
struct ExprListSetup {
    setup: Vec<PendingLine>,
    items: Vec<String>,
    values: String,
}

#[derive(Debug)]
struct ConditionSetup {
    setup: Vec<PendingLine>,
    value: String,
    precedence: LuaPrecedence,
}

#[derive(Debug, Clone)]
struct PlaceSetup {
    setup: Vec<PendingLine>,
    read_expr: String,
    write_target: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PlaceMode {
    Direct,
    Stable,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum LuaPrecedence {
    Or,
    And,
    Compare,
    Concat,
    Add,
    Mul,
    Unary,
    Power,
    Primary,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BinaryOperandSide {
    Left,
    Right,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FinalCallKind {
    Inline,
    AlreadyEmittedInSetup,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ExprEmitMode<'a> {
    Statement,
    Return,
    AssignInto(&'a str),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BlockTailAction<'a> {
    Statement,
    Return,
    ReturnNil,
    AssignInto(&'a str),
}

#[derive(Debug, Clone)]
struct LoopControlContext {
    break_flag: Option<String>,
}

#[derive(Debug, Clone)]
struct CompiledMatchArm {
    condition: Option<String>,
    bindings: Vec<PendingLine>,
    body: IrExprOrBlock,
    origin: Origin,
}

#[derive(Debug, Clone, Default)]
struct CompiledMatchPattern {
    conditions: Vec<String>,
    bindings: Vec<PendingLine>,
}

#[derive(Debug, Clone)]
struct IrEnumCatalog<'a> {
    by_name: BTreeMap<&'a str, &'a IrEnumDecl>,
}

impl<'a> IrEnumCatalog<'a> {
    fn collect(module: &'a IrModule) -> Self {
        let mut catalog = Self {
            by_name: BTreeMap::new(),
        };
        for stmt in &module.body {
            catalog.collect_stmt(stmt);
        }
        catalog
    }

    fn collect_stmt(&mut self, stmt: &'a IrStmt) {
        match &stmt.kind {
            IrStmtKind::EnumDecl(decl) => {
                self.by_name.insert(decl.name.as_str(), decl);
            }
            IrStmtKind::If {
                then_block,
                else_block,
                ..
            } => {
                self.collect_block(then_block);
                if let Some(block) = else_block {
                    self.collect_block(block);
                }
            }
            IrStmtKind::While { body, .. }
            | IrStmtKind::NumericFor { body, .. }
            | IrStmtKind::GenericFor { body, .. }
            | IrStmtKind::RepeatUntil { body, .. }
            | IrStmtKind::Do(body) => self.collect_block(body),
            _ => {}
        }
    }

    fn collect_block(&mut self, block: &'a IrBlock) {
        for stmt in &block.statements {
            self.collect_stmt(stmt);
        }
    }

    fn lookup_variant(&self, path: &[String]) -> Option<(&'a IrEnumDecl, &'a IrEnumVariant)> {
        if path.len() == 2 {
            let enum_decl = self.by_name.get(path[0].as_str()).copied()?;
            let variant = enum_decl
                .variants
                .iter()
                .find(|variant| variant.name == path[1])?;
            return Some((enum_decl, variant));
        }

        if path.len() == 1 {
            let mut found = None;
            for enum_decl in self.by_name.values().copied() {
                if let Some(variant) = enum_decl
                    .variants
                    .iter()
                    .find(|variant| variant.name == path[0])
                {
                    if found.is_some() {
                        return None;
                    }
                    found = Some((enum_decl, variant));
                }
            }
            return found;
        }

        None
    }
}

fn prefers_direct_assignment_expr(expr: &IrExpr) -> bool {
    matches!(
        expr.kind,
        IrExprKind::Conditional { .. } | IrExprKind::Do(_) | IrExprKind::Match(_)
    ) || matches!(
        expr.kind,
        IrExprKind::Binary {
            op: BinaryOp::Coalesce,
            ..
        }
    )
}

fn single_namespace_import(specifiers: &[IrImportSpecifier]) -> Option<&IrImportSpecifier> {
    let [specifier] = specifiers else {
        return None;
    };
    specifier.namespace.then_some(specifier)
}

fn comparison_nil_guard_condition(
    left: &ExprSetup,
    left_value: &str,
    right: &ExprSetup,
    right_value: &str,
) -> String {
    let mut checks = Vec::new();
    if left.needs_nil_guard() {
        checks.push(format!("{left_value} ~= nil"));
    }
    if right.needs_nil_guard() {
        checks.push(format!("{right_value} ~= nil"));
    }
    if checks.is_empty() {
        "true".into()
    } else {
        checks.join(" and ")
    }
}

fn condition_precedence(op: BinaryOp) -> LuaPrecedence {
    match op {
        BinaryOp::And => LuaPrecedence::And,
        BinaryOp::Or => LuaPrecedence::Or,
        _ => unreachable!("only logical operators have condition precedence"),
    }
}

fn parenthesize_condition_operand(
    value: String,
    operand_precedence: LuaPrecedence,
    parent_op: BinaryOp,
) -> String {
    let parent_precedence = condition_precedence(parent_op);
    if operand_precedence < parent_precedence {
        format!("({value})")
    } else {
        value
    }
}

fn collect_coalesce_terms<'a>(expr: &'a IrExpr, out: &mut Vec<&'a IrExpr>) {
    if let IrExprKind::Binary {
        op: BinaryOp::Coalesce,
        left,
        right,
    } = &expr.kind
    {
        collect_coalesce_terms(left, out);
        collect_coalesce_terms(right, out);
    } else {
        out.push(expr);
    }
}

fn simple_lua_identifier(value: &str) -> Option<&str> {
    let mut chars = value.chars();
    let first = chars.next()?;
    if !(first == '_' || first.is_ascii_alphabetic()) {
        return None;
    }
    if !chars.all(|ch| ch == '_' || ch.is_ascii_alphanumeric()) {
        return None;
    }
    if is_lua_keyword(value) {
        return None;
    }
    Some(value)
}

fn sanitize_temp_hint(hint: &str) -> Option<String> {
    let mut out = String::new();
    let mut prev_underscore = false;
    for ch in hint.chars() {
        let next = if ch == '_' || ch.is_ascii_alphanumeric() {
            ch
        } else {
            '_'
        };
        if next == '_' {
            if prev_underscore || out.is_empty() {
                prev_underscore = true;
                continue;
            }
            prev_underscore = true;
        } else {
            prev_underscore = false;
        }
        out.push(next);
    }
    while out.ends_with('_') {
        out.pop();
    }
    if out.is_empty() {
        return None;
    }
    if out.chars().next().is_some_and(|ch| ch.is_ascii_digit()) {
        out.insert(0, '_');
    }
    if is_lua_keyword(&out) {
        out.push_str("_value");
    }
    Some(out)
}

fn is_lua_keyword(value: &str) -> bool {
    matches!(
        value,
        "and"
            | "break"
            | "do"
            | "else"
            | "elseif"
            | "end"
            | "false"
            | "for"
            | "function"
            | "if"
            | "in"
            | "local"
            | "nil"
            | "not"
            | "or"
            | "repeat"
            | "return"
            | "then"
            | "true"
            | "until"
            | "while"
    )
}

fn expr_temp_hint(expr: &IrExpr) -> Option<String> {
    match &expr.kind {
        IrExprKind::Identifier(name) => Some(name.clone()),
        IrExprKind::Unary { argument, .. } => expr_temp_hint(argument),
        IrExprKind::Binary {
            op: BinaryOp::Coalesce,
            left,
            ..
        } => expr_temp_hint(left),
        IrExprKind::Chain(chain) => chain_temp_hint(chain),
        IrExprKind::Nil
        | IrExprKind::Boolean(_)
        | IrExprKind::Number(_)
        | IrExprKind::String(_)
        | IrExprKind::Vararg
        | IrExprKind::PipelinePlaceholder
        | IrExprKind::Template(_)
        | IrExprKind::Table(_)
        | IrExprKind::Binary { .. }
        | IrExprKind::Conditional { .. }
        | IrExprKind::Match(_)
        | IrExprKind::Do(_)
        | IrExprKind::Function(_) => None,
    }
}

fn chain_temp_hint(chain: &IrChain) -> Option<String> {
    let mut hint = expr_temp_hint(&chain.base);
    for segment in &chain.segments {
        match &segment.kind {
            IrChainSegmentKind::Member { name, .. } => {
                hint = Some(name.clone());
            }
            IrChainSegmentKind::Index { index, .. } => {
                hint = expr_temp_hint(index).or(hint);
            }
            IrChainSegmentKind::Call { args, .. } => {
                if args.len() == 1 {
                    hint = expr_temp_hint(&args[0]).or(hint);
                }
            }
            IrChainSegmentKind::SafeDotCall { name, .. }
            | IrChainSegmentKind::MethodCall { name, .. } => {
                hint = Some(name.clone());
            }
        }
    }
    hint
}

#[cfg(test)]
#[path = "lua/tests.rs"]
mod tests;
