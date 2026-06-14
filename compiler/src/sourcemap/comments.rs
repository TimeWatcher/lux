use std::collections::BTreeMap;

use crate::source::{SourceFile, SourceSpan};

use super::{GeneratedSpan, SourceMap};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SourceCommentMode {
    None,
    Readable,
    Boundary,
    Dense,
}

impl SourceCommentMode {
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "none" | "off" | "false" => Some(Self::None),
            "readable" | "debug-readable" | "dev-readable" | "dev" => Some(Self::Readable),
            "boundary" | "boundaries" => Some(Self::Boundary),
            "dense" | "debug" | "all" | "true" => Some(Self::Dense),
            _ => None,
        }
    }

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Readable => "readable",
            Self::Boundary => "boundary",
            Self::Dense => "dense",
        }
    }
}

impl Default for SourceCommentMode {
    fn default() -> Self {
        Self::None
    }
}

pub fn with_source_comments(
    lua: &str,
    source_map: &SourceMap,
    file: &SourceFile,
    mode: SourceCommentMode,
) -> String {
    let by_line = source_comment_lines(lua, source_map, file, mode);
    if by_line.is_empty() {
        return lua.to_string();
    }

    let mut out = String::new();
    for (index, line) in lua.lines().enumerate() {
        let generated_line = index + 1;
        if let Some(span) = by_line.get(&generated_line) {
            let (source_line, _) = file.line_col(span.byte_start);
            out.push_str(&format!(
                "--#lux source: {}:{}\n",
                file.display_name(),
                source_line
            ));
        }
        out.push_str(line);
        out.push('\n');
    }
    out
}

pub fn source_comment_count(
    lua: &str,
    source_map: &SourceMap,
    file: &SourceFile,
    mode: SourceCommentMode,
) -> usize {
    source_comment_lines(lua, source_map, file, mode).len()
}

pub fn map_after_source_comments(
    lua: &str,
    source_map: &SourceMap,
    file: &SourceFile,
    mode: SourceCommentMode,
) -> SourceMap {
    let commented_lines = source_comment_lines(lua, source_map, file, mode)
        .keys()
        .copied()
        .collect::<std::collections::BTreeSet<_>>();

    let mut out = SourceMap::new();
    for mapping in source_map.mappings() {
        let inserted_before_or_at = commented_lines.range(..=mapping.generated_line).count();
        out.push(GeneratedSpan {
            generated_line: mapping.generated_line + inserted_before_or_at,
            generated_col_start: mapping.generated_col_start,
            generated_col_end: mapping.generated_col_end,
            source: mapping.source,
        });
    }
    out
}

fn source_comment_lines(
    lua: &str,
    source_map: &SourceMap,
    file: &SourceFile,
    mode: SourceCommentMode,
) -> BTreeMap<usize, SourceSpan> {
    match mode {
        SourceCommentMode::None => BTreeMap::new(),
        SourceCommentMode::Dense => source_comment_lines_dense(source_map),
        SourceCommentMode::Readable => source_comment_lines_readable(lua, source_map, file),
        SourceCommentMode::Boundary => source_comment_lines_boundary(source_map, file),
    }
}

fn source_comment_lines_dense(source_map: &SourceMap) -> BTreeMap<usize, SourceSpan> {
    let mut by_line = BTreeMap::<usize, SourceSpan>::new();
    for mapping in source_map.mappings() {
        by_line
            .entry(mapping.generated_line)
            .or_insert(mapping.source);
    }
    by_line
}

fn source_comment_lines_boundary(
    source_map: &SourceMap,
    file: &SourceFile,
) -> BTreeMap<usize, SourceSpan> {
    let mut by_line = BTreeMap::<usize, SourceSpan>::new();
    let mut last_source_line = None;

    for mapping in source_map.mappings() {
        if by_line.contains_key(&mapping.generated_line) {
            continue;
        }

        let source_line = file.line_col(mapping.source.byte_start).0;
        if last_source_line == Some(source_line) {
            continue;
        }

        by_line.insert(mapping.generated_line, mapping.source);
        last_source_line = Some(source_line);
    }

    by_line
}

fn source_comment_lines_readable(
    lua: &str,
    source_map: &SourceMap,
    file: &SourceFile,
) -> BTreeMap<usize, SourceSpan> {
    let dense = source_comment_lines_dense(source_map);
    let mut out = BTreeMap::new();
    let mut last_source_line = None;
    for (index, line) in lua.lines().enumerate() {
        let generated_line = index + 1;
        if !is_readable_source_anchor(line) {
            continue;
        }
        if let Some(span) = dense.get(&generated_line).copied() {
            let source_line = file.line_col(span.byte_start).0;
            if last_source_line == Some(source_line) {
                continue;
            }
            out.insert(generated_line, span);
            last_source_line = Some(source_line);
        }
    }
    out
}

