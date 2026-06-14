use crate::ir::Origin;

use super::{GeneratedSpan, SourceMap};

#[derive(Debug, Clone)]
pub struct LuaWriter {
    output: String,
    source_map: SourceMap,
    indent: usize,
    line: usize,
}

impl Default for LuaWriter {
    fn default() -> Self {
        Self {
            output: String::new(),
            source_map: SourceMap::new(),
            indent: 0,
            line: 1,
        }
    }
}

impl LuaWriter {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn indent(&mut self) {
        self.indent += 1;
    }

    pub fn dedent(&mut self) {
        self.indent = self.indent.saturating_sub(1);
    }

    pub fn line(&mut self, text: impl AsRef<str>, origin: Option<&Origin>) {
        let text = text.as_ref();
        if text.contains('\n') {
            let mut parts = text.split('\n').peekable();
            while let Some(part) = parts.next() {
                if part.is_empty() && parts.peek().is_none() {
                    break;
                }
                self.line(part, origin);
            }
            return;
        }

        if text.is_empty() {
            self.output.push('\n');
            self.line += 1;
            return;
        }

        let col_start = self.indent * 2 + 1;
        for _ in 0..self.indent {
            self.output.push_str("  ");
        }
        self.output.push_str(text);
        self.output.push('\n');

        if let Some(origin) = origin {
            self.source_map.push(GeneratedSpan {
                generated_line: self.line,
                generated_col_start: col_start,
                generated_col_end: col_start + text.len(),
                source: origin.span(),
            });
        }

        self.line += 1;
    }

    pub fn source_comment(&mut self, origin: &Origin, display_path: &str, line: usize) {
        self.line(
            format!("--#lux source: {display_path}:{line}"),
            Some(origin),
        );
    }

    pub fn finish(self) -> (String, SourceMap) {
        (self.output, self.source_map)
    }
}

#[cfg(test)]
mod tests {
    use crate::ir::Origin;
    use crate::source::{FileId, SourceSpan};

    use super::LuaWriter;

    #[test]
    fn records_generated_line_mapping() {
        let mut writer = LuaWriter::new();
        let origin = Origin::source(SourceSpan::new(FileId(0), 10, 20));
        writer.line("return 1", Some(&origin));
        let (_lua, map) = writer.finish();

        assert_eq!(map.mappings().len(), 1);
        assert_eq!(map.mappings()[0].generated_line, 1);
        assert_eq!(map.mappings()[0].source.byte_start, 10);
    }
}
