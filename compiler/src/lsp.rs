use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use crate::analysis::{
    AnalysisCodeAction, AnalysisConfig, AnalysisDiagnostic, AnalysisEditKind, AnalysisFile,
    AnalysisRange, AnalysisSemanticToken, AnalysisSignatureHelp, AnalysisWorkspace,
    CompletionCandidate, CompletionCandidateKind, ProjectAnalysis, SemanticTokenKind, format_text,
};
use crate::diag::Severity;
use crate::lex::{Lexer, Token, TokenKind};
use crate::module::{RealmAvailability, RealmSet};
use crate::package_manager::{
    DependencySource, InstallRequest, LUX_STD_REPO, PACKAGE_SET_MANIFEST, install_package,
    package_set_source_roots,
};
use crate::packages::{discover_compile_time_phases, discover_runtime_phases};
use crate::project::ProjectManifest;
use crate::source::SourceFile;
use crossbeam_channel::RecvTimeoutError;
use gmod_api_db::{ApiIndex, entry_markdown, hook_markdown};
use lsp_server::{Connection, ExtractError, Message, Notification, Request, RequestId, Response};
use lsp_types::notification::{
    DidChangeTextDocument, DidChangeWatchedFiles, DidCloseTextDocument, DidOpenTextDocument,
    DidSaveTextDocument, Notification as LspNotification, PublishDiagnostics, ShowMessage,
};
use lsp_types::request::{
    CodeActionRequest, Completion, ExecuteCommand, Formatting, GotoDefinition, HoverRequest,
    Request as LspRequest, ResolveCompletionItem, SemanticTokensFullRequest, SignatureHelpRequest,
};
use lsp_types::{
    CodeAction, CodeActionKind, CodeActionOrCommand, CodeActionParams, CompletionItem,
    CompletionItemKind, CompletionItemLabelDetails, CompletionItemTag, CompletionOptions,
    CompletionParams, CompletionResponse, Diagnostic, DiagnosticRelatedInformation,
    DiagnosticSeverity, DidChangeTextDocumentParams, DidChangeWatchedFilesParams,
    DidCloseTextDocumentParams, DidOpenTextDocumentParams, DidSaveTextDocumentParams,
    DocumentFormattingParams, Documentation, ExecuteCommandOptions, ExecuteCommandParams,
    GotoDefinitionParams, GotoDefinitionResponse, Hover, HoverContents, HoverParams,
    InitializeParams, InsertTextFormat, Location, MarkupContent, MarkupKind, MessageType, OneOf,
    ParameterInformation, ParameterLabel, Position, PublishDiagnosticsParams, Range, SemanticToken,
    SemanticTokenType, SemanticTokens, SemanticTokensLegend, SemanticTokensOptions,
    SemanticTokensParams, SemanticTokensResult, ServerCapabilities, ShowMessageParams,
    SignatureHelp, SignatureHelpOptions, SignatureInformation, TextDocumentContentChangeEvent,
    TextDocumentSyncCapability, TextDocumentSyncKind, TextEdit, Uri, WorkDoneProgressOptions,
};
use url::Url;

pub fn run() -> Result<(), String> {
    let (connection, io_threads) = Connection::stdio();
    let server_capabilities = serde_json::to_value(server_capabilities())
        .map_err(|err| format!("failed to encode capabilities: {err}"))?;
    let initialize_params = connection
        .initialize(server_capabilities)
        .map_err(|err| format!("initialize failed: {err}"))?;
    let initialize_params: InitializeParams = serde_json::from_value(initialize_params)
        .map_err(|err| format!("invalid initialize params: {err}"))?;

    // Drop the connection before joining stdio threads; otherwise the writer
    // side stays alive after shutdown/exit and the process can hang.
    {
        let mut server = Server::new(connection, initialize_params);
        server.event_loop()?;
    }
    io_threads
        .join()
        .map_err(|err| format!("stdio thread failed: {err:?}"))?;
    Ok(())
}

const ANALYSIS_DEBOUNCE: Duration = Duration::from_millis(180);
const INSTALL_STD_PACKAGES_COMMAND: &str = "lux.installStdPackages";

fn server_capabilities() -> ServerCapabilities {
    ServerCapabilities {
        text_document_sync: Some(TextDocumentSyncCapability::Kind(TextDocumentSyncKind::FULL)),
        hover_provider: Some(lsp_types::HoverProviderCapability::Simple(true)),
        completion_provider: Some(CompletionOptions {
            resolve_provider: Some(true),
            trigger_characters: Some(vec![".".into(), ":".into(), "{".into(), "\"".into()]),
            all_commit_characters: None,
            work_done_progress_options: WorkDoneProgressOptions::default(),
            completion_item: Some(lsp_types::CompletionOptionsCompletionItem {
                label_details_support: Some(true),
            }),
        }),
        signature_help_provider: Some(SignatureHelpOptions {
            trigger_characters: Some(vec!["(".into(), ",".into(), "\"".into()]),
            retrigger_characters: Some(vec![",".into()]),
            work_done_progress_options: WorkDoneProgressOptions::default(),
        }),
        definition_provider: Some(OneOf::Left(true)),
        document_formatting_provider: Some(OneOf::Left(true)),
        code_action_provider: Some(lsp_types::CodeActionProviderCapability::Simple(true)),
        execute_command_provider: Some(ExecuteCommandOptions {
            commands: vec![INSTALL_STD_PACKAGES_COMMAND.into()],
            work_done_progress_options: WorkDoneProgressOptions::default(),
        }),
        semantic_tokens_provider: Some(
            lsp_types::SemanticTokensServerCapabilities::SemanticTokensOptions(
                SemanticTokensOptions {
                    work_done_progress_options: WorkDoneProgressOptions::default(),
                    legend: semantic_tokens_legend(),
                    range: Some(false),
                    full: Some(lsp_types::SemanticTokensFullOptions::Bool(true)),
                },
            ),
        ),
        ..ServerCapabilities::default()
    }
}

fn semantic_tokens_legend() -> SemanticTokensLegend {
    SemanticTokensLegend {
        token_types: vec![
            SemanticTokenType::KEYWORD,
            SemanticTokenType::new("realm"),
            SemanticTokenType::FUNCTION,
            SemanticTokenType::PARAMETER,
            SemanticTokenType::VARIABLE,
            SemanticTokenType::PROPERTY,
            SemanticTokenType::NAMESPACE,
            SemanticTokenType::TYPE,
            SemanticTokenType::STRING,
            SemanticTokenType::NUMBER,
            SemanticTokenType::COMMENT,
            SemanticTokenType::OPERATOR,
            SemanticTokenType::new("export"),
            SemanticTokenType::new("import"),
            SemanticTokenType::new("external"),
            SemanticTokenType::new("unknownExternal"),
        ],
        token_modifiers: Vec::new(),
    }
}

struct Server {
    connection: Connection,
    root: PathBuf,
    documents: HashMap<Uri, String>,
    document_versions: HashMap<Uri, i32>,
    published_diagnostics: BTreeSet<Uri>,
    workspaces: BTreeMap<String, AnalysisWorkspace>,
    gmod_api: ApiIndex,
    analysis_due: Option<Instant>,
}

struct DocumentSnapshot {
    path: Option<PathBuf>,
    file: crate::source::SourceFile,
    offset: usize,
}

impl Server {
    fn new(connection: Connection, initialize: InitializeParams) -> Self {
        let root = workspace_root(&initialize);
        Self {
            connection,
            root,
            documents: HashMap::new(),
            document_versions: HashMap::new(),
            published_diagnostics: BTreeSet::new(),
            workspaces: BTreeMap::new(),
            gmod_api: ApiIndex::bundled(),
            analysis_due: None,
        }
    }

    fn event_loop(&mut self) -> Result<(), String> {
        debug_log(format!(
            "start root={} exe={}",
            self.root.display(),
            std::env::current_exe()
                .map(|path| path.display().to_string())
                .unwrap_or_else(|err| format!("<unknown: {err}>"))
        ));
        self.reanalyze_and_publish();
        loop {
            let Some(message) = self.next_message()? else {
                break;
            };
            match message {
                Message::Request(request) => {
                    if self
                        .connection
                        .handle_shutdown(&request)
                        .map_err(|err| err.to_string())?
                    {
                        return Ok(());
                    }
                    self.handle_request(request)?;
                }
                Message::Notification(notification) => {
                    self.handle_notification(notification)?;
                }
                Message::Response(_) => {}
            }
        }
        Ok(())
    }

    fn next_message(&mut self) -> Result<Option<Message>, String> {
        loop {
            let Some(due) = self.analysis_due else {
                return match self.connection.receiver.recv() {
                    Ok(message) => Ok(Some(message)),
                    Err(_) => Ok(None),
                };
            };
            let now = Instant::now();
            if now >= due {
                self.reanalyze_and_publish();
                continue;
            }
            match self
                .connection
                .receiver
                .recv_timeout(due.duration_since(now))
            {
                Ok(message) => return Ok(Some(message)),
                Err(RecvTimeoutError::Timeout) => {
                    self.reanalyze_and_publish();
                }
                Err(RecvTimeoutError::Disconnected) => return Ok(None),
            }
        }
    }

    fn handle_request(&mut self, request: Request) -> Result<(), String> {
        let request = match self.try_request::<HoverRequest>(request, Self::hover)? {
            Some(request) => request,
            None => return Ok(()),
        };
        let request = match self.try_request::<Completion>(request, Self::completion)? {
            Some(request) => request,
            None => return Ok(()),
        };
        let request =
            match self.try_request::<ResolveCompletionItem>(request, Self::completion_resolve)? {
                Some(request) => request,
                None => return Ok(()),
            };
        let request =
            match self.try_request::<SignatureHelpRequest>(request, Self::signature_help)? {
                Some(request) => request,
                None => return Ok(()),
            };
        let request = match self.try_request::<GotoDefinition>(request, Self::definition)? {
            Some(request) => request,
            None => return Ok(()),
        };
        let request = match self.try_request::<Formatting>(request, Self::formatting)? {
            Some(request) => request,
            None => return Ok(()),
        };
        let request =
            match self.try_request::<SemanticTokensFullRequest>(request, Self::semantic_tokens)? {
                Some(request) => request,
                None => return Ok(()),
            };
        let request = match self.try_request::<CodeActionRequest>(request, Self::code_actions)? {
            Some(request) => request,
            None => return Ok(()),
        };
        let request = match self.try_request::<ExecuteCommand>(request, Self::execute_command)? {
            Some(request) => request,
            None => return Ok(()),
        };

        self.respond_error(
            request.id,
            lsp_server::ErrorCode::MethodNotFound as i32,
            format!("unsupported request `{}`", request.method),
        )
    }

    fn try_request<R>(
        &mut self,
        request: Request,
        handler: fn(&mut Self, R::Params) -> Result<serde_json::Value, String>,
    ) -> Result<Option<Request>, String>
    where
        R: LspRequest,
        R::Params: serde::de::DeserializeOwned,
    {
        let invalid_id = request.id.clone();
        match request.extract::<R::Params>(R::METHOD) {
            Ok((id, params)) => {
                let result = handler(self, params);
                match result {
                    Ok(value) => self.respond(id, value),
                    Err(err) => {
                        self.respond_error(id, lsp_server::ErrorCode::InternalError as i32, err)
                    }
                }?;
                Ok(None)
            }
            Err(ExtractError::MethodMismatch(request)) => Ok(Some(request)),
            Err(ExtractError::JsonError { method, error }) => {
                self.respond_error(
                    invalid_id,
                    lsp_server::ErrorCode::InvalidParams as i32,
                    format!("invalid params for {method}: {error}"),
                )?;
                Ok(None)
            }
        }
    }

    fn handle_notification(&mut self, notification: Notification) -> Result<(), String> {
        match notification.method.as_str() {
            DidOpenTextDocument::METHOD => {
                let params: DidOpenTextDocumentParams = serde_json::from_value(notification.params)
                    .map_err(|err| format!("invalid didOpen params: {err}"))?;
                let raw_uri = params.text_document.uri;
                let uri = document_uri_key(&raw_uri);
                let version = params.text_document.version;
                let text = params.text_document.text;
                debug_log(format!(
                    "didOpen raw_uri={raw_uri:?} key={uri:?} version={version} len={} focus={}",
                    text.len(),
                    focus_lines(&text)
                ));
                self.document_versions.insert(uri.clone(), version);
                self.documents.insert(uri, text);
                self.reanalyze_and_publish();
            }
            DidChangeTextDocument::METHOD => {
                let params: DidChangeTextDocumentParams =
                    serde_json::from_value(notification.params)
                        .map_err(|err| format!("invalid didChange params: {err}"))?;
                let raw_uri = params.text_document.uri;
                let uri = document_uri_key(&raw_uri);
                let version = params.text_document.version;
                let current = self
                    .documents
                    .get(&uri)
                    .cloned()
                    .or_else(|| {
                        url_to_path(&uri).and_then(|path| std::fs::read_to_string(path).ok())
                    })
                    .unwrap_or_default();
                let change_summary = document_change_summary(&params.content_changes);
                let text = apply_document_changes(current.clone(), params.content_changes);
                debug_log(format!(
                    "didChange raw_uri={raw_uri:?} key={uri:?} version={version} before_len={} after_len={} changes=[{}] focus={}",
                    current.len(),
                    text.len(),
                    change_summary,
                    focus_lines(&text)
                ));
                self.document_versions.insert(uri.clone(), version);
                self.documents.insert(uri.clone(), text);
                self.schedule_reanalysis();
            }
            DidSaveTextDocument::METHOD => {
                let params: DidSaveTextDocumentParams = serde_json::from_value(notification.params)
                    .map_err(|err| format!("invalid didSave params: {err}"))?;
                let raw_uri = params.text_document.uri;
                let uri = document_uri_key(&raw_uri);
                if let Some(text) = params.text {
                    debug_log(format!(
                        "didSave raw_uri={raw_uri:?} key={uri:?} full_text_len={} focus={}",
                        text.len(),
                        focus_lines(&text)
                    ));
                    self.documents.insert(uri.clone(), text);
                } else {
                    debug_log(format!(
                        "didSave raw_uri={raw_uri:?} key={uri:?} no full text"
                    ));
                }
                self.reanalyze_and_publish();
            }
            DidCloseTextDocument::METHOD => {
                let params: DidCloseTextDocumentParams =
                    serde_json::from_value(notification.params)
                        .map_err(|err| format!("invalid didClose params: {err}"))?;
                let uri = document_uri_key(&params.text_document.uri);
                self.documents.remove(&uri);
                self.document_versions.remove(&uri);
                self.reanalyze_and_publish();
            }
            DidChangeWatchedFiles::METHOD => {
                let params: DidChangeWatchedFilesParams =
                    serde_json::from_value(notification.params)
                        .map_err(|err| format!("invalid didChangeWatchedFiles params: {err}"))?;
                if params.changes.iter().any(|event| {
                    url_to_path(&event.uri).is_some_and(|path| is_lux_analysis_watched_path(&path))
                }) {
                    self.workspaces.clear();
                    self.reanalyze_and_publish();
                }
            }
            _ => {}
        }
        Ok(())
    }

    fn schedule_reanalysis(&mut self) {
        self.analysis_due = Some(Instant::now() + ANALYSIS_DEBOUNCE);
    }

    fn flush_pending_analysis(&mut self) {
        if self.analysis_due.take().is_some() {
            self.reanalyze_and_publish();
        }
    }

    fn hover(&mut self, params: HoverParams) -> Result<serde_json::Value, String> {
        self.flush_pending_analysis();
        let uri = &params.text_document_position_params.text_document.uri;
        let position = params.text_document_position_params.position;
        let snapshot = self.document_snapshot(uri, position);
        if let Some((analysis, path, offset)) = self.analysis_and_offset(uri, position) {
            if let Some(symbol) = analysis.symbol_at_path_offset(&path, offset) {
                match symbol.external_availability.as_ref() {
                    Some(RealmAvailability::Known(_)) => {
                        if let Some(markdown) =
                            external_api_hover_markdown(analysis, &self.gmod_api, &path, offset)
                        {
                            return json_result(Some(markdown_hover(markdown)));
                        }
                    }
                    Some(RealmAvailability::UnknownExternal) | None => {
                        if let Some(markdown) =
                            analysis.hover_markdown_at_path_offset(&path, offset)
                        {
                            return json_result(Some(markdown_hover(markdown)));
                        }
                    }
                }
            }
        }
        if let Some(markdown) =
            hook_hover_markdown_from_text(&self.gmod_api, &snapshot.file.text, snapshot.offset)
        {
            return json_result(Some(markdown_hover(markdown)));
        }
        if let Some(markdown) =
            api_hover_markdown_from_text(&self.gmod_api, &snapshot.file.text, snapshot.offset)
        {
            return json_result(Some(markdown_hover(markdown)));
        }
        json_result::<Option<Hover>>(None)
    }

    fn completion(&mut self, params: CompletionParams) -> Result<serde_json::Value, String> {
        self.flush_pending_analysis();
        let uri = &params.text_document_position.text_document.uri;
        let snapshot = self.document_snapshot(uri, params.text_document_position.position);
        let path = snapshot.path;
        let analysis = path
            .as_deref()
            .and_then(|path| self.analysis_for_path(path))
            .or_else(|| self.analysis());
        let offset = snapshot.offset;
        let line_prefix = snapshot.file.text[..offset]
            .rsplit('\n')
            .next()
            .unwrap_or_default();
        let line_suffix = snapshot.file.text[offset..]
            .split('\n')
            .next()
            .unwrap_or_default();

        let items = match completion_context(line_prefix, line_suffix) {
            CompletionContext::ImportSource => analysis
                .map(ProjectAnalysis::module_path_completions)
                .unwrap_or_default()
                .into_iter()
                .map(completion_item)
                .collect::<Vec<_>>(),
            CompletionContext::ImportSpecifierList { source } => analysis
                .zip(path.as_deref())
                .map(|(analysis, path)| {
                    let active_realms = analysis
                        .active_realms_at_path_offset(path, offset)
                        .unwrap_or(RealmSet::SHARED);
                    match source.as_deref() {
                        Some(source) => analysis.importable_exports(path, source, active_realms),
                        None => analysis.importable_exports_for_all_sources(path, active_realms),
                    }
                })
                .unwrap_or_default()
                .into_iter()
                .map(|candidate| import_completion_item(candidate, source.is_none()))
                .collect::<Vec<_>>(),
            CompletionContext::ExportList => analysis
                .zip(path.as_deref())
                .map(|(analysis, path)| analysis.exportable_bindings(path))
                .unwrap_or_default()
                .into_iter()
                .map(completion_item)
                .collect::<Vec<_>>(),
            CompletionContext::ApiMember { prefix } => api_completion_candidates(
                &self.gmod_api,
                &prefix,
                (!snapshot.file.text.is_empty()).then_some(snapshot.file.text.as_str()),
            ),
            CompletionContext::General => {
                let current_prefix = identifier_prefix(line_prefix);
                let mut items = analysis
                    .zip(path.as_deref())
                    .map(|(analysis, path)| {
                        general_binding_completions(analysis, path, offset, &snapshot.file)
                    })
                    .unwrap_or_else(|| lexical_binding_completions(&snapshot.file, offset))
                    .into_iter()
                    .map(completion_item)
                    .collect::<Vec<_>>();
                let mut existing_labels = items
                    .iter()
                    .map(|item| item.label.clone())
                    .collect::<BTreeSet<_>>();
                let fallback = lexical_binding_completions(&snapshot.file, offset);
                items.extend(
                    fallback
                        .into_iter()
                        .filter(|candidate| existing_labels.insert(candidate.label.clone()))
                        .map(completion_item),
                );
                let mut existing_labels = items
                    .iter()
                    .map(|item| item.label.clone())
                    .collect::<BTreeSet<_>>();
                items.extend(
                    keyword_completion_items(current_prefix)
                        .into_iter()
                        .filter(|item| existing_labels.insert(item.label.clone())),
                );
                items.extend(
                    api_root_completion_candidates(&self.gmod_api, current_prefix)
                        .into_iter()
                        .filter(|item| !existing_labels.contains(&item.label)),
                );
                items
            }
        };

        json_result(Some(CompletionResponse::Array(items)))
    }

