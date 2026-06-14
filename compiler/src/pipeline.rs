use crate::ast::Module;
use crate::compile_time::CompileTimePackageRegistry;
use crate::diag::{Diagnostic, Severity};
use crate::lex::Token;
use crate::macro_expansion::{MacroRegistry, expand_macros_with_registry};
use crate::parse::Parser;
use crate::resolve::{ResolveOutput, Resolver};
use crate::source::SourceFile;

#[derive(Debug)]
pub struct ParseExpandResolveOutput {
    pub module: Module,
    pub resolved: ResolveOutput,
    pub diagnostics: Vec<Diagnostic>,
}

impl ParseExpandResolveOutput {
    pub fn has_errors(&self) -> bool {
        self.diagnostics
            .iter()
            .any(|diagnostic| diagnostic.severity == Severity::Error)
    }
}

pub fn parse_expand_resolve(file: &SourceFile, tokens: &[Token]) -> ParseExpandResolveOutput {
    let mut macro_registry = MacroRegistry::empty();
    let mut diagnostics = Vec::new();
    match CompileTimePackageRegistry::load_default() {
        Ok(compile_time) => {
            if let Err(err) = compile_time.register_macros(&mut macro_registry) {
                diagnostics.push(Diagnostic::error(err.to_string()).with_code("CTLOAD001"));
            }
        }
        Err(err) => {
            diagnostics.push(Diagnostic::error(err.to_string()).with_code("CTLOAD001"));
        }
    }
    parse_expand_resolve_inner(file, tokens, &macro_registry, diagnostics)
}

pub fn parse_expand_resolve_with_registry(
    file: &SourceFile,
    tokens: &[Token],
    macro_registry: &MacroRegistry,
) -> ParseExpandResolveOutput {
    parse_expand_resolve_inner(file, tokens, macro_registry, Vec::new())
}

fn parse_expand_resolve_inner(
    file: &SourceFile,
    tokens: &[Token],
    macro_registry: &MacroRegistry,
    mut diagnostics: Vec<Diagnostic>,
) -> ParseExpandResolveOutput {
    let parsed = Parser::new(tokens).parse_module();
    diagnostics.extend(parsed.diagnostics);
    if diagnostics
        .iter()
        .any(|diagnostic| diagnostic.severity == Severity::Error)
    {
        let span = parsed.module.span;
        return ParseExpandResolveOutput {
            module: parsed.module,
            resolved: Resolver::resolve(&Module {
                body: Vec::new(),
                span,
            }),
            diagnostics,
        };
    }

    let expanded = expand_macros_with_registry(file, &parsed.module, macro_registry);
    diagnostics.extend(expanded.diagnostics);
    if diagnostics
        .iter()
        .any(|diagnostic| diagnostic.severity == Severity::Error)
    {
        return ParseExpandResolveOutput {
            module: expanded.module,
            resolved: Resolver::resolve(&Module {
                body: Vec::new(),
                span: parsed.module.span,
            }),
            diagnostics,
        };
    }

    let resolved = Resolver::resolve(&expanded.module);
    diagnostics.extend(resolved.diagnostics.clone());

    ParseExpandResolveOutput {
        module: expanded.module,
        resolved,
        diagnostics,
    }
}
