use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use crate::ast::{Module, Realm};
use crate::compile_time::CompileTimePackageRegistry;
use crate::diag::{Diagnostic, Severity};
use crate::format::{FormatOutput, format_source};
use crate::lex::Lexer;
use crate::macro_expansion::{MacroRegistry, expand_macros_with_registry};
use crate::module::{
    ModuleExport, ModuleGraph, ModuleId, ModuleImport, ModuleImportSpecifier, PackageId,
    RealmAvailability, RealmSet,
};
use crate::parse::Parser;
use crate::part_order::{PartOrderInput, is_module_entry_path, sort_module_parts};
use crate::project::{
    ProjectManifest, discover_lux_sources, infer_module_path, infer_part_realm,
    resolve_import_target,
};
use crate::resolve::{Binding, BindingKind, ResolveOutput, ResolvePart, Resolver, ResolverOptions};
use crate::source::{FileId, SourceFile, SourceSpan};

#[derive(Debug, Clone)]
pub struct AnalysisConfig {
    pub source_root: PathBuf,
    pub package_id: Option<PackageId>,
    pub package_roots: Vec<PathBuf>,
    pub resolver_options: ResolverOptions,
}

impl AnalysisConfig {
    pub fn new(source_root: impl Into<PathBuf>) -> Self {
        Self {
            source_root: source_root.into(),
            package_id: None,
            package_roots: Vec::new(),
            resolver_options: ResolverOptions::gmod_default(),
        }
    }

    pub fn from_manifest(manifest: ProjectManifest) -> Self {
        let mut resolver_options = ResolverOptions::gmod_default();
        if let Some(policy) = manifest.gmod_unknown_external {
            resolver_options = resolver_options.with_unknown_external(policy);
        }
        resolver_options = resolver_options.with_externs(manifest.gmod_externs);
        Self {
            source_root: manifest.source_root,
            package_id: manifest.package_id.map(PackageId::new),
            package_roots: manifest.package_roots,
            resolver_options,
        }
    }

    pub fn with_package_id(mut self, package_id: impl Into<String>) -> Self {
        self.package_id = Some(PackageId::new(package_id.into()));
        self
    }

    pub fn with_package_roots(mut self, roots: Vec<PathBuf>) -> Self {
        self.package_roots = roots;
        self
    }

    pub fn with_resolver_options(mut self, options: ResolverOptions) -> Self {
        self.resolver_options = options;
        self
    }

    fn effective_package_id(&self) -> PackageId {
        self.package_id.clone().unwrap_or_else(|| {
            let name = self
                .source_root
                .parent()
                .and_then(Path::file_name)
                .or_else(|| self.source_root.file_name())
                .map(|name| name.to_string_lossy().to_string())
                .filter(|name| !name.is_empty())
                .unwrap_or_else(|| "app".into());
            PackageId::from_dir_name(&name)
        })
    }
}

#[derive(Debug, Clone)]
pub struct AnalysisFile {
    pub path: PathBuf,
    pub text: String,
}

#[derive(Debug)]
pub enum AnalysisError {
    Io { path: PathBuf, source: io::Error },
}

impl std::fmt::Display for AnalysisError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io { path, source } => {
                write!(f, "failed to read {}: {source}", path.display())
            }
        }
    }
}

impl std::error::Error for AnalysisError {}