    fn completion_resolve(
        &mut self,
        mut item: CompletionItem,
    ) -> Result<serde_json::Value, String> {
        if let Some(path) = gmod_completion_path(&item)
            && let Some(entry) = self.gmod_api.entry(path)
        {
            item.detail = Some(api_completion_detail(entry));
            item.documentation = Some(markdown_documentation(entry_markdown(entry)));
            item.label_details = api_completion_label_details(entry, &item.label);
            item.tags = completion_tags_for_api(entry);
        }
        json_result(item)
    }

    fn signature_help(
        &mut self,
        params: lsp_types::SignatureHelpParams,
    ) -> Result<serde_json::Value, String> {
        self.flush_pending_analysis();
        let Some((analysis, path, offset)) = self.analysis_and_offset(
            &params.text_document_position_params.text_document.uri,
            params.text_document_position_params.position,
        ) else {
            return json_result::<Option<SignatureHelp>>(None);
        };
        if let Some(help) = analysis.signature_help_at_path_offset(&path, offset) {
            return json_result(Some(signature_help_from_analysis(help)));
        }
        let Some(file) = analysis.file_by_path(&path) else {
            return json_result::<Option<SignatureHelp>>(None);
        };
        let Some(help) = signature_help_at(file, &self.gmod_api, offset) else {
            return json_result::<Option<SignatureHelp>>(None);
        };
        json_result(Some(help))
    }

    fn definition(&mut self, params: GotoDefinitionParams) -> Result<serde_json::Value, String> {
        self.flush_pending_analysis();
        let Some((analysis, path, offset)) = self.analysis_and_offset(
            &params.text_document_position_params.text_document.uri,
            params.text_document_position_params.position,
        ) else {
            return json_result::<Option<GotoDefinitionResponse>>(None);
        };
        let Some(symbol) = analysis.symbol_at_path_offset(&path, offset) else {
            return json_result::<Option<GotoDefinitionResponse>>(None);
        };
        let Some(def_span) = symbol.definition_span else {
            return json_result::<Option<GotoDefinitionResponse>>(None);
        };
        let Some(def_path) = symbol.definition_path else {
            return json_result::<Option<GotoDefinitionResponse>>(None);
        };
        let Some(file) = analysis.file_by_id(def_span.file_id) else {
            return json_result::<Option<GotoDefinitionResponse>>(None);
        };
        let Some(uri) = path_to_url(&def_path) else {
            return json_result::<Option<GotoDefinitionResponse>>(None);
        };
        json_result(Some(GotoDefinitionResponse::Scalar(Location {
            uri,
            range: range(file, def_span),
        })))
    }

    fn formatting(
        &mut self,
        params: DocumentFormattingParams,
    ) -> Result<serde_json::Value, String> {
        let Some(path) = url_to_path(&params.text_document.uri) else {
            return json_result::<Option<Vec<TextEdit>>>(None);
        };
        let text = self
            .documents
            .get(&params.text_document.uri)
            .cloned()
            .or_else(|| std::fs::read_to_string(&path).ok())
            .unwrap_or_default();
        let output = format_text(path.clone(), text.clone());
        let file = crate::source::SourceFile::new(0, Some(path), text);
        let edits = if output.text == file.text {
            Vec::new()
        } else {
            vec![TextEdit {
                range: Range {
                    start: Position {
                        line: 0,
                        character: 0,
                    },
                    end: range(
                        &file,
                        crate::source::SourceSpan::new(file.id, 0, file.text.len()),
                    )
                    .end,
                },
                new_text: output.text,
            }]
        };
        json_result(Some(edits))
    }

    fn semantic_tokens(
        &mut self,
        params: SemanticTokensParams,
    ) -> Result<serde_json::Value, String> {
        self.flush_pending_analysis();
        let Some(path) = url_to_path(&params.text_document.uri) else {
            return json_result::<Option<SemanticTokensResult>>(None);
        };
        let Some(analysis) = self.analysis_for_path(&path) else {
            return json_result::<Option<SemanticTokensResult>>(None);
        };
        let Some(file) = analysis.file_by_path(&path) else {
            return json_result::<Option<SemanticTokensResult>>(None);
        };
        let data = encode_semantic_tokens(file, analysis.semantic_tokens_for_path(&path));
        json_result(Some(SemanticTokensResult::Tokens(SemanticTokens {
            result_id: None,
            data,
        })))
    }

    fn code_actions(&mut self, params: CodeActionParams) -> Result<serde_json::Value, String> {
        self.flush_pending_analysis();
        let Some(path) = url_to_path(&params.text_document.uri) else {
            return json_result::<Option<Vec<CodeActionOrCommand>>>(None);
        };
        let Some(analysis) = self.analysis_for_path(&path) else {
            return json_result::<Option<Vec<CodeActionOrCommand>>>(None);
        };
        let actions = analysis
            .code_actions_for_path(&path)
            .into_iter()
            .map(|action| code_action(action, &params.text_document.uri))
            .chain(api_doc_code_actions(
                analysis,
                &self.gmod_api,
                &path,
                &params.text_document.uri,
            ))
            .chain(manifest_extern_code_actions(analysis, &path, &self.root))
            .chain(std_package_code_actions(
                analysis,
                &path,
                &self.root,
                &params.text_document.uri,
            ))
            .collect::<Vec<_>>();
        json_result(Some(actions))
    }

    fn execute_command(
        &mut self,
        params: ExecuteCommandParams,
    ) -> Result<serde_json::Value, String> {
        match params.command.as_str() {
            "lux.showModuleExports" => {
                let command = CommandDocumentPosition::from_arguments(&params.arguments)?;
                let Some(analysis) = command
                    .as_ref()
                    .and_then(|position| url_to_path(&position.uri))
                    .and_then(|path| self.analysis_for_path(&path))
                    .or_else(|| self.analysis())
                else {
                    return json_result(CommandResult::message("Lux analysis is not ready."));
                };
                json_result(module_exports_command(analysis, command.as_ref()))
            }
            "lux.showActiveRealm" => {
                let command = CommandDocumentPosition::from_arguments(&params.arguments)?;
                let Some(analysis) = command
                    .as_ref()
                    .and_then(|position| url_to_path(&position.uri))
                    .and_then(|path| self.analysis_for_path(&path))
                    .or_else(|| self.analysis())
                else {
                    return json_result(CommandResult::message("Lux analysis is not ready."));
                };
                json_result(active_realm_command(analysis, command.as_ref()))
            }
            "lux.gmodApiCoverage" => json_result(gmod_api_coverage_command(&self.gmod_api)),
            "lux.reloadWorkspace" => {
                self.workspaces.clear();
                self.reanalyze_and_publish();
                json_result(CommandResult::message("Lux workspace analysis reloaded."))
            }
            INSTALL_STD_PACKAGES_COMMAND => json_result(self.install_std_packages(&params)?),
            other => Err(format!("unsupported command `{other}`")),
        }
    }

    fn install_std_packages(
        &mut self,
        params: &ExecuteCommandParams,
    ) -> Result<CommandResult, String> {
        let command = InstallStdPackagesCommand::from_arguments(&params.arguments)?;
        let Some(project_root) = command.project_root else {
            let message = "No Lux project root was provided for package installation.";
            self.show_message(MessageType::ERROR, message);
            return Ok(CommandResult::message(message));
        };
        let packages = command
            .packages
            .into_iter()
            .filter(|package| is_official_lux_package(package))
            .collect::<BTreeSet<_>>();
        if packages.is_empty() {
            let message = "No missing official Lux packages were found for installation.";
            self.show_message(MessageType::WARNING, message);
            return Ok(CommandResult::message(message));
        }

        let mut installed = Vec::new();
        for package in &packages {
            let request = InstallRequest {
                project_root: project_root.clone(),
                package: package.clone(),
                source: DependencySource::Github {
                    repo: LUX_STD_REPO.into(),
                    tag: None,
                    branch: None,
                    commit: None,
                },
            };
            if let Err(err) = install_package(&request) {
                let message = format!("Failed to install {package}: {err}");
                self.show_message(MessageType::ERROR, &message);
                if !installed.is_empty() {
                    self.workspaces.clear();
                    self.reanalyze_and_publish();
                }
                return Ok(CommandResult::message(message));
            }
            installed.push(package.clone());
        }

        self.workspaces.clear();
        self.reanalyze_and_publish();
        let message = format!(
            "Installed {} from github:{LUX_STD_REPO}.",
            installed.join(", ")
        );
        self.show_message(MessageType::INFO, &message);
        Ok(CommandResult::message(message))
    }

    fn analysis_and_offset(
        &self,
        uri: &Uri,
        position: Position,
    ) -> Option<(&ProjectAnalysis, PathBuf, usize)> {
        let path = url_to_path(uri)?;
        let analysis = self.analysis_for_path(&path)?;
        let offset = analysis.offset_for_position(
            &path,
            position.line as usize,
            position.character as usize,
        )?;
        Some((analysis, path, offset))
    }

    fn document_snapshot(&self, uri: &Uri, position: Position) -> DocumentSnapshot {
        let key = document_uri_key(uri);
        let path = url_to_path(uri);
        let text = self
            .documents
            .get(&key)
            .cloned()
            .or_else(|| {
                path.as_deref().and_then(|path| {
                    self.analysis_for_path(path)
                        .and_then(|analysis| analysis.file_by_path(path))
                        .map(|file| file.text.clone())
                })
            })
            .or_else(|| {
                path.as_deref()
                    .and_then(|path| std::fs::read_to_string(path).ok())
            })
            .unwrap_or_default();
        let file = crate::source::SourceFile::new(0, path.clone(), text);
        let offset =
            file.offset_at_line_col_utf16(position.line as usize, position.character as usize);
        DocumentSnapshot { path, file, offset }
    }

    fn reanalyze_and_publish(&mut self) {
        self.analysis_due = None;
        let configs = analysis_configs(&self.root, &self.documents);
        debug_log(format!(
            "reanalyze root={} configs=[{}] open_documents={}",
            self.root.display(),
            analysis_config_summary(&configs),
            self.documents.len()
        ));
        if configs.is_empty() {
            self.workspaces.clear();
            self.clear_all_diagnostics();
            return;
        }
        let overlays = self
            .documents
            .iter()
            .filter_map(|(uri, text): (&Uri, &String)| {
                Some(AnalysisFile {
                    path: url_to_path(uri)?,
                    text: text.clone(),
                })
            })
            .collect::<Vec<_>>();
        let desired_keys = configs
            .iter()
            .map(analysis_config_key)
            .collect::<BTreeSet<_>>();
        let obsolete = self
            .workspaces
            .keys()
            .filter(|key| !desired_keys.contains(*key))
            .cloned()
            .collect::<Vec<_>>();
        for key in obsolete {
            self.workspaces.remove(&key);
        }

        for config in configs {
            let key = analysis_config_key(&config);
            let config_overlays = overlays_for_config(&config, &overlays);
            debug_log(format!(
                "reanalyze config={} key={} overlays=[{}]",
                analysis_config_label(&config),
                key,
                overlays_summary(&config_overlays)
            ));
            let result = if let Some(workspace) = self.workspaces.get_mut(&key) {
                workspace
                    .update_source_root(config.clone(), config_overlays)
                    .map(|_| ())
            } else {
                AnalysisWorkspace::load(config, config_overlays).map(|workspace| {
                    self.workspaces.insert(key.clone(), workspace);
                })
            };
            if let Err(err) = result {
                eprintln!("analysis failed for {key}: {err}");
            }
        }

        if self.workspaces.is_empty() {
            self.clear_all_diagnostics();
            return;
        }
        self.publish_all_diagnostics();
    }

    fn analysis_for_path(&self, path: &Path) -> Option<&ProjectAnalysis> {
        let selected = self
            .workspaces
            .values()
            .filter_map(|workspace| {
                let analysis = workspace.analysis();
                analysis
                    .file_by_path(path)
                    .is_some()
                    .then_some((analysis_path_score(analysis, path), analysis))
            })
            .max_by_key(|(score, _)| *score)
            .map(|(_, analysis)| analysis)
            .or_else(|| self.analysis());
        debug_log(format!(
            "analysis_for_path path={} selected={}",
            path.display(),
            selected
                .map(analysis_config_label_for_analysis)
                .unwrap_or_else(|| "<none>".into())
        ));
        selected
    }

    fn analysis(&self) -> Option<&ProjectAnalysis> {
        self.workspaces
            .values()
            .next()
            .map(AnalysisWorkspace::analysis)
    }

    fn publish_all_diagnostics(&mut self) {
        let analyses = self
            .workspaces
            .values()
            .map(|workspace| workspace.analysis().clone())
            .collect::<Vec<_>>();
        self.publish_diagnostics(&analyses);
    }

    fn publish_diagnostics(&mut self, analyses: &[ProjectAnalysis]) {
        let mut diagnostics_by_url = BTreeMap::<Uri, Vec<Diagnostic>>::new();
        for analysis in analyses {
            for file in &analysis.files {
                let Some(path) = file.path.as_ref() else {
                    continue;
                };
                let Some(uri) = path_to_url(path) else {
                    continue;
                };
                let document_text = self
                    .documents
                    .get(&uri)
                    .map(String::as_str)
                    .unwrap_or(file.text.as_str());
                let is_open = self.documents.contains_key(&uri);
                let raw_diagnostics = analysis.lsp_diagnostics_for_path(path);
                let raw_summary = diagnostic_summary(&raw_diagnostics);
                let suppress_parse_cascade = is_open
                    && raw_diagnostics.iter().any(|diagnostic| {
                        is_transient_import_parse_diagnostic(diagnostic, document_text)
                    });
                let diagnostics = raw_diagnostics
                    .into_iter()
                    .filter(|diagnostic| {
                        should_publish_diagnostic(
                            diagnostic,
                            document_text,
                            is_open,
                            suppress_parse_cascade,
                        )
                    })
                    .map(lsp_diagnostic)
                    .collect::<Vec<_>>();
                if is_open || !diagnostics.is_empty() {
                    debug_log(format!(
                        "publish config={} uri={uri:?} version={:?} is_open={is_open} raw=[{}] sent={} suppress_parse_cascade={suppress_parse_cascade} focus={}",
                        analysis_config_label(&analysis.config),
                        self.document_versions.get(&uri),
                        raw_summary,
                        diagnostics.len(),
                        focus_lines(document_text)
                    ));
                }
                diagnostics_by_url
                    .entry(uri)
                    .or_default()
                    .extend(diagnostics);
            }
        }
        for uri in self
            .published_diagnostics
            .difference(&diagnostics_by_url.keys().cloned().collect::<BTreeSet<_>>())
        {
            self.send_empty_diagnostics(uri.clone());
        }
        self.published_diagnostics = diagnostics_by_url.keys().cloned().collect();
        for (uri, diagnostics) in diagnostics_by_url {
            let version = self.document_versions.get(&uri).copied();
            let params = PublishDiagnosticsParams {
                uri,
                diagnostics,
                version,
            };
            let _ = self
                .connection
                .sender
                .send(Message::Notification(Notification {
                    method: PublishDiagnostics::METHOD.into(),
                    params: serde_json::to_value(params).unwrap_or_default(),
                }));
        }
    }

    fn clear_all_diagnostics(&mut self) {
        for uri in std::mem::take(&mut self.published_diagnostics) {
            self.send_empty_diagnostics(uri);
        }
    }

    fn send_empty_diagnostics(&self, uri: Uri) {
        let version = self.document_versions.get(&uri).copied();
        debug_log(format!("clear uri={uri:?} version={version:?}"));
        let params = PublishDiagnosticsParams {
            uri,
            diagnostics: Vec::new(),
            version,
        };
        let _ = self
            .connection
            .sender
            .send(Message::Notification(Notification {
                method: PublishDiagnostics::METHOD.into(),
                params: serde_json::to_value(params).unwrap_or_default(),
            }));
    }

    fn respond(&self, id: RequestId, result: serde_json::Value) -> Result<(), String> {
        self.connection
            .sender
            .send(Message::Response(Response {
                id,
                result: Some(result),
                error: None,
            }))
            .map_err(|err| format!("failed to send response: {err}"))
    }

    fn respond_error(&self, id: RequestId, code: i32, message: String) -> Result<(), String> {
        self.connection
            .sender
            .send(Message::Response(Response {
                id,
                result: None,
                error: Some(lsp_server::ResponseError {
                    code,
                    message,
                    data: None,
                }),
            }))
            .map_err(|err| format!("failed to send error response: {err}"))
    }

    fn show_message(&self, typ: MessageType, message: impl Into<String>) {
        let params = ShowMessageParams {
            typ,
            message: message.into(),
        };
        let _ = self
            .connection
            .sender
            .send(Message::Notification(Notification {
                method: ShowMessage::METHOD.into(),
                params: serde_json::to_value(params).unwrap_or_default(),
            }));
    }
}

#[derive(Debug, Clone, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct CommandDocumentPosition {
    uri: Uri,
    line: Option<u32>,
    character: Option<u32>,
}

impl CommandDocumentPosition {
    fn from_arguments(arguments: &[serde_json::Value]) -> Result<Option<Self>, String> {
        let Some(value) = arguments.first() else {
            return Ok(None);
        };
        serde_json::from_value(value.clone())
            .map(Some)
            .map_err(|err| format!("invalid command document position: {err}"))
    }
}