fn is_readable_source_anchor(line: &str) -> bool {
    let line = line.trim_start();
    line == "else"
        || line == "repeat"
        || line.starts_with("if ")
        || line.starts_with("elseif ")
        || line.starts_with("for ")
        || line.starts_with("while ")
        || line.starts_with("until ")
        || line.starts_with("function ")
        || line.contains(" = function(")
}

#[cfg(test)]
mod tests {
    use crate::source::{FileId, SourceFile, SourceSpan};
    use crate::sourcemap::{GeneratedSpan, SourceMap};

    use super::{
        SourceCommentMode, map_after_source_comments, source_comment_count, with_source_comments,
    };

    #[test]
    fn inserts_source_comments_before_mapped_lines() {
        let file = SourceFile::new(0, Some("src/demo.lux".into()), "fn a() = 1\n");
        let mut map = SourceMap::new();
        map.push(GeneratedSpan {
            generated_line: 1,
            generated_col_start: 1,
            generated_col_end: 10,
            source: SourceSpan::new(FileId(0), 0, 8),
        });
        map.push(GeneratedSpan {
            generated_line: 1,
            generated_col_start: 11,
            generated_col_end: 20,
            source: SourceSpan::new(FileId(0), 0, 8),
        });

        let lua = with_source_comments("return 1\n", &map, &file, SourceCommentMode::Dense);
        assert_eq!(
            source_comment_count("return 1\n", &map, &file, SourceCommentMode::Dense),
            1
        );
        assert!(lua.starts_with("--#lux source: src/demo.lux:1\nreturn 1\n"));

        let shifted =
            map_after_source_comments("return 1\n", &map, &file, SourceCommentMode::Dense);
        assert_eq!(shifted.mappings()[0].generated_line, 2);
    }

    #[test]
    fn boundary_comments_only_track_source_line_changes() {
        let file = SourceFile::new(0, Some("src/demo.lux".into()), "fn a() = 1\nfn b() = 2\n");
        let mut map = SourceMap::new();
        map.push(GeneratedSpan {
            generated_line: 1,
            generated_col_start: 1,
            generated_col_end: 10,
            source: SourceSpan::new(FileId(0), 0, 8),
        });
        map.push(GeneratedSpan {
            generated_line: 2,
            generated_col_start: 1,
            generated_col_end: 10,
            source: SourceSpan::new(FileId(0), 0, 8),
        });
        map.push(GeneratedSpan {
            generated_line: 3,
            generated_col_start: 1,
            generated_col_end: 10,
            source: SourceSpan::new(FileId(0), 11, 19),
        });

        let lua = with_source_comments(
            "local a\na = function()\nlocal b\n",
            &map,
            &file,
            SourceCommentMode::Boundary,
        );

        assert_eq!(
            source_comment_count(
                "local a\na = function()\nlocal b\n",
                &map,
                &file,
                SourceCommentMode::Dense
            ),
            3
        );
        assert_eq!(
            source_comment_count(
                "local a\na = function()\nlocal b\n",
                &map,
                &file,
                SourceCommentMode::Boundary
            ),
            2
        );
        assert_eq!(lua.matches("--#lux source:").count(), 2);
    }

    #[test]
    fn readable_comments_only_track_review_anchors() {
        let file = SourceFile::new(0, Some("src/demo.lux".into()), "fn a() = 1\nif ok { 2 }\n");
        let lua =
            "local a\na = function()\n  local tmp = 1\n  if ok then\n    tmp = 2\n  end\nend\n";
        let mut map = SourceMap::new();
        for line in 1..=7 {
            let source_start = if line >= 4 { 11 } else { 0 };
            map.push(GeneratedSpan {
                generated_line: line,
                generated_col_start: 1,
                generated_col_end: 10,
                source: SourceSpan::new(FileId(0), source_start, source_start + 8),
            });
        }

        let commented = with_source_comments(lua, &map, &file, SourceCommentMode::Readable);
        assert_eq!(commented.matches("--#lux source:").count(), 2);
        assert!(commented.contains("--#lux source: src/demo.lux:1\na = function()"));
        assert!(commented.contains("--#lux source: src/demo.lux:2\n  if ok then"));
        assert_eq!(
            source_comment_count(lua, &map, &file, SourceCommentMode::Readable),
            2
        );
    }
}
