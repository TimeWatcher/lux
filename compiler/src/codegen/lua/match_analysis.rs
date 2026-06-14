use std::collections::BTreeSet;

use crate::ir::*;

use super::IrEnumCatalog;

pub(super) fn join_conditions(conditions: Vec<String>) -> Option<String> {
    let conditions = conditions
        .into_iter()
        .filter(|condition| !condition.is_empty())
        .collect::<Vec<_>>();
    if conditions.is_empty() {
        None
    } else {
        Some(conditions.join(" and "))
    }
}

pub(super) fn ir_enum_payload_fields(payload: &IrEnumVariantPayload) -> Vec<String> {
    match payload {
        IrEnumVariantPayload::None => Vec::new(),
        IrEnumVariantPayload::Tuple(fields) | IrEnumVariantPayload::Record(fields) => {
            fields.clone()
        }
    }
}

pub(super) fn collect_match_tag_field(
    enums: &IrEnumCatalog<'_>,
    pattern: &IrMatchPattern,
    out: &mut Option<String>,
) {
    match &pattern.kind {
        IrMatchPatternKind::Or(patterns) => {
            for pattern in patterns {
                collect_match_tag_field(enums, pattern, out);
            }
        }
        IrMatchPatternKind::Variant { path, .. } => {
            let Some((enum_decl, _)) = enums.lookup_variant(path) else {
                return;
            };
            let field = match &enum_decl.repr {
                IrEnumRepr::Table { tag_field } | IrEnumRepr::Existing { tag_field } => tag_field,
                IrEnumRepr::String | IrEnumRepr::Number => return,
            };
            if out.as_ref().map(|current| current == field).unwrap_or(true) {
                *out = Some(field.clone());
            }
        }
        IrMatchPatternKind::Object(fields) => {
            for field in fields {
                collect_match_tag_field(enums, &field.pattern, out);
            }
        }
        IrMatchPatternKind::Array(items) => {
            for item in items {
                collect_match_tag_field(enums, &item.pattern, out);
            }
        }
        IrMatchPatternKind::Wildcard
        | IrMatchPatternKind::Binding(_)
        | IrMatchPatternKind::Literal(_) => {}
    }
}

pub(super) fn reachable_match_arms<'a>(
    enums: &IrEnumCatalog<'_>,
    arms: &'a [IrMatchArm],
) -> Vec<&'a IrMatchArm> {
    let mut out = Vec::new();
    let mut seen_unconditional = false;
    let mut seen_literals = BTreeSet::<String>::new();
    let mut enum_name: Option<String> = None;
    let mut covered_variants = BTreeSet::<String>::new();
    let mut mixed_or_opaque = false;

    for arm in arms {
        if seen_unconditional {
            continue;
        }
        if !mixed_or_opaque
            && let Some(name) = enum_name.as_deref()
            && ir_enum_is_fully_covered(enums.by_name.get(name).copied(), &covered_variants)
        {
            continue;
        }

        let units = ir_match_pattern_coverage_units(enums, &arm.pattern);
        let mut reachable = false;
        let mut arm_unconditional = false;

        for unit in units {
            match unit {
                IrMatchCoverageUnit::Unconditional => {
                    reachable = true;
                    arm_unconditional = true;
                }
                IrMatchCoverageUnit::Literal(key) => {
                    mixed_or_opaque = true;
                    if seen_literals.insert(key) {
                        reachable = true;
                    }
                }
                IrMatchCoverageUnit::EnumVariant {
                    enum_name: unit_enum,
                    variant_name,
                    total,
                } => {
                    if let Some(current) = enum_name.as_deref() {
                        if current != unit_enum {
                            mixed_or_opaque = true;
                        }
                    } else {
                        enum_name = Some(unit_enum);
                    }

                    if total {
                        if covered_variants.insert(variant_name) {
                            reachable = true;
                        }
                    } else {
                        mixed_or_opaque = true;
                        reachable = true;
                    }
                }
                IrMatchCoverageUnit::Opaque => {
                    mixed_or_opaque = true;
                    reachable = true;
                }
            }
        }

        if reachable {
            out.push(arm);
        }
        if arm_unconditional {
            seen_unconditional = true;
        }
    }

    out
}