#[derive(Debug, Clone, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct InstallStdPackagesCommand {
    project_root: Option<PathBuf>,
    packages: Vec<String>,
}

impl InstallStdPackagesCommand {
    fn from_arguments(arguments: &[serde_json::Value]) -> Result<Self, String> {
        let Some(value) = arguments.first() else {
            return Ok(Self {
                project_root: None,
                packages: Vec::new(),
            });
        };
        serde_json::from_value(value.clone())
            .map_err(|err| format!("invalid install std packages command: {err}"))
    }
}

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct CommandResult {
    kind: String,
    title: String,
    markdown: String,
    items: Vec<CommandItem>,
}

impl CommandResult {
    fn message(message: impl Into<String>) -> Self {
        let message = message.into();
        Self {
            kind: "message".into(),
            title: "Lux".into(),
            markdown: message.clone(),
            items: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct CommandItem {
    label: String,
    detail: String,
    description: String,
    markdown: String,
}

fn module_exports_command(
    analysis: &ProjectAnalysis,
    position: Option<&CommandDocumentPosition>,
) -> CommandResult {
    let module = position
        .and_then(|position| url_to_path(&position.uri))
        .and_then(|path| analysis.module_for_path(&path))
        .or_else(|| analysis.modules.first());
    let Some(module) = module else {
        return CommandResult::message("No Lux module is available in this workspace.");
    };
    let mut items = module
        .exports
        .iter()
        .map(|export| CommandItem {
            label: export.name.clone(),
            detail: export.realms.display_name().into(),
            description: module.id.as_str().into(),
            markdown: format!(
                "`{}` exported from `{}` for **{}**.",
                export.name,
                module.id,
                export.realms.display_name()
            ),
        })
        .collect::<Vec<_>>();
    items.sort_by(|a, b| a.label.cmp(&b.label));
    let markdown = if items.is_empty() {
        format!("Module `{}` has no public exports.", module.id)
    } else {
        let mut lines = vec![format!("Module `{}` exports:", module.id), String::new()];
        for item in &items {
            lines.push(format!("- `{}` - {}", item.label, item.detail));
        }
        lines.join("\n")
    };
    CommandResult {
        kind: "moduleExports".into(),
        title: format!("Lux Exports: {}", module.id),
        markdown,
        items,
    }
}

fn active_realm_command(
    analysis: &ProjectAnalysis,
    position: Option<&CommandDocumentPosition>,
) -> CommandResult {
    let Some(position) = position else {
        return CommandResult::message("No active editor position was provided.");
    };
    let Some(path) = url_to_path(&position.uri) else {
        return CommandResult::message("The active editor is not a file URI.");
    };
    let line = position.line.unwrap_or(0) as usize;
    let character = position.character.unwrap_or(0) as usize;
    let Some(realms) = analysis.active_realms_at_position(&path, line, character) else {
        return CommandResult::message("No Lux realm information is available at this position.");
    };
    let module_id = analysis
        .module_for_path(&path)
        .map(|module| module.id.as_str().to_string())
        .unwrap_or_else(|| "<unknown module>".into());
    let markdown = format!(
        "Active Lux realm at `{}`:{}:{} is **{}**.",
        path.display(),
        line + 1,
        character + 1,
        realms.display_name()
    );
    CommandResult {
        kind: "activeRealm".into(),
        title: "Lux Active Realm".into(),
        markdown: markdown.clone(),
        items: vec![CommandItem {
            label: realms.display_name().into(),
            detail: module_id,
            description: path.display().to_string(),
            markdown,
        }],
    }
}

fn gmod_api_coverage_command(api: &ApiIndex) -> CommandResult {
    let database = api.database();
    let coverage = database.coverage.as_ref();
    let document_pages = coverage
        .map(|coverage| coverage.document_page_count)
        .unwrap_or_else(|| database.documents.len());
    let official_pages = coverage
        .map(|coverage| coverage.official_page_count)
        .unwrap_or(document_pages);
    let api_candidates = coverage
        .map(|coverage| coverage.api_candidate_count)
        .unwrap_or_default();
    let structured_pages = coverage
        .map(|coverage| coverage.structured_page_count)
        .unwrap_or_default();
    let fallback_pages = coverage
        .map(|coverage| coverage.fallback_page_count)
        .unwrap_or_default();
    let failed_pages = coverage
        .map(|coverage| coverage.failed_page_count)
        .unwrap_or_default();
    let markdown = format!(
        "# GMod API Database\n\n- Official pages: {}\n- Document records: {}\n- API candidate pages: {}\n- Structured API pages: {}\n- Fallback pages: {}\n- Failed pages: {}\n- Entries: {}\n- Hooks: {}\n- Classes: {}\n- Source: `{}`\n- Parser: `{}`",
        official_pages,
        document_pages,
        api_candidates,
        structured_pages,
        fallback_pages,
        failed_pages,
        database.entries.len(),
        database.hooks.len(),
        database.classes.len(),
        database.source_url,
        database.parser_version
    );
    CommandResult {
        kind: "gmodApiCoverage".into(),
        title: "Lux GMod API Coverage".into(),
        markdown,
        items: vec![
            CommandItem {
                label: "Official pages".into(),
                detail: official_pages.to_string(),
                description: "Facepunch pagelist baseline".into(),
                markdown: String::new(),
            },
            CommandItem {
                label: "Document records".into(),
                detail: document_pages.to_string(),
                description: "Generated documents[] records".into(),
                markdown: String::new(),
            },
            CommandItem {
                label: "Structured API pages".into(),
                detail: structured_pages.to_string(),
                description: "API pages parsed into entries/hooks/classes".into(),
                markdown: String::new(),
            },
        ],
    }
}

fn analysis_configs(root: &Path, documents: &HashMap<Uri, String>) -> Vec<AnalysisConfig> {
    let mut configs = Vec::new();
    let mut seen = BTreeSet::<String>::new();

    if let Some(config) = root_analysis_config(root) {
        insert_analysis_config(&mut configs, &mut seen, config);
    }

    for path in documents.keys().filter_map(url_to_path) {
        if let Some(config) = analysis_config_for_path(root, &path) {
            insert_analysis_config(&mut configs, &mut seen, config);
        }
    }

    if configs.is_empty() && !documents.is_empty() {
        insert_analysis_config(&mut configs, &mut seen, AnalysisConfig::standalone(root));
    }

    configs
}

fn root_analysis_config(root: &Path) -> Option<AnalysisConfig> {
    find_package_set_manifest(root)
        .and_then(|path| package_set_analysis_config(root, &path))
        .or_else(|| {
            find_manifest(root)
                .and_then(|path| ProjectManifest::load(path).ok())
                .map(AnalysisConfig::from_manifest)
        })
}

fn analysis_config_for_path(root: &Path, path: &Path) -> Option<AnalysisConfig> {
    let project_manifest = find_named_manifest_for_path(root, path, "lux.toml");
    let package_set_manifest = find_named_manifest_for_path(root, path, PACKAGE_SET_MANIFEST);
    match (project_manifest, package_set_manifest) {
        (Some(project), Some(package_set)) => {
            if manifest_is_deeper(&project, &package_set) {
                ProjectManifest::load(project)
                    .ok()
                    .map(AnalysisConfig::from_manifest)
            } else {
                package_set_analysis_config(root, &package_set)
            }
        }
        (Some(project), None) => ProjectManifest::load(project)
            .ok()
            .map(AnalysisConfig::from_manifest),
        (None, Some(package_set)) => package_set_analysis_config(root, &package_set),
        (None, None) => root_analysis_config(root),
    }
}

fn package_set_analysis_config(root: &Path, package_set_path: &Path) -> Option<AnalysisConfig> {
    let package_root = package_set_path.parent().unwrap_or(root);
    let source_roots = package_set_source_roots(package_root).unwrap_or_default();
    Some(AnalysisConfig::package_set(package_root, source_roots))
}

fn overlays_for_config(config: &AnalysisConfig, overlays: &[AnalysisFile]) -> Vec<AnalysisFile> {
    overlays
        .iter()
        .filter(|overlay| analysis_config_contains_path(config, &overlay.path))
        .cloned()
        .collect()
}

fn analysis_config_contains_path(config: &AnalysisConfig, path: &Path) -> bool {
    if config.is_package_set() {
        return package_set_config_contains_path(config, path);
    }
    if config.is_standalone() {
        return path.extension().is_some_and(|extension| extension == "lux")
            && path_is_under(path, &config.source_root);
    }
    path_is_under(path, &config.source_root)
}

fn package_set_config_contains_path(config: &AnalysisConfig, path: &Path) -> bool {
    config.package_roots.iter().any(|root| {
        discover_runtime_phases(root)
            .into_iter()
            .chain(discover_compile_time_phases(root))
            .flatten()
            .flat_map(|phase| phase.source_paths)
            .any(|source_path| same_path(&source_path, path))
    })
}

fn insert_analysis_config(
    configs: &mut Vec<AnalysisConfig>,
    seen: &mut BTreeSet<String>,
    config: AnalysisConfig,
) {
    let key = analysis_config_key(&config);
    if seen.insert(key) {
        configs.push(config);
    }
}

fn analysis_config_key(config: &AnalysisConfig) -> String {
    let mode = if config.is_package_set() {
        "package-set"
    } else if config.is_standalone() {
        "standalone"
    } else {
        "project"
    };
    let package_id = config
        .package_id
        .as_ref()
        .map(|id| id.as_str())
        .unwrap_or_default();
    let package_roots = config
        .package_roots
        .iter()
        .map(|path| normalized_path(path))
        .collect::<Vec<_>>()
        .join("|");
    let externs = config
        .resolver_options
        .externs
        .iter()
        .map(|symbol| {
            format!(
                "{}:{:?}:{}",
                symbol.path_string(),
                symbol.availability,
                symbol.span.is_some()
            )
        })
        .collect::<Vec<_>>()
        .join("|");
    let unknown_external = match config.resolver_options.unknown_external {
        crate::resolve::UnknownExternalPolicy::Allow => "allow",
        crate::resolve::UnknownExternalPolicy::Warn => "warn",
        crate::resolve::UnknownExternalPolicy::Error => "error",
    };
    let gmod_api = if config.resolver_options.gmod_api.is_some() {
        "api"
    } else {
        "no-api"
    };
    format!(
        "{mode}:{}:{package_id}:{package_roots}:{unknown_external}:{}:{gmod_api}:{externs}",
        normalized_path(&config.source_root),
        config.resolver_options.compile_time_package
    )
}

fn analysis_path_score(analysis: &ProjectAnalysis, path: &Path) -> (bool, usize) {
    (
        !analysis.config.is_package_set(),
        common_path_prefix_len(&analysis.config.source_root, path),
    )
}

fn common_path_prefix_len(left: &Path, right: &Path) -> usize {
    left.components()
        .zip(right.components())
        .take_while(|(left, right)| left == right)
        .count()
}

fn path_is_under(path: &Path, root: &Path) -> bool {
    let path = normalized_path(path);
    let root = normalized_path(root).trim_end_matches('/').to_string();
    path == root || path.starts_with(&(root + "/"))
}

fn analysis_config_summary(configs: &[AnalysisConfig]) -> String {
    configs
        .iter()
        .map(analysis_config_label)
        .collect::<Vec<_>>()
        .join("; ")
}

fn analysis_config_label_for_analysis(analysis: &ProjectAnalysis) -> String {
    analysis_config_label(&analysis.config)
}

fn analysis_config_label(config: &AnalysisConfig) -> String {
    let mode = if config.is_package_set() {
        "package-set"
    } else if config.is_standalone() {
        "standalone"
    } else {
        "project"
    };
    format!(
        "{}:{}:{}",
        mode,
        normalized_path(&config.source_root),
        config
            .package_id
            .as_ref()
            .map(|id| id.as_str())
            .unwrap_or("<none>")
    )
}

fn overlays_summary(overlays: &[AnalysisFile]) -> String {
    overlays
        .iter()
        .map(|overlay| normalized_path(&overlay.path))
        .collect::<Vec<_>>()
        .join("; ")
}

fn manifest_is_deeper(left: &Path, right: &Path) -> bool {
    left.components().count() >= right.components().count()
}

#[allow(deprecated)]
fn workspace_root(initialize: &InitializeParams) -> PathBuf {
    initialize
        .workspace_folders
        .as_ref()
        .and_then(|folders| folders.first())
        .and_then(|folder| url_to_path(&folder.uri))
        .or_else(|| initialize.root_uri.as_ref().and_then(url_to_path))
        .or_else(|| initialize.root_path.as_ref().map(PathBuf::from))
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")))
}

fn find_manifest(root: &Path) -> Option<PathBuf> {
    let mut current = root.to_path_buf();
    loop {
        let candidate = current.join("lux.toml");
        if candidate.exists() {
            return Some(candidate);
        }
        if !current.pop() {
            return None;
        }
    }
}

fn find_package_set_manifest(root: &Path) -> Option<PathBuf> {
    find_named_manifest(root, PACKAGE_SET_MANIFEST)
}

fn find_manifest_for_path(root: &Path, path: &Path) -> Option<PathBuf> {
    find_named_manifest_for_path(root, path, "lux.toml").or_else(|| find_manifest(root))
}

fn find_named_manifest(root: &Path, name: &str) -> Option<PathBuf> {
    let mut current = root.to_path_buf();
    loop {
        let candidate = current.join(name);
        if candidate.exists() {
            return Some(candidate);
        }
        if !current.pop() {
            return None;
        }
    }
}

fn find_named_manifest_for_path(root: &Path, path: &Path, name: &str) -> Option<PathBuf> {
    let mut current = path
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| path.to_path_buf());
    loop {
        let candidate = current.join(name);
        if candidate.exists() {
            return Some(candidate);
        }
        if current == root {
            break;
        }
        if !current.pop() {
            break;
        }
    }
    None
}

fn is_lux_analysis_watched_path(path: &Path) -> bool {
    path.file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| matches!(name, "lux.toml" | "lux.lock" | "lux.package.toml"))
}

fn manifest_extern_code_actions(
    analysis: &ProjectAnalysis,
    path: &Path,
    root: &Path,
) -> Vec<CodeActionOrCommand> {
    let Some(manifest_path) = find_manifest_for_path(root, path) else {
        return Vec::new();
    };
    analysis
        .diagnostics_for_path(path)
        .into_iter()
        .filter(|diagnostic| diagnostic.code.as_deref() == Some("REALM_UNKNOWN"))
        .filter_map(|diagnostic| diagnostic_symbol_name(&diagnostic.message))
        .flat_map(|symbol| {
            ["shared", "client", "server"]
                .into_iter()
                .map(move |realm| (symbol.clone(), realm))
        })
        .filter_map(|(symbol, realm)| {
            let uri = path_to_url(&manifest_path)?;
            let edit = manifest_extern_edit(&manifest_path, &symbol, realm);
            let mut changes = HashMap::<Uri, Vec<TextEdit>>::new();
            changes.insert(uri, vec![edit]);
            Some(CodeActionOrCommand::CodeAction(CodeAction {
                title: format!("Add package extern {realm} {symbol}"),
                kind: Some(CodeActionKind::QUICKFIX),
                diagnostics: None,
                edit: Some(lsp_types::WorkspaceEdit {
                    changes: Some(changes),
                    document_changes: None,
                    change_annotations: None,
                }),
                command: None,
                is_preferred: None,
                disabled: None,
                data: None,
            }))
        })
        .collect()
}

fn std_package_code_actions(
    analysis: &ProjectAnalysis,
    path: &Path,
    root: &Path,
    _uri: &Uri,
) -> Vec<CodeActionOrCommand> {
    let Some(manifest_path) = find_manifest_for_path(root, path) else {
        return Vec::new();
    };
    let Some(project_root) = manifest_path.parent() else {
        return Vec::new();
    };
    let packages = analysis
        .diagnostics_for_path(path)
        .into_iter()
        .filter(|diagnostic| diagnostic.code.as_deref() == Some("MODULE001"))
        .filter_map(|diagnostic| diagnostic_symbol_name(&diagnostic.message))
        .filter(|module| is_official_lux_package(module))
        .collect::<BTreeSet<_>>();
    if packages.is_empty() {
        return Vec::new();
    }
    let packages = packages.into_iter().collect::<Vec<_>>();
    vec![CodeActionOrCommand::CodeAction(CodeAction {
        title: "Fix: Install std packages".into(),
        kind: Some(CodeActionKind::QUICKFIX),
        diagnostics: None,
        edit: None,
        command: Some(lsp_types::Command {
            title: "Fix: Install std packages".into(),
            command: INSTALL_STD_PACKAGES_COMMAND.into(),
            arguments: Some(vec![serde_json::json!({
                "projectRoot": project_root,
                "packages": packages,
            })]),
        }),
        is_preferred: None,
        disabled: None,
        data: None,
    })]
}

fn is_official_lux_package(package: &str) -> bool {
    matches!(
        package,
        "@lux/std"
            | "@lux/reactive"
            | "@lux/gmod"
            | "@lux/gmod/macros"
            | "@lux/ui"
            | "@lux/macros"
            | "@lux/compile/macro"
            | "@lux/compile/host"
    )
}

fn manifest_extern_edit(manifest_path: &Path, symbol: &str, realm: &str) -> TextEdit {
    let text = std::fs::read_to_string(manifest_path).unwrap_or_default();
    let escaped_symbol = symbol.replace('\\', "\\\\").replace('"', "\\\"");
    let new_entry = format!("{escaped_symbol} = \"{realm}\"\n");
    if let Some((line, character)) = manifest_section_insert_position(&text, "target.gmod.extern") {
        TextEdit {
            range: Range {
                start: Position { line, character },
                end: Position { line, character },
            },
            new_text: new_entry,
        }
    } else {
        let prefix = if text.trim().is_empty() || text.ends_with('\n') {
            ""
        } else {
            "\n"
        };
        TextEdit {
            range: end_of_document_range(&text),
            new_text: format!("{prefix}\n[target.gmod.extern]\n{new_entry}"),
        }
    }
}

fn manifest_section_insert_position(text: &str, section: &str) -> Option<(u32, u32)> {
    let mut in_section = false;
    let mut insert_line = None;
    for (index, line) in text.lines().enumerate() {
        let trimmed = line.trim();
        if trimmed.starts_with('[') && trimmed.ends_with(']') {
            if in_section {
                return Some((index as u32, 0));
            }
            in_section = trimmed == format!("[{section}]");
        } else if in_section && !trimmed.is_empty() {
            insert_line = Some(index + 1);
        }
    }
    in_section.then_some((
        insert_line.unwrap_or_else(|| text.lines().count()) as u32,
        0,
    ))
}

fn end_of_document_range(text: &str) -> Range {
    let line_count = text.lines().count();
    let last_line_len = text.lines().last().map(utf16_len).unwrap_or(0);
    let line = if text.ends_with('\n') {
        line_count as u32
    } else {
        line_count.saturating_sub(1) as u32
    };
    let character = if text.ends_with('\n') {
        0
    } else {
        last_line_len as u32
    };
    Range {
        start: Position { line, character },
        end: Position { line, character },
    }
}

fn utf16_len(text: &str) -> usize {
    text.encode_utf16().count()
}

fn debug_log(message: impl AsRef<str>) {
    if std::env::var_os("LUXC_LSP_DEBUG").is_some() {
        eprintln!("[luxc-lsp-debug] {}", message.as_ref());
    }
}

fn document_change_summary(changes: &[TextDocumentContentChangeEvent]) -> String {
    changes
        .iter()
        .enumerate()
        .map(|(index, change)| match change.range {
            Some(range) => format!(
                "#{index}:range {}:{}-{}:{} len={} text={:?}",
                range.start.line,
                range.start.character,
                range.end.line,
                range.end.character,
                change.text.len(),
                preview_text(&change.text)
            ),
            None => format!(
                "#{index}:full len={} text={:?}",
                change.text.len(),
                preview_text(&change.text)
            ),
        })
        .collect::<Vec<_>>()
        .join("; ")
}

fn diagnostic_summary(diagnostics: &[AnalysisDiagnostic]) -> String {
    diagnostics
        .iter()
        .map(|diagnostic| {
            format!(
                "{}@{}:{}:{}",
                diagnostic.code.as_deref().unwrap_or("<none>"),
                diagnostic.range.start.line + 1,
                diagnostic.range.start.character + 1,
                diagnostic.message
            )
        })
        .collect::<Vec<_>>()
        .join("; ")
}

fn focus_lines(text: &str) -> String {
    text.lines()
        .take(12)
        .enumerate()
        .map(|(index, line)| format!("{}:{}", index + 1, preview_text(line)))
        .collect::<Vec<_>>()
        .join(" | ")
}

fn preview_text(text: &str) -> String {
    let mut value = text
        .replace('\r', "\\r")
        .replace('\n', "\\n")
        .replace('\t', "\\t");
    if value.len() > 120 {
        value.truncate(120);
        value.push_str("...");
    }
    value
}

fn apply_document_changes(
    mut text: String,
    changes: Vec<TextDocumentContentChangeEvent>,
) -> String {
    for change in changes {
        if let Some(range) = change.range {
            let file = SourceFile::new(0, None, text.clone());
            let start = file.offset_at_line_col_utf16(
                range.start.line as usize,
                range.start.character as usize,
            );
            let end = file
                .offset_at_line_col_utf16(range.end.line as usize, range.end.character as usize);
            if start <= end && end <= text.len() {
                text.replace_range(start..end, &change.text);
            } else {
                text = change.text;
            }
        } else {
            text = change.text;
        }
    }
    text
}

fn lsp_diagnostic(diagnostic: AnalysisDiagnostic) -> Diagnostic {
    Diagnostic {
        range: lsp_range(diagnostic.range),
        severity: Some(match diagnostic.severity {
            Severity::Error => DiagnosticSeverity::ERROR,
            Severity::Warning => DiagnosticSeverity::WARNING,
            Severity::Note => DiagnosticSeverity::INFORMATION,
        }),
        code: diagnostic.code.map(lsp_types::NumberOrString::String),
        code_description: None,
        source: Some("luxc".into()),
        message: diagnostic.message,
        related_information: if diagnostic.notes.is_empty() && diagnostic.help.is_none() {
            None
        } else {
            Some(
                diagnostic
                    .notes
                    .into_iter()
                    .chain(diagnostic.help)
                    .map(|message| DiagnosticRelatedInformation {
                        location: Location {
                            uri: path_to_url(&diagnostic.path)
                                .unwrap_or_else(|| uri_from_url(Url::parse("file:///").unwrap())),
                            range: lsp_range(diagnostic.range),
                        },
                        message,
                    })
                    .collect(),
            )
        },
        tags: None,
        data: None,
    }
}

fn should_publish_diagnostic(
    diagnostic: &AnalysisDiagnostic,
    document_text: &str,
    is_open: bool,
    suppress_parse_cascade: bool,
) -> bool {
    if !is_open || diagnostic.severity != Severity::Error {
        return true;
    }
    let Some(code) = diagnostic.code.as_deref() else {
        return true;
    };
    if !code.starts_with("PARSE") {
        return true;
    }
    if suppress_parse_cascade {
        return false;
    }
    !is_transient_parse_diagnostic(diagnostic, document_text)
}

fn is_transient_parse_diagnostic(diagnostic: &AnalysisDiagnostic, document_text: &str) -> bool {
    if is_transient_import_parse_diagnostic(diagnostic, document_text) {
        return true;
    }
    if is_position_at_document_end(document_text, diagnostic.range.start) {
        return true;
    }
    if diagnostic.code.as_deref() != Some("PARSE005") {
        return false;
    }
    let start = diagnostic.range.start;
    let Some(line) = line_at(document_text, start.line) else {
        return false;
    };
    let prefix = line_prefix_utf16(line, start.character).trim_end();
    prefix.is_empty()
        || prefix.ends_with('{')
        || prefix.ends_with('(')
        || prefix.ends_with('[')
        || prefix.ends_with(',')
        || prefix.ends_with('.')
        || prefix.ends_with(':')
        || prefix.ends_with(" import")
        || prefix.ends_with(" export")
        || prefix.ends_with(" from")
        || prefix.ends_with(" as")
}

fn is_transient_import_parse_diagnostic(
    diagnostic: &AnalysisDiagnostic,
    document_text: &str,
) -> bool {
    let Some(code) = diagnostic.code.as_deref() else {
        return false;
    };
    if !matches!(code, "PARSE001" | "PARSE005" | "PARSE006" | "PARSE007") {
        return false;
    }
    let start = diagnostic.range.start;
    let Some(line) = line_at(document_text, start.line) else {
        return false;
    };
    let trimmed = line.trim_start();
    if trimmed.starts_with("import ") || trimmed == "import" {
        return true;
    }

    let mut previous_line = start.line;
    while previous_line > 0 {
        previous_line -= 1;
        let Some(previous) = line_at(document_text, previous_line) else {
            break;
        };
        let previous = previous.trim();
        if previous.is_empty() {
            continue;
        }
        return previous.starts_with("import ")
            && !previous.contains('\n')
            && (previous.contains('{') || previous.contains(" from "));
    }
    false
}

fn is_position_at_document_end(
    document_text: &str,
    position: crate::analysis::AnalysisPosition,
) -> bool {
    let end = end_position_utf16(document_text);
    position.line > end.line || (position.line == end.line && position.character >= end.character)
}

fn end_position_utf16(text: &str) -> crate::analysis::AnalysisPosition {
    let range = end_of_document_range(text);
    crate::analysis::AnalysisPosition {
        line: range.start.line,
        character: range.start.character,
    }
}

fn line_at(text: &str, line: u32) -> Option<&str> {
    text.split('\n')
        .nth(line as usize)
        .map(|line| line.strip_suffix('\r').unwrap_or(line))
}

fn line_prefix_utf16(line: &str, character: u32) -> &str {
    if character == 0 {
        return "";
    }
    let mut utf16 = 0u32;
    for (index, ch) in line.char_indices() {
        if utf16 >= character {
            return &line[..index];
        }
        utf16 += ch.len_utf16() as u32;
    }
    line
}

fn code_action(action: AnalysisCodeAction, uri: &Uri) -> CodeActionOrCommand {
    let diagnostics = action
        .diagnostics
        .iter()
        .map(|code| Diagnostic {
            range: Range::default(),
            severity: None,
            code: Some(lsp_types::NumberOrString::String(code.clone())),
            code_description: None,
            source: Some("luxc".into()),
            message: code.clone(),
            related_information: None,
            tags: None,
            data: None,
        })
        .collect::<Vec<_>>();
    let mut changes = HashMap::<Uri, Vec<TextEdit>>::new();
    for edit in action.edits {
        let edit_uri = path_to_url(&edit.path).unwrap_or_else(|| uri.clone());
        changes.entry(edit_uri).or_default().push(TextEdit {
            range: lsp_range(edit.range),
            new_text: edit.new_text,
        });
    }
    CodeActionOrCommand::CodeAction(CodeAction {
        title: action.title,
        kind: Some(match action.kind {
            AnalysisEditKind::Safe => CodeActionKind::QUICKFIX,
            AnalysisEditKind::Guided => CodeActionKind::QUICKFIX,
            AnalysisEditKind::Refactor => CodeActionKind::REFACTOR,
        }),
        diagnostics: Some(diagnostics),
        edit: (!changes.is_empty()).then_some(lsp_types::WorkspaceEdit {
            changes: Some(changes),
            document_changes: None,
            change_annotations: None,
        }),
        command: action.command.map(|command| lsp_types::Command {
            title: command.clone(),
            command,
            arguments: None,
        }),
        is_preferred: None,
        disabled: None,
        data: None,
    })
}

fn api_doc_code_actions(
    analysis: &ProjectAnalysis,
    api: &ApiIndex,
    path: &Path,
    _uri: &Uri,
) -> Vec<CodeActionOrCommand> {
    analysis
        .diagnostics_for_path(path)
        .into_iter()
        .filter(|diagnostic| diagnostic.code.as_deref() == Some("REALM001"))
        .filter_map(|diagnostic| diagnostic_symbol_name(&diagnostic.message))
        .filter_map(|symbol| {
            api.entry(&symbol)
                .and_then(|entry| entry.official_url.as_ref())
        })
        .map(|url| {
            CodeActionOrCommand::CodeAction(CodeAction {
                title: "Open official GMod documentation".into(),
                kind: Some(CodeActionKind::QUICKFIX),
                diagnostics: None,
                edit: None,
                command: Some(lsp_types::Command {
                    title: "Open official GMod documentation".into(),
                    command: "lux.openGmodDocs".into(),
                    arguments: Some(vec![serde_json::Value::String(url.clone())]),
                }),
                is_preferred: None,
                disabled: None,
                data: None,
            })
        })
        .collect()
}

fn diagnostic_symbol_name(message: &str) -> Option<String> {
    let start = message.find('`')? + 1;
    let end = message[start..].find('`')? + start;
    Some(message[start..end].to_string())
}

fn completion_item(candidate: CompletionCandidate) -> CompletionItem {
    completion_item_with_source(candidate, None)
}

fn completion_item_with_source(
    candidate: CompletionCandidate,
    override_source: Option<String>,
) -> CompletionItem {
    let sort_text = completion_candidate_sort_text(&candidate);
    let source = override_source.or(candidate.source);
    let mut item = CompletionItem {
        label: candidate.label,
        kind: Some(completion_item_kind(candidate.kind)),
        detail: candidate.detail,
        documentation: candidate.documentation.map(markdown_documentation),
        sort_text: Some(sort_text),
        ..CompletionItem::default()
    };
    if let Some(source) = source {
        item.label_details = Some(CompletionItemLabelDetails {
            detail: None,
            description: Some(source),
        });
    }
    item
}

fn completion_candidate_sort_text(candidate: &CompletionCandidate) -> String {
    let group = match candidate.kind {
        CompletionCandidateKind::Parameter => "00",
        CompletionCandidateKind::Variable | CompletionCandidateKind::Constant => "01",
        CompletionCandidateKind::Reference => "02",
        CompletionCandidateKind::Function | CompletionCandidateKind::Method => "03",
        CompletionCandidateKind::Module => "04",
        CompletionCandidateKind::Field | CompletionCandidateKind::Property => "05",
        CompletionCandidateKind::Class
        | CompletionCandidateKind::Enum
        | CompletionCandidateKind::Event
        | CompletionCandidateKind::Struct
        | CompletionCandidateKind::Value => "06",
    };
    format!("{group}:{}", candidate.label.to_ascii_lowercase())
}

fn import_completion_item(candidate: CompletionCandidate, needs_source: bool) -> CompletionItem {
    let source = candidate.source.clone();
    let mut item = completion_item_with_source(candidate, source.clone());
    if needs_source && let Some(source) = source {
        item.detail = Some(match item.detail {
            Some(detail) => format!("{detail} | import from `{source}`"),
            None => format!("import from `{source}`"),
        });
        item.insert_text = Some(format!("{} }} from \"{}\"", item.label, source));
        item.insert_text_format = Some(InsertTextFormat::PLAIN_TEXT);
    }
    item
}

struct KeywordCompletion {
    label: &'static str,
    insert_text: &'static str,
    detail: &'static str,
}

const KEYWORD_COMPLETIONS: &[KeywordCompletion] = &[
    KeywordCompletion {
        label: "import",
        insert_text: "import { ",
        detail: "Import named exports from another Lux module.",
    },
    KeywordCompletion {
        label: "export",
        insert_text: "export ",
        detail: "Expose a module binding as public API.",
    },
    KeywordCompletion {
        label: "extern",
        insert_text: "extern ",
        detail: "Declare the realm of an external GMod or third-party symbol.",
    },
    KeywordCompletion {
        label: "fn",
        insert_text: "fn ",
        detail: "Declare a Lux function.",
    },
    KeywordCompletion {
        label: "local",
        insert_text: "local ",
        detail: "Declare a local binding.",
    },
    KeywordCompletion {
        label: "const",
        insert_text: "const ",
        detail: "Declare an immutable binding.",
    },
    KeywordCompletion {
        label: "match",
        insert_text: "match ",
        detail: "Match a value against patterns.",
    },
    KeywordCompletion {
        label: "if",
        insert_text: "if ",
        detail: "Start a conditional expression or statement.",
    },
    KeywordCompletion {
        label: "then",
        insert_text: "then ",
        detail: "Separate a Lux condition from its true branch.",
    },
    KeywordCompletion {
        label: "else",
        insert_text: "else ",
        detail: "Start the fallback branch of a conditional.",
    },
    KeywordCompletion {
        label: "elseif",
        insert_text: "elseif ",
        detail: "Start another branch in a conditional block.",
    },
    KeywordCompletion {
        label: "while",
        insert_text: "while ",
        detail: "Start a while loop.",
    },
    KeywordCompletion {
        label: "for",
        insert_text: "for ",
        detail: "Start a for loop.",
    },
    KeywordCompletion {
        label: "in",
        insert_text: "in ",
        detail: "Introduce the iterator expression in a for loop.",
    },
    KeywordCompletion {
        label: "return",
        insert_text: "return ",
        detail: "Return from the current function.",
    },
    KeywordCompletion {
        label: "break",
        insert_text: "break",
        detail: "Exit the nearest loop.",
    },
    KeywordCompletion {
        label: "continue",
        insert_text: "continue",
        detail: "Continue the nearest loop.",
    },
    KeywordCompletion {
        label: "stopif",
        insert_text: "stopif ",
        detail: "Return early when the condition is true.",
    },
    KeywordCompletion {
        label: "stopifn",
        insert_text: "stopifn ",
        detail: "Return early when the condition is false.",
    },
    KeywordCompletion {
        label: "breakif",
        insert_text: "breakif ",
        detail: "Break when the condition is true.",
    },
    KeywordCompletion {
        label: "breakifn",
        insert_text: "breakifn ",
        detail: "Break when the condition is false.",
    },
    KeywordCompletion {
        label: "continueif",
        insert_text: "continueif ",
        detail: "Continue when the condition is true.",
    },
    KeywordCompletion {
        label: "continueifn",
        insert_text: "continueifn ",
        detail: "Continue when the condition is false.",
    },
    KeywordCompletion {
        label: "client",
        insert_text: "client ",
        detail: "Mark a declaration or block as client-only.",
    },
    KeywordCompletion {
        label: "server",
        insert_text: "server ",
        detail: "Mark a declaration or block as server-only.",
    },
    KeywordCompletion {
        label: "shared",
        insert_text: "shared ",
        detail: "Mark a declaration or block as shared.",
    },
    KeywordCompletion {
        label: "enum",
        insert_text: "enum ",
        detail: "Declare an explicit Lux enum.",
    },
    KeywordCompletion {
        label: "repr",
        insert_text: "repr ",
        detail: "Choose the enum representation.",
    },
    KeywordCompletion {
        label: "nil",
        insert_text: "nil",
        detail: "The nil value.",
    },
    KeywordCompletion {
        label: "true",
        insert_text: "true",
        detail: "Boolean true.",
    },
    KeywordCompletion {
        label: "false",
        insert_text: "false",
        detail: "Boolean false.",
    },
    KeywordCompletion {
        label: "and",
        insert_text: "and ",
        detail: "Logical and.",
    },
    KeywordCompletion {
        label: "or",
        insert_text: "or ",
        detail: "Logical or.",
    },
    KeywordCompletion {
        label: "not",
        insert_text: "not ",
        detail: "Logical not.",
    },
];

fn keyword_completion_items(prefix: &str) -> Vec<CompletionItem> {
    let prefix = prefix.to_ascii_lowercase();
    KEYWORD_COMPLETIONS
        .iter()
        .filter(|keyword| prefix.is_empty() || keyword.label.starts_with(&prefix))
        .map(|keyword| CompletionItem {
            label: keyword.label.into(),
            kind: Some(CompletionItemKind::KEYWORD),
            detail: Some(keyword.detail.into()),
            documentation: Some(markdown_documentation(format!(
                "`{}` is a Lux keyword.",
                keyword.label
            ))),
            sort_text: Some(format!("20:{}", keyword.label)),
            filter_text: Some(keyword.label.into()),
            insert_text: Some(keyword.insert_text.into()),
            insert_text_format: Some(InsertTextFormat::PLAIN_TEXT),
            ..CompletionItem::default()
        })
        .collect()
}

fn markdown_hover(markdown: String) -> Hover {
    Hover {
        contents: HoverContents::Markup(MarkupContent {
            kind: MarkupKind::Markdown,
            value: markdown,
        }),
        range: None,
    }
}

fn markdown_documentation(value: String) -> Documentation {
    Documentation::MarkupContent(MarkupContent {
        kind: MarkupKind::Markdown,
        value,
    })
}

fn completion_item_kind(kind: CompletionCandidateKind) -> CompletionItemKind {
    match kind {
        CompletionCandidateKind::Module => CompletionItemKind::MODULE,
        CompletionCandidateKind::Function => CompletionItemKind::FUNCTION,
        CompletionCandidateKind::Method => CompletionItemKind::METHOD,
        CompletionCandidateKind::Variable => CompletionItemKind::VARIABLE,
        CompletionCandidateKind::Parameter => CompletionItemKind::VARIABLE,
        CompletionCandidateKind::Constant => CompletionItemKind::CONSTANT,
        CompletionCandidateKind::Field => CompletionItemKind::FIELD,
        CompletionCandidateKind::Class => CompletionItemKind::CLASS,
        CompletionCandidateKind::Enum => CompletionItemKind::ENUM,
        CompletionCandidateKind::Event => CompletionItemKind::EVENT,
        CompletionCandidateKind::Reference => CompletionItemKind::REFERENCE,
        CompletionCandidateKind::Struct => CompletionItemKind::STRUCT,
        CompletionCandidateKind::Property => CompletionItemKind::PROPERTY,
        CompletionCandidateKind::Value => CompletionItemKind::VALUE,
    }
}

fn encode_semantic_tokens(
    file: &crate::source::SourceFile,
    mut tokens: Vec<AnalysisSemanticToken>,
) -> Vec<SemanticToken> {
    tokens.sort_by_key(|token| {
        let token_range = range(file, token.span);
        (
            token_range.start.line,
            token_range.start.character,
            token_range.end.character,
            semantic_token_priority(&token.kind),
        )
    });
    let mut encoded = Vec::new();
    let mut last_line = 0u32;
    let mut last_start = 0u32;
    let mut last_end_by_line = std::collections::BTreeMap::<u32, u32>::new();
    for token in tokens {
        let token_range = range(file, token.span);
        if token_range.start.line != token_range.end.line {
            continue;
        }
        let line = token_range.start.line;
        let start = token_range.start.character;
        let end = token_range.end.character;
        let length = end.saturating_sub(start);
        if length == 0 {
            continue;
        }
        if last_end_by_line
            .get(&line)
            .is_some_and(|last_end| start < *last_end)
        {
            continue;
        }
        let delta_line = line.saturating_sub(last_line);
        let delta_start = if delta_line == 0 {
            start.saturating_sub(last_start)
        } else {
            start
        };
        encoded.push(SemanticToken {
            delta_line,
            delta_start,
            length,
            token_type: semantic_token_type(token.kind),
            token_modifiers_bitset: 0,
        });
        last_line = line;
        last_start = start;
        last_end_by_line.insert(line, end);
    }
    encoded
}

fn semantic_token_priority(kind: &SemanticTokenKind) -> u8 {
    match kind {
        SemanticTokenKind::Keyword
        | SemanticTokenKind::Realm
        | SemanticTokenKind::String
        | SemanticTokenKind::Number
        | SemanticTokenKind::Comment
        | SemanticTokenKind::Operator => 0,
        SemanticTokenKind::Function
        | SemanticTokenKind::Parameter
        | SemanticTokenKind::Variable
        | SemanticTokenKind::Property
        | SemanticTokenKind::Namespace
        | SemanticTokenKind::Type
        | SemanticTokenKind::Export
        | SemanticTokenKind::Import => 1,
        SemanticTokenKind::External | SemanticTokenKind::UnknownExternal => 2,
    }
}

fn semantic_token_type(kind: SemanticTokenKind) -> u32 {
    match kind {
        SemanticTokenKind::Keyword => 0,
        SemanticTokenKind::Realm => 1,
        SemanticTokenKind::Function => 2,
        SemanticTokenKind::Parameter => 3,
        SemanticTokenKind::Variable => 4,
        SemanticTokenKind::Property => 5,
        SemanticTokenKind::Namespace => 6,
        SemanticTokenKind::Type => 7,
        SemanticTokenKind::String => 8,
        SemanticTokenKind::Number => 9,
        SemanticTokenKind::Comment => 10,
        SemanticTokenKind::Operator => 11,
        SemanticTokenKind::Export => 12,
        SemanticTokenKind::Import => 13,
        SemanticTokenKind::External => 14,
        SemanticTokenKind::UnknownExternal => 15,
    }
}

fn range(file: &crate::source::SourceFile, span: crate::source::SourceSpan) -> Range {
    let analysis_range = AnalysisRange {
        start: {
            let (line, col) = file.line_col_utf16(span.byte_start);
            crate::analysis::AnalysisPosition {
                line: line.saturating_sub(1) as u32,
                character: col.saturating_sub(1) as u32,
            }
        },
        end: {
            let (line, col) = file.line_col_utf16(span.byte_end);
            crate::analysis::AnalysisPosition {
                line: line.saturating_sub(1) as u32,
                character: col.saturating_sub(1) as u32,
            }
        },
    };
    lsp_range(analysis_range)
}

fn lsp_range(range: AnalysisRange) -> Range {
    Range {
        start: Position {
            line: range.start.line,
            character: range.start.character,
        },
        end: Position {
            line: range.end.line,
            character: range.end.character,
        },
    }
}

fn external_api_hover_markdown(
    analysis: &ProjectAnalysis,
    api: &ApiIndex,
    path: &Path,
    offset: usize,
) -> Option<String> {
    let file = analysis.file_by_path(path)?;
    let symbol = analysis.symbol_at_path_offset(path, offset)?;
    let external = symbol.external_availability.as_ref()?;
    if matches!(external, crate::module::RealmAvailability::UnknownExternal) {
        return None;
    }
    api.entry(&symbol.name)
        .or_else(|| api_hover_entry_from_text(api, &file.text, offset))
        .map(entry_markdown)
}

fn hook_hover_markdown_from_text(api: &ApiIndex, text: &str, offset: usize) -> Option<String> {
    let hook_name = hook_name_at_offset(text, offset)?;
    api.hook(&hook_name).map(hook_markdown)
}

fn api_hover_markdown_from_text(api: &ApiIndex, text: &str, offset: usize) -> Option<String> {
    api_hover_entry_from_text(api, text, offset).map(entry_markdown)
}

fn api_hover_entry_from_text<'a>(
    api: &'a ApiIndex,
    text: &str,
    offset: usize,
) -> Option<&'a gmod_api_db::ApiEntry> {
    let facts = GmodTypeFacts::from_text(text);
    if let Some(method_path) = method_path_at_offset(text, offset) {
        if let Some(resolved_path) = resolve_typed_method_path(api, &facts, &method_path)
            && let Some(entry) = api.entry(&resolved_path)
        {
            return Some(entry);
        }
        if let Some(entry) = api.entry(&method_path) {
            return Some(entry);
        }
    }
    let path = api_path_at_offset(text, offset)?;
    if path.contains(':')
        && let Some(resolved_path) = resolve_typed_method_path(api, &facts, &path)
        && let Some(entry) = api.entry(&resolved_path)
    {
        return Some(entry);
    }
    api.entry(&path).or_else(|| api.longest_match_text(&path))
}

