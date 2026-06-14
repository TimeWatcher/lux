use crate::source::{FileId, SourceFile, SourceSpan};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GeneratedSpan {
    pub generated_line: usize,
    pub generated_col_start: usize,
    pub generated_col_end: usize,
    pub source: SourceSpan,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SourceMap {
    mappings: Vec<GeneratedSpan>,
}

impl SourceMap {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn push(&mut self, mapping: GeneratedSpan) {
        self.mappings.push(mapping);
    }

    pub fn mappings(&self) -> &[GeneratedSpan] {
        &self.mappings
    }

    pub fn is_empty(&self) -> bool {
        self.mappings.is_empty()
    }

    pub fn shifted(&self, line_delta: usize, column_delta: usize) -> Self {
        let mappings = self
            .mappings
            .iter()
            .map(|mapping| GeneratedSpan {
                generated_line: mapping.generated_line + line_delta,
                generated_col_start: mapping.generated_col_start + column_delta,
                generated_col_end: mapping.generated_col_end + column_delta,
                source: mapping.source,
            })
            .collect();
        Self { mappings }
    }

    pub fn to_json(&self, sources: &[&SourceFile]) -> String {
        let mut out = String::new();
        out.push_str("{\n");
        out.push_str("  \"version\": 1,\n");
        out.push_str("  \"mappings\": [\n");

        for (index, mapping) in self.mappings.iter().enumerate() {
            let source = find_source(sources, mapping.source.file_id);
            let (source_file, source_line, source_col) = if let Some(source) = source {
                let (line, col) = source.line_col(mapping.source.byte_start);
                (
                    format!("\"{}\"", escape_json(&source.display_name())),
                    line.to_string(),
                    col.to_string(),
                )
            } else {
                ("null".into(), "null".into(), "null".into())
            };

            out.push_str("    {\n");
            out.push_str(&format!(
                "      \"generated\": {{ \"line\": {}, \"columnStart\": {}, \"columnEnd\": {} }},\n",
                mapping.generated_line, mapping.generated_col_start, mapping.generated_col_end
            ));
            out.push_str(&format!(
                "      \"source\": {{ \"fileId\": {}, \"file\": {}, \"byteStart\": {}, \"byteEnd\": {}, \"line\": {}, \"column\": {} }}\n",
                mapping.source.file_id.0,
                source_file,
                mapping.source.byte_start,
                mapping.source.byte_end,
                source_line,
                source_col
            ));
            out.push_str("    }");
            if index + 1 != self.mappings.len() {
                out.push(',');
            }
            out.push('\n');
        }

        out.push_str("  ]\n");
        out.push_str("}\n");
        out
    }
}

fn find_source<'a>(sources: &'a [&SourceFile], file_id: FileId) -> Option<&'a SourceFile> {
    sources.iter().copied().find(|source| source.id == file_id)
}

fn escape_json(value: &str) -> String {
    let mut out = String::new();
    for ch in value.chars() {
        match ch {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if c.is_control() => out.push_str(&format!("\\u{:04x}", c as u32)),
            other => out.push(other),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use crate::source::{FileId, SourceFile, SourceSpan};

    use super::{GeneratedSpan, SourceMap};

    #[test]
    fn serializes_source_map_json_with_source_positions() {
        let file = SourceFile::new(7, Some("src/foo.lux".into()), "one\ntwo\n");
        let mut map = SourceMap::new();
        map.push(GeneratedSpan {
            generated_line: 3,
            generated_col_start: 1,
            generated_col_end: 8,
            source: SourceSpan::new(FileId(7), 4, 7),
        });

        let json = map.to_json(&[&file]);
        assert!(json.contains("\"version\": 1"));
        assert!(json.contains("\"fileId\": 7"));
        assert!(json.contains("\"line\": 2"));
    }
}
