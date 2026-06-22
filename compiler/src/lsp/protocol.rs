use std::path::{Path, PathBuf};

use crate::analysis::{
    AnalysisPosition, AnalysisRange, AnalysisSemanticToken, AnalysisSignatureHelp,
    SemanticTokenKind,
};
use crate::source::{SourceFile, SourceSpan};
use lsp_types::{
    CompletionOptions, Documentation, ExecuteCommandOptions, Hover, HoverContents, MarkupContent,
    MarkupKind, OneOf, ParameterInformation, ParameterLabel, Position, Range, SemanticToken,
    SemanticTokenType, SemanticTokensLegend, SemanticTokensOptions, ServerCapabilities,
    SignatureHelp, SignatureHelpOptions, SignatureInformation, TextDocumentSyncCapability,
    TextDocumentSyncKind, Uri, WorkDoneProgressOptions,
};
use url::Url;

pub(crate) const INSTALL_STD_PACKAGES_COMMAND: &str = "lux.installStdPackages";

pub(crate) fn server_capabilities() -> ServerCapabilities {
    ServerCapabilities {
        text_document_sync: Some(TextDocumentSyncCapability::Kind(TextDocumentSyncKind::FULL)),
        hover_provider: Some(lsp_types::HoverProviderCapability::Simple(true)),
        completion_provider: Some(CompletionOptions {
            resolve_provider: Some(true),
            trigger_characters: Some(vec![
                ".".into(),
                ":".into(),
                "{".into(),
                ",".into(),
                " ".into(),
                "\"".into(),
            ]),
            all_commit_characters: None,
            work_done_progress_options: WorkDoneProgressOptions::default(),
            completion_item: Some(lsp_types::CompletionOptionsCompletionItem {
                label_details_support: Some(true),
            }),
        }),
        signature_help_provider: Some(SignatureHelpOptions {
            trigger_characters: Some(vec![
                "(".into(),
                ",".into(),
                " ".into(),
                "\"".into(),
                ">".into(),
            ]),
            retrigger_characters: Some(vec![",".into(), " ".into(), ">".into()]),
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

pub(crate) fn markdown_hover(markdown: String) -> Hover {
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

pub(crate) fn encode_semantic_tokens(
    file: &SourceFile,
    mut tokens: Vec<AnalysisSemanticToken>,
) -> Vec<SemanticToken> {
    tokens.sort_by_key(|token| {
        let token_range = source_range(file, token.span);
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
        let token_range = source_range(file, token.span);
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

pub(crate) fn source_range(file: &SourceFile, span: SourceSpan) -> Range {
    let analysis_range = AnalysisRange {
        start: {
            let (line, col) = file.line_col_utf16(span.byte_start);
            AnalysisPosition {
                line: line.saturating_sub(1) as u32,
                character: col.saturating_sub(1) as u32,
            }
        },
        end: {
            let (line, col) = file.line_col_utf16(span.byte_end);
            AnalysisPosition {
                line: line.saturating_sub(1) as u32,
                character: col.saturating_sub(1) as u32,
            }
        },
    };
    lsp_range(analysis_range)
}

pub(crate) fn lsp_range(range: AnalysisRange) -> Range {
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

pub(crate) fn signature_help_from_analysis(help: AnalysisSignatureHelp) -> SignatureHelp {
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
                        documentation: parameter.documentation.map(markdown_documentation).or_else(
                            || {
                                parameter
                                    .optional
                                    .then(|| Documentation::String("optional".into()))
                            },
                        ),
                    })
                    .collect(),
            ),
            active_parameter: None,
        }],
        active_signature: Some(0),
        active_parameter,
    }
}

pub(crate) fn json_result<T: serde::Serialize>(value: T) -> Result<serde_json::Value, String> {
    serde_json::to_value(value).map_err(|err| format!("failed to encode LSP result: {err}"))
}

pub(crate) fn url_to_path(uri: &Uri) -> Option<PathBuf> {
    let parsed = Url::parse(uri.as_str()).ok()?;
    parsed.to_file_path().ok()
}

pub(crate) fn document_uri_key(uri: &Uri) -> Uri {
    url_to_path(uri)
        .as_deref()
        .and_then(path_to_url)
        .unwrap_or_else(|| uri.clone())
}

pub(crate) fn path_to_url(path: &Path) -> Option<Uri> {
    Url::from_file_path(path).ok().map(uri_from_url)
}

pub(crate) fn uri_from_url(url: Url) -> Uri {
    url.as_str()
        .parse()
        .expect("file URL should be a valid URI")
}