fn hook_name_at_offset(text: &str, offset: usize) -> Option<String> {
    let clamped = offset.min(text.len());
    let before = &text[..clamped];
    let after = &text[clamped..];
    let quote_start = before.rfind(['"', '\''])?;
    let quote = before[quote_start..].chars().next()?;
    let hook_prefix = before[..quote_start].trim_end();
    if !hook_prefix.ends_with("hook.Add(") {
        return None;
    }
    let quote_end = after.find(quote).unwrap_or(after.len());
    Some(format!(
        "{}{}",
        &before[quote_start + quote.len_utf8()..],
        &after[..quote_end]
    ))
}

fn signature_help_at(
    file: &crate::source::SourceFile,
    api: &ApiIndex,
    offset: usize,
) -> Option<SignatureHelp> {
    let text = &file.text[..offset.min(file.text.len())];
    if let Some(hook_name) = hook_name_in_call_prefix(text)
        && let Some(hook) = api.hook(&hook_name)
    {
        return Some(signature_help_from_hook(hook));
    }
    let call_path = call_path_before_cursor(text)?;
    let facts = GmodTypeFacts::from_text(&file.text);
    let resolved_call_path =
        resolve_typed_method_path(api, &facts, &call_path).unwrap_or(call_path);
    let entry = api.entry(&resolved_call_path)?;
    signature_help_from_entry(entry)
}

