use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use crate::ast::{Block, FunctionBody, Module, Realm, Stmt, StmtKind};
use crate::compile_time::CompileTimePackageRegistry;
use crate::diag::{Diagnostic, Severity};
use crate::format::{FormatOutput, format_source};
use crate::lex::{Lexer, Token, TokenKind};
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
use crate::runtime::RuntimePackageRegistry;
use crate::source::{FileId, SourceFile, SourceSpan};

#[derive(Debug, Clone)]
pub struct AnalysisConfig {
    pub source_root: PathBuf,
    pub package_id: Option<PackageId>,
    pub package_roots: Vec<PathBuf>,
    pub resolver_options: ResolverOptions,
}

impl PartialEq for AnalysisConfig {
    fn eq(&self, other: &Self) -> bool {
        same_path(&self.source_root, &other.source_root)
            && self.package_id == other.package_id
            && paths_equal(&self.package_roots, &other.package_roots)
            && self.resolver_options == other.resolver_options
    }
}

impl Eq for AnalysisConfig {}

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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AnalysisChangeKind {
    Full,
    Incremental,
}

#[derive(Debug, Clone)]
pub struct AnalysisChange {
    pub kind: AnalysisChangeKind,
    pub affected_modules: Vec<ModuleId>,
}

#[derive(Debug, Clone)]
struct CachedAnalysisFile {
    path: PathBuf,
    text: String,
    module_id: ModuleId,
}

#[derive(Debug, Clone)]
pub struct AnalysisWorkspace {
    config: AnalysisConfig,
    files: BTreeMap<PathBuf, CachedAnalysisFile>,
    analysis: ProjectAnalysis,
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
    pub external_exports: BTreeMap<ModuleId, Vec<ModuleExport>>,
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
    pub kind: CompletionCandidateKind,
    pub detail: Option<String>,
    pub documentation: Option<String>,
    pub source: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompletionCandidateKind {
    Module,
    Function,
    Method,
    Variable,
    Parameter,
    Constant,
    Field,
    Class,
    Enum,
    Event,
    Reference,
    Struct,
    Property,
    Value,
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
    analyze_file_map(config, files)
}

pub fn analyze_files(
    config: AnalysisConfig,
    files: impl IntoIterator<Item = AnalysisFile>,
) -> Result<ProjectAnalysis, AnalysisError> {
    analyze_file_map(config, collect_file_map(files)?)
}

pub fn analyze_file_map(
    config: AnalysisConfig,
    files: BTreeMap<PathBuf, String>,
) -> Result<ProjectAnalysis, AnalysisError> {
    let package_id = config.effective_package_id();
    let mut diagnostics = Vec::<Diagnostic>::new();
    let mut modules = BTreeMap::<ModuleId, AnalyzedModule>::new();
    let mut source_files = Vec::<SourceFile>::new();
    let macro_registry = load_macro_registry(&config, &mut diagnostics);

    for (index, (path, text)) in files.into_iter().enumerate() {
        let file = SourceFile::new(index as u32, Some(path.clone()), text);
        let module_path = infer_module_path(&config.source_root, &path);
        let module_id = ModuleId::from_package_path(&package_id, &module_path);
        source_files.push(file.clone());

        let default_realm = match infer_part_realm(&config.source_root, &path, file.id) {
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
                path,
                default_realm,
                module: expanded.module,
                source_file: file,
            });
    }

    for module in modules.values_mut() {
        finalize_analyzed_module(&config, module, &mut diagnostics);
    }

