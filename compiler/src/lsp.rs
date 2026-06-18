use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

mod commands;
mod completion;
mod cursor;
mod diagnostics;
mod gmod_api;
mod lexical_completion;
mod protocol;
mod semantic;
mod text_sync;
mod workspace;

use crate::analysis::{AnalysisFile, AnalysisWorkspace, ProjectAnalysis, format_text};
use crate::module::RealmAvailability;
use crate::package_manager::{DependencySource, InstallRequest, LUX_STD_REPO, install_package};
use commands::{
    CommandDocumentPosition, CommandResult, InstallStdPackagesCommand, active_realm_command,
    gmod_api_coverage_command, module_exports_command,
};
use completion::{CompletionInput, completion_items, resolve_completion_item};
use crossbeam_channel::RecvTimeoutError;
use cursor::{
    completion_context_at, previous_non_whitespace_char, should_flush_analysis_for_completion,
};
use diagnostics::{
    api_doc_code_actions, code_action, is_official_lux_package,
    is_transient_import_parse_diagnostic, lsp_diagnostic, manifest_extern_code_actions,
    should_publish_diagnostic, std_package_code_actions,
};
use gmod_api::{
    api_hover_markdown_from_text, external_api_hover_markdown, hook_hover_markdown_from_text,
    signature_help_at,
};
use gmod_api_db::ApiIndex;
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
    CodeActionOrCommand, CodeActionParams, CompletionItem, CompletionParams, CompletionResponse,
    Diagnostic, DidChangeTextDocumentParams, DidChangeWatchedFilesParams,
    DidCloseTextDocumentParams, DidOpenTextDocumentParams, DidSaveTextDocumentParams,
    DocumentFormattingParams, ExecuteCommandParams, GotoDefinitionParams, GotoDefinitionResponse,
    Hover, HoverParams, InitializeParams, Location, MessageType, Position,
    PublishDiagnosticsParams, Range, SemanticTokens, SemanticTokensParams, SemanticTokensResult,
    ShowMessageParams, SignatureHelp, TextEdit, Uri,
};
use protocol::{
    INSTALL_STD_PACKAGES_COMMAND, document_uri_key, encode_semantic_tokens, json_result,
    markdown_hover, path_to_url, server_capabilities, signature_help_from_analysis, source_range,
    url_to_path,
};
use semantic::{
    package_member_signature_help_from_snapshot, package_member_symbol_from_snapshot,
    should_flush_analysis_for_position, symbol_hover_markdown as analysis_symbol_hover_markdown,
};
use text_sync::{
    apply_document_changes, debug_log, diagnostic_summary, document_change_summary, focus_lines,
};
use workspace::{
    analysis_config_key, analysis_config_label, analysis_config_label_for_analysis,
    analysis_config_summary, analysis_configs, analysis_path_score, is_lux_analysis_watched_path,
    overlays_for_config, overlays_summary, workspace_root,
};

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
        let uri = &params.text_document_position_params.text_document.uri;
        let position = params.text_document_position_params.position;
        let snapshot = self.document_snapshot(uri, position);
        if should_flush_analysis_for_position(&snapshot) {
            self.flush_pending_analysis();
        }
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
            if let Some(markdown) =
                self.package_member_hover_from_snapshot(analysis, &path, &snapshot)
            {
                return json_result(Some(markdown_hover(markdown)));
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
        let uri = &params.text_document_position.text_document.uri;
        let snapshot = self.document_snapshot(uri, params.text_document_position.position);
        let path = snapshot.path;
        let offset = snapshot.offset;
        let line_prefix = snapshot.file.text[..offset]
            .rsplit('\n')
            .next()
            .unwrap_or_default();
        let context = completion_context_at(&snapshot.file.text, offset);
        if is_ignored_space_completion_trigger(&params, &snapshot.file.text, offset, &context) {
            return json_result(Some(CompletionResponse::Array(Vec::new())));
        }
        if should_flush_analysis_for_completion(&context) {
            self.flush_pending_analysis();
        }
        let analysis = path
            .as_deref()
            .and_then(|path| self.analysis_for_path(path))
            .or_else(|| self.analysis());

        let items = completion_items(CompletionInput {
            context,
            analysis,
            path: path.as_deref(),
            offset,
            line_prefix,
            current_file: &snapshot.file,
            gmod_api: &self.gmod_api,
        });

        json_result(Some(CompletionResponse::Array(items)))
    }

    fn completion_resolve(&mut self, item: CompletionItem) -> Result<serde_json::Value, String> {
        json_result(resolve_completion_item(&self.gmod_api, item))
    }

    fn signature_help(
        &mut self,
        params: lsp_types::SignatureHelpParams,
    ) -> Result<serde_json::Value, String> {
        let uri = &params.text_document_position_params.text_document.uri;
        let position = params.text_document_position_params.position;
        let snapshot = self.document_snapshot(uri, position);
        if is_ignored_space_signature_trigger(&params, &snapshot.file.text, snapshot.offset) {
            return json_result::<Option<SignatureHelp>>(None);
        }
        if should_flush_analysis_for_position(&snapshot) {
            self.flush_pending_analysis();
        }
        let Some((analysis, path, offset)) = self.analysis_and_offset(uri, position) else {
            return json_result::<Option<SignatureHelp>>(None);
        };
        if let Some(help) = analysis.signature_help_at_path_offset(&path, offset) {
            return json_result(Some(signature_help_from_analysis(help)));
        }
        if let Some(help) = package_member_signature_help_from_snapshot(analysis, &path, &snapshot)
        {
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
        let uri = &params.text_document_position_params.text_document.uri;
        let position = params.text_document_position_params.position;
        let snapshot = self.document_snapshot(uri, position);
        if should_flush_analysis_for_position(&snapshot) {
            self.flush_pending_analysis();
        }
        let Some((analysis, path, offset)) = self.analysis_and_offset(uri, position) else {
            return json_result::<Option<GotoDefinitionResponse>>(None);
        };
        let symbol = analysis
            .symbol_at_path_offset(&path, offset)
            .or_else(|| package_member_symbol_from_snapshot(analysis, &path, &snapshot));
        let Some(symbol) = symbol else {
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
            range: source_range(file, def_span),
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
                    end: source_range(
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

impl Server {
    fn package_member_hover_from_snapshot(
        &self,
        analysis: &ProjectAnalysis,
        path: &Path,
        snapshot: &DocumentSnapshot,
    ) -> Option<String> {
        package_member_symbol_from_snapshot(analysis, path, snapshot).and_then(|symbol| {
            symbol
                .signature
                .as_ref()
                .map(|_| analysis_symbol_hover_markdown(&symbol))
        })
    }
}

fn is_ignored_space_completion_trigger(
    params: &CompletionParams,
    text: &str,
    offset: usize,
    context: &cursor::CompletionContext,
) -> bool {
    if !is_space_completion_trigger(params) {
        return false;
    }
    previous_non_whitespace_char(text, offset) != Some(',')
        || !matches!(
            context,
            cursor::CompletionContext::ImportSpecifierList { .. }
                | cursor::CompletionContext::ExportList
        )
}

fn is_space_completion_trigger(params: &CompletionParams) -> bool {
    params
        .context
        .as_ref()
        .and_then(|context| context.trigger_character.as_deref())
        == Some(" ")
}

fn is_ignored_space_signature_trigger(
    params: &lsp_types::SignatureHelpParams,
    text: &str,
    offset: usize,
) -> bool {
    if !is_space_signature_trigger(params) {
        return false;
    }
    previous_non_whitespace_char(text, offset) != Some(',')
}

fn is_space_signature_trigger(params: &lsp_types::SignatureHelpParams) -> bool {
    params
        .context
        .as_ref()
        .and_then(|context| context.trigger_character.as_deref())
        == Some(" ")
}

#[cfg(test)]
mod tests;
