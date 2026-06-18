use crate::analysis::AnalysisDiagnostic;
use crate::source::SourceFile;
use lsp_types::TextDocumentContentChangeEvent;

pub(crate) fn debug_log(message: impl AsRef<str>) {
    if std::env::var_os("LUXC_LSP_DEBUG").is_some() {
        eprintln!("[luxc-lsp-debug] {}", message.as_ref());
    }
}

pub(crate) fn document_change_summary(changes: &[TextDocumentContentChangeEvent]) -> String {
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

pub(crate) fn diagnostic_summary(diagnostics: &[AnalysisDiagnostic]) -> String {
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

pub(crate) fn focus_lines(text: &str) -> String {
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

pub(crate) fn apply_document_changes(
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
