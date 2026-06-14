use std::fs;
use std::path::Path;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeSourceLocation {
    pub source_file: Option<String>,
    pub source_line: Option<usize>,
    pub source_column: Option<usize>,
}

pub fn map_generated_line(
    map_path: impl AsRef<Path>,
    generated_line: usize,
) -> Result<Option<RuntimeSourceLocation>, String> {
    let text = fs::read_to_string(map_path.as_ref())
        .map_err(|err| format!("failed to read {}: {err}", map_path.as_ref().display()))?;
    Ok(find_mapping(&text, generated_line))
}

fn find_mapping(text: &str, generated_line: usize) -> Option<RuntimeSourceLocation> {
    let needle = format!("\"generated\": {{ \"line\": {generated_line},");
    let start = text.find(&needle)?;
    let rest = &text[start..];
    let source_start = rest.find("\"source\"")?;
    let source = &rest[source_start..rest.find("\n    }").unwrap_or(rest.len())];
    Some(RuntimeSourceLocation {
        source_file: extract_json_string(source, "\"file\": "),
        source_line: extract_json_usize(source, "\"line\": "),
        source_column: extract_json_usize(source, "\"column\": "),
    })
}

fn extract_json_string(text: &str, key: &str) -> Option<String> {
    let start = text.find(key)? + key.len();
    let rest = text[start..].trim_start();
    if rest.starts_with("null") {
        return None;
    }
    let rest = rest.strip_prefix('"')?;
    let end = rest.find('"')?;
    Some(rest[..end].replace("\\\\", "\\").replace("\\\"", "\""))
}

fn extract_json_usize(text: &str, key: &str) -> Option<usize> {
    let start = text.find(key)? + key.len();
    let rest = text[start..].trim_start();
    if rest.starts_with("null") {
        return None;
    }
    let digits = rest
        .chars()
        .take_while(|ch| ch.is_ascii_digit())
        .collect::<String>();
    digits.parse().ok()
}

#[cfg(test)]
mod tests {
    use super::find_mapping;

    #[test]
    fn maps_generated_line_to_source_location() {
        let json = r#"
{
  "version": 1,
  "mappings": [
    {
      "generated": { "line": 12, "columnStart": 1, "columnEnd": 4 },
      "source": { "fileId": 0, "file": "src\\main.lux", "byteStart": 0, "byteEnd": 3, "line": 2, "column": 5 }
    }
  ]
}
"#;
        let found = find_mapping(json, 12).expect("mapping");
        assert_eq!(found.source_file.as_deref(), Some("src\\main.lux"));
        assert_eq!(found.source_line, Some(2));
        assert_eq!(found.source_column, Some(5));
    }
}