#[derive(Debug, Clone)]
enum IrMatchCoverageUnit {
    Unconditional,
    Literal(String),
    EnumVariant {
        enum_name: String,
        variant_name: String,
        total: bool,
    },
    Opaque,
}

fn ir_match_pattern_coverage_units(
    enums: &IrEnumCatalog<'_>,
    pattern: &IrMatchPattern,
) -> Vec<IrMatchCoverageUnit> {
    match &pattern.kind {
        IrMatchPatternKind::Or(patterns) => patterns
            .iter()
            .flat_map(|pattern| ir_match_pattern_coverage_units(enums, pattern))
            .collect(),
        IrMatchPatternKind::Wildcard | IrMatchPatternKind::Binding(_) => {
            vec![IrMatchCoverageUnit::Unconditional]
        }
        IrMatchPatternKind::Literal(literal) => {
            vec![IrMatchCoverageUnit::Literal(ir_match_literal_key(literal))]
        }
        IrMatchPatternKind::Variant { path, payload } => {
            let Some((enum_decl, variant)) = enums.lookup_variant(path) else {
                return vec![IrMatchCoverageUnit::Opaque];
            };
            vec![IrMatchCoverageUnit::EnumVariant {
                enum_name: enum_decl.name.clone(),
                variant_name: variant.name.clone(),
                total: payload
                    .as_ref()
                    .map(ir_match_payload_is_irrefutable)
                    .unwrap_or(true),
            }]
        }
        IrMatchPatternKind::Object(_) | IrMatchPatternKind::Array(_) => {
            vec![IrMatchCoverageUnit::Opaque]
        }
    }
}

fn ir_enum_is_fully_covered(enum_decl: Option<&IrEnumDecl>, covered: &BTreeSet<String>) -> bool {
    let Some(enum_decl) = enum_decl else {
        return false;
    };
    if matches!(enum_decl.repr, IrEnumRepr::Existing { .. }) {
        return false;
    }
    !enum_decl.variants.is_empty()
        && enum_decl
            .variants
            .iter()
            .all(|variant| covered.contains(&variant.name))
}

fn ir_match_literal_key(literal: &IrMatchLiteral) -> String {
    match literal {
        IrMatchLiteral::Nil => "nil".into(),
        IrMatchLiteral::Boolean(value) => format!("bool:{value}"),
        IrMatchLiteral::Number(value) => format!("number:{value}"),
        IrMatchLiteral::String(value) => format!("string:{value}"),
    }
}

fn ir_match_payload_is_irrefutable(payload: &IrMatchPatternPayload) -> bool {
    match payload {
        IrMatchPatternPayload::Tuple(patterns) => {
            patterns.iter().all(ir_match_pattern_is_irrefutable)
        }
        IrMatchPatternPayload::Record(fields) => fields
            .iter()
            .all(|field| ir_match_pattern_is_irrefutable(&field.pattern)),
    }
}

fn ir_match_pattern_is_irrefutable(pattern: &IrMatchPattern) -> bool {
    match &pattern.kind {
        IrMatchPatternKind::Wildcard | IrMatchPatternKind::Binding(_) => true,
        IrMatchPatternKind::Or(patterns) => patterns.iter().any(ir_match_pattern_is_irrefutable),
        IrMatchPatternKind::Variant { payload, .. } => payload
            .as_ref()
            .map(ir_match_payload_is_irrefutable)
            .unwrap_or(true),
        IrMatchPatternKind::Object(fields) => fields
            .iter()
            .all(|field| ir_match_pattern_is_irrefutable(&field.pattern)),
        IrMatchPatternKind::Array(items) => items
            .iter()
            .all(|item| ir_match_pattern_is_irrefutable(&item.pattern)),
        IrMatchPatternKind::Literal(_) => false,
    }
}

pub(super) fn ir_match_pattern_is_unconditional(pattern: &IrMatchPattern) -> bool {
    match &pattern.kind {
        IrMatchPatternKind::Wildcard | IrMatchPatternKind::Binding(_) => true,
        IrMatchPatternKind::Or(patterns) => patterns.iter().any(ir_match_pattern_is_unconditional),
        IrMatchPatternKind::Variant { .. }
        | IrMatchPatternKind::Object(_)
        | IrMatchPatternKind::Array(_)
        | IrMatchPatternKind::Literal(_) => false,
    }
}