fn hook_name_in_call_prefix(text: &str) -> Option<String> {
    let hook_index = text.rfind("hook.Add(")?;
    let after = &text[hook_index + "hook.Add(".len()..];
    let quote = after.chars().find(|ch| *ch == '"' || *ch == '\'')?;
    let start = after.find(quote)? + quote.len_utf8();
    let rest = &after[start..];
    let end = rest.find(quote)?;
    Some(rest[..end].to_string())
}

fn call_path_before_cursor(text: &str) -> Option<String> {
    let open = text.rfind('(')?;
    let before = text[..open].trim_end();
    let token = before
        .rsplit(|ch: char| !(ch.is_ascii_alphanumeric() || matches!(ch, '_' | '.' | ':')))
        .next()
        .unwrap_or_default();
    (!token.is_empty()).then(|| token.to_string())
}

fn method_path_at_offset(text: &str, offset: usize) -> Option<String> {
    let offset = offset.min(text.len());
    let before = &text[..offset];
    let after = &text[offset..];
    let left = before
        .rsplit(|ch: char| !(ch.is_ascii_alphanumeric() || matches!(ch, '_' | ':')))
        .next()
        .unwrap_or_default();
    let right = after
        .split(|ch: char| !(ch.is_ascii_alphanumeric() || ch == '_'))
        .next()
        .unwrap_or_default();
    let path = format!("{left}{right}");
    path.contains(':').then_some(path)
}

fn api_path_at_offset(text: &str, offset: usize) -> Option<String> {
    let offset = offset.min(text.len());
    let before = &text[..offset];
    let after = &text[offset..];
    let left = before
        .rsplit(|ch: char| !is_api_path_char(ch))
        .next()
        .unwrap_or_default();
    let right = after
        .split(|ch: char| !is_api_path_char(ch))
        .next()
        .unwrap_or_default();
    let path = format!("{left}{right}");
    let path = path.trim_matches(['.', ':']);
    (!path.is_empty()).then(|| path.to_string())
}

fn is_api_path_char(ch: char) -> bool {
    ch.is_ascii_alphanumeric() || matches!(ch, '_' | '.' | ':')
}

fn resolve_typed_method_path(api: &ApiIndex, facts: &GmodTypeFacts, path: &str) -> Option<String> {
    let (receiver, method) = path.split_once(':')?;
    if receiver.is_empty() || method.is_empty() {
        return None;
    }
    if let Some(class_name) = facts.receiver_class(receiver)
        && let Some(entry) = api.method_for_class_or_base(&class_name, method)
    {
        return Some(entry.path.clone());
    }
    api.method_for_class_or_base(receiver, method)
        .map(|entry| entry.path.clone())
}

fn signature_help_from_entry(entry: &gmod_api_db::ApiEntry) -> Option<SignatureHelp> {
    if entry.signatures.is_empty() {
        return None;
    }
    let documentation = Some(markdown_documentation(entry_markdown(entry)));
    Some(SignatureHelp {
        signatures: entry
            .signatures
            .iter()
            .map(|signature| signature_information(signature, documentation.clone()))
            .collect(),
        active_signature: Some(0),
        active_parameter: Some(0),
    })
}

fn signature_help_from_hook(hook: &gmod_api_db::HookEntry) -> SignatureHelp {
    SignatureHelp {
        signatures: vec![signature_information(
            &hook.callback,
            Some(markdown_documentation(hook_markdown(hook))),
        )],
        active_signature: Some(0),
        active_parameter: Some(0),
    }
}

fn signature_help_from_analysis(help: AnalysisSignatureHelp) -> SignatureHelp {
    let signature = help.signature;
    let active_parameter = if signature.parameters.is_empty() {
        None
    } else {
        Some(help.active_parameter.min(signature.parameters.len() - 1) as u32)
    };
    SignatureHelp {
        signatures: vec![SignatureInformation {
            label: signature.label,
            documentation: Some(markdown_documentation(format!(
                "Defined in `{}`",
                signature.module_id
            ))),
            parameters: Some(
                signature
                    .parameters
                    .into_iter()
                    .map(|parameter| ParameterInformation {
                        label: ParameterLabel::Simple(parameter.name),
                        documentation: parameter
                            .optional
                            .then(|| Documentation::String("optional".into())),
                    })
                    .collect(),
            ),
            active_parameter: None,
        }],
        active_signature: Some(0),
        active_parameter,
    }
}

fn signature_information(
    signature: &gmod_api_db::ApiSignature,
    documentation: Option<Documentation>,
) -> SignatureInformation {
    SignatureInformation {
        label: signature.label.clone(),
        documentation,
        parameters: Some(
            signature
                .parameters
                .iter()
                .map(|parameter| ParameterInformation {
                    label: ParameterLabel::Simple(parameter.name.clone()),
                    documentation: Some(lsp_types::Documentation::String(format!(
                        "{} - {}",
                        parameter.ty, parameter.description
                    ))),
                })
                .collect(),
        ),
        active_parameter: None,
    }
}

fn json_result<T: serde::Serialize>(value: T) -> Result<serde_json::Value, String> {
    serde_json::to_value(value).map_err(|err| format!("failed to encode LSP result: {err}"))
}

fn url_to_path(uri: &Uri) -> Option<PathBuf> {
    let parsed = Url::parse(uri.as_str()).ok()?;
    parsed.to_file_path().ok()
}

fn document_uri_key(uri: &Uri) -> Uri {
    url_to_path(uri)
        .as_deref()
        .and_then(path_to_url)
        .unwrap_or_else(|| uri.clone())
}

fn path_to_url(path: &Path) -> Option<Uri> {
    Url::from_file_path(path).ok().map(uri_from_url)
}