#[derive(Debug, Clone)]
pub struct ProjectAnalysis {
    pub config: AnalysisConfig,
    pub files: Vec<SourceFile>,
    pub modules: Vec<AnalyzedModule>,
    pub graph: Option<ModuleGraph>,
    pub diagnostics: Vec<Diagnostic>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AnalysisPosition {
    pub line: u32,
    pub character: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AnalysisRange {
    pub start: AnalysisPosition,
    pub end: AnalysisPosition,
}

#[derive(Debug, Clone)]
pub struct AnalysisDiagnostic {
    pub path: PathBuf,
    pub range: AnalysisRange,
    pub severity: Severity,
    pub code: Option<String>,
    pub message: String,
    pub notes: Vec<String>,
    pub help: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AnalysisEditKind {
    Safe,
    Guided,
    Refactor,
}

#[derive(Debug, Clone)]
pub struct AnalysisTextEdit {
    pub path: PathBuf,
    pub range: AnalysisRange,
    pub new_text: String,
}

#[derive(Debug, Clone)]
pub struct AnalysisCodeAction {
    pub title: String,
    pub kind: AnalysisEditKind,
    pub diagnostics: Vec<String>,
    pub edits: Vec<AnalysisTextEdit>,
    pub command: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SemanticTokenKind {
    Keyword,
    Realm,
    Function,
    Parameter,
    Variable,
    Property,
    Namespace,
    Type,
    String,
    Number,
    Comment,
    Operator,
    Export,
    Import,
    External,
    UnknownExternal,
}

#[derive(Debug, Clone)]
pub struct AnalysisSemanticToken {
    pub span: SourceSpan,
    pub kind: SemanticTokenKind,
}

#[derive(Debug, Clone)]
pub struct AnalyzedModule {
    pub id: ModuleId,
    pub package_id: PackageId,
    pub module_path: String,
    pub parts: Vec<AnalyzedPart>,
    pub resolved: ResolveOutput,
    pub exports: Vec<ModuleExport>,
    pub imports: Vec<ModuleImport>,
}

#[derive(Debug, Clone)]
pub struct AnalyzedPart {
    pub path: PathBuf,
    pub default_realm: Realm,
    pub module: Module,
    pub source_file: SourceFile,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AnalysisSymbolKind {
    Binding,
    Export,
    External,
}

#[derive(Debug, Clone)]
pub struct AnalysisSymbol {
    pub kind: AnalysisSymbolKind,
    pub name: String,
    pub detail: String,
    pub span: SourceSpan,
    pub definition_span: Option<SourceSpan>,
    pub definition_path: Option<PathBuf>,
    pub module_id: Option<String>,
    pub available_realms: Option<RealmSet>,
    pub exported_as: Vec<String>,
    pub imported_from: Option<(String, String)>,
    pub external_availability: Option<RealmAvailability>,
}

#[derive(Debug, Clone)]
pub struct CompletionCandidate {
    pub label: String,
    pub detail: Option<String>,
    pub documentation: Option<String>,
}

pub fn analyze_source_root(
    config: AnalysisConfig,
    overlays: impl IntoIterator<Item = AnalysisFile>,
) -> Result<ProjectAnalysis, AnalysisError> {
    let mut files = BTreeMap::<PathBuf, String>::new();
    for path in discover_lux_sources(&config.source_root).map_err(project_error_to_analysis)? {
        let text = fs::read_to_string(&path).map_err(|source| AnalysisError::Io {
            path: path.clone(),
            source,
        })?;
        files.insert(path, text);
    }
    for overlay in overlays {
        if overlay
            .path
            .extension()
            .is_some_and(|extension| extension == "lux")
        {
            files.insert(overlay.path, overlay.text);
        }
    }
    analyze_files(
        config,
        files
            .into_iter()
            .map(|(path, text)| AnalysisFile { path, text }),
    )
}

pub fn analyze_files(
    config: AnalysisConfig,
    files: impl IntoIterator<Item = AnalysisFile>,
) -> Result<ProjectAnalysis, AnalysisError> {
    let package_id = config.effective_package_id();
    let mut diagnostics = Vec::<Diagnostic>::new();
    let mut modules = BTreeMap::<ModuleId, AnalyzedModule>::new();
    let mut source_files = Vec::<SourceFile>::new();
    let macro_registry = load_macro_registry(&config, &mut diagnostics);

    let mut input_files = files.into_iter().collect::<Vec<_>>();
    input_files.sort_by(|a, b| a.path.cmp(&b.path));

    for (index, input) in input_files.into_iter().enumerate() {
        let file = SourceFile::new(index as u32, Some(input.path.clone()), input.text);
        let module_path = infer_module_path(&config.source_root, &input.path);
        let module_id = ModuleId::from_package_path(&package_id, &module_path);
        source_files.push(file.clone());

        let default_realm = match infer_part_realm(&config.source_root, &input.path, file.id) {
            Ok(realm) => realm,
            Err(diagnostic) => {
                diagnostics.push(diagnostic);
                continue;
            }
        };

        let lex = Lexer::new(&file).lex_all();
        if lex.has_errors() {
            diagnostics.extend(lex.diagnostics);
            continue;
        }

        let parsed = Parser::new(&lex.tokens).parse_module();
        if parsed.has_errors() {
            diagnostics.extend(parsed.diagnostics);
            continue;
        }

        let expanded = expand_macros_with_registry(&file, &parsed.module, &macro_registry);
        if expanded.has_errors() {
            diagnostics.extend(expanded.diagnostics);
            continue;
        }

        modules
            .entry(module_id.clone())
            .or_insert_with(|| AnalyzedModule {
                id: module_id.clone(),
                package_id: package_id.clone(),
                module_path: module_path.clone(),
                parts: Vec::new(),
                resolved: Resolver::resolve(&Module {
                    body: Vec::new(),
                    span: expanded.module.span,
                }),
                exports: Vec::new(),
                imports: Vec::new(),
            })
            .parts
            .push(AnalyzedPart {
                path: input.path,
                default_realm,
                module: expanded.module,
                source_file: file,
            });
    }

    for module in modules.values_mut() {
        module.parts.sort_by(|a, b| a.path.cmp(&b.path));
        let inputs = module
            .parts
            .iter()
            .map(|part| PartOrderInput {
                path: &part.path,
                module: &part.module,
                is_entry: is_module_entry_path(&part.path),
            })
            .collect::<Vec<_>>();
        match sort_module_parts(&inputs) {
            Ok(order) => {
                module.parts = order
                    .into_iter()
                    .map(|index| module.parts[index].clone())
                    .collect();
            }
            Err(part_order_diagnostics) => {
                diagnostics.extend(part_order_diagnostics);
            }
        }

        let parts = module
            .parts
            .iter()
            .map(|part| ResolvePart {
                module: &part.module,
                default_realm: part.default_realm,
            })
            .collect::<Vec<_>>();
        let resolved =
            Resolver::resolve_parts_with_options(&parts, config.resolver_options.clone());
        diagnostics.extend(resolved.diagnostics.clone());
        module.resolved = resolved;
        module.exports = module_exports(module);
        module.imports = module_imports(module);
    }

    let module_vec = modules.into_values().collect::<Vec<_>>();
    let graph = match ModuleGraph::build(module_vec.iter().map(module_input).collect()) {
        Ok(graph) => Some(graph),
        Err(graph_diagnostics) => {
            diagnostics.extend(graph_diagnostics);
            None
        }
    };

    Ok(ProjectAnalysis {
        config,
        files: source_files,
        modules: module_vec,
        graph,
        diagnostics,
    })
}

pub fn analyze_text(path: impl Into<PathBuf>, text: impl Into<String>) -> ProjectAnalysis {
    let path = path.into();
    let source_root = path
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."));
    analyze_files(
        AnalysisConfig::new(source_root),
        [AnalysisFile {
            path,
            text: text.into(),
        }],
    )
    .expect("in-memory analysis does not perform IO")
}

pub fn format_text(path: impl Into<PathBuf>, text: impl Into<String>) -> FormatOutput {
    let file = SourceFile::new(0, Some(path.into()), text.into());
    format_source(&file)
}

impl ProjectAnalysis {
    pub fn file_by_path(&self, path: &Path) -> Option<&SourceFile> {
        self.files.iter().find(|file| {
            file.path
                .as_deref()
                .is_some_and(|file_path| same_path(file_path, path))
        })
    }

    pub fn file_by_id(&self, id: FileId) -> Option<&SourceFile> {
        self.files.iter().find(|file| file.id == id)
    }

    pub fn module_for_path(&self, path: &Path) -> Option<&AnalyzedModule> {
        self.modules
            .iter()
            .find(|module| module.parts.iter().any(|part| same_path(&part.path, path)))
    }

    pub fn part_for_path(&self, path: &Path) -> Option<&AnalyzedPart> {
        self.modules
            .iter()
            .flat_map(|module| module.parts.iter())
            .find(|part| same_path(&part.path, path))
    }

    pub fn diagnostics_for_path(&self, path: &Path) -> Vec<&Diagnostic> {
        let Some(file) = self.file_by_path(path) else {
            return Vec::new();
        };
        self.diagnostics
            .iter()
            .filter(|diagnostic| {
                diagnostic.labels.is_empty()
                    || diagnostic
                        .labels
                        .iter()
                        .any(|label| label.span.file_id == file.id)
            })
            .collect()
    }

    pub fn lsp_diagnostics_for_path(&self, path: &Path) -> Vec<AnalysisDiagnostic> {
        let Some(file) = self.file_by_path(path) else {
            return Vec::new();
        };
        self.diagnostics_for_path(path)
            .into_iter()
            .map(|diagnostic| {
                let span = diagnostic
                    .labels
                    .first()
                    .map(|label| label.span)
                    .filter(|span| span.file_id == file.id)
                    .unwrap_or_else(|| SourceSpan::new(file.id, 0, 0));
                AnalysisDiagnostic {
                    path: path.to_path_buf(),
                    range: self
                        .range_for_span(span)
                        .unwrap_or_else(|| zero_range(file)),
                    severity: diagnostic.severity,
                    code: diagnostic.code.clone(),
                    message: diagnostic.message.clone(),
                    notes: diagnostic.notes.clone(),
                    help: diagnostic.help.clone(),
                }
            })
            .collect()
    }

    pub fn code_actions_for_path(&self, path: &Path) -> Vec<AnalysisCodeAction> {
        let mut actions = Vec::new();
        let Some(file) = self.file_by_path(path) else {
            return actions;
        };
        for diagnostic in self.diagnostics_for_path(path) {
            let code = diagnostic.code.as_deref().unwrap_or_default();
            match code {
                "REALM_UNKNOWN" => {
                    if let Some(symbol) = diagnostic_external_symbol(diagnostic) {
                        for realm in ["shared", "client", "server"] {
                            actions.push(AnalysisCodeAction {
                                title: format!("Add extern {realm} {symbol}"),
                                kind: AnalysisEditKind::Guided,
                                diagnostics: vec![code.to_string()],
                                edits: vec![insert_top_level_edit(
                                    file,
                                    format!("extern {realm} {symbol}\n"),
                                )],
                                command: None,
                            });
                        }
                    }
                }
                "REALM001" => {
                    if let Some(label) = diagnostic.labels.first()
                        && let Some(range) = self.range_for_span(label.span)
                    {
                        actions.push(AnalysisCodeAction {
                            title: "Wrap in server { ... }".into(),
                            kind: AnalysisEditKind::Guided,
                            diagnostics: vec![code.to_string()],
                            edits: vec![AnalysisTextEdit {
                                path: path.to_path_buf(),
                                range,
                                new_text: format!(
                                    "server {{\n  {}\n}}",
                                    file.slice(label.span).trim()
                                ),
                            }],
                            command: None,
                        });
                        actions.push(AnalysisCodeAction {
                            title: "Wrap in client { ... }".into(),
                            kind: AnalysisEditKind::Guided,
                            diagnostics: vec![code.to_string()],
                            edits: vec![AnalysisTextEdit {
                                path: path.to_path_buf(),
                                range,
                                new_text: format!(
                                    "client {{\n  {}\n}}",
                                    file.slice(label.span).trim()
                                ),
                            }],
                            command: None,
                        });
                    }
                }
                "MODULE008" => {
                    if let Some(help) = &diagnostic.help {
                        actions.push(AnalysisCodeAction {
                            title: help.clone(),
                            kind: AnalysisEditKind::Guided,
                            diagnostics: vec![code.to_string()],
                            edits: Vec::new(),
                            command: Some("lux.showModuleExports".into()),
                        });
                    }
                }
                "FMT001" | "FMT002" | "FMT003" | "FMT004" => {
                    actions.push(AnalysisCodeAction {
                        title: "Format document".into(),
                        kind: AnalysisEditKind::Safe,
                        diagnostics: vec![code.to_string()],
                        edits: Vec::new(),
                        command: Some("lux.formatDocument".into()),
                    });
                }
                _ => {}
            }
        }
        actions
    }

    pub fn semantic_tokens_for_path(&self, path: &Path) -> Vec<AnalysisSemanticToken> {
        let Some(file) = self.file_by_path(path) else {
            return Vec::new();
        };
        let lex = Lexer::new(file).lex_all();
        let mut tokens = lex
            .tokens
            .iter()
            .filter_map(|token| token_kind_to_semantic(&token.kind).map(|kind| (token.span, kind)))
            .map(|(span, kind)| AnalysisSemanticToken { span, kind })
            .collect::<Vec<_>>();

        if let Some(module) = self.module_for_path(path) {
            for binding in &module.resolved.bindings {
                if binding.span.file_id == file.id {
                    tokens.push(AnalysisSemanticToken {
                        span: binding.span,
                        kind: semantic_kind_for_binding(binding.kind),
                    });
                }
            }
            for export in &module.resolved.exports {
                if export.span.file_id == file.id {
                    tokens.push(AnalysisSemanticToken {
                        span: export.span,
                        kind: SemanticTokenKind::Export,
                    });
                }
            }
            for (span, external) in &module.resolved.external_symbols_by_span {
                if span.file_id == file.id {
                    tokens.push(AnalysisSemanticToken {
                        span: *span,
                        kind: match external.availability {
                            RealmAvailability::Known(_) => SemanticTokenKind::External,
                            RealmAvailability::UnknownExternal => {
                                SemanticTokenKind::UnknownExternal
                            }
                        },
                    });
                }
            }
        }
        tokens.sort_by_key(|token| (token.span.byte_start, token.span.byte_end));
        tokens
    }

    pub fn range_for_span(&self, span: SourceSpan) -> Option<AnalysisRange> {
        let file = self.file_by_id(span.file_id)?;
        Some(range_for_span(file, span))
    }

    pub fn offset_for_position(
        &self,
        path: &Path,
        zero_based_line: usize,
        zero_based_character: usize,
    ) -> Option<usize> {
        self.file_by_path(path)
            .map(|file| file.offset_at_line_col_utf16(zero_based_line, zero_based_character))
    }

    pub fn symbol_at_path_offset(&self, path: &Path, offset: usize) -> Option<AnalysisSymbol> {
        let module = self.module_for_path(path)?;

        if let Some((span, external)) =
            find_containing_span(module.resolved.external_symbols_by_span.iter(), offset)
        {
            return Some(AnalysisSymbol {
                kind: AnalysisSymbolKind::External,
                name: external.path.join("."),
                detail: "external symbol".into(),
                span: *span,
                definition_span: None,
                definition_path: None,
                module_id: Some(module.id.as_str().to_string()),
                available_realms: match external.availability {
                    RealmAvailability::Known(realms) => Some(realms),
                    RealmAvailability::UnknownExternal => None,
                },
                exported_as: Vec::new(),
                imported_from: None,
                external_availability: Some(external.availability.clone()),
            });
        }

        if let Some((span, symbol)) =
            find_containing_span(module.resolved.symbols_by_span.iter(), offset)
        {
            let binding = module.resolved.bindings.get(symbol.binding.0)?;
            return Some(self.binding_symbol(module, binding, *span));
        }

        if let Some(export) = module
            .resolved
            .exports
            .iter()
            .find(|export| contains_offset(export.span, offset))
        {
            let binding = module.resolved.bindings.get(export.binding.0)?;
            return Some(AnalysisSymbol {
                kind: AnalysisSymbolKind::Export,
                name: export.name.clone(),
                detail: format!("public export of `{}`", export.local_name),
                span: export.span,
                definition_span: Some(binding.span),
                definition_path: self.path_for_span(binding.span),
                module_id: Some(module.id.as_str().to_string()),
                available_realms: Some(
                    export
                        .realm
                        .map(RealmSet::from_realm)
                        .unwrap_or(binding.available_realms),
                ),
                exported_as: vec![export.name.clone()],
                imported_from: None,
                external_availability: None,
            });
        }

        module
            .resolved
            .bindings
            .iter()
            .filter(|binding| contains_offset(binding.span, offset))
            .min_by_key(|binding| binding.span.len())
            .map(|binding| self.binding_symbol(module, binding, binding.span))
    }

    pub fn hover_markdown_at_path_offset(&self, path: &Path, offset: usize) -> Option<String> {
        let symbol = self.symbol_at_path_offset(path, offset)?;
        Some(symbol_hover_markdown(&symbol))
    }

    pub fn module_path_completions(&self) -> Vec<CompletionCandidate> {
        self.modules
            .iter()
            .map(|module| CompletionCandidate {
                label: module.module_path.clone(),
                detail: Some(module.id.as_str().to_string()),
                documentation: Some(format!("Lux module `{}`", module.id)),
            })
            .collect()
    }

    pub fn exportable_bindings(&self, path: &Path) -> Vec<CompletionCandidate> {
        let Some(module) = self.module_for_path(path) else {
            return Vec::new();
        };
        let exported = module
            .resolved
            .exports
            .iter()
            .map(|export| export.local_name.as_str())
            .collect::<BTreeSet<_>>();
        module
            .resolved
            .bindings
            .iter()
            .filter(|binding| binding.module_scope)
            .filter(|binding| {
                !matches!(binding.kind, BindingKind::Import | BindingKind::MacroImport)
            })
            .filter(|binding| !exported.contains(binding.name.as_str()))
            .map(|binding| CompletionCandidate {
                label: binding.name.clone(),
                detail: Some(format!(
                    "{} binding, {}",
                    binding_kind_name(binding.kind),
                    binding.available_realms.display_name()
                )),
                documentation: Some(format!(
                    "`{}` is module-private until explicitly exported.",
                    binding.name
                )),
            })
            .collect()
    }

    pub fn importable_exports(
        &self,
        current_path: &Path,
        raw_source: &str,
        active_realms: RealmSet,
    ) -> Vec<CompletionCandidate> {
        let Some(current_module) = self.module_for_path(current_path) else {
            return Vec::new();
        };
        let Some(target_id) = resolve_import_target(
            &current_module.package_id,
            &current_module.module_path,
            raw_source,
        ) else {
            return Vec::new();
        };
        let Some(target_module) = self.modules.iter().find(|module| module.id == target_id) else {
            return Vec::new();
        };
        target_module
            .exports
            .iter()
            .filter(|export| export.realms.contains_all(active_realms))
            .map(|export| CompletionCandidate {
                label: export.name.clone(),
                detail: Some(export.realms.display_name().to_string()),
                documentation: Some(format!("Exported by `{}`", target_module.id)),
            })
            .collect()
    }

    fn binding_symbol(
        &self,
        module: &AnalyzedModule,
        binding: &Binding,
        span: SourceSpan,
    ) -> AnalysisSymbol {
        let exported_as = module
            .resolved
            .exports
            .iter()
            .filter(|export| export.binding == binding.id)
            .map(|export| export.name.clone())
            .collect::<Vec<_>>();
        let import_target = binding
            .source_module
            .as_ref()
            .zip(binding.imported_name.as_ref())
            .map(|(source, imported)| (source.clone(), imported.clone()));
        let (definition_span, definition_path) = import_target
            .as_ref()
            .and_then(|(source, imported)| self.import_definition(module, source, imported))
            .map(|span| (Some(span), self.path_for_span(span)))
            .unwrap_or_else(|| (Some(binding.span), self.path_for_span(binding.span)));
        AnalysisSymbol {
            kind: AnalysisSymbolKind::Binding,
            name: binding.name.clone(),
            detail: format!("{} binding", binding_kind_name(binding.kind)),
            span,
            definition_span,
            definition_path,
            module_id: Some(module.id.as_str().to_string()),
            available_realms: Some(binding.available_realms),
            exported_as,
            imported_from: import_target,
            external_availability: None,
        }
    }

    fn import_definition(
        &self,
        module: &AnalyzedModule,
        raw_source: &str,
        imported: &str,
    ) -> Option<SourceSpan> {
        let target_id = resolve_import_target(&module.package_id, &module.module_path, raw_source)?;
        let target_module = self.modules.iter().find(|module| module.id == target_id)?;
        target_module
            .resolved
            .exports
            .iter()
            .find(|export| export.name == imported)
            .map(|export| export.span)
    }

    pub fn path_for_span(&self, span: SourceSpan) -> Option<PathBuf> {
        self.file_by_id(span.file_id)
            .and_then(|file| file.path.clone())
    }
}

fn zero_range(file: &SourceFile) -> AnalysisRange {
    range_for_span(file, SourceSpan::new(file.id, 0, 0))
}

fn range_for_span(file: &SourceFile, span: SourceSpan) -> AnalysisRange {
    let (start_line, start_col) = file.line_col_utf16(span.byte_start);
    let (end_line, end_col) = file.line_col_utf16(span.byte_end);
    AnalysisRange {
        start: AnalysisPosition {
            line: start_line.saturating_sub(1) as u32,
            character: start_col.saturating_sub(1) as u32,
        },
        end: AnalysisPosition {
            line: end_line.saturating_sub(1) as u32,
            character: end_col.saturating_sub(1) as u32,
        },
    }
}

fn insert_top_level_edit(file: &SourceFile, new_text: String) -> AnalysisTextEdit {
    AnalysisTextEdit {
        path: file.path.clone().unwrap_or_default(),
        range: AnalysisRange {
            start: AnalysisPosition {
                line: 0,
                character: 0,
            },
            end: AnalysisPosition {
                line: 0,
                character: 0,
            },
        },
        new_text,
    }
}

fn diagnostic_external_symbol(diagnostic: &Diagnostic) -> Option<String> {
    let message = &diagnostic.message;
    let start = message.find('`')? + 1;
    let end = message[start..].find('`')? + start;
    Some(message[start..end].to_string())
}

fn token_kind_to_semantic(kind: &crate::lex::TokenKind) -> Option<SemanticTokenKind> {
    use crate::lex::TokenKind;
    match kind {
        TokenKind::KwFn | TokenKind::KwFunction => Some(SemanticTokenKind::Keyword),
        TokenKind::KwIf
        | TokenKind::KwThen
        | TokenKind::KwElse
        | TokenKind::KwElseIf
        | TokenKind::KwLocal
        | TokenKind::KwConst
        | TokenKind::KwNil
        | TokenKind::KwTrue
        | TokenKind::KwFalse
        | TokenKind::KwAnd
        | TokenKind::KwOr
        | TokenKind::KwNot
        | TokenKind::KwImport
        | TokenKind::KwExport
        | TokenKind::KwEnd
        | TokenKind::KwDo
        | TokenKind::KwWhile
        | TokenKind::KwFor
        | TokenKind::KwRepeat
        | TokenKind::KwUntil
        | TokenKind::KwBreak
        | TokenKind::KwReturn
        | TokenKind::KwIn => Some(SemanticTokenKind::Keyword),
        TokenKind::Identifier(value)
            if matches!(value.as_str(), "server" | "client" | "shared") =>
        {
            Some(SemanticTokenKind::Realm)
        }
        TokenKind::Identifier(value) if matches!(value.as_str(), "enum") => {
            Some(SemanticTokenKind::Type)
        }
        TokenKind::Number(_) => Some(SemanticTokenKind::Number),
        TokenKind::String(_)
        | TokenKind::TemplateStringStart
        | TokenKind::TemplateStringText(_)
        | TokenKind::TemplateStringEnd => Some(SemanticTokenKind::String),
        TokenKind::Eq
        | TokenKind::PlusEq
        | TokenKind::MinusEq
        | TokenKind::StarEq
        | TokenKind::SlashEq
        | TokenKind::PercentEq
        | TokenKind::CaretEq
        | TokenKind::DotDotEq
        | TokenKind::Plus
        | TokenKind::Minus
        | TokenKind::Star
        | TokenKind::Slash
        | TokenKind::Percent
        | TokenKind::Caret
        | TokenKind::Hash
        | TokenKind::Pipe
        | TokenKind::PipeGt
        | TokenKind::EqEq
        | TokenKind::NotEq
        | TokenKind::Lt
        | TokenKind::LtEq
        | TokenKind::Gt
        | TokenKind::GtEq
        | TokenKind::ArrowNormal
        | TokenKind::ArrowImplicitSelf => Some(SemanticTokenKind::Operator),
        _ => None,
    }
}

fn semantic_kind_for_binding(kind: BindingKind) -> SemanticTokenKind {
    match kind {
        BindingKind::Function => SemanticTokenKind::Function,
        BindingKind::Param => SemanticTokenKind::Parameter,
        BindingKind::Import | BindingKind::MacroImport => SemanticTokenKind::Import,
        BindingKind::Local | BindingKind::Const => SemanticTokenKind::Variable,
    }
}

fn symbol_hover_markdown(symbol: &AnalysisSymbol) -> String {
    let mut out = String::new();
    out.push_str("### ");
    out.push_str(&symbol.name);
    out.push_str("\n\n");
    out.push_str(&symbol.detail);
    out.push('\n');

    if let Some(module_id) = &symbol.module_id {
        out.push_str("\n\n**Module:** `");
        out.push_str(module_id);
        out.push('`');
    }
    if let Some(realms) = symbol.available_realms {
        out.push_str("\n\n**Realm:** ");
        out.push_str(realms.display_name());
    }
    if !symbol.exported_as.is_empty() {
        out.push_str("\n\n**Exported as:** ");
        out.push_str(
            &symbol
                .exported_as
                .iter()
                .map(|name| format!("`{name}`"))
                .collect::<Vec<_>>()
                .join(", "),
        );
    }
    if let Some((source, imported)) = &symbol.imported_from {
        out.push_str("\n\n**Imported from:** `");
        out.push_str(source);
        out.push_str("` as `");
        out.push_str(imported);
        out.push('`');
    }
    if let Some(availability) = &symbol.external_availability {
        match availability {
            RealmAvailability::Known(realms) => {
                out.push_str("\n\nKnown external symbol, available in ");
                out.push_str(realms.display_name());
                out.push('.');
            }
            RealmAvailability::UnknownExternal => {
                out.push_str(
                    "\n\nUnknown external symbol. Lux cannot verify its realm availability.",
                );
                out.push_str("\n\nAdd an `extern` declaration to make this strict.");
            }
        }
    }
    out
}

fn load_macro_registry(
    config: &AnalysisConfig,
    diagnostics: &mut Vec<Diagnostic>,
) -> MacroRegistry {
    let mut registry = MacroRegistry::empty();
    match CompileTimePackageRegistry::load_default_with_package_roots(&config.package_roots) {
        Ok(compile_time) => {
            if let Err(err) = compile_time.register_macros(&mut registry) {
                diagnostics.push(Diagnostic::error(err.to_string()).with_code("CTLOAD001"));
            }
        }
        Err(err) => {
            diagnostics.push(Diagnostic::error(err.to_string()).with_code("CTLOAD001"));
        }
    }
    registry
}

fn module_input(module: &AnalyzedModule) -> crate::module::ModuleInput {
    crate::module::ModuleInput::new(
        module.package_id.clone(),
        module.module_path.clone(),
        module
            .parts
            .first()
            .map(|part| part.module.span)
            .unwrap_or(SourceSpan::new(FileId(0), 0, 0)),
        module.exports.clone(),
        module.imports.clone(),
    )
}

fn module_exports(module: &AnalyzedModule) -> Vec<ModuleExport> {
    module
        .resolved
        .exports
        .iter()
        .filter_map(|export| {
            let binding = module.resolved.bindings.get(export.binding.0)?;
            let realms = export
                .realm
                .map(RealmSet::from_realm)
                .unwrap_or(binding.available_realms);
            Some(ModuleExport {
                name: export.name.clone(),
                realms,
                span: export.span,
            })
        })
        .collect()
}

fn module_imports(module: &AnalyzedModule) -> Vec<ModuleImport> {
    module
        .resolved
        .module_edges
        .iter()
        .filter(|edge| {
            edge.source.starts_with('@') || edge.source.starts_with('.') || !edge.source.is_empty()
        })
        .filter_map(|edge| {
            let target =
                resolve_import_target(&module.package_id, &module.module_path, &edge.source)?;
            let active_realms = if edge.side_effect_only {
                RealmSet::SHARED
            } else {
                RealmSet::NONE
            };
            Some(ModuleImport {
                raw_source: edge.source.clone(),
                target,
                specifiers: edge
                    .specifiers
                    .iter()
                    .map(|specifier| ModuleImportSpecifier {
                        imported: specifier.imported.clone(),
                        local: specifier.local.clone(),
                        namespace: specifier.namespace,
                        active_realms: specifier.active_realms,
                        span: specifier.span,
                    })
                    .collect(),
                side_effect_only: edge.side_effect_only,
                active_realms,
                span: edge.span,
            })
        })
        .collect()
}

fn binding_kind_name(kind: BindingKind) -> &'static str {
    match kind {
        BindingKind::Local => "local",
        BindingKind::Const => "const",
        BindingKind::Param => "parameter",
        BindingKind::Function => "function",
        BindingKind::Import => "import",
        BindingKind::MacroImport => "macro import",
    }
}

fn contains_offset(span: SourceSpan, offset: usize) -> bool {
    span.byte_start <= offset && offset <= span.byte_end
}

fn find_containing_span<'a, T>(
    iter: impl Iterator<Item = (&'a SourceSpan, T)>,
    offset: usize,
) -> Option<(&'a SourceSpan, T)> {
    iter.filter(|(span, _)| contains_offset(**span, offset))
        .min_by_key(|(span, _)| span.len())
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

fn project_error_to_analysis(error: crate::project::ProjectError) -> AnalysisError {
    match error {
        crate::project::ProjectError::Io { path, source } => AnalysisError::Io { path, source },
        other => AnalysisError::Io {
            path: PathBuf::from("<project>"),
            source: io::Error::other(other.to_string()),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::{AnalysisConfig, AnalysisFile, AnalysisSymbolKind, analyze_files};
    use crate::module::{RealmAvailability, RealmSet};

    #[test]
    fn analyzes_multi_part_module_scope() {
        let root = std::path::PathBuf::from("src");
        let output = analyze_files(
            AnalysisConfig::new(&root),
            [
                AnalysisFile {
                    path: root.join("inventory/module.lux"),
                    text: "part order { \"helpers\" }\nexport { p_inv = player_inventory }".into(),
                },
                AnalysisFile {
                    path: root.join("inventory/helpers.lux"),
                    text: "local player_inventory = {}".into(),
                },
            ],
        )
        .expect("analysis");

        let module = output
            .modules
            .iter()
            .find(|module| module.module_path == "inventory")
            .expect("inventory module");
        assert_eq!(module.parts.len(), 2);
        assert!(module.exports.iter().any(|export| export.name == "p_inv"));
        assert!(
            output
                .importable_exports(
                    &root.join("inventory/module.lux"),
                    "inventory",
                    RealmSet::SHARED
                )
                .iter()
                .any(|candidate| candidate.label == "p_inv")
        );
    }

    #[test]
    fn resolves_export_alias_as_public_api_name() {
        let root = std::path::PathBuf::from("src");
        let output = analyze_files(
            AnalysisConfig::new(&root),
            [
                AnalysisFile {
                    path: root.join("inventory/module.lux"),
                    text: "part order { \"state\" }\nexport { p_inv = player_inventory }".into(),
                },
                AnalysisFile {
                    path: root.join("inventory/state.lux"),
                    text: "local player_inventory = {}".into(),
                },
                AnalysisFile {
                    path: root.join("hud/module.lux"),
                    text: "import { p_inv as inventory } from \"inventory\"\nfn read() = inventory"
                        .into(),
                },
            ],
        )
        .expect("analysis");

        let imports =
            output.importable_exports(&root.join("hud/module.lux"), "inventory", RealmSet::SHARED);
        assert!(imports.iter().any(|candidate| candidate.label == "p_inv"));
        assert!(
            !imports
                .iter()
                .any(|candidate| candidate.label == "player_inventory")
        );

        let hud_path = root.join("hud/module.lux");
        let offset = output
            .offset_for_position(&hud_path, 1, "fn read() = inv".len())
            .expect("offset");
        let symbol = output
            .symbol_at_path_offset(&hud_path, offset)
            .expect("import binding symbol");
        assert_eq!(symbol.kind, AnalysisSymbolKind::Binding);
        assert_eq!(symbol.name, "inventory");
        assert_eq!(
            symbol.imported_from,
            Some(("inventory".into(), "p_inv".into()))
        );

        let definition_path = symbol.definition_path.expect("definition path");
        assert!(definition_path.ends_with("inventory/module.lux"));
        let definition_span = symbol.definition_span.expect("definition span");
        let definition_file = output.file_by_id(definition_span.file_id).expect("file");
        assert_eq!(definition_file.slice(definition_span), "p_inv");
    }

    #[test]
    fn unknown_external_hover_and_quick_fixes_are_guided() {
        let root = std::path::PathBuf::from("src");
        let path = root.join("module.lux");
        let output = analyze_files(
            AnalysisConfig::new(&root),
            [AnalysisFile {
                path: path.clone(),
                text: "fn run() = ThirdPartyAddon.DoThing()".into(),
            }],
        )
        .expect("analysis");

        let offset = output
            .offset_for_position(&path, 0, "fn run() = ThirdPartyAddon.Do".len())
            .expect("offset");
        let symbol = output
            .symbol_at_path_offset(&path, offset)
            .expect("external symbol");
        assert_eq!(symbol.kind, AnalysisSymbolKind::External);
        assert_eq!(
            symbol.external_availability,
            Some(RealmAvailability::UnknownExternal)
        );

        let hover = output
            .hover_markdown_at_path_offset(&path, offset)
            .expect("hover");
        assert!(hover.contains("Unknown external symbol"));

        let actions = output.code_actions_for_path(&path);
        assert!(
            actions
                .iter()
                .any(|action| action.title == "Add extern shared ThirdPartyAddon.DoThing")
        );
        assert!(
            actions
                .iter()
                .any(|action| action.title == "Add extern client ThirdPartyAddon.DoThing")
        );
        assert!(
            actions
                .iter()
                .any(|action| action.title == "Add extern server ThirdPartyAddon.DoThing")
        );
    }
}
