use std::path::Path;

use crate::analysis::{AnalysisSignatureHelp, AnalysisSymbol, ProjectAnalysis};
use crate::module::RealmSet;
use crate::resolve::BindingKind;

use super::DocumentSnapshot;
use super::cursor::{package_member_call_at_offset, package_member_path_at_offset};

pub(crate) fn package_member_symbol_from_snapshot(
    analysis: &ProjectAnalysis,
    path: &Path,
    snapshot: &DocumentSnapshot,
) -> Option<AnalysisSymbol> {
    let member_path = package_member_path_at_offset(&snapshot.file.text, snapshot.offset)?;
    package_member_symbol_from_path(analysis, path, snapshot.offset, &member_path)
}

pub(crate) fn package_member_signature_help_from_snapshot(
    analysis: &ProjectAnalysis,
    path: &Path,
    snapshot: &DocumentSnapshot,
) -> Option<AnalysisSignatureHelp> {
    let call = package_member_call_at_offset(&snapshot.file.text, snapshot.offset)?;
    let symbol = package_member_symbol_from_path(analysis, path, snapshot.offset, &call.path)?;
    let signature = symbol.signature?;
    let max_active = signature.parameters.len().saturating_sub(1);
    Some(AnalysisSignatureHelp {
        signature,
        active_parameter: call.active_parameter.min(max_active),
    })
}

pub(crate) fn symbol_hover_markdown(symbol: &AnalysisSymbol) -> String {
    let mut out = String::new();
    out.push_str("### ");
    out.push_str(&symbol.name);
    out.push_str("\n\n");
    out.push_str(&symbol.detail);
    out.push('\n');

    if let Some(signature) = &symbol.signature {
        out.push_str("\n\n**Signature:** `");
        out.push_str(&signature.label);
        out.push('`');
        out.push_str("\n\n**Defined in:** `");
        out.push_str(&signature.module_id);
        out.push('`');
    }
    if let Some(module_id) = &symbol.module_id {
        out.push_str("\n\n**Module:** `");
        out.push_str(module_id);
        out.push('`');
    }
    if let Some(realms) = symbol.available_realms {
        out.push_str("\n\n**Realm:** ");
        out.push_str(realms.display_name());
    }
    if !symbol.exported_as.is_empty() {
        out.push_str("\n\n**Exported as:** ");
        out.push_str(
            &symbol
                .exported_as
                .iter()
                .map(|name| format!("`{name}`"))
                .collect::<Vec<_>>()
                .join(", "),
        );
    }
    if let Some((source, imported)) = &symbol.imported_from {
        out.push_str("\n\n**Imported from:** `");
        out.push_str(source);
        out.push_str("` as `");
        out.push_str(imported);
        out.push('`');
    }
    out
}

fn package_member_symbol_from_path(
    analysis: &ProjectAnalysis,
    path: &Path,
    offset: usize,
    member_path: &[&str],
) -> Option<AnalysisSymbol> {
    let (namespace, members) = member_path.split_first()?;
    let module = analysis.module_for_path(path)?;
    let active_realms = analysis
        .active_realms_at_path_offset(path, offset)
        .unwrap_or(RealmSet::SHARED);
    for binding in &module.resolved.bindings {
        if binding.name != *namespace {
            continue;
        }
        if binding.kind != BindingKind::Import {
            continue;
        }
        if !binding.available_realms.intersects(active_realms) {
            continue;
        }
        let Some(source) = binding.source_module.as_deref() else {
            continue;
        };
        let Some(imported) = binding.imported_name.as_deref() else {
            continue;
        };
        if let Some(symbol) = analysis.package_member_symbol_from_import_path(
            source,
            imported,
            members,
            active_realms,
        ) {
            return Some(symbol);
        }
    }
    None
}
