use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use super::{FileId, SourceSpan};

#[derive(Debug, Clone)]
pub struct SourceFile {
    pub id: FileId,
    pub path: Option<PathBuf>,
    pub text: String,
    line_starts: Vec<usize>,
}

impl SourceFile {
    pub fn new(id: u32, path: Option<PathBuf>, text: impl Into<String>) -> Self {
        let text = text.into();
        let line_starts = compute_line_starts(&text);
        Self {
            id: FileId(id),
            path,
            text,
            line_starts,
        }
    }

    pub fn load(id: u32, path: impl AsRef<Path>) -> io::Result<Self> {
        let path = path.as_ref();
        let text = fs::read_to_string(path)?;
        Ok(Self::new(id, Some(path.to_path_buf()), text))
    }

    pub fn display_name(&self) -> String {
        self.path
            .as_ref()
            .map(|path| path.display().to_string())
            .unwrap_or_else(|| "<memory>".to_string())
    }

    pub fn slice(&self, span: SourceSpan) -> &str {
        &self.text[span.byte_start..span.byte_end]
    }

    pub fn line_col(&self, offset: usize) -> (usize, usize) {
        let clamped = offset.min(self.text.len());
        let index = match self.line_starts.binary_search(&clamped) {
            Ok(index) => index,
            Err(next) => next.saturating_sub(1),
        };

        let line_start = self.line_starts[index];
        (index + 1, clamped - line_start + 1)
    }

    pub fn line_text(&self, one_based_line: usize) -> Option<&str> {
        let index = one_based_line.checked_sub(1)?;
        let start = *self.line_starts.get(index)?;
        let end = self
            .line_starts
            .get(index + 1)
            .copied()
            .unwrap_or(self.text.len());

        let mut slice = &self.text[start..end];
        if let Some(stripped) = slice.strip_suffix('\n') {
            slice = stripped;
        }
        if let Some(stripped) = slice.strip_suffix('\r') {
            slice = stripped;
        }
        Some(slice)
    }
}

fn compute_line_starts(text: &str) -> Vec<usize> {
    let mut line_starts = vec![0];
    for (index, byte) in text.bytes().enumerate() {
        if byte == b'\n' {
            line_starts.push(index + 1);
        }
    }
    line_starts
}

#[cfg(test)]
mod tests {
    use super::SourceFile;

    #[test]
    fn line_and_column_are_one_based() {
        let file = SourceFile::new(0, None, "a\nbc\n");
        assert_eq!(file.line_col(0), (1, 1));
        assert_eq!(file.line_col(2), (2, 1));
        assert_eq!(file.line_col(3), (2, 2));
        assert_eq!(file.line_col(5), (3, 1));
    }
}
