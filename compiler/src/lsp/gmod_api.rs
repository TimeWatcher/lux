use std::collections::HashMap;
use std::path::Path;

use crate::analysis::{CompletionCandidateKind, ProjectAnalysis};
use crate::module::RealmAvailability;
use crate::source::SourceFile;
use gmod_api_db::{ApiIndex, entry_markdown, hook_markdown};
use lsp_types::{
    CompletionItem, CompletionItemKind, CompletionItemLabelDetails, CompletionItemTag,
    Documentation, InsertTextFormat, MarkupContent, MarkupKind, ParameterInformation,
    ParameterLabel, SignatureHelp, SignatureInformation,
};

pub(crate) fn external_api_hover_markdown(
    analysis: &ProjectAnalysis,
    api: &ApiIndex,
    path: &Path,
    offset: usize,
) -> Option<String> {
    let file = analysis.file_by_path(path)?;
    let symbol = analysis.symbol_at_path_offset(path, offset)?;
    let external = symbol.external_availability.as_ref()?;
    if matches!(external, RealmAvailability::UnknownExternal) {
        return None;
    }
    let api_name = symbol.external_name.as_deref().unwrap_or(&symbol.name);
    api.entry(api_name)
        .or_else(|| api_hover_entry_from_text(api, &file.text, offset))
        .map(entry_markdown)
}

pub(crate) fn hook_hover_markdown_from_text(
    api: &ApiIndex,
    text: &str,
    offset: usize,
) -> Option<String> {
    let hook_name = hook_name_at_offset(text, offset)?;
    api.hook(&hook_name).map(hook_markdown)
}

pub(crate) fn api_hover_markdown_from_text(
    api: &ApiIndex,
    text: &str,
    offset: usize,
) -> Option<String> {
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

pub(crate) fn hook_name_at_offset(text: &str, offset: usize) -> Option<String> {
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

pub(crate) fn signature_help_at(
    file: &SourceFile,
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

pub(crate) fn method_path_at_offset(text: &str, offset: usize) -> Option<String> {
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

pub(crate) fn api_path_at_offset(text: &str, offset: usize) -> Option<String> {
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

pub(crate) fn resolve_typed_method_path(
    api: &ApiIndex,
    facts: &GmodTypeFacts,
    path: &str,
) -> Option<String> {
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
                    documentation: Some(Documentation::String(format!(
                        "{} - {}",
                        parameter.ty, parameter.description
                    ))),
                })
                .collect(),
        ),
        active_parameter: None,
    }
}

pub(crate) fn api_root_completion_candidates(
    api: &ApiIndex,
    typed_prefix: &str,
) -> Vec<CompletionItem> {
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

pub(crate) fn api_completion_candidates(
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

pub(crate) fn infer_receiver_class(text: &str, receiver: &str) -> Option<String> {
    GmodTypeFacts::from_text(text).receiver_class(receiver)
}

#[derive(Debug, Default)]
pub(crate) struct GmodTypeFacts {
    locals: HashMap<String, String>,
    functions: HashMap<String, String>,
}

impl GmodTypeFacts {
    pub(crate) fn from_text(text: &str) -> Self {
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

pub(crate) fn api_entry_completion_item(entry: &gmod_api_db::ApiEntry) -> CompletionItem {
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

pub(crate) fn api_completion_detail(entry: &gmod_api_db::ApiEntry) -> String {
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

pub(crate) fn api_completion_label_details(
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

pub(crate) fn completion_tags_for_api(
    entry: &gmod_api_db::ApiEntry,
) -> Option<Vec<CompletionItemTag>> {
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

pub(crate) fn gmod_completion_path(item: &CompletionItem) -> Option<&str> {
    let data = item.data.as_ref()?;
    if data.get("lux")?.as_str()? != "gmodApi" {
        return None;
    }
    data.get("path")?.as_str()
}

pub(crate) fn completion_kind_for_api(kind: gmod_api_db::ApiKind) -> CompletionCandidateKind {
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

fn markdown_documentation(value: String) -> Documentation {
    Documentation::MarkupContent(MarkupContent {
        kind: MarkupKind::Markdown,
        value,
    })
}