    let module_vec = modules.into_values().collect::<Vec<_>>();
    let external_exports = runtime_external_exports(&config, &mut diagnostics);
    let graph = match build_analysis_graph(&module_vec, external_exports.clone()) {
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
        external_exports,
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

impl AnalysisWorkspace {
    pub fn load(
        config: AnalysisConfig,
        overlays: impl IntoIterator<Item = AnalysisFile>,
    ) -> Result<Self, AnalysisError> {
        let files = load_source_root_files(&config, overlays)?;
        Self::from_file_map(config, files)
    }

    pub fn from_files(
        config: AnalysisConfig,
        files: impl IntoIterator<Item = AnalysisFile>,
    ) -> Result<Self, AnalysisError> {
        Self::from_file_map(config, collect_file_map(files)?)
    }

    pub fn from_file_map(
        config: AnalysisConfig,
        files: BTreeMap<PathBuf, String>,
    ) -> Result<Self, AnalysisError> {
        let analysis = analyze_file_map(config.clone(), files.clone())?;
        let files = cached_files(&config, files);
        Ok(Self {
            config,
            files,
            analysis,
        })
    }

    pub fn analysis(&self) -> &ProjectAnalysis {
        &self.analysis
    }

    pub fn into_analysis(self) -> ProjectAnalysis {
        self.analysis
    }

    pub fn update_files(
        &mut self,
        config: AnalysisConfig,
        files: impl IntoIterator<Item = AnalysisFile>,
    ) -> Result<AnalysisChange, AnalysisError> {
        self.update_file_map(config, collect_file_map(files)?)
    }

    pub fn update_source_root(
        &mut self,
        config: AnalysisConfig,
        overlays: impl IntoIterator<Item = AnalysisFile>,
    ) -> Result<AnalysisChange, AnalysisError> {
        let files = load_source_root_files(&config, overlays)?;
        self.update_file_map(config, files)
    }

    pub fn update_file_map(
        &mut self,
        config: AnalysisConfig,
        files: BTreeMap<PathBuf, String>,
    ) -> Result<AnalysisChange, AnalysisError> {
        if self.config != config || file_keys_changed(&self.files, &files) {
            self.config = config;
            self.files = cached_files(&self.config, files);
            self.analysis = analyze_cached_files(&self.config, &self.files)?;
            return Ok(AnalysisChange {
                kind: AnalysisChangeKind::Full,
                affected_modules: self
                    .analysis
                    .modules
                    .iter()
                    .map(|module| module.id.clone())
                    .collect(),
            });
        }

        let changed_modules = changed_modules(&self.files, &files);
        if changed_modules.is_empty() {
            return Ok(AnalysisChange {
                kind: AnalysisChangeKind::Incremental,
                affected_modules: Vec::new(),
            });
        }

        self.files = cached_files(&config, files);
        let affected_modules = affected_modules_for_change(&self.analysis, &changed_modules);
        let affected_set = affected_modules.iter().cloned().collect::<BTreeSet<_>>();
        self.reanalyze_modules(&affected_set)?;
        Ok(AnalysisChange {
            kind: AnalysisChangeKind::Incremental,
            affected_modules,
        })
    }

    fn reanalyze_modules(
        &mut self,
        affected_modules: &BTreeSet<ModuleId>,
    ) -> Result<(), AnalysisError> {
        let mut existing_modules = self
            .analysis
            .modules
            .iter()
            .cloned()
            .map(|module| (module.id.clone(), module))
            .collect::<BTreeMap<_, _>>();
        let mut unaffected_diagnostics = self
            .analysis
            .diagnostics
            .iter()
            .filter(|diagnostic| {
                !diagnostic_touches_modules(diagnostic, &self.analysis, affected_modules)
            })
            .cloned()
            .collect::<Vec<_>>();
        let macro_registry = load_macro_registry(&self.config, &mut unaffected_diagnostics);

        for module_id in affected_modules {
            if let Some(module) = analyze_single_module(
                &self.config,
                &self.files,
                module_id,
                &macro_registry,
                &mut unaffected_diagnostics,
            )? {
                existing_modules.insert(module_id.clone(), module);
            } else {
                existing_modules.remove(module_id);
            }
        }

        let module_vec = existing_modules.into_values().collect::<Vec<_>>();
        let external_exports = runtime_external_exports(&self.config, &mut unaffected_diagnostics);
        let graph = match build_analysis_graph(&module_vec, external_exports.clone()) {
            Ok(graph) => Some(graph),
            Err(graph_diagnostics) => {
                unaffected_diagnostics.extend(graph_diagnostics);
                None
            }
        };
        self.analysis = ProjectAnalysis {
            config: self.config.clone(),
            files: source_files_from_cache(&self.files),
            modules: module_vec,
            graph,
            external_exports,
            diagnostics: unaffected_diagnostics,
        };
        Ok(())
    }
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
                "REALM002" => {
                    if let Some(label) = diagnostic.labels.first()
                        && let Some(target_realm) = narrowed_export_realm(&diagnostic.message)
                        && let Some(edit) =
                            export_realm_narrowing_edit(file, label.span, target_realm)
                    {
                        actions.push(AnalysisCodeAction {
                            title: format!("Change export realm to {target_realm}"),
                            kind: AnalysisEditKind::Guided,
                            diagnostics: vec![code.to_string()],
                            edits: vec![edit],
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

    pub fn active_realms_at_path_offset(&self, path: &Path, offset: usize) -> Option<RealmSet> {
        let part = self.part_for_path(path)?;
        let default_realms = RealmSet::from_realm(part.default_realm);
        Some(active_realms_in_stmts(
            &part.module.body,
            offset,
            default_realms,
        ))
    }

    pub fn active_realms_at_position(
        &self,
        path: &Path,
        zero_based_line: usize,
        zero_based_character: usize,
    ) -> Option<RealmSet> {
        let offset = self.offset_for_position(path, zero_based_line, zero_based_character)?;
        self.active_realms_at_path_offset(path, offset)
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
                kind: CompletionCandidateKind::Module,
                detail: Some(module.id.as_str().to_string()),
                documentation: Some(format!("Lux module `{}`", module.id)),
                source: None,
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
                kind: completion_kind_for_binding(binding.kind),
                detail: Some(format!(
                    "{} binding, {}",
                    binding_kind_name(binding.kind),
                    binding.available_realms.display_name()
                )),
                documentation: Some(format!(
                    "`{}` is module-private until explicitly exported.",
                    binding.name
                )),
                source: None,
            })
            .collect()
    }

    pub fn visible_bindings_at_path_offset(
        &self,
        path: &Path,
        offset: usize,
    ) -> Vec<CompletionCandidate> {
        let Some(file) = self.file_by_path(path) else {
            return Vec::new();
        };
        let Some(module) = self.module_for_path(path) else {
            return Vec::new();
        };
        let active_realms = self
            .active_realms_at_path_offset(path, offset)
            .unwrap_or(RealmSet::SHARED);
        let mut candidates = BTreeMap::<String, CompletionCandidate>::new();

        for binding in &module.resolved.bindings {
            if !binding.module_scope {
                continue;
            }
            if !binding.available_realms.contains_all(active_realms) {
                continue;
            }
            candidates.entry(binding.name.clone()).or_insert_with(|| {
                binding_completion_candidate(
                    binding,
                    Some(format!(
                        "module {} binding, {}",
                        binding_kind_name(binding.kind),
                        binding.available_realms.display_name()
                    )),
                )
            });
        }

        let visible_local_ids = visible_local_binding_ids(module, file.id, offset);
        for binding_id in visible_local_ids {
            let Some(binding) = module.resolved.bindings.get(binding_id.0) else {
                continue;
            };
            if !binding.available_realms.contains_all(active_realms) {
                continue;
            }
            candidates.insert(
                binding.name.clone(),
                binding_completion_candidate(
                    binding,
                    Some(format!(
                        "{} binding, {}",
                        binding_kind_name(binding.kind),
                        binding.available_realms.display_name()
                    )),
                ),
            );
        }

        candidates.into_values().collect()
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
        let source_label = raw_source.to_string();
        if let Some(exports) = self.external_exports.get(&target_id) {
            return exports
                .iter()
                .filter(|export| export.realms.contains_all(active_realms))
                .map(|export| CompletionCandidate {
                    label: export.name.clone(),
                    kind: CompletionCandidateKind::Reference,
                    detail: Some(format!(
                        "{} from `{}`",
                        export.realms.display_name(),
                        source_label
                    )),
                    documentation: Some(format!("Exported by runtime package `{source_label}`")),
                    source: Some(source_label.clone()),
                })
                .collect();
        }
        let Some(target_module) = self.modules.iter().find(|module| module.id == target_id) else {
            return Vec::new();
        };
        importable_exports_from_module(target_module, &source_label, active_realms)
    }

    pub fn importable_exports_for_all_sources(
        &self,
        current_path: &Path,
        active_realms: RealmSet,
    ) -> Vec<CompletionCandidate> {
        let current_module = self.module_for_path(current_path);
        let mut candidates = Vec::new();
        for module in &self.modules {
            if current_module.is_some_and(|current| module.id == current.id) {
                continue;
            }
            let raw_source = current_module
                .map(|current| import_source_for_module(current, module))
                .unwrap_or_else(|| module.module_path.clone());
            candidates.extend(importable_exports_from_module(
                module,
                &raw_source,
                active_realms,
            ));
        }
        for (module_id, exports) in &self.external_exports {
            let source = format!("@{}", module_id.as_str());
            candidates.extend(
                exports
                    .iter()
                    .filter(|export| export.realms.contains_all(active_realms))
                    .map(|export| CompletionCandidate {
                        label: export.name.clone(),
                        kind: CompletionCandidateKind::Reference,
                        detail: Some(format!(
                            "{} from `{}`",
                            export.realms.display_name(),
                            source
                        )),
                        documentation: Some(format!("Exported by runtime package `{source}`")),
                        source: Some(source.clone()),
                    }),
            );
        }
        candidates.sort_by(|left, right| {
            left.label
                .cmp(&right.label)
                .then_with(|| left.detail.cmp(&right.detail))
        });
        candidates.dedup_by(|left, right| left.label == right.label && left.detail == right.detail);
        candidates
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

fn active_realms_in_stmts(stmts: &[Stmt], offset: usize, current: RealmSet) -> RealmSet {
    for stmt in stmts {
        if !contains_offset(stmt.span, offset) {
            continue;
        }
        if let Some(realms) = active_realms_in_stmt(stmt, offset, current) {
            return realms;
        }
    }
    current
}

fn active_realms_in_stmt(stmt: &Stmt, offset: usize, current: RealmSet) -> Option<RealmSet> {
    match &stmt.kind {
        StmtKind::RealmDecl { realm, stmt: inner } => {
            let narrowed = current.intersection(RealmSet::from_realm(*realm));
            if contains_offset(inner.span, offset) {
                return Some(active_realms_in_stmt(inner, offset, narrowed).unwrap_or(narrowed));
            }
            Some(narrowed)
        }
        StmtKind::ExportDecl {
            realm, stmt: inner, ..
        } => {
            let narrowed = realm
                .map(RealmSet::from_realm)
                .map(|realms| current.intersection(realms))
                .unwrap_or(current);
            if contains_offset(inner.span, offset) {
                return Some(active_realms_in_stmt(inner, offset, narrowed).unwrap_or(narrowed));
            }
            Some(narrowed)
        }
        StmtKind::RealmBlock { realm, block } => {
            let narrowed = current.intersection(RealmSet::from_realm(*realm));
            if contains_offset(block.span, offset) {
                return Some(active_realms_in_block(block, offset, narrowed));
            }
            Some(narrowed)
        }
        StmtKind::InitDecl { realm, block } => {
            let narrowed = realm
                .map(RealmSet::from_realm)
                .map(|realms| current.intersection(realms))
                .unwrap_or(current);
            if contains_offset(block.span, offset) {
                return Some(active_realms_in_block(block, offset, narrowed));
            }
            Some(narrowed)
        }
        StmtKind::FunctionDecl(decl) => {
            Some(active_realms_in_function_body(&decl.body, offset, current))
        }
        StmtKind::If {
            then_block,
            else_block,
            ..
        } => {
            if contains_offset(then_block.span, offset) {
                return Some(active_realms_in_block(then_block, offset, current));
            }
            if let Some(block) = else_block
                && contains_offset(block.span, offset)
            {
                return Some(active_realms_in_block(block, offset, current));
            }
            Some(current)
        }
        StmtKind::While { body, .. }
        | StmtKind::NumericFor { body, .. }
        | StmtKind::GenericFor { body, .. }
        | StmtKind::RepeatUntil { body, .. }
        | StmtKind::Do(body) => {
            if contains_offset(body.span, offset) {
                return Some(active_realms_in_block(body, offset, current));
            }
            Some(current)
        }
        _ => Some(current),
    }
}

fn active_realms_in_function_body(
    body: &FunctionBody,
    offset: usize,
    current: RealmSet,
) -> RealmSet {
    match body {
        FunctionBody::Expr(_) => current,
        FunctionBody::Block(block) => active_realms_in_block(block, offset, current),
    }
}

fn active_realms_in_block(block: &Block, offset: usize, current: RealmSet) -> RealmSet {
    active_realms_in_stmts(&block.statements, offset, current)
}

fn collect_file_map(
    files: impl IntoIterator<Item = AnalysisFile>,
) -> Result<BTreeMap<PathBuf, String>, AnalysisError> {
    let mut map = BTreeMap::new();
    for file in files {
        map.insert(file.path, file.text);
    }
    Ok(map)
}

fn load_source_root_files(
    config: &AnalysisConfig,
    overlays: impl IntoIterator<Item = AnalysisFile>,
) -> Result<BTreeMap<PathBuf, String>, AnalysisError> {
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
    Ok(files)
}

fn cached_files(
    config: &AnalysisConfig,
    files: BTreeMap<PathBuf, String>,
) -> BTreeMap<PathBuf, CachedAnalysisFile> {
    let package_id = config.effective_package_id();
    files
        .into_iter()
        .map(|(path, text)| {
            let module_path = infer_module_path(&config.source_root, &path);
            let module_id = ModuleId::from_package_path(&package_id, &module_path);
            (
                path.clone(),
                CachedAnalysisFile {
                    path,
                    text,
                    module_id,
                },
            )
        })
        .collect()
}

fn analyze_cached_files(
    config: &AnalysisConfig,
    files: &BTreeMap<PathBuf, CachedAnalysisFile>,
) -> Result<ProjectAnalysis, AnalysisError> {
    analyze_file_map(
        config.clone(),
        files
            .iter()
            .map(|(path, file)| (path.clone(), file.text.clone()))
            .collect(),
    )
}

fn source_files_from_cache(files: &BTreeMap<PathBuf, CachedAnalysisFile>) -> Vec<SourceFile> {
    files
        .values()
        .enumerate()
        .map(|(index, file)| {
            SourceFile::new(index as u32, Some(file.path.clone()), file.text.clone())
        })
        .collect()
}

fn file_keys_changed(
    previous: &BTreeMap<PathBuf, CachedAnalysisFile>,
    next: &BTreeMap<PathBuf, String>,
) -> bool {
    previous.keys().ne(next.keys())
}

fn changed_modules(
    previous: &BTreeMap<PathBuf, CachedAnalysisFile>,
    next: &BTreeMap<PathBuf, String>,
) -> BTreeSet<ModuleId> {
    previous
        .iter()
        .filter_map(|(path, old)| {
            let new_text = next.get(path)?;
            (old.text != *new_text).then(|| old.module_id.clone())
        })
        .collect()
}

fn affected_modules_for_change(
    analysis: &ProjectAnalysis,
    changed_modules: &BTreeSet<ModuleId>,
) -> Vec<ModuleId> {
    let mut affected = changed_modules.clone();
    let mut queue = changed_modules.iter().cloned().collect::<VecDeque<_>>();
    while let Some(changed) = queue.pop_front() {
        let dependents = analysis
            .modules
            .iter()
            .filter(|module| !affected.contains(&module.id))
            .filter(|module| module.imports.iter().any(|import| import.target == changed))
            .map(|module| module.id.clone())
            .collect::<Vec<_>>();
        for dependent in dependents {
            if affected.insert(dependent.clone()) {
                queue.push_back(dependent);
            }
        }
    }
    analysis
        .modules
        .iter()
        .map(|module| module.id.clone())
        .filter(|module_id| affected.contains(module_id))
        .collect()
}

fn analyze_single_module(
    config: &AnalysisConfig,
    files: &BTreeMap<PathBuf, CachedAnalysisFile>,
    module_id: &ModuleId,
    macro_registry: &MacroRegistry,
    diagnostics: &mut Vec<Diagnostic>,
) -> Result<Option<AnalyzedModule>, AnalysisError> {
    let module_files = files
        .values()
        .filter(|file| &file.module_id == module_id)
        .map(|file| (file.path.clone(), file.text.clone()))
        .collect::<BTreeMap<_, _>>();
    if module_files.is_empty() {
        return Ok(None);
    }

    let package_id = config.effective_package_id();
    let mut module = None::<AnalyzedModule>;
    for (index, (path, text)) in module_files.into_iter().enumerate() {
        let source_file = SourceFile::new(index as u32, Some(path.clone()), text);
        let module_path = infer_module_path(&config.source_root, &path);
        let parsed_module_id = ModuleId::from_package_path(&package_id, &module_path);
        let default_realm = match infer_part_realm(&config.source_root, &path, source_file.id) {
            Ok(realm) => realm,
            Err(diagnostic) => {
                diagnostics.push(diagnostic);
                continue;
            }
        };
        let lex = Lexer::new(&source_file).lex_all();
        if lex.has_errors() {
            diagnostics.extend(lex.diagnostics);
            continue;
        }
        let parsed = Parser::new(&lex.tokens).parse_module();
        if parsed.has_errors() {
            diagnostics.extend(parsed.diagnostics);
            continue;
        }
        let expanded = expand_macros_with_registry(&source_file, &parsed.module, macro_registry);
        if expanded.has_errors() {
            diagnostics.extend(expanded.diagnostics);
            continue;
        }
        module
            .get_or_insert_with(|| AnalyzedModule {
                id: parsed_module_id.clone(),
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
                path,
                default_realm,
                module: expanded.module,
                source_file,
            });
    }

    let Some(mut module) = module else {
        return Ok(None);
    };
    finalize_analyzed_module(config, &mut module, diagnostics);
    Ok(Some(module))
}

fn finalize_analyzed_module(
    config: &AnalysisConfig,
    module: &mut AnalyzedModule,
    diagnostics: &mut Vec<Diagnostic>,
) {
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
    let resolved = Resolver::resolve_parts_with_options(&parts, config.resolver_options.clone());
    diagnostics.extend(resolved.diagnostics.clone());
    module.resolved = resolved;
    module.exports = module_exports(module);
    module.imports = module_imports(module);
}

fn diagnostic_touches_modules(
    diagnostic: &Diagnostic,
    analysis: &ProjectAnalysis,
    modules: &BTreeSet<ModuleId>,
) -> bool {
    diagnostic.labels.iter().any(|label| {
        analysis
            .path_for_span(label.span)
            .and_then(|path| analysis.module_for_path(&path))
            .is_some_and(|module| modules.contains(&module.id))
    })
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

fn narrowed_export_realm(message: &str) -> Option<&'static str> {
    if message.contains(" from server to ") {
        Some("server")
    } else if message.contains(" from client to ") {
        Some("client")
    } else {
        None
    }
}

fn export_realm_narrowing_edit(
    file: &SourceFile,
    span: SourceSpan,
    target_realm: &str,
) -> Option<AnalysisTextEdit> {
    let line_start = file.text[..span.byte_start]
        .rfind('\n')
        .map_or(0, |index| index + 1);
    let line_end = file.text[span.byte_start..]
        .find('\n')
        .map_or(file.text.len(), |offset| span.byte_start + offset);
    let line = &file.text[line_start..line_end];
    let shared_offset = line.find("shared")?;
    let start = line_start + shared_offset;
    let end = start + "shared".len();
    Some(AnalysisTextEdit {
        path: file.path.clone()?,
        range: range_for_span(file, SourceSpan::new(file.id, start, end)),
        new_text: target_realm.to_string(),
    })
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

fn completion_kind_for_binding(kind: BindingKind) -> CompletionCandidateKind {
    match kind {
        BindingKind::Function => CompletionCandidateKind::Function,
        BindingKind::Const => CompletionCandidateKind::Constant,
        BindingKind::Param => CompletionCandidateKind::Parameter,
        BindingKind::Import | BindingKind::MacroImport => CompletionCandidateKind::Reference,
        BindingKind::Local => CompletionCandidateKind::Variable,
    }
}

fn binding_completion_candidate(binding: &Binding, detail: Option<String>) -> CompletionCandidate {
    CompletionCandidate {
        label: binding.name.clone(),
        kind: completion_kind_for_binding(binding.kind),
        detail,
        documentation: Some(binding_documentation(binding)),
        source: None,
    }
}

fn binding_documentation(binding: &Binding) -> String {
    let mut out = format!(
        "`{}` is a {} binding available in {}.",
        binding.name,
        binding_kind_name(binding.kind),
        binding.available_realms.display_name()
    );
    if let Some(source) = &binding.source_module {
        out.push_str("\n\nImported from `");
        out.push_str(source);
        out.push('`');
        if let Some(imported) = &binding.imported_name {
            out.push_str(" as `");
            out.push_str(imported);
            out.push('`');
        }
        out.push('.');
    }
    out
}

fn visible_local_binding_ids(
    module: &AnalyzedModule,
    file_id: FileId,
    offset: usize,
) -> Vec<crate::resolve::BindingId> {
    let mut visible = Vec::new();
    let Some(part) = module
        .parts
        .iter()
        .find(|part| part.source_file.id == file_id)
    else {
        return visible;
    };
    let lex = Lexer::new(&part.source_file).lex_all();
    if lex.has_errors() {
        return visible;
    }
    let tokens = lex
        .tokens
        .iter()
        .filter(|token| !matches!(token.kind, TokenKind::Eof))
        .collect::<Vec<_>>();
    for binding in &module.resolved.bindings {
        if binding.module_scope || binding.span.file_id != file_id {
            continue;
        }
        if local_binding_is_visible(&part.source_file, &tokens, binding.span, offset) {
            visible.push(binding.id);
        }
    }
    visible
}

fn local_binding_is_visible(
    file: &SourceFile,
    tokens: &[&Token],
    binding_span: SourceSpan,
    offset: usize,
) -> bool {
    if offset < binding_span.byte_end {
        return false;
    }
    let Some(index) = tokens.iter().position(|token| token.span == binding_span) else {
        return false;
    };
    let Some(scope) = lexical_scope_for_binding(file, tokens, index) else {
        return false;
    };
    scope.byte_start <= offset && offset <= scope.byte_end
}

fn lexical_scope_for_binding(
    file: &SourceFile,
    tokens: &[&Token],
    index: usize,
) -> Option<SourceSpan> {
    let file_id = tokens.get(index)?.span.file_id;
    let fallback_end = tokens
        .last()
        .map(|token| token.span.byte_end)
        .unwrap_or(tokens[index].span.byte_end);
    let Some(function_start) = enclosing_function_token_index(file, tokens, index) else {
        let block_start = innermost_open_block_token_index(
            file,
            tokens,
            0,
            tokens.len().saturating_sub(1),
            index,
        );
        if let Some(block_start) = block_start {
            let block_end =
                matching_scope_end(file, tokens, block_start).unwrap_or(tokens.len() - 1);
            return Some(SourceSpan::new(
                file_id,
                tokens[block_start].span.byte_start,
                tokens[block_end].span.byte_end,
            ));
        }
        return Some(SourceSpan::new(
            file_id,
            tokens[index].span.byte_start,
            fallback_end,
        ));
    };
    let function_end = matching_scope_end(file, tokens, function_start)?;
    let fn_scope = SourceSpan::new(
        file_id,
        tokens[function_start].span.byte_start,
        tokens[function_end].span.byte_end,
    );
    if is_function_parameter(tokens, function_start, index) {
        return Some(fn_scope);
    }
    let block_start =
        innermost_open_block_token_index(file, tokens, function_start, function_end, index)
            .unwrap_or(function_start);
    let block_end = matching_scope_end(file, tokens, block_start).unwrap_or(function_end);
    Some(SourceSpan::new(
        tokens[block_start].span.file_id,
        tokens[block_start].span.byte_start,
        tokens[block_end].span.byte_end,
    ))
}

fn enclosing_function_token_index(
    file: &SourceFile,
    tokens: &[&Token],
    index: usize,
) -> Option<usize> {
    let mut best = None;
    for candidate in 0..=index {
        if !matches!(tokens[candidate].kind, TokenKind::KwFn) {
            continue;
        }
        if let Some(end) = matching_scope_end(file, tokens, candidate)
            && candidate < index
            && index <= end
        {
            best = Some(candidate);
        }
    }
    best
}

fn innermost_open_block_token_index(
    file: &SourceFile,
    tokens: &[&Token],
    function_start: usize,
    function_end: usize,
    index: usize,
) -> Option<usize> {
    let mut best = None;
    for candidate in function_start..index {
        if !is_scope_start_token(tokens, candidate) {
            continue;
        }
        if let Some(end) = matching_scope_end(file, tokens, candidate)
            && index <= end
            && end <= function_end
        {
            best = Some(candidate);
        }
    }
    best
}

fn is_function_parameter(tokens: &[&Token], function_start: usize, index: usize) -> bool {
    let Some(open) = next_token_index(tokens, function_start + 1, |kind| {
        matches!(kind, TokenKind::LParen)
    }) else {
        return false;
    };
    let Some(close) = matching_delimiter(tokens, open, TokenKindDiscriminant::LParen) else {
        return false;
    };
    open < index && index < close
}

fn matching_scope_end(file: &SourceFile, tokens: &[&Token], start: usize) -> Option<usize> {
    match &tokens.get(start)?.kind {
        TokenKind::KwFn => function_scope_end(file, tokens, start),
        TokenKind::LBrace => matching_delimiter(tokens, start, TokenKindDiscriminant::LBrace),
        TokenKind::KwIf => block_keyword_scope_end(tokens, start),
        TokenKind::KwDo | TokenKind::KwWhile | TokenKind::KwFor | TokenKind::KwRepeat => {
            block_keyword_scope_end(tokens, start)
        }
        _ => None,
    }
}

fn function_scope_end(
    file: &SourceFile,
    tokens: &[&Token],
    function_start: usize,
) -> Option<usize> {
    if let Some(open) = next_token_index(tokens, function_start + 1, |kind| {
        matches!(kind, TokenKind::LParen)
    }) && let Some(close) = matching_delimiter(tokens, open, TokenKindDiscriminant::LParen)
        && let Some(after) = tokens.get(close + 1)
    {
        if matches!(after.kind, TokenKind::LBrace) {
            return matching_delimiter(tokens, close + 1, TokenKindDiscriminant::LBrace);
        }
        if matches!(
            after.kind,
            TokenKind::Eq | TokenKind::ArrowNormal | TokenKind::ArrowImplicitSelf
        ) {
            return expression_scope_end(file, tokens, close + 1);
        }
    }
    block_keyword_scope_end(tokens, function_start)
}

fn block_keyword_scope_end(tokens: &[&Token], start: usize) -> Option<usize> {
    let mut depth = 0usize;
    for (index, token) in tokens.iter().enumerate().skip(start) {
        match token.kind {
            TokenKind::KwFn
            | TokenKind::KwIf
            | TokenKind::KwDo
            | TokenKind::KwWhile
            | TokenKind::KwFor
            | TokenKind::KwRepeat => {
                depth += 1;
            }
            TokenKind::KwEnd | TokenKind::KwUntil => {
                depth = depth.saturating_sub(1);
                if depth == 0 {
                    return Some(index);
                }
            }
            _ => {}
        }
    }
    tokens.len().checked_sub(1)
}

fn expression_scope_end(file: &SourceFile, tokens: &[&Token], start: usize) -> Option<usize> {
    let line = file.line_col(tokens.get(start)?.span.byte_start).0;
    tokens
        .iter()
        .enumerate()
        .skip(start + 1)
        .find(|(_, token)| {
            file.line_col(token.span.byte_start).0 > line
                && matches!(
                    token.kind,
                    TokenKind::KwFn
                        | TokenKind::KwLocal
                        | TokenKind::KwConst
                        | TokenKind::KwImport
                        | TokenKind::KwExport
                )
        })
        .map(|(index, _)| index.saturating_sub(1))
        .or_else(|| tokens.len().checked_sub(1))
}

fn is_scope_start_token(tokens: &[&Token], index: usize) -> bool {
    match &tokens[index].kind {
        TokenKind::KwFn
        | TokenKind::KwIf
        | TokenKind::KwDo
        | TokenKind::KwWhile
        | TokenKind::KwFor
        | TokenKind::KwRepeat => true,
        TokenKind::LBrace => !is_import_or_export_list_brace(tokens, index),
        _ => false,
    }
}

fn is_import_or_export_list_brace(tokens: &[&Token], brace_index: usize) -> bool {
    tokens[..brace_index]
        .iter()
        .rev()
        .take_while(|token| !matches!(token.kind, TokenKind::Semicolon))
        .any(|token| matches!(token.kind, TokenKind::KwImport | TokenKind::KwExport))
}

#[derive(Debug, Clone, Copy)]
enum TokenKindDiscriminant {
    LParen,
    LBrace,
}

fn matching_delimiter(
    tokens: &[&Token],
    open: usize,
    delimiter: TokenKindDiscriminant,
) -> Option<usize> {
    let (open_matches, close_matches): (fn(&TokenKind) -> bool, fn(&TokenKind) -> bool) =
        match delimiter {
            TokenKindDiscriminant::LParen => (
                |kind: &TokenKind| matches!(kind, TokenKind::LParen),
                |kind: &TokenKind| matches!(kind, TokenKind::RParen),
            ),
            TokenKindDiscriminant::LBrace => (
                |kind: &TokenKind| matches!(kind, TokenKind::LBrace),
                |kind: &TokenKind| matches!(kind, TokenKind::RBrace),
            ),
        };
    let mut depth = 0usize;
    for (index, token) in tokens.iter().enumerate().skip(open) {
        if open_matches(&token.kind) {
            depth += 1;
        } else if close_matches(&token.kind) {
            depth = depth.saturating_sub(1);
            if depth == 0 {
                return Some(index);
            }
        }
    }
    None
}

fn next_token_index(
    tokens: &[&Token],
    start: usize,
    predicate: impl Fn(&TokenKind) -> bool,
) -> Option<usize> {
    tokens
        .iter()
        .enumerate()
        .skip(start)
        .find(|(_, token)| predicate(&token.kind))
        .map(|(index, _)| index)
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

fn build_analysis_graph(
    modules: &[AnalyzedModule],
    external_exports: BTreeMap<ModuleId, Vec<ModuleExport>>,
) -> Result<ModuleGraph, Vec<Diagnostic>> {
    let inputs = modules.iter().map(module_input).collect::<Vec<_>>();
    ModuleGraph::build_with_config(
        inputs,
        crate::module::ModuleGraphConfig {
            external_modules: external_exports.keys().cloned().collect(),
            external_exports,
        },
    )
}

fn runtime_external_exports(
    config: &AnalysisConfig,
    diagnostics: &mut Vec<Diagnostic>,
) -> BTreeMap<ModuleId, Vec<ModuleExport>> {
    match RuntimePackageRegistry::load_default_with_package_roots(&config.package_roots) {
        Ok(runtime_registry) => runtime_registry.export_metadata(),
        Err(err) => {
            diagnostics.push(Diagnostic::error(format!(
                "failed to load Lux runtime package metadata: {err}"
            )));
            BTreeMap::new()
        }
    }
}

fn import_source_for_module(
    current_module: &AnalyzedModule,
    target_module: &AnalyzedModule,
) -> String {
    if current_module.package_id == target_module.package_id {
        target_module.module_path.clone()
    } else {
        format!("@{}", target_module.id.as_str())
    }
}

fn importable_exports_from_module(
    module: &AnalyzedModule,
    source_label: &str,
    active_realms: RealmSet,
) -> Vec<CompletionCandidate> {
    module
        .exports
        .iter()
        .filter(|export| export.realms.contains_all(active_realms))
        .map(|export| {
            let kind = module
                .resolved
                .exports
                .iter()
                .find(|resolved| resolved.name == export.name && resolved.span == export.span)
                .and_then(|resolved| module.resolved.bindings.get(resolved.binding.0))
                .map(|binding| completion_kind_for_binding(binding.kind))
                .unwrap_or(CompletionCandidateKind::Reference);
            CompletionCandidate {
                label: export.name.clone(),
                kind,
                detail: Some(format!(
                    "{} from `{}`",
                    export.realms.display_name(),
                    source_label
                )),
                documentation: Some(format!("Exported by `{}`", module.id)),
                source: Some(source_label.to_string()),
            }
        })
        .collect()
}

fn same_path(a: &Path, b: &Path) -> bool {
    normalized_path(a) == normalized_path(b)
}

fn paths_equal(a: &[PathBuf], b: &[PathBuf]) -> bool {
    a.len() == b.len() && a.iter().zip(b).all(|(a, b)| same_path(a, b))
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
    use super::{AnalysisChangeKind, AnalysisWorkspace};
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
    fn import_completion_uses_active_realm_at_cursor() {
        let root = std::path::PathBuf::from("src");
        let consumer = root.join("hud/module.lux");
        let output = analyze_files(
            AnalysisConfig::new(&root),
            [
                AnalysisFile {
                    path: root.join("api/module.lux"),
                    text: "export client fn open_panel() = nil\nexport server fn grant_item() = nil\nexport fn id(x) = x"
                        .into(),
                },
                AnalysisFile {
                    path: consumer.clone(),
                    text: "client {\n  import { id } from \"api\"\n}\nserver {\n  import { id } from \"api\"\n}\n"
                        .into(),
                },
            ],
        )
        .expect("analysis");

        let client_realms = output
            .active_realms_at_position(&consumer, 1, "  import { ".len())
            .expect("client realms");
        assert_eq!(client_realms, RealmSet::CLIENT);
        let client_exports = output.importable_exports(&consumer, "api", client_realms);
        assert!(
            client_exports
                .iter()
                .any(|candidate| candidate.label == "open_panel")
        );
        assert!(
            !client_exports
                .iter()
                .any(|candidate| candidate.label == "grant_item")
        );
        assert!(
            client_exports
                .iter()
                .any(|candidate| candidate.label == "id")
        );

        let server_realms = output
            .active_realms_at_position(&consumer, 4, "  import { ".len())
            .expect("server realms");
        assert_eq!(server_realms, RealmSet::SERVER);
        let server_exports = output.importable_exports(&consumer, "api", server_realms);
        assert!(
            server_exports
                .iter()
                .any(|candidate| candidate.label == "grant_item")
        );
        assert!(
            !server_exports
                .iter()
                .any(|candidate| candidate.label == "open_panel")
        );
        assert!(
            server_exports
                .iter()
                .any(|candidate| candidate.label == "id")
        );
    }

    #[test]
    fn import_completion_without_source_lists_runtime_exports() {
        let root = std::path::PathBuf::from("src");
        let consumer = root.join("client/ui.lux");
        let output = analyze_files(
            AnalysisConfig::new(&root).with_package_id("game"),
            [AnalysisFile {
                path: consumer.clone(),
                text: "import { Bu".into(),
            }],
        )
        .expect("analysis");

        let exports = output.importable_exports_for_all_sources(&consumer, RealmSet::CLIENT);
        let button = exports
            .iter()
            .find(|candidate| candidate.label == "Button")
            .expect("Button export from @lux/ui");
        assert_eq!(button.source.as_deref(), Some("@lux/ui"));
        assert!(exports.iter().any(|candidate| candidate.label == "signal"
            && candidate.source.as_deref() == Some("@lux/reactive")));
    }

    #[test]
    fn visible_binding_completion_includes_parameters_and_locals() {
        let root = std::path::PathBuf::from("src");
        let path = root.join("client/ui.lux");
        let text = "import { Button } from \"@lux/ui\"\nlocal module_state = {}\nexport fn mount(panel, players, mode = \"compact\") {\n  local selected = players\n  pla\n}\n";
        let output = analyze_files(
            AnalysisConfig::new(&root).with_package_id("game"),
            [AnalysisFile {
                path: path.clone(),
                text: text.into(),
            }],
        )
        .expect("analysis");
        let offset = output
            .offset_for_position(&path, 4, "  pla".len())
            .expect("offset");
        let labels = output
            .visible_bindings_at_path_offset(&path, offset)
            .into_iter()
            .map(|candidate| candidate.label)
            .collect::<Vec<_>>();

        assert!(labels.iter().any(|label| label == "players"), "{labels:#?}");
        assert!(
            labels.iter().any(|label| label == "selected"),
            "{labels:#?}"
        );
        assert!(
            labels.iter().any(|label| label == "module_state"),
            "{labels:#?}"
        );
        assert!(labels.iter().any(|label| label == "Button"), "{labels:#?}");
    }

    #[test]
    fn analysis_accepts_default_runtime_package_imports() {
        let root = std::path::PathBuf::from("src");
        let output = analyze_files(
            AnalysisConfig::new(&root).with_package_id("game"),
            [AnalysisFile {
                path: root.join("client/ui.lux"),
                text: "import { signal } from \"@lux/reactive\"\nimport { Button } from \"@lux/ui\"\nexport fn run() = signal(0)"
                    .into(),
            }],
        )
        .expect("analysis");

        assert!(
            !output
                .diagnostics
                .iter()
                .any(|diagnostic| diagnostic.code.as_deref() == Some("MODULE001")),
            "{:#?}",
            output.diagnostics
        );
    }

    #[test]
    fn definition_crosses_part_files_for_module_scope_bindings() {
        let root = std::path::PathBuf::from("src");
        let module_path = root.join("inventory/module.lux");
        let state_path = root.join("inventory/state.lux");
        let output = analyze_files(
            AnalysisConfig::new(&root),
            [
                AnalysisFile {
                    path: module_path.clone(),
                    text: "part order { \"state\" }\nfn read() = build_item()".into(),
                },
                AnalysisFile {
                    path: state_path.clone(),
                    text: "fn build_item() = {}".into(),
                },
            ],
        )
        .expect("analysis");

        let offset = output
            .offset_for_position(&module_path, 1, "fn read() = build".len())
            .expect("offset");
        let symbol = output
            .symbol_at_path_offset(&module_path, offset)
            .expect("symbol");

        assert_eq!(symbol.name, "build_item");
        assert_eq!(symbol.definition_path, Some(state_path.clone()));
        let definition_span = symbol.definition_span.expect("definition span");
        let definition_file = output.file_by_id(definition_span.file_id).expect("file");
        assert_eq!(definition_file.slice(definition_span), "build_item");
    }

    #[test]
    fn workspace_update_reanalyzes_changed_module_and_dependents() {
        let root = std::path::PathBuf::from("src");
        let api_path = root.join("api/module.lux");
        let hud_path = root.join("hud/module.lux");
        let other_path = root.join("other/module.lux");
        let mut workspace = AnalysisWorkspace::from_files(
            AnalysisConfig::new(&root),
            [
                AnalysisFile {
                    path: api_path.clone(),
                    text: "export fn old_name() = nil".into(),
                },
                AnalysisFile {
                    path: hud_path.clone(),
                    text: "import { old_name } from \"api\"\nfn draw() = old_name()".into(),
                },
                AnalysisFile {
                    path: other_path.clone(),
                    text: "fn keep() = 1".into(),
                },
            ],
        )
        .expect("workspace");

        let change = workspace
            .update_files(
                AnalysisConfig::new(&root),
                [
                    AnalysisFile {
                        path: api_path.clone(),
                        text: "export fn new_name() = nil".into(),
                    },
                    AnalysisFile {
                        path: hud_path.clone(),
                        text: "import { old_name } from \"api\"\nfn draw() = old_name()".into(),
                    },
                    AnalysisFile {
                        path: other_path.clone(),
                        text: "fn keep() = 1".into(),
                    },
                ],
            )
            .expect("incremental update");

        assert_eq!(change.kind, AnalysisChangeKind::Incremental);
        let affected = change
            .affected_modules
            .iter()
            .map(|id| id.as_str().to_string())
            .collect::<Vec<_>>();
        assert!(affected.iter().any(|id| id.ends_with("/api")));
        assert!(affected.iter().any(|id| id.ends_with("/hud")));
        assert!(!affected.iter().any(|id| id.ends_with("/other")));
        assert!(
            workspace
                .analysis()
                .diagnostics
                .iter()
                .any(|diagnostic| diagnostic.code.as_deref() == Some("MODULE008")),
            "{:#?}",
            workspace.analysis().diagnostics
        );
    }

    #[test]
    fn workspace_source_root_update_keeps_disk_files_not_in_overlays() {
        let root = std::env::temp_dir().join(format!(
            "lux-analysis-workspace-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("time")
                .as_nanos()
        ));
        let api_dir = root.join("api");
        let hud_dir = root.join("hud");
        std::fs::create_dir_all(&api_dir).expect("api dir");
        std::fs::create_dir_all(&hud_dir).expect("hud dir");
        let api_path = api_dir.join("module.lux");
        let hud_path = hud_dir.join("module.lux");
        std::fs::write(&api_path, "export fn old_name() = nil").expect("api file");
        std::fs::write(
            &hud_path,
            "import { old_name } from \"api\"\nfn draw() = old_name()",
        )
        .expect("hud file");

        let mut workspace = AnalysisWorkspace::load(AnalysisConfig::new(&root), std::iter::empty())
            .expect("workspace");
        let change = workspace
            .update_source_root(
                AnalysisConfig::new(&root),
                [AnalysisFile {
                    path: api_path.clone(),
                    text: "export fn new_name() = nil".into(),
                }],
            )
            .expect("overlay update");

        assert_eq!(change.kind, AnalysisChangeKind::Incremental);
        assert!(workspace.analysis().file_by_path(&hud_path).is_some());
        assert!(
            workspace
                .analysis()
                .diagnostics
                .iter()
                .any(|diagnostic| diagnostic.code.as_deref() == Some("MODULE008")),
            "{:#?}",
            workspace.analysis().diagnostics
        );

        std::fs::remove_dir_all(&root).ok();
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

    #[test]
    fn export_realm_widening_has_narrowing_quick_fix() {
        let root = std::path::PathBuf::from("src");
        let path = root.join("module.lux");
        let output = analyze_files(
            AnalysisConfig::new(&root),
            [AnalysisFile {
                path: path.clone(),
                text: "server fn grant() = 1\nexport shared { grant }".into(),
            }],
        )
        .expect("analysis");

        assert!(
            output
                .diagnostics
                .iter()
                .any(|diagnostic| diagnostic.code.as_deref() == Some("REALM002")),
            "{:#?}",
            output.diagnostics
        );
        let actions = output.code_actions_for_path(&path);
        let action = actions
            .iter()
            .find(|action| action.title == "Change export realm to server")
            .expect("realm narrowing action");
        assert_eq!(action.edits.len(), 1);
        assert_eq!(action.edits[0].new_text, "server");
    }
}
