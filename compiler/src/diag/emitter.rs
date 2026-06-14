use std::fmt::Write;

use crate::source::{SourceFile, SourceSpan};

use super::{Diagnostic, LabelStyle};

pub struct DiagnosticEmitter;

impl DiagnosticEmitter {
    pub fn render(diagnostic: &Diagnostic, file: &SourceFile) -> String {
        let mut out = String::new();
        let code = diagnostic
            .code
            .as_deref()
            .map(|code| format!("[{code}]"))
            .unwrap_or_default();

        let _ = writeln!(
            out,
            "{}{}: {}",
            diagnostic.severity.as_str(),
            code,
            diagnostic.message
        );

        if let Some(label) = diagnostic.labels.first() {
            let (line, col) = file.line_col(label.span.byte_start);
            let _ = writeln!(out, " --> {}:{line}:{col}", file.display_name());
        }

        for label in &diagnostic.labels {
            Self::render_label(&mut out, label.style, label.span, &label.message, file);
        }

        for note in &diagnostic.notes {
            let _ = writeln!(out, " note: {note}");
        }

        if let Some(help) = &diagnostic.help {
            let _ = writeln!(out, " help: {help}");
        }

        out.trim_end().to_string()
    }

    fn render_label(
        out: &mut String,
        style: LabelStyle,
        span: SourceSpan,
        message: &str,
        file: &SourceFile,
    ) {
        let (line, col) = file.line_col(span.byte_start);
        let source = file.line_text(line).unwrap_or("");
        let marker_len = marker_len(span, source, file, line, col);
        let caret = match style {
            LabelStyle::Primary => '^',
            LabelStyle::Secondary => '-',
        };

        let _ = writeln!(out, "  |");
        let _ = writeln!(out, "{line:>2} | {source}");
        let _ = writeln!(
            out,
            "  | {padding}{markers} {message}",
            padding = " ".repeat(col.saturating_sub(1)),
            markers = caret.to_string().repeat(marker_len),
        );
    }
}

fn marker_len(
    span: SourceSpan,
    source: &str,
    file: &SourceFile,
    _line: usize,
    col: usize,
) -> usize {
    let (_, end_col) = file.line_col(span.byte_end.max(span.byte_start + 1));
    if span.is_empty() || span.byte_start == span.byte_end {
        return 1;
    }

    let max_line_len = source.chars().count().saturating_sub(col.saturating_sub(1));
    let same_line_len = end_col.saturating_sub(col).max(1);
    same_line_len.min(max_line_len.max(1))
}
