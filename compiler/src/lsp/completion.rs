use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use crate::analysis::{CompletionCandidate, CompletionCandidateKind, ProjectAnalysis};
use crate::module::RealmSet;
use crate::source::SourceFile;
use gmod_api_db::ApiIndex;
use lsp_types::{
    CompletionItem, CompletionItemKind, CompletionItemLabelDetails, Documentation,
    InsertTextFormat, MarkupContent, MarkupKind,
};

use super::cursor::{CompletionContext, identifier_prefix, member_path_from_prefix};
use super::gmod_api::{
    api_completion_candidates, api_completion_label_details, api_entry_completion_item,
    api_root_completion_candidates, completion_tags_for_api, gmod_completion_path,
};
use super::lexical_completion::{lexical_binding_completions, module_part_lexical_completions};

pub(crate) struct CompletionInput<'a> {
    pub(crate) context: CompletionContext,
    pub(crate) analysis: Option<&'a ProjectAnalysis>,
    pub(crate) path: Option<&'a Path>,
    pub(crate) offset: usize,
    pub(crate) line_prefix: &'a str,
    pub(crate) current_file: &'a SourceFile,
    pub(crate) gmod_api: &'a ApiIndex,
}

pub(crate) fn completion_items(input: CompletionInput<'_>) -> Vec<CompletionItem> {
    match input.context {
        CompletionContext::ImportSource => input
            .analysis
            .map(ProjectAnalysis::module_path_completions)
            .unwrap_or_default()
            .into_iter()
            .map(completion_item)
            .collect(),
        CompletionContext::ImportSpecifierList { source } => input
            .analysis
            .zip(input.path)
            .map(|(analysis, path)| {
                let active_realms = analysis
                    .active_realms_at_path_offset(path, input.offset)
                    .unwrap_or(RealmSet::SHARED);
                match source.as_deref() {
                    Some(source) => analysis.importable_exports(path, source, active_realms),
                    None => analysis.importable_exports_for_all_sources(path, active_realms),
                }
            })
            .unwrap_or_default()
            .into_iter()
            .map(|candidate| import_completion_item(candidate, source.is_none()))
            .collect(),
        CompletionContext::ExportList => input
            .analysis
            .zip(input.path)
            .map(|(analysis, path)| analysis.exportable_bindings(path))
            .unwrap_or_default()
            .into_iter()
            .map(completion_item)
            .collect(),
        CompletionContext::ApiMember { prefix } => {
            let namespace_items = namespace_member_completion_items(
                input.analysis,
                input.path,
                input.offset,
                &prefix,
            );
            if namespace_items.is_empty() {
                api_completion_candidates(
                    input.gmod_api,
                    &prefix,
                    (!input.current_file.text.is_empty())
                        .then_some(input.current_file.text.as_str()),
                )
            } else {
                namespace_items
            }
        }
        CompletionContext::General => general_completion_items(input),
    }
}

fn general_completion_items(input: CompletionInput<'_>) -> Vec<CompletionItem> {
    let current_prefix = identifier_prefix(input.line_prefix);
    let mut items = input
        .analysis
        .zip(input.path)
        .map(|(analysis, path)| {
            general_binding_completions(analysis, path, input.offset, input.current_file)
        })
        .unwrap_or_else(|| lexical_binding_completions(input.current_file, input.offset))
        .into_iter()
        .map(completion_item)
        .collect::<Vec<_>>();
    let mut existing_labels = items
        .iter()
        .map(|item| item.label.clone())
        .collect::<BTreeSet<_>>();
    let fallback = lexical_binding_completions(input.current_file, input.offset);
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
        api_root_completion_candidates(input.gmod_api, current_prefix)
            .into_iter()
            .filter(|item| !existing_labels.contains(&item.label)),
    );
    items
}

pub(crate) fn resolve_completion_item(api: &ApiIndex, mut item: CompletionItem) -> CompletionItem {
    if let Some(path) = gmod_completion_path(&item)
        && let Some(entry) = api.entry(path)
    {
        let resolved = api_entry_completion_item(entry);
        item.detail = resolved.detail;
        item.documentation = resolved.documentation;
        item.label_details = api_completion_label_details(entry, &item.label);
        item.tags = completion_tags_for_api(entry);
    }
    item
}

pub(crate) fn completion_item(candidate: CompletionCandidate) -> CompletionItem {
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

pub(crate) fn import_completion_item(
    candidate: CompletionCandidate,
    needs_source: bool,
) -> CompletionItem {
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

pub(crate) fn keyword_completion_items(prefix: &str) -> Vec<CompletionItem> {
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

pub(crate) fn general_binding_completions(
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

pub(crate) fn namespace_member_completion_items(
    analysis: Option<&ProjectAnalysis>,
    path: Option<&Path>,
    offset: usize,
    prefix: &str,
) -> Vec<CompletionItem> {
    let Some(member_path) = member_path_from_prefix(prefix) else {
        return Vec::new();
    };
    analysis
        .zip(path)
        .map(|(analysis, document_path)| {
            analysis.member_path_completions(document_path, offset, &member_path)
        })
        .unwrap_or_default()
        .into_iter()
        .map(completion_item)
        .collect()
}

fn markdown_documentation(value: String) -> Documentation {
    Documentation::MarkupContent(MarkupContent {
        kind: MarkupKind::Markdown,
        value,
    })
}
