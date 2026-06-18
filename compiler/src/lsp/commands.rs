use std::path::PathBuf;

use crate::analysis::ProjectAnalysis;
use gmod_api_db::ApiIndex;
use lsp_types::Uri;

use super::protocol::url_to_path;
#[derive(Debug, Clone, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CommandDocumentPosition {
    pub(crate) uri: Uri,
    pub(crate) line: Option<u32>,
    pub(crate) character: Option<u32>,
}

impl CommandDocumentPosition {
    pub(crate) fn from_arguments(arguments: &[serde_json::Value]) -> Result<Option<Self>, String> {
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
pub(crate) struct InstallStdPackagesCommand {
    pub(crate) project_root: Option<PathBuf>,
    pub(crate) packages: Vec<String>,
}

impl InstallStdPackagesCommand {
    pub(crate) fn from_arguments(arguments: &[serde_json::Value]) -> Result<Self, String> {
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
pub(crate) struct CommandResult {
    pub(crate) kind: String,
    pub(crate) title: String,
    pub(crate) markdown: String,
    pub(crate) items: Vec<CommandItem>,
}

impl CommandResult {
    pub(crate) fn message(message: impl Into<String>) -> Self {
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
pub(crate) struct CommandItem {
    pub(crate) label: String,
    pub(crate) detail: String,
    pub(crate) description: String,
    pub(crate) markdown: String,
}

pub(crate) fn module_exports_command(
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

pub(crate) fn active_realm_command(
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

pub(crate) fn gmod_api_coverage_command(api: &ApiIndex) -> CommandResult {
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
