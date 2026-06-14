use crate::source::SourceSpan;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Severity {
    Error,
    Warning,
    Note,
}

impl Severity {
    pub const fn as_str(self) -> &'static str {
        match self {
            Severity::Error => "error",
            Severity::Warning => "warning",
            Severity::Note => "note",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LabelStyle {
    Primary,
    Secondary,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Label {
    pub style: LabelStyle,
    pub span: SourceSpan,
    pub message: String,
}

impl Label {
    pub fn primary(span: SourceSpan, message: impl Into<String>) -> Self {
        Self {
            style: LabelStyle::Primary,
            span,
            message: message.into(),
        }
    }

    pub fn secondary(span: SourceSpan, message: impl Into<String>) -> Self {
        Self {
            style: LabelStyle::Secondary,
            span,
            message: message.into(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Diagnostic {
    pub severity: Severity,
    pub code: Option<String>,
    pub message: String,
    pub labels: Vec<Label>,
    pub notes: Vec<String>,
    pub help: Option<String>,
}

impl Diagnostic {
    pub fn new(severity: Severity, message: impl Into<String>) -> Self {
        Self {
            severity,
            code: None,
            message: message.into(),
            labels: Vec::new(),
            notes: Vec::new(),
            help: None,
        }
    }

    pub fn error(message: impl Into<String>) -> Self {
        Self::new(Severity::Error, message)
    }

    pub fn with_code(mut self, code: impl Into<String>) -> Self {
        self.code = Some(code.into());
        self
    }

    pub fn with_label(mut self, label: Label) -> Self {
        self.labels.push(label);
        self
    }

    pub fn with_note(mut self, note: impl Into<String>) -> Self {
        self.notes.push(note.into());
        self
    }

    pub fn with_help(mut self, help: impl Into<String>) -> Self {
        self.help = Some(help.into());
        self
    }
}