fn uri_from_url(url: Url) -> Uri {
    url.as_str()
        .parse()
        .expect("file URL should be a valid URI")
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum CompletionContext {
    ImportSource,
    ImportSpecifierList { source: Option<String> },
    ExportList,
    ApiMember { prefix: String },
    General,
}

fn completion_context(prefix: &str, suffix: &str) -> CompletionContext {
    let line = format!("{prefix}{suffix}");
    let cursor = prefix.len();
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

fn api_member_prefix(prefix: &str) -> Option<String> {
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

fn identifier_prefix(prefix: &str) -> &str {
    prefix
        .rsplit(|ch: char| !(ch.is_ascii_alphanumeric() || ch == '_'))
        .next()
        .unwrap_or_default()
}

fn general_binding_completions(
    analysis: &ProjectAnalysis,
    path: &Path,
    offset: usize,
    current_file: &SourceFile,
) -> Vec<CompletionCandidate> {
    let mut candidates = analysis
        .visible_bindings_at_path_offset(path, offset)
        .into_iter()
        .map(|candidate| (candidate.label.clone(), candidate))
        .collect::<BTreeMap<_, _>>();
    for candidate in module_part_lexical_completions(analysis, path, current_file, offset) {
        candidates
            .entry(candidate.label.clone())
            .or_insert(candidate);
    }
    candidates.into_values().collect()
}

fn lexical_binding_completions(file: &SourceFile, offset: usize) -> Vec<CompletionCandidate> {
    let tokens = lex_completion_tokens(file);
    let mut collector = LexicalCompletionCollector::new(file, &tokens, offset);
    collector.collect_current_part();
    collector.into_candidates()
}

fn module_part_lexical_completions(
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

fn api_root_completion_candidates(api: &ApiIndex, typed_prefix: &str) -> Vec<CompletionItem> {
    let typed_prefix = typed_prefix.to_ascii_lowercase();
    api.roots()
        .into_iter()
        .filter(|entry| {
            typed_prefix.is_empty()
                || entry.path.to_ascii_lowercase().starts_with(&typed_prefix)
                || entry
                    .path
                    .rsplit(['.', ':'])
                    .next()
                    .is_some_and(|label| label.to_ascii_lowercase().starts_with(&typed_prefix))
        })
        .map(api_entry_completion_item)
        .collect()
}

fn api_completion_candidates(
    api: &ApiIndex,
    prefix: &str,
    file_text: Option<&str>,
) -> Vec<CompletionItem> {
    if prefix.ends_with(':') {
        let receiver = prefix.trim_end_matches(':');
        if let Some(class_name) = file_text.and_then(|text| infer_receiver_class(text, receiver)) {
            let candidates = api
                .methods_for_class_and_bases(&class_name)
                .into_iter()
                .map(api_entry_completion_item)
                .collect::<Vec<_>>();
            if !candidates.is_empty() {
                return candidates;
            }
        }
        let candidates = api
            .methods_for_class_and_bases(receiver)
            .into_iter()
            .map(api_entry_completion_item)
            .collect::<Vec<_>>();
        if !candidates.is_empty() {
            return candidates;
        }
    }
    let needle = if prefix.ends_with('.') || prefix.ends_with(':') {
        prefix.to_string()
    } else {
        format!("{prefix}.")
    };
    api.completions_for_member_prefix(&needle)
        .into_iter()
        .map(api_entry_completion_item)
        .collect()
}

fn infer_receiver_class(text: &str, receiver: &str) -> Option<String> {
    GmodTypeFacts::from_text(text).receiver_class(receiver)
}

#[derive(Debug, Default)]
struct GmodTypeFacts {
    locals: HashMap<String, String>,
    functions: HashMap<String, String>,
}

impl GmodTypeFacts {
    fn from_text(text: &str) -> Self {
        let mut facts = Self::default();
        for line in text.lines() {
            facts.learn_line(line.trim());
        }
        facts
    }

    fn receiver_class(&self, receiver: &str) -> Option<String> {
        self.locals
            .get(receiver)
            .cloned()
            .or_else(|| self.functions.get(receiver).cloned())
            .or_else(|| gmod_constructor_class(receiver).map(str::to_string))
    }

    fn learn_line(&mut self, line: &str) {
        if line.starts_with("--") || line.is_empty() {
            return;
        }
        if let Some(rest) = line.strip_prefix("fn ")
            && let Some((name, expr)) = split_function_expr(rest)
            && let Some(class_name) = self.class_for_expr(expr)
        {
            self.functions.insert(name.to_string(), class_name);
            return;
        }
        if let Some(rest) = line.strip_prefix("local ") {
            self.learn_assignment(rest);
            return;
        }
        self.learn_assignment(line);
    }

    fn learn_assignment(&mut self, input: &str) {
        let Some((name, expr)) = input.split_once('=') else {
            return;
        };
        let name = name.trim();
        if !is_simple_ident(name) {
            return;
        }
        if let Some(class_name) = self.class_for_expr(expr.trim()) {
            self.locals.insert(name.to_string(), class_name);
        }
    }

    fn class_for_expr(&self, expr: &str) -> Option<String> {
        let expr = expr.trim();
        if expr.starts_with("LocalPlayer(") || expr.starts_with("Player(") {
            Some("Player".to_string())
        } else if expr.starts_with("Entity(") {
            Some("Entity".to_string())
        } else if let Some(rest) = expr.strip_prefix("vgui.Create(") {
            quoted_first_arg(rest).or_else(|| Some("Panel".to_string()))
        } else if let Some(name) = expr.strip_suffix("()").filter(|name| is_simple_ident(name)) {
            self.functions.get(name).cloned()
        } else if is_simple_ident(expr) {
            self.locals.get(expr).cloned()
        } else {
            None
        }
    }
}

fn split_function_expr(input: &str) -> Option<(&str, &str)> {
    let (name_and_args, expr) = input.split_once('=')?;
    let name = name_and_args.split('(').next()?.trim();
    is_simple_ident(name).then_some((name, expr.trim()))
}

fn is_simple_ident(value: &str) -> bool {
    let mut chars = value.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    (first.is_ascii_alphabetic() || first == '_')
        && chars.all(|ch| ch.is_ascii_alphanumeric() || ch == '_')
}

fn gmod_constructor_class(name: &str) -> Option<&'static str> {
    match name {
        "LocalPlayer" | "Player" => Some("Player"),
        "Entity" => Some("Entity"),
        _ => None,
    }
}

fn quoted_first_arg(text: &str) -> Option<String> {
    let text = text.trim_start();
    let quote = text.chars().next()?;
    if quote != '"' && quote != '\'' {
        return None;
    }
    let rest = &text[quote.len_utf8()..];
    let end = rest.find(quote)?;
    Some(rest[..end].to_string())
}

fn api_entry_completion_item(entry: &gmod_api_db::ApiEntry) -> CompletionItem {
    let label = entry
        .path
        .rsplit(['.', ':'])
        .next()
        .unwrap_or(&entry.path)
        .to_string();
    let (insert_text, insert_text_format) = api_completion_insert_text(entry, &label);
    CompletionItem {
        label: label.clone(),
        kind: Some(completion_item_kind(completion_kind_for_api(entry.kind))),
        detail: Some(api_completion_detail(entry)),
        documentation: Some(markdown_documentation(entry_markdown(entry))),
        label_details: api_completion_label_details(entry, &label),
        sort_text: Some(api_completion_sort_text(entry)),
        filter_text: Some(api_completion_filter_text(entry, &label)),
        insert_text: Some(insert_text),
        insert_text_format: Some(insert_text_format),
        data: Some(serde_json::json!({
            "lux": "gmodApi",
            "path": entry.path,
        })),
        tags: completion_tags_for_api(entry),
        deprecated: api_entry_is_deprecated(entry).then_some(true),
        ..CompletionItem::default()
    }
}

fn api_completion_insert_text(
    entry: &gmod_api_db::ApiEntry,
    label: &str,
) -> (String, InsertTextFormat) {
    if !matches!(
        entry.kind,
        gmod_api_db::ApiKind::Function | gmod_api_db::ApiKind::Method
    ) {
        return (label.to_string(), InsertTextFormat::PLAIN_TEXT);
    }
    let Some(signature) = entry.signatures.first() else {
        return (format!("{label}()"), InsertTextFormat::PLAIN_TEXT);
    };
    if signature.parameters.is_empty() {
        return (format!("{label}()"), InsertTextFormat::PLAIN_TEXT);
    }
    let args = signature
        .parameters
        .iter()
        .enumerate()
        .map(|(index, parameter)| {
            let fallback = format!("arg{}", index + 1);
            let name = parameter
                .name
                .trim()
                .split_whitespace()
                .next()
                .filter(|name| !name.is_empty())
                .unwrap_or(&fallback);
            format!("${{{}:{}}}", index + 1, snippet_placeholder_escape(name))
        })
        .collect::<Vec<_>>()
        .join(", ");
    (format!("{label}({args})"), InsertTextFormat::SNIPPET)
}

fn snippet_placeholder_escape(value: &str) -> String {
    value
        .replace('\\', "\\\\")
        .replace('$', "\\$")
        .replace('}', "\\}")
}

fn api_completion_detail(entry: &gmod_api_db::ApiEntry) -> String {
    let signature = entry
        .signatures
        .first()
        .map(|signature| signature.label.as_str())
        .unwrap_or(entry.path.as_str());
    format!(
        "GMod {} | {} | {}",
        entry.kind.label(),
        entry.realm.as_str(),
        signature
    )
}

fn api_completion_label_details(
    entry: &gmod_api_db::ApiEntry,
    label: &str,
) -> Option<CompletionItemLabelDetails> {
    let path_context = entry
        .path
        .strip_suffix(label)
        .unwrap_or(&entry.path)
        .trim_end_matches(['.', ':']);
    Some(CompletionItemLabelDetails {
        detail: entry
            .signatures
            .first()
            .and_then(|signature| signature.label.strip_prefix(&entry.path))
            .map(str::to_string),
        description: Some(if path_context.is_empty() {
            format!("GMod {}, {}", entry.kind.label(), entry.realm.as_str())
        } else {
            format!("{path_context} | {}", entry.realm.as_str())
        }),
    })
}

fn api_completion_sort_text(entry: &gmod_api_db::ApiEntry) -> String {
    let group = match entry.kind {
        gmod_api_db::ApiKind::Library => "80",
        gmod_api_db::ApiKind::Function | gmod_api_db::ApiKind::Method => "81",
        gmod_api_db::ApiKind::Class | gmod_api_db::ApiKind::Panel => "82",
        gmod_api_db::ApiKind::Enum | gmod_api_db::ApiKind::Constant => "83",
        _ => "84",
    };
    format!("{group}:{}", entry.path.to_ascii_lowercase())
}

fn api_completion_filter_text(entry: &gmod_api_db::ApiEntry, label: &str) -> String {
    format!("{label} {}", entry.path)
}

fn completion_tags_for_api(entry: &gmod_api_db::ApiEntry) -> Option<Vec<CompletionItemTag>> {
    api_entry_is_deprecated(entry).then_some(vec![CompletionItemTag::DEPRECATED])
}

fn api_entry_is_deprecated(entry: &gmod_api_db::ApiEntry) -> bool {
    let contains_deprecated = |value: &str| value.to_ascii_lowercase().contains("deprecated");
    contains_deprecated(&entry.summary)
        || entry
            .warnings
            .iter()
            .any(|value| contains_deprecated(value))
        || entry.notes.iter().any(|value| contains_deprecated(value))
        || entry
            .source
            .as_ref()
            .is_some_and(|source| contains_deprecated(&source.tags))
}

fn gmod_completion_path(item: &CompletionItem) -> Option<&str> {
    let data = item.data.as_ref()?;
    if data.get("lux")?.as_str()? != "gmodApi" {
        return None;
    }
    data.get("path")?.as_str()
}

fn completion_kind_for_api(kind: gmod_api_db::ApiKind) -> CompletionCandidateKind {
    match kind {
        gmod_api_db::ApiKind::Global => CompletionCandidateKind::Value,
        gmod_api_db::ApiKind::Library => CompletionCandidateKind::Module,
        gmod_api_db::ApiKind::Function => CompletionCandidateKind::Function,
        gmod_api_db::ApiKind::Hook => CompletionCandidateKind::Event,
        gmod_api_db::ApiKind::Class => CompletionCandidateKind::Class,
        gmod_api_db::ApiKind::Method => CompletionCandidateKind::Method,
        gmod_api_db::ApiKind::Field => CompletionCandidateKind::Field,
        gmod_api_db::ApiKind::Enum => CompletionCandidateKind::Enum,
        gmod_api_db::ApiKind::Constant => CompletionCandidateKind::Constant,
        gmod_api_db::ApiKind::Struct => CompletionCandidateKind::Struct,
        gmod_api_db::ApiKind::Panel => CompletionCandidateKind::Class,
        gmod_api_db::ApiKind::Page => CompletionCandidateKind::Reference,
    }
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
    use super::{
        CommandDocumentPosition, GmodTypeFacts, active_realm_command, api_completion_candidates,
        api_entry_completion_item, api_hover_markdown_from_text, api_path_at_offset,
        api_root_completion_candidates, apply_document_changes, completion_item, document_uri_key,
        external_api_hover_markdown, general_binding_completions, gmod_api_coverage_command,
        hook_name_at_offset, identifier_prefix, import_completion_item, infer_receiver_class,
        is_lux_analysis_watched_path, keyword_completion_items, lexical_binding_completions,
        manifest_section_insert_position, method_path_at_offset, module_exports_command,
        path_to_url, resolve_typed_method_path, same_path, server_capabilities, signature_help_at,
        std_package_code_actions,
    };
    use super::{
        CompletionContext, Server, analysis_configs, completion_context, encode_semantic_tokens,
        url_to_path,
    };
    use crate::analysis::{
        AnalysisConfig, AnalysisDiagnostic, AnalysisFile, AnalysisPosition, AnalysisRange,
        AnalysisSemanticToken, AnalysisWorkspace, CompletionCandidate, SemanticTokenKind,
        analyze_files,
    };
    use crate::diag::Severity;
    use crate::package_manager::{LockRequest, lock_project};
    use crate::source::{SourceFile, SourceSpan};
    use gmod_api_db::ApiIndex;
    use lsp_types::notification::{Notification as _, PublishDiagnostics};
    use lsp_types::{
        CodeActionOrCommand, CompletionItemKind, Documentation, InitializeParams, InsertTextFormat,
        PublishDiagnosticsParams, SemanticToken, TextDocumentContentChangeEvent,
    };
    use std::collections::HashMap;
    use std::path::PathBuf;
    use std::time::Duration;

    fn temp_root(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "lux_lsp_{name}_{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("time")
                .as_nanos()
        ))
    }

    fn write_runtime_package(root: &std::path::Path, package_id: &str, export: &str) {
        let package_path = package_id.trim_start_matches('@').replace('/', "_");
        let package_src = root.join(format!("packages/{package_path}/src"));
        std::fs::create_dir_all(&package_src).expect("package source");
        std::fs::write(
            root.join("lux.package.toml"),
            format!(
                "name = \"test-packages\"\n\n[[package]]\nid = \"{package_id}\"\nversion = \"0.1.0\"\npath = \"packages/{package_path}\"\n",
            ),
        )
        .expect("package manifest");
        std::fs::write(
            package_src.join("module.lux"),
            format!("export fn {export}() = true\n"),
        )
        .expect("package module");
    }

    fn write_package_set_macro_package(root: &std::path::Path) -> (PathBuf, PathBuf) {
        let runtime_src = root.join("packages/vendor_caps/src");
        let macro_src = root.join("packages/vendor_caps/compiletime");
        std::fs::create_dir_all(&runtime_src).expect("runtime source");
        std::fs::create_dir_all(&macro_src).expect("macro source");
        std::fs::write(
            root.join("lux.package.toml"),
            "name = \"test-packages\"\n\n[[package]]\nid = \"@vendor/caps\"\nversion = \"0.1.0\"\npath = \"packages/vendor_caps\"\n",
        )
        .expect("package manifest");
        let runtime_path = runtime_src.join("module.lux");
        std::fs::write(
            &runtime_path,
            "import macro { defineValue } from \"@vendor/caps\"\n\ndefineValue()\n",
        )
        .expect("runtime module");
        let macro_path = macro_src.join("module.lux");
        std::fs::write(
            &macro_path,
            "export macro fn defineValue(ctx, call) = nil\n",
        )
        .expect("macro module");
        (runtime_path, macro_path)
    }

    fn published_diagnostics_for(
        connection: &lsp_server::Connection,
        path: &std::path::Path,
    ) -> Vec<lsp_types::Diagnostic> {
        let uri = path_to_url(path).expect("uri");
        let mut latest = None;
        while let Ok(message) = connection.receiver.try_recv() {
            let lsp_server::Message::Notification(notification) = message else {
                continue;
            };
            if notification.method != PublishDiagnostics::METHOD {
                continue;
            }
            let params: PublishDiagnosticsParams =
                serde_json::from_value(notification.params).expect("publish diagnostics params");
            if params.uri == uri {
                latest = Some(params.diagnostics);
            }
        }
        latest.unwrap_or_default()
    }

    #[test]
    fn initialize_capabilities_are_not_double_wrapped() {
        let value = serde_json::to_value(server_capabilities()).expect("capabilities");
        assert!(value.get("completionProvider").is_some());
        assert!(value.get("hoverProvider").is_some());
        assert!(value.get("semanticTokensProvider").is_some());
        let execute_commands = value
            .get("executeCommandProvider")
            .and_then(|provider| provider.get("commands"))
            .and_then(|commands| commands.as_array())
            .expect("execute commands");
        assert!(
            execute_commands
                .iter()
                .any(|command| command.as_str() == Some(super::INSTALL_STD_PACKAGES_COMMAND))
        );
        assert!(value.get("capabilities").is_none());
    }

    #[test]
    fn unresolved_official_lux_package_offers_install_std_packages_fix() {
        let root = temp_root("std_package_fix");
        let source_root = root.join("src");
        std::fs::create_dir_all(&source_root).expect("source root");
        std::fs::write(
            root.join("lux.toml"),
            "package_id = \"demo\"\nbundle_id = \"demo\"\n\n[target.gmod]\nsource_root = \"src\"\nout = \"generated/lua\"\nruntime_base = \"lux/demo\"\nautorun = true\n\n[dependencies]\n",
        )
        .expect("manifest");
        let path = source_root.join("ui.lux");
        let analysis = analyze_files(
            AnalysisConfig::new(&source_root),
            [AnalysisFile {
                path: path.clone(),
                text: "import { signal } from \"@lux/reactive\"\nexport fn run() = signal(0)\n"
                    .into(),
            }],
        )
        .expect("analysis");
        assert!(
            analysis
                .diagnostics_for_path(&path)
                .iter()
                .any(|diagnostic| diagnostic.code.as_deref() == Some("MODULE001")),
            "{:#?}",
            analysis.diagnostics
        );

        let uri = path_to_url(&path).expect("uri");
        let actions = std_package_code_actions(&analysis, &path, &root, &uri);
        let action = actions
            .iter()
            .find_map(|action| match action {
                CodeActionOrCommand::CodeAction(action)
                    if action.title == "Fix: Install std packages" =>
                {
                    Some(action)
                }
                _ => None,
            })
            .expect("install std packages action");
        let command = action.command.as_ref().expect("command");
        assert_eq!(command.command, super::INSTALL_STD_PACKAGES_COMMAND);
        let arguments = command.arguments.as_ref().expect("arguments");
        assert_eq!(arguments.len(), 1);
        assert_eq!(arguments[0]["packages"][0], "@lux/reactive");

        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn event_loop_exits_after_shutdown_exit_sequence() {
        let root = temp_root("shutdown_exit");
        std::fs::create_dir_all(&root).expect("root");
        let initialize: InitializeParams = serde_json::from_value(serde_json::json!({
            "processId": null,
            "rootUri": path_to_url(&root).expect("root uri"),
            "capabilities": {}
        }))
        .expect("initialize params");
        let (server_connection, client_connection) = lsp_server::Connection::memory();
        client_connection
            .sender
            .send(lsp_server::Message::Request(lsp_server::Request::new(
                lsp_server::RequestId::from(1),
                "shutdown".to_string(),
                (),
            )))
            .expect("send shutdown");
        client_connection
            .sender
            .send(lsp_server::Message::Notification(
                lsp_server::Notification::new("exit".to_string(), ()),
            ))
            .expect("send exit");

        let mut server = Server::new(server_connection, initialize);
        server.event_loop().expect("event loop");

        let response = client_connection
            .receiver
            .recv_timeout(Duration::from_secs(1))
            .expect("shutdown response");
        let lsp_server::Message::Response(response) = response else {
            panic!("expected shutdown response");
        };
        assert_eq!(response.id, lsp_server::RequestId::from(1));
        assert!(response.error.is_none());

        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn reanalysis_keeps_package_set_diagnostics_when_nested_project_document_opens() {
        let root = temp_root("mixed_publish");
        let package_source = root.join("lux/mgfx/src/module.lux");
        std::fs::create_dir_all(package_source.parent().expect("package parent"))
            .expect("package dir");
        std::fs::write(
            root.join("lux.package.toml"),
            "name = \"mixed\"\n\n[[package]]\nid = \"@lux/mgfx\"\nversion = \"0.1.0\"\npath = \"lux/mgfx\"\n",
        )
        .expect("package manifest");
        std::fs::write(
            &package_source,
            "export fn needsArg(value) = value\nneedsArg()\n",
        )
        .expect("package source");

        let nested_root = root.join("precompiled");
        let nested_source = nested_root.join("src/cl_mgfx.lux");
        std::fs::create_dir_all(nested_source.parent().expect("nested parent"))
            .expect("nested dir");
        std::fs::write(
            nested_root.join("lux.toml"),
            "package_id = \"mgfx_precompiled\"\nbundle_id = \"mgfx\"\n\n[target.gmod]\nsource_root = \"src\"\nout = \"../dist/lua\"\nruntime_base = \"mgfx\"\nautorun = true\nsource_comments = \"none\"\n\n[dependencies]\n",
        )
        .expect("nested manifest");
        std::fs::write(&nested_source, "local ok = true\n").expect("nested source");

        let initialize: InitializeParams = serde_json::from_value(serde_json::json!({
            "processId": null,
            "rootUri": path_to_url(&root).expect("root uri"),
            "capabilities": {}
        }))
        .expect("initialize params");
        let (server_connection, client_connection) = lsp_server::Connection::memory();
        let mut server = Server::new(server_connection, initialize);

        server.reanalyze_and_publish();
        assert!(
            published_diagnostics_for(&client_connection, &package_source)
                .iter()
                .any(
                    |diagnostic| diagnostic.code.as_ref().and_then(|code| match code {
                        lsp_types::NumberOrString::String(value) => Some(value.as_str()),
                        lsp_types::NumberOrString::Number(_) => None,
                    }) == Some("CALL001")
                )
        );

        server.documents.insert(
            path_to_url(&nested_source).expect("nested uri"),
            std::fs::read_to_string(&nested_source).expect("nested text"),
        );
        server.reanalyze_and_publish();
        assert!(
            published_diagnostics_for(&client_connection, &package_source)
                .iter()
                .any(
                    |diagnostic| diagnostic.code.as_ref().and_then(|code| match code {
                        lsp_types::NumberOrString::String(value) => Some(value.as_str()),
                        lsp_types::NumberOrString::Number(_) => None,
                    }) == Some("CALL001")
                )
        );

        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn completion_context_detects_import_source_and_specifier_lists() {
        assert_eq!(
            completion_context("import { p_", " } from \"inventory\""),
            CompletionContext::ImportSpecifierList {
                source: Some("inventory".into())
            }
        );
        assert_eq!(
            completion_context("import { p_", ""),
            CompletionContext::ImportSpecifierList { source: None }
        );
        assert_eq!(
            completion_context("  import { } from \"", ""),
            CompletionContext::ImportSource
        );
        assert_eq!(
            completion_context("export { player_", " }"),
            CompletionContext::ExportList
        );
        assert_eq!(
            completion_context("net.", ""),
            CompletionContext::ApiMember {
                prefix: "net.".into()
            }
        );
        assert_eq!(
            completion_context("fn run() = inv", ""),
            CompletionContext::General
        );
    }

    #[test]
    fn general_completion_prefix_is_extracted_from_current_token() {
        assert_eq!(identifier_prefix("fn run() = Cre"), "Cre");
        assert_eq!(identifier_prefix("local x = draw.Simple"), "Simple");
        assert_eq!(identifier_prefix("  "), "");
    }

    #[test]
    fn hook_hover_context_extracts_hook_names() {
        let text = "hook.Add(\"PlayerInitialSpawn\", \"id\", function(ply) end)";
        let offset = text.find("Initial").expect("offset");
        assert_eq!(
            hook_name_at_offset(text, offset),
            Some("PlayerInitialSpawn".into())
        );
    }

    #[test]
    fn transient_parse_identifier_diagnostics_are_suppressed_only_for_open_documents() {
        let diagnostic = AnalysisDiagnostic {
            path: PathBuf::from("module.lux"),
            range: AnalysisRange {
                start: AnalysisPosition {
                    line: 0,
                    character: "import { ".len() as u32,
                },
                end: AnalysisPosition {
                    line: 0,
                    character: "import { ".len() as u32,
                },
            },
            severity: Severity::Error,
            code: Some("PARSE005".into()),
            message: "expected identifier".into(),
            notes: Vec::new(),
            help: None,
        };
        assert!(!super::should_publish_diagnostic(
            &diagnostic,
            "import { ",
            true,
            false
        ));
        assert!(super::should_publish_diagnostic(
            &diagnostic,
            "import { ",
            false,
            false
        ));
        assert!(!super::should_publish_diagnostic(
            &AnalysisDiagnostic {
                code: Some("PARSE006".into()),
                message: "expected `from`".into(),
                ..diagnostic.clone()
            },
            "import { bind",
            true,
            true
        ));
    }

    #[test]
    fn document_changes_apply_all_incremental_completion_edits() {
        let initial = "import {  } from \"@lux/reactive\"\n".to_string();
        let text = apply_document_changes(
            initial,
            vec![
                TextDocumentContentChangeEvent {
                    range: Some(lsp_types::Range {
                        start: lsp_types::Position {
                            line: 0,
                            character: 9,
                        },
                        end: lsp_types::Position {
                            line: 0,
                            character: 9,
                        },
                    }),
                    range_length: None,
                    text: "batch".into(),
                },
                TextDocumentContentChangeEvent {
                    range: Some(lsp_types::Range {
                        start: lsp_types::Position {
                            line: 1,
                            character: 0,
                        },
                        end: lsp_types::Position {
                            line: 1,
                            character: 0,
                        },
                    }),
                    range_length: None,
                    text: "local ok = true\n".into(),
                },
            ],
        );
        assert_eq!(
            text,
            "import { batch } from \"@lux/reactive\"\nlocal ok = true\n"
        );
    }

    #[test]
    fn document_changes_accept_full_document_replacement() {
        let text = apply_document_changes(
            "broken".into(),
            vec![TextDocumentContentChangeEvent {
                range: None,
                range_length: None,
                text: "fn ok() = 1".into(),
            }],
        );
        assert_eq!(text, "fn ok() = 1");
    }

    #[test]
    fn signature_help_uses_gmod_api_database() {
        let api = ApiIndex::bundled();
        let file = SourceFile::new(0, None, "net.Start(");
        let help = signature_help_at(&file, &api, file.text.len()).expect("signature help");
        assert_eq!(
            help.signatures[0].label,
            "net.Start(messageName, unreliable = false)"
        );
        assert_eq!(help.signatures[0].parameters.as_ref().unwrap().len(), 2);
    }

    #[test]
    fn manifest_extern_insert_position_targets_existing_section() {
        let text = "[target.gmod]\nsource_root = \"src\"\nout = \"generated/lua\"\n\n[target.gmod.extern]\nA = \"shared\"\n";
        assert_eq!(
            manifest_section_insert_position(text, "target.gmod.extern"),
            Some((6, 0))
        );
    }

    #[test]
    fn analysis_configs_include_nearest_open_document_manifest() {
        let root = std::env::temp_dir().join(format!(
            "lux_lsp_manifest_test_{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("time")
                .as_nanos()
        ));
        let project = root.join("examples/gmod_project");
        let source = project.join("src/client/ui.lux");
        std::fs::create_dir_all(source.parent().expect("source parent")).expect("source dir");
        std::fs::write(
            root.join("lux.toml"),
            "[target.gmod]\nsource_root = \"src\"\nout = \"generated/lua\"\n",
        )
        .expect("root manifest");
        std::fs::write(
            project.join("lux.toml"),
            "[target.gmod]\nsource_root = \"src\"\nout = \"generated/lua\"\n",
        )
        .expect("project manifest");
        std::fs::write(&source, "").expect("source");

        let mut documents = HashMap::new();
        documents.insert(path_to_url(&source).expect("source uri"), String::new());
        let configs = analysis_configs(&root, &documents);
        assert!(
            configs
                .iter()
                .any(|config| same_path(&config.source_root, &project.join("src"))),
            "{configs:#?}"
        );

        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn analysis_configs_keep_package_set_root_when_nested_project_exists() {
        let root = temp_root("mixed_workspace");
        std::fs::create_dir_all(&root).expect("root");
        let package_runtime = root.join("lux/mgfx/src/module.lux");
        std::fs::create_dir_all(package_runtime.parent().expect("runtime parent"))
            .expect("runtime dir");
        std::fs::write(
            root.join("lux.package.toml"),
            "name = \"mixed\"\n\n[[package]]\nid = \"@lux/mgfx\"\nversion = \"0.1.0\"\npath = \"lux/mgfx\"\n",
        )
        .expect("package manifest");
        std::fs::write(
            &package_runtime,
            "import { installGlobal } from \"@lux/mgfx\"\n",
        )
        .expect("package source");

        let nested_root = root.join("precompiled");
        std::fs::create_dir_all(nested_root.join("src")).expect("nested dir");
        std::fs::write(
            nested_root.join("lux.toml"),
            "package_id = \"mgfx_precompiled\"\nbundle_id = \"mgfx\"\n\n[target.gmod]\nsource_root = \"src\"\nout = \"../dist/lua\"\nruntime_base = \"mgfx\"\nautorun = true\nsource_comments = \"none\"\n\n[dependencies]\n",
        )
        .expect("nested manifest");
        std::fs::write(
            nested_root.join("src/cl_mgfx.lux"),
            "import { installGlobal } from \"@lux/mgfx\"\n",
        )
        .expect("nested source");

        let documents = HashMap::from([
            (
                path_to_url(&package_runtime).expect("runtime uri"),
                std::fs::read_to_string(&package_runtime).expect("runtime text"),
            ),
            (
                path_to_url(&nested_root.join("src/cl_mgfx.lux")).expect("nested uri"),
                "import { installGlobal } from \"@lux/mgfx\"\n".to_string(),
            ),
        ]);

        let configs = super::analysis_configs(&root, &documents);
        assert!(
            configs.iter().any(|config| config.is_package_set()),
            "{configs:#?}"
        );
        assert!(
            configs.iter().any(|config| !config.is_package_set()
                && same_path(&config.source_root, &nested_root.join("src"))),
            "{configs:#?}"
        );

        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn overlays_are_filtered_to_the_owning_analysis_config() {
        let root = temp_root("mixed_overlays");
        std::fs::create_dir_all(&root).expect("root");
        let package_runtime = root.join("lux/mgfx/src/module.lux");
        std::fs::create_dir_all(package_runtime.parent().expect("runtime parent"))
            .expect("runtime dir");
        std::fs::write(
            root.join("lux.package.toml"),
            "name = \"mixed\"\n\n[[package]]\nid = \"@lux/mgfx\"\nversion = \"0.1.0\"\npath = \"lux/mgfx\"\n",
        )
        .expect("package manifest");
        std::fs::write(&package_runtime, "export fn installGlobal() = true\n")
            .expect("package source");

        let nested_root = root.join("precompiled");
        let nested_source = nested_root.join("src/cl_mgfx.lux");
        std::fs::create_dir_all(nested_source.parent().expect("nested parent"))
            .expect("nested dir");
        std::fs::write(
            nested_root.join("lux.toml"),
            "package_id = \"mgfx_precompiled\"\nbundle_id = \"mgfx\"\n\n[target.gmod]\nsource_root = \"src\"\nout = \"../dist/lua\"\nruntime_base = \"mgfx\"\nautorun = true\nsource_comments = \"none\"\n\n[dependencies]\n",
        )
        .expect("nested manifest");
        std::fs::write(&nested_source, "local ok = true\n").expect("nested source");

        let documents = HashMap::from([
            (
                path_to_url(&package_runtime).expect("runtime uri"),
                std::fs::read_to_string(&package_runtime).expect("runtime text"),
            ),
            (
                path_to_url(&nested_source).expect("nested uri"),
                std::fs::read_to_string(&nested_source).expect("nested text"),
            ),
        ]);
        let overlays = documents
            .iter()
            .map(|(uri, text)| AnalysisFile {
                path: url_to_path(uri).expect("overlay path"),
                text: text.clone(),
            })
            .collect::<Vec<_>>();
        let configs = analysis_configs(&root, &documents);
        let package_config = configs
            .iter()
            .find(|config| config.is_package_set())
            .expect("package-set config");
        let project_config = configs
            .iter()
            .find(|config| !config.is_package_set())
            .expect("project config");

        let package_overlays = super::overlays_for_config(package_config, &overlays);
        assert_eq!(package_overlays.len(), 1, "{package_overlays:#?}");
        assert!(same_path(&package_overlays[0].path, &package_runtime));

        let project_overlays = super::overlays_for_config(project_config, &overlays);
        assert_eq!(project_overlays.len(), 1, "{project_overlays:#?}");
        assert!(same_path(&project_overlays[0].path, &nested_source));

        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn analysis_config_waits_for_open_document_when_root_has_no_manifest() {
        let root = std::env::temp_dir().join(format!(
            "lux_lsp_empty_manifest_test_{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("time")
                .as_nanos()
        ));
        std::fs::create_dir_all(&root).expect("root");
        let documents = HashMap::new();

        assert!(super::analysis_configs(&root, &documents).is_empty());

        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn no_manifest_workspace_uses_standalone_analysis_for_open_lux_files() {
        let root = temp_root("standalone_workspace");
        let examples = root.join("examples");
        std::fs::create_dir_all(&examples).expect("examples");
        let features = examples.join("features.lux");
        let diagnostics = examples.join("match_diagnostics.lux");
        std::fs::write(&features, "fn feature() = 1").expect("features");
        std::fs::write(&diagnostics, "fn demo() = 2").expect("diagnostics");

        let mut documents = HashMap::new();
        documents.insert(
            path_to_url(&features).expect("features uri"),
            std::fs::read_to_string(&features).expect("features text"),
        );
        documents.insert(
            path_to_url(&diagnostics).expect("diagnostics uri"),
            std::fs::read_to_string(&diagnostics).expect("diagnostics text"),
        );

        let configs = analysis_configs(&root, &documents);
        assert_eq!(configs.len(), 1, "{configs:#?}");
        assert!(configs[0].is_standalone());

        let overlays = documents
            .iter()
            .map(|(uri, text)| AnalysisFile {
                path: url_to_path(uri).expect("overlay path"),
                text: text.clone(),
            })
            .collect::<Vec<_>>();
        let workspace = AnalysisWorkspace::load(configs[0].clone(), overlays).expect("analysis");
        let analysis = workspace.analysis();

        assert!(
            analysis
                .lsp_diagnostics_for_path(&features)
                .iter()
                .all(|diagnostic| diagnostic.code.as_deref() != Some("PART007")),
            "{:#?}",
            analysis.lsp_diagnostics_for_path(&features)
        );
        assert!(
            analysis
                .module_for_path(&features)
                .is_some_and(|module| module.module_path == "examples/features")
        );
        assert!(
            analysis
                .module_for_path(&diagnostics)
                .is_some_and(|module| module.module_path == "examples/match_diagnostics")
        );

        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn analysis_config_loads_package_set_workspace_without_project_manifest() {
        let root = temp_root("package_set_workspace");
        std::fs::create_dir_all(&root).expect("root");
        let (runtime_path, macro_path) = write_package_set_macro_package(&root);

        let mut documents = HashMap::new();
        documents.insert(path_to_url(&runtime_path).expect("runtime uri"), {
            std::fs::read_to_string(&runtime_path).expect("runtime text")
        });
        let config = analysis_configs(&root, &documents)
            .into_iter()
            .find(AnalysisConfig::is_package_set)
            .expect("analysis config");
        assert!(config.is_package_set());
        assert!(
            config.package_roots.iter().any(|path| path == &root),
            "{:?}",
            config.package_roots
        );

        let workspace = AnalysisWorkspace::load(config, Vec::new()).expect("analysis");
        let analysis = workspace.analysis();
        assert!(analysis.file_by_path(&runtime_path).is_some());
        assert!(analysis.file_by_path(&macro_path).is_some());
        assert!(
            analysis
                .lsp_diagnostics_for_path(&macro_path)
                .iter()
                .all(|diagnostic| diagnostic.code.as_deref() != Some("RESOLVE006")),
            "{:#?}",
            analysis.lsp_diagnostics_for_path(&macro_path)
        );
        assert!(
            analysis
                .lsp_diagnostics_for_path(&runtime_path)
                .iter()
                .all(|diagnostic| diagnostic.code.as_deref() != Some("MACRO001")),
            "{:#?}",
            analysis.lsp_diagnostics_for_path(&runtime_path)
        );
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn analysis_config_loads_locked_package_roots_for_completion_and_diagnostics() {
        let root = std::env::temp_dir().join(format!(
            "lux_lsp_lock_package_test_{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("time")
                .as_nanos()
        ));
        let project = root.join("project");
        let source_root = project.join("src");
        let package_root = root.join("package-set");
        std::fs::create_dir_all(&source_root).expect("project source");
        write_runtime_package(&package_root, "@vendor/ui", "mount");
        std::fs::write(
            project.join("lux.toml"),
            "package_id = \"demo\"\nbundle_id = \"demo\"\n\n[target.gmod]\nsource_root = \"src\"\nout = \"generated/lua\"\nruntime_base = \"lux/demo\"\nautorun = true\n\n[dependencies]\n\"@vendor/ui\" = { path = \"../package-set\" }\n",
        )
        .expect("manifest");
        let source = source_root.join("module.lux");
        std::fs::write(&source, "import { mount } from \"@vendor/ui\"\nmount()\n").expect("source");
        lock_project(&LockRequest {
            project_root: project.clone(),
        })
        .expect("lock project");

        let config = analysis_configs(&project, &HashMap::new())
            .into_iter()
            .next()
            .expect("analysis config");
        assert!(
            config
                .package_roots
                .iter()
                .any(|root| root == &package_root),
            "{:?}",
            config.package_roots
        );
        let workspace = AnalysisWorkspace::load(config, Vec::new()).expect("analysis");
        let analysis = workspace.analysis();
        let diagnostics = analysis.lsp_diagnostics_for_path(&source);
        assert!(
            diagnostics.iter().all(|diagnostic| !diagnostic
                .message
                .contains("failed to load Lux runtime package metadata")),
            "{diagnostics:#?}"
        );
        let exports =
            analysis.importable_exports(&source, "@vendor/ui", crate::module::RealmSet::SHARED);
        assert!(exports.iter().any(|candidate| candidate.label == "mount"));
        let all_exports =
            analysis.importable_exports_for_all_sources(&source, crate::module::RealmSet::SHARED);
        assert!(all_exports.iter().any(|candidate| {
            candidate.label == "mount" && candidate.source.as_deref() == Some("@vendor/ui")
        }));
        let offset = analysis
            .offset_for_position(&source, 1, "mount".len())
            .expect("offset");
        let symbol = analysis
            .symbol_at_path_offset(&source, offset)
            .expect("symbol");
        assert!(
            symbol
                .definition_path
                .as_ref()
                .is_some_and(|path| path.ends_with("package-set/packages/vendor_ui/src/module.lux")),
            "{:?}",
            symbol.definition_path
        );
        let hover = analysis
            .hover_markdown_at_path_offset(&source, offset)
            .expect("hover");
        assert!(hover.contains("**Signature:** `mount()`"), "{hover}");
        let signature_help = analysis
            .signature_help_at_path_offset(
                &source,
                "import { mount } from \"@vendor/ui\"\nmount(".len(),
            )
            .expect("signature help");
        assert_eq!(signature_help.signature.label, "mount()");

        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn analysis_watched_paths_include_manifest_and_lockfile() {
        assert!(is_lux_analysis_watched_path(
            PathBuf::from("lux.toml").as_path()
        ));
        assert!(is_lux_analysis_watched_path(
            PathBuf::from("lux.lock").as_path()
        ));
        assert!(!is_lux_analysis_watched_path(
            PathBuf::from("module.lux").as_path()
        ));
    }

    #[test]
    fn infers_gmod_receiver_class_from_common_constructors() {
        let text = "fn current() = LocalPlayer()\nlocal ply = current()\nlocal alias = ply\nlocal button = vgui.Create(\"DButton\")\n";
        assert_eq!(infer_receiver_class(text, "ply"), Some("Player".into()));
        assert_eq!(infer_receiver_class(text, "alias"), Some("Player".into()));
        assert_eq!(infer_receiver_class(text, "button"), Some("DButton".into()));
    }

    #[test]
    fn api_completion_uses_receiver_class_methods_for_colon_calls() {
        let api = ApiIndex::bundled();
        let text = "local ply = LocalPlayer()\nply:";
        let labels = api_completion_candidates(&api, "ply:", Some(text))
            .into_iter()
            .map(|candidate| candidate.label)
            .collect::<Vec<_>>();
        assert!(labels.iter().any(|label| label == "Nick"), "{labels:#?}");
    }

    #[test]
    fn api_hover_extracts_dot_paths_without_project_analysis() {
        let api = ApiIndex::bundled();
        let text = "draw.SimpleText(\"HP\", \"DermaDefault\", 0, 0)";
        let offset = text.find("SimpleText").expect("offset");
        assert_eq!(
            api_path_at_offset(text, offset),
            Some("draw.SimpleText".into())
        );
        let markdown =
            api_hover_markdown_from_text(&api, text, offset).expect("official API hover");
        assert!(markdown.contains("draw.SimpleText"), "{markdown}");
        assert!(markdown.contains("Official documentation"), "{markdown}");
    }

    #[test]
    fn lux_import_hover_takes_precedence_over_gmod_api_names() {
        let root = PathBuf::from("src");
        let path = root.join("client/ui.lux");
        let text = "import { Button } from \"@lux/ui\"\nexport fn mount(panel) = Button({})\n";
        let analysis = analyze_files(
            AnalysisConfig::new(&root).with_package_id("game"),
            [AnalysisFile {
                path: path.clone(),
                text: text.into(),
            }],
        )
        .expect("analysis");
        let offset = analysis
            .offset_for_position(&path, 0, "import { Bu".len())
            .expect("offset");
        let lux_hover = analysis
            .hover_markdown_at_path_offset(&path, offset)
            .expect("Lux hover");
        assert!(lux_hover.contains("Imported from"), "{lux_hover}");

        let api = ApiIndex::bundled();
        assert!(
            external_api_hover_markdown(&analysis, &api, &path, offset).is_none(),
            "Lux import binding must not be treated as GMod API"
        );
    }

    #[test]
    fn import_completion_without_from_inserts_source() {
        let root = temp_root("import_completion");
        let project = root.join("project");
        let source_root = project.join("src");
        let package_root = root.join("package-set");
        std::fs::create_dir_all(&source_root).expect("project source");
        write_runtime_package(&package_root, "@vendor/ui", "Button");
        std::fs::write(
            project.join("lux.toml"),
            "package_id = \"game\"\nbundle_id = \"game\"\n\n[target.gmod]\nsource_root = \"src\"\nout = \"generated/lua\"\nruntime_base = \"lux/game\"\nautorun = true\n\n[dependencies]\n\"@vendor/ui\" = { path = \"../package-set\" }\n",
        )
        .expect("manifest");
        lock_project(&LockRequest {
            project_root: project.clone(),
        })
        .expect("lock project");
        let path = source_root.join("client/ui.lux");
        let config = analysis_configs(&project, &HashMap::new())
            .into_iter()
            .find(|config| !config.is_package_set())
            .expect("analysis config");
        let analysis = analyze_files(
            config,
            [AnalysisFile {
                path: path.clone(),
                text: "import { Bu".into(),
            }],
        )
        .expect("analysis");
        let candidate = analysis
            .importable_exports_for_all_sources(&path, crate::module::RealmSet::CLIENT)
            .into_iter()
            .find(|candidate| {
                candidate.label == "Button" && candidate.source.as_deref() == Some("@vendor/ui")
            })
            .expect("Button import candidate");
        let item = import_completion_item(candidate, true);
        assert_eq!(item.label, "Button");
        assert_eq!(
            item.insert_text.as_deref(),
            Some("Button } from \"@vendor/ui\"")
        );
        assert!(item.label_details.is_some());

        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn keyword_completion_includes_import_and_conditional_controls() {
        let import = keyword_completion_items("imp")
            .into_iter()
            .find(|item| item.label == "import")
            .expect("import keyword");
        assert_eq!(import.kind, Some(CompletionItemKind::KEYWORD));
        assert_eq!(import.insert_text.as_deref(), Some("import { "));
        assert_eq!(
            import.insert_text_format,
            Some(InsertTextFormat::PLAIN_TEXT)
        );

        let stop_labels = keyword_completion_items("sto")
            .into_iter()
            .map(|item| item.label)
            .collect::<Vec<_>>();
        assert!(stop_labels.iter().any(|label| label == "stopif"));
        assert!(stop_labels.iter().any(|label| label == "stopifn"));
    }

    #[test]
    fn general_completion_includes_user_parameters_and_locals() {
        let root = PathBuf::from("src");
        let path = root.join("client/ui.lux");
        let text = "export fn mount(panel, players) {\n  local selected = players\n  pla\n}\n";
        let analysis = analyze_files(
            AnalysisConfig::new(&root).with_package_id("game"),
            [AnalysisFile {
                path: path.clone(),
                text: text.into(),
            }],
        )
        .expect("analysis");
        let offset = analysis
            .offset_for_position(&path, 2, "  pla".len())
            .expect("offset");
        let file = analysis.file_by_path(&path).expect("analysis file");
        let labels = general_binding_completions(&analysis, &path, offset, file)
            .into_iter()
            .map(|candidate| candidate.label)
            .collect::<Vec<_>>();
        assert!(labels.iter().any(|label| label == "players"), "{labels:#?}");
        assert!(
            labels.iter().any(|label| label == "selected"),
            "{labels:#?}"
        );
    }

    #[test]
    fn lexical_completion_survives_incomplete_function_body() {
        let text = "export fn mount(panel, players) {\n  local selected = players\n  pla";
        let file = SourceFile::new(0, None, text);
        let labels = lexical_binding_completions(&file, text.len())
            .into_iter()
            .map(|candidate| candidate.label)
            .collect::<Vec<_>>();
        assert!(labels.iter().any(|label| label == "panel"), "{labels:#?}");
        assert!(labels.iter().any(|label| label == "players"), "{labels:#?}");
        assert!(
            labels.iter().any(|label| label == "selected"),
            "{labels:#?}"
        );
    }

    #[test]
    fn lexical_completion_sorts_before_gmod_api_candidates() {
        let local = completion_item(CompletionCandidate {
            label: "players".into(),
            kind: crate::analysis::CompletionCandidateKind::Parameter,
            detail: Some("function parameter".into()),
            documentation: None,
            source: None,
        });
        let api = api_entry_completion_item(ApiIndex::bundled().entry("player").expect("player"));
        assert!(
            local.sort_text.as_deref() < api.sort_text.as_deref(),
            "local sort={:?}, api sort={:?}",
            local.sort_text,
            api.sort_text
        );
    }

    #[test]
    fn lexical_completion_includes_part_imports_without_word_suggestions() {
        let text =
            "import { Button, Column as Stack } from \"@lux/ui\"\nexport fn mount(panel) {\n  Bu";
        let file = SourceFile::new(0, None, text);
        let labels = lexical_binding_completions(&file, text.len())
            .into_iter()
            .map(|candidate| candidate.label)
            .collect::<Vec<_>>();
        assert!(labels.iter().any(|label| label == "Button"), "{labels:#?}");
        assert!(labels.iter().any(|label| label == "Stack"), "{labels:#?}");
        assert!(!labels.iter().any(|label| label == "Column"), "{labels:#?}");
    }

    #[test]
    fn gmod_api_completion_items_use_specific_kinds() {
        let api = ApiIndex::bundled();
        let entry = api.entry("player.GetAll").expect("player.GetAll");
        let item = api_entry_completion_item(entry);
        assert_eq!(item.insert_text.as_deref(), Some("GetAll()"));
        assert_eq!(item.insert_text_format, Some(InsertTextFormat::PLAIN_TEXT));

        let entry = api.entry("draw.SimpleText").expect("draw.SimpleText");
        let item = api_entry_completion_item(entry);
        assert_eq!(item.kind, Some(CompletionItemKind::FUNCTION));
        assert_eq!(
            item.insert_text.as_deref(),
            Some(
                "SimpleText(${1:text}, ${2:font}, ${3:x}, ${4:y}, ${5:color}, ${6:xAlign}, ${7:yAlign})"
            )
        );
        assert_eq!(item.insert_text_format, Some(InsertTextFormat::SNIPPET));
        let doc = completion_documentation_text(&item.documentation);
        assert!(doc.contains("draw.SimpleText"), "{doc}");
        assert!(doc.contains("**Parameters**"), "{doc}");
        assert!(doc.contains("**Returns**"), "{doc}");
        assert!(doc.contains("Official documentation"), "{doc}");

        let entry = api.entry("Player:Nick").expect("Player:Nick");
        let item = api_entry_completion_item(entry);
        assert_eq!(item.kind, Some(CompletionItemKind::METHOD));
        assert!(item.label_details.is_some());
    }

    #[test]
    fn root_api_completion_uses_typed_prefix() {
        let api = ApiIndex::bundled();
        let labels = api_root_completion_candidates(&api, "CreateClient")
            .into_iter()
            .map(|candidate| candidate.label)
            .collect::<Vec<_>>();
        assert!(
            labels.iter().any(|label| label == "CreateClientConVar"),
            "{labels:#?}"
        );
        assert!(
            labels.iter().all(|label| label.starts_with("CreateClient")),
            "{labels:#?}"
        );
    }

    #[test]
    fn api_member_completion_excludes_root_prefix_matches() {
        let api = ApiIndex::bundled();
        let labels = api_completion_candidates(&api, "player.", None)
            .into_iter()
            .map(|candidate| candidate.label)
            .collect::<Vec<_>>();

        assert!(labels.iter().any(|label| label == "GetAll"), "{labels:#?}");
        assert!(!labels.iter().any(|label| label == "player"), "{labels:#?}");
        assert!(
            !labels.iter().any(|label| label == "player_manager"),
            "{labels:#?}"
        );
    }

    #[test]
    fn api_completion_uses_official_class_parent_chain_for_panels() {
        let api = ApiIndex::bundled();
        let text = "local button = vgui.Create(\"DButton\")\nbutton:";
        let labels = api_completion_candidates(&api, "button:", Some(text))
            .into_iter()
            .map(|candidate| candidate.label)
            .collect::<Vec<_>>();
        assert!(
            labels.iter().any(|label| label == "SetImage"),
            "{labels:#?}"
        );
        assert!(labels.iter().any(|label| label == "SetSize"), "{labels:#?}");
    }

    #[test]
    fn signature_help_uses_receiver_type_facts_for_method_calls() {
        let api = ApiIndex::bundled();
        let file = SourceFile::new(0, None, "local ply = LocalPlayer()\nply:Nick(");
        let help = signature_help_at(&file, &api, file.text.len()).expect("signature help");
        assert_eq!(help.signatures[0].label, "Player:Nick()");
    }

    #[test]
    fn signature_help_uses_official_parent_chain_for_panel_methods() {
        let api = ApiIndex::bundled();
        let file = SourceFile::new(
            0,
            None,
            "local button = vgui.Create(\"DButton\")\nbutton:SetSize(",
        );
        let help = signature_help_at(&file, &api, file.text.len()).expect("signature help");
        assert_eq!(help.signatures[0].label, "Panel:SetSize(width, height)");
    }

    #[test]
    fn hover_method_path_uses_receiver_type_facts() {
        let api = ApiIndex::bundled();
        let text = "local ply = LocalPlayer()\nply:Nick()";
        let offset = text.find("Nick").expect("offset");
        let path = method_path_at_offset(text, offset).expect("method path");
        let facts = GmodTypeFacts::from_text(text);
        assert_eq!(path, "ply:Nick");
        assert_eq!(
            resolve_typed_method_path(&api, &facts, &path),
            Some("Player:Nick".into())
        );
    }

    #[test]
    fn hover_method_path_uses_official_parent_chain_for_panels() {
        let api = ApiIndex::bundled();
        let text = "local button = vgui.Create(\"DButton\")\nbutton:SetSize(24, 24)";
        let offset = text.find("SetSize").expect("offset");
        let path = method_path_at_offset(text, offset).expect("method path");
        let facts = GmodTypeFacts::from_text(text);
        assert_eq!(path, "button:SetSize");
        assert_eq!(
            resolve_typed_method_path(&api, &facts, &path),
            Some("Panel:SetSize".into())
        );
    }

    #[test]
    fn semantic_tokens_are_sorted_and_delta_encoded() {
        let file = SourceFile::new(0, None, "fn run()\n  local value = 1\n");
        let tokens = vec![
            AnalysisSemanticToken {
                span: SourceSpan::new(file.id, 11, 16),
                kind: SemanticTokenKind::Keyword,
            },
            AnalysisSemanticToken {
                span: SourceSpan::new(file.id, 3, 6),
                kind: SemanticTokenKind::Function,
            },
        ];

        let encoded = encode_semantic_tokens(&file, tokens);
        assert_eq!(
            encoded,
            vec![
                SemanticToken {
                    delta_line: 0,
                    delta_start: 3,
                    length: 3,
                    token_type: 2,
                    token_modifiers_bitset: 0,
                },
                SemanticToken {
                    delta_line: 1,
                    delta_start: 2,
                    length: 5,
                    token_type: 0,
                    token_modifiers_bitset: 0,
                },
            ]
        );
    }

    #[test]
    fn semantic_tokens_prefer_non_overlapping_tokens_on_the_same_line() {
        let file = SourceFile::new(0, None, "local name = self?:GetName() ?? \"panel\"\n");
        let tokens = vec![
            AnalysisSemanticToken {
                span: SourceSpan::new(file.id, 0, 5),
                kind: SemanticTokenKind::Keyword,
            },
            AnalysisSemanticToken {
                span: SourceSpan::new(file.id, 6, 10),
                kind: SemanticTokenKind::Variable,
            },
            AnalysisSemanticToken {
                span: SourceSpan::new(file.id, 13, 17),
                kind: SemanticTokenKind::Variable,
            },
            AnalysisSemanticToken {
                span: SourceSpan::new(file.id, 13, 28),
                kind: SemanticTokenKind::External,
            },
            AnalysisSemanticToken {
                span: SourceSpan::new(file.id, 29, 31),
                kind: SemanticTokenKind::Operator,
            },
            AnalysisSemanticToken {
                span: SourceSpan::new(file.id, 32, 39),
                kind: SemanticTokenKind::String,
            },
        ];

        let encoded = encode_semantic_tokens(&file, tokens);
        assert_eq!(
            encoded,
            vec![
                SemanticToken {
                    delta_line: 0,
                    delta_start: 0,
                    length: 5,
                    token_type: 0,
                    token_modifiers_bitset: 0,
                },
                SemanticToken {
                    delta_line: 0,
                    delta_start: 6,
                    length: 4,
                    token_type: 4,
                    token_modifiers_bitset: 0,
                },
                SemanticToken {
                    delta_line: 0,
                    delta_start: 7,
                    length: 4,
                    token_type: 4,
                    token_modifiers_bitset: 0,
                },
                SemanticToken {
                    delta_line: 0,
                    delta_start: 16,
                    length: 2,
                    token_type: 11,
                    token_modifiers_bitset: 0,
                },
                SemanticToken {
                    delta_line: 0,
                    delta_start: 3,
                    length: 7,
                    token_type: 8,
                    token_modifiers_bitset: 0,
                },
            ]
        );
    }

    #[test]
    fn semantic_tokens_flush_pending_analysis_before_reading() {
        let root = temp_root("semantic_flush");
        std::fs::create_dir_all(&root).expect("root");
        let source = root.join("module.lux");
        std::fs::write(&source, "fn run() = 1\n").expect("source");
        let initialize: InitializeParams = serde_json::from_value(serde_json::json!({
            "processId": null,
            "rootUri": path_to_url(&root).expect("root uri"),
            "capabilities": {}
        }))
        .expect("initialize params");
        let (server_connection, client_connection) = lsp_server::Connection::memory();
        let mut server = Server::new(server_connection, initialize);
        server.analysis_due = Some(std::time::Instant::now() + Duration::from_secs(60));
        server.documents.insert(
            path_to_url(&source).expect("source uri"),
            std::fs::read_to_string(&source).expect("source text"),
        );

        let params = lsp_types::SemanticTokensParams {
            text_document: lsp_types::TextDocumentIdentifier {
                uri: path_to_url(&source).expect("source uri"),
            },
            work_done_progress_params: Default::default(),
            partial_result_params: Default::default(),
        };
        let _ = server.semantic_tokens(params).expect("semantic tokens");
        assert!(server.analysis_due.is_none());
        drop(client_connection);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn file_uri_round_trip_preserves_paths() {
        let path = std::env::current_dir()
            .expect("cwd")
            .join("src")
            .join("module.lux");
        let uri = path_to_url(&path).expect("file uri");
        let round_tripped = url_to_path(&uri).expect("path");
        assert_eq!(round_tripped, path);
    }

    #[test]
    fn document_uri_key_normalizes_encoded_windows_drive_uris() {
        if !cfg!(windows) {
            return;
        }
        let encoded: lsp_types::Uri =
            "file:///c%3A/Development/gmod/lux/examples/gmod_project/src/client/ui.lux"
                .parse()
                .expect("encoded uri");
        let canonical: lsp_types::Uri =
            "file:///C:/Development/gmod/lux/examples/gmod_project/src/client/ui.lux"
                .parse()
                .expect("canonical uri");
        assert_eq!(document_uri_key(&encoded), document_uri_key(&canonical));
    }

    #[test]
    fn command_document_position_accepts_camel_case_arguments() {
        let uri = path_to_url(&std::env::current_dir().expect("cwd").join("src/module.lux"))
            .expect("uri");
        let value = serde_json::json!({
            "uri": uri,
            "line": 2,
            "character": 4
        });
        let parsed = CommandDocumentPosition::from_arguments(&[value])
            .expect("valid args")
            .expect("position");
        assert_eq!(parsed.line, Some(2));
        assert_eq!(parsed.character, Some(4));
    }

    #[test]
    fn command_results_use_analysis_for_exports_and_realm() {
        let root = std::env::temp_dir().join(format!(
            "lux_lsp_command_test_{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("time")
                .as_nanos()
        ));
        std::fs::create_dir_all(&root).expect("root");
        let source = root.join("module.lux");
        std::fs::write(
            &source,
            "client fn paint() = 1\nserver fn grant() = 2\nexport client { paint }\n",
        )
        .expect("source");
        let workspace =
            AnalysisWorkspace::load(AnalysisConfig::new(&root), Vec::new()).expect("analysis");
        let analysis = workspace.analysis();
        let uri = path_to_url(&source).expect("uri");
        let position = CommandDocumentPosition {
            uri,
            line: Some(0),
            character: Some(3),
        };

        let exports = module_exports_command(analysis, Some(&position));
        assert_eq!(exports.kind, "moduleExports");
        assert!(exports.items.iter().any(|item| item.label == "paint"));

        let realm = active_realm_command(analysis, Some(&position));
        assert_eq!(realm.kind, "activeRealm");
        assert_eq!(realm.items[0].label, "client");

        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn gmod_api_coverage_command_reports_full_official_docs() {
        let api = ApiIndex::bundled();
        let result = gmod_api_coverage_command(&api);
        assert_eq!(result.kind, "gmodApiCoverage");
        assert!(result.markdown.contains("Official pages"));
        assert!(
            result
                .items
                .iter()
                .any(|item| item.label == "Document records")
        );
    }

    fn completion_documentation_text(documentation: &Option<Documentation>) -> String {
        match documentation {
            Some(Documentation::MarkupContent(markup)) => markup.value.clone(),
            Some(Documentation::String(value)) => value.clone(),
            None => String::new(),
        }
    }
}
