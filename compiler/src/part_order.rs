use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use crate::ast::{Module, PartOrderKind, PartOrderRelation, StmtKind};
use crate::diag::{Diagnostic, Label};

pub(crate) fn is_module_entry_path(path: &Path) -> bool {
    path.file_stem()
        .map(|stem| is_entry_stem(&stem.to_string_lossy()))
        .unwrap_or(false)
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct PartOrderInput<'a> {
    pub path: &'a Path,
    pub module: &'a Module,
    pub is_entry: bool,
}

#[derive(Debug, Clone)]
struct PartInfo {
    index: usize,
    path: PathBuf,
    key: String,
}

#[derive(Debug, Clone)]
struct PartAliases {
    canonical: BTreeMap<String, usize>,
    aliases: BTreeMap<String, Option<usize>>,
}

#[derive(Debug, Clone)]
struct Edge {
    from: usize,
    to: usize,
    span: crate::source::SourceSpan,
}

pub(crate) fn sort_module_parts(
    parts: &[PartOrderInput<'_>],
) -> Result<Vec<usize>, Vec<Diagnostic>> {
    if parts.len() <= 1 {
        return Ok((0..parts.len()).collect());
    }

    let mut diagnostics = validate_entry_parts(parts);
    let base = common_parent(parts.iter().map(|part| part.path));
    let mut infos = parts
        .iter()
        .enumerate()
        .map(|(index, part)| PartInfo {
            index,
            path: part.path.to_path_buf(),
            key: part_key(part.path, &base),
        })
        .collect::<Vec<_>>();
    infos.sort_by(|a, b| part_stable_cmp(a, b));

    let aliases = build_aliases(&infos);
    let mut edges = Vec::new();

    for (current_index, part) in parts.iter().enumerate() {
        for stmt in &part.module.body {
            let StmtKind::PartOrderDecl(decl) = &stmt.kind else {
                continue;
            };
            match &decl.kind {
                PartOrderKind::Order { .. } if !part.is_entry => {
                    diagnostics.push(
                        Diagnostic::error("complete part order must be declared in the module entry part")
                            .with_code("PART006")
                            .with_label(Label::primary(
                                stmt.span,
                                "move this `part order` declaration to `module.lux`",
                            ))
                            .with_help(
                                "the module entry part is `module.lux`, or a realm-prefixed `cl_module.lux`, `sv_module.lux`, or `sh_module.lux`",
                            ),
                    );
                    continue;
                }
                PartOrderKind::Relative { relation, target } => {
                    let Some(target_index) =
                        resolve_target(target, &aliases, stmt.span, &mut diagnostics)
                    else {
                        continue;
                    };
                    if target_index == current_index {
                        diagnostics.push(
                            Diagnostic::error("part order cannot reference the current part")
                                .with_code("PART003")
                                .with_label(Label::primary(
                                    stmt.span,
                                    "this relation points back to its own part",
                                )),
                        );
                        continue;
                    }
                    let (from, to) = match relation {
                        PartOrderRelation::Before => (current_index, target_index),
                        PartOrderRelation::After => (target_index, current_index),
                    };
                    edges.push(Edge {
                        from,
                        to,
                        span: stmt.span,
                    });
                }
                PartOrderKind::Order { targets } => {
                    let mut seen = BTreeMap::<usize, String>::new();
                    let mut resolved_targets = Vec::new();
                    for target in targets {
                        let Some(target_index) =
                            resolve_target(target, &aliases, stmt.span, &mut diagnostics)
                        else {
                            continue;
                        };
                        if let Some(previous) =
                            seen.insert(target_index, normalize_target_name(target))
                        {
                            diagnostics.push(
                                Diagnostic::error(format!(
                                    "part order lists `{previous}` more than once"
                                ))
                                .with_code("PART004")
                                .with_label(Label::primary(
                                    stmt.span,
                                    "duplicate part in this order list",
                                )),
                            );
                            continue;
                        }
                        resolved_targets.push(target_index);
                    }
                    for window in resolved_targets.windows(2) {
                        edges.push(Edge {
                            from: window[0],
                            to: window[1],
                            span: stmt.span,
                        });
                    }
                }
            }
        }
    }

    if !diagnostics.is_empty() {
        return Err(diagnostics);
    }

    topo_sort(parts.len(), &infos, &edges)
}

fn validate_entry_parts(parts: &[PartOrderInput<'_>]) -> Vec<Diagnostic> {
    if parts.len() <= 1 {
        return Vec::new();
    }
    let entries = parts
        .iter()
        .enumerate()
        .filter(|(_, part)| part.is_entry)
        .collect::<Vec<_>>();
    match entries.len() {
        1 => Vec::new(),
        0 => {
            let span = parts.first().map(|part| part.module.span).unwrap_or(
                crate::source::SourceSpan::new(crate::source::FileId(0), 0, 0),
            );
            vec![
                Diagnostic::error("multi-part module is missing an entry part")
                    .with_code("PART007")
                    .with_label(Label::primary(
                        span,
                        "this module has multiple parts but no `module.lux` entry",
                    ))
                    .with_help(
                        "add `module.lux`, or a realm-prefixed `cl_module.lux`, `sv_module.lux`, or `sh_module.lux`, and put module-level `part order` there",
                    ),
            ]
        }
        _ => entries
            .into_iter()
            .map(|(_, part)| {
                Diagnostic::error("multi-part module has more than one entry part")
                    .with_code("PART008")
                    .with_label(Label::primary(
                        part.module.span,
                        "only one module entry part is allowed",
                    ))
            })
            .collect(),
    }
}

fn part_stable_cmp(a: &PartInfo, b: &PartInfo) -> std::cmp::Ordering {
    let a_entry = is_entry_key(&a.key);
    let b_entry = is_entry_key(&b.key);
    b_entry.cmp(&a_entry).then(a.path.cmp(&b.path))
}

fn is_entry_key(key: &str) -> bool {
    Path::new(key)
        .file_name()
        .map(|stem| is_entry_stem(&stem.to_string_lossy()))
        .unwrap_or(false)
}

fn is_entry_stem(stem: &str) -> bool {
    strip_realm_prefix(stem) == "module"
}

fn strip_realm_prefix(stem: &str) -> &str {
    stem.strip_prefix("cl_")
        .or_else(|| stem.strip_prefix("sv_"))
        .or_else(|| stem.strip_prefix("sh_"))
        .unwrap_or(stem)
}

fn topo_sort(
    len: usize,
    stable_order: &[PartInfo],
    edges: &[Edge],
) -> Result<Vec<usize>, Vec<Diagnostic>> {
    let mut stable_pos = vec![0usize; len];
    for (pos, info) in stable_order.iter().enumerate() {
        stable_pos[info.index] = pos;
    }

    let mut graph = vec![BTreeSet::<usize>::new(); len];
    let mut indegree = vec![0usize; len];
    for edge in edges {
        if graph[edge.from].insert(edge.to) {
            indegree[edge.to] += 1;
        }
    }

    let mut ready = BTreeSet::<(usize, usize)>::new();
    for index in 0..len {
        if indegree[index] == 0 {
            ready.insert((stable_pos[index], index));
        }
    }

    let mut out = Vec::with_capacity(len);
    while let Some((_, index)) = ready.pop_first() {
        out.push(index);
        for next in graph[index].clone() {
            indegree[next] -= 1;
            if indegree[next] == 0 {
                ready.insert((stable_pos[next], next));
            }
        }
    }

    if out.len() == len {
        return Ok(out);
    }

    let cycle_nodes = indegree
        .iter()
        .enumerate()
        .filter_map(|(index, degree)| (*degree > 0).then_some(index))
        .collect::<BTreeSet<_>>();
    let label_span = edges
        .iter()
        .find(|edge| cycle_nodes.contains(&edge.from) && cycle_nodes.contains(&edge.to))
        .or_else(|| edges.first())
        .map(|edge| edge.span);
    let mut diagnostic = Diagnostic::error("part order cycle detected").with_code("PART005");
    if let Some(span) = label_span {
        diagnostic = diagnostic.with_label(Label::primary(
            span,
            "this order constraint participates in a cycle",
        ));
    }
    Err(vec![diagnostic.with_note(format!(
        "cycle includes: {}",
        stable_order
            .iter()
            .filter(|info| cycle_nodes.contains(&info.index))
            .map(|info| info.key.as_str())
            .collect::<Vec<_>>()
            .join(", ")
    ))])
}

fn resolve_target(
    target: &str,
    aliases: &PartAliases,
    span: crate::source::SourceSpan,
    diagnostics: &mut Vec<Diagnostic>,
) -> Option<usize> {
    let normalized = normalize_target_name(target);
    if let Some(index) = aliases.canonical.get(&normalized) {
        return Some(*index);
    }
    match aliases.aliases.get(&normalized).copied().flatten() {
        Some(index) => Some(index),
        None if aliases.aliases.contains_key(&normalized) => {
            diagnostics.push(
                Diagnostic::error(format!("part order target `{target}` is ambiguous"))
                    .with_code("PART002")
                    .with_label(Label::primary(
                        span,
                        "use a path relative to the module directory",
                    )),
            );
            None
        }
        None => {
            diagnostics.push(
                Diagnostic::error(format!("part order target `{target}` was not found"))
                    .with_code("PART001")
                    .with_label(Label::primary(span, "unknown part in this module"))
                    .with_help(
                        "use the part path without the `.lux` extension, such as `client/view`",
                    ),
            );
            None
        }
    }
}

fn build_aliases(infos: &[PartInfo]) -> PartAliases {
    let mut canonical = BTreeMap::new();
    let mut aliases = BTreeMap::<String, Option<usize>>::new();
    for info in infos {
        canonical.insert(info.key.clone(), info.index);
        insert_alias(&mut aliases, info.key.clone(), info.index);
        if let Some(stem) = Path::new(&info.key)
            .file_name()
            .map(|stem| stem.to_string_lossy().to_string())
        {
            insert_alias(&mut aliases, stem, info.index);
        }
    }
    PartAliases { canonical, aliases }
}

fn insert_alias(aliases: &mut BTreeMap<String, Option<usize>>, alias: String, index: usize) {
    aliases
        .entry(alias)
        .and_modify(|current| {
            if *current != Some(index) {
                *current = None;
            }
        })
        .or_insert(Some(index));
}

fn common_parent<'a>(mut paths: impl Iterator<Item = &'a Path>) -> PathBuf {
    let Some(first) = paths.next() else {
        return PathBuf::new();
    };
    let mut base = first.parent().unwrap_or(first).to_path_buf();
    for path in paths {
        let parent = path.parent().unwrap_or(path);
        while !parent.starts_with(&base) {
            if !base.pop() {
                break;
            }
        }
    }
    base
}

fn part_key(path: &Path, base: &Path) -> String {
    let rel = path.strip_prefix(base).unwrap_or(path);
    let mut parts = rel
        .components()
        .filter_map(|component| match component {
            std::path::Component::Normal(part) => Some(part.to_string_lossy().to_string()),
            _ => None,
        })
        .collect::<Vec<_>>();
    if let Some(last) = parts.last_mut() {
        if let Some(stripped) = last.strip_suffix(".lux") {
            *last = stripped.to_string();
        }
    }
    normalize_target_name(&parts.join("/"))
}

fn normalize_target_name(target: &str) -> String {
    let mut target = target.replace('\\', "/");
    while let Some(rest) = target.strip_prefix("./") {
        target = rest.to_string();
    }
    target
        .strip_suffix(".lux")
        .unwrap_or(&target)
        .trim_matches('/')
        .to_string()
}
