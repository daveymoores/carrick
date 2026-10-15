//! The route-path literals a module compares a value against (carrick#2382).
//!
//! A service that dispatches by hand, with no router, states each route it
//! serves as a comparison: `pathname === "/items"`, `case "/health":`,
//! `pathname.startsWith("/items/")`. Those are the language's own comparison
//! forms, so they are read here with no library or framework named.
//!
//! What counts, and nothing else:
//! - an operand of `===`, `==`, `!==` or `!=`;
//! - the test of a `switch` `case`;
//! - the first argument of a `.startsWith(` member call (the ECMAScript
//!   String API).
//!
//! An operand may be the literal itself, a template literal with no
//! interpolation, or a bare identifier that names a top-level
//! `const NAME = "<literal>"`. A name declared more than once anywhere in the
//! file is not resolved, because without scope analysis the comparison may
//! read the other binding. The bindings are collected in a first pass and the
//! comparisons in a second, so declaration order does not matter.
//!
//! Left out on purpose, so the silence is not read as coverage: a literal
//! used only as a call argument (a request target such as `fetch("/x")`), an
//! object-literal key, a regex, `URLPattern`, `split("/")`, a template
//! literal with an interpolation, and a member of a constant object
//! (`ROUTES.items`).
//!
//! The reader is shared: the join keeps a model route whose echoed id names
//! no listed site on it (`RouteLiteralWitness` in
//! `src/agents/file_orchestrator.rs`), and a candidate pass for hand-written
//! dispatch can raise sites from the same positions (carrick#2386).

use std::collections::HashMap;

use swc_common::Span;
use swc_ecma_ast::{
    BinExpr, BinaryOp, CallExpr, Callee, ClassDecl, Decl, Expr, FnDecl, FnExpr, Ident,
    ImportDefaultSpecifier, ImportNamedSpecifier, ImportStarAsSpecifier, MemberProp, Module,
    ModuleDecl, ModuleItem, Pat, Stmt, SwitchCase, VarDeclKind,
};
use swc_ecma_visit::{Visit, VisitWith};

use crate::swc_scanner::is_producer_route_path;
use crate::wrapper_request_shape::literal_string;

/// Where a route literal stood.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ComparisonPosition {
    /// An operand of `===`, `==`, `!==` or `!=`.
    Equality,
    /// The test of a `switch` `case`.
    Case,
    /// The first argument of a `.startsWith(` call.
    StartsWith,
}

/// One route-path literal in a comparison position.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ComparedRouteLiteral {
    /// The literal's value, already shown to pass
    /// [`is_producer_route_path`].
    pub value: String,
    pub position: ComparisonPosition,
    /// The comparison the literal stands in: the binary expression, the
    /// `case` clause, or the `startsWith` call. Its offsets are the module's
    /// own source map's.
    pub span: Span,
}

/// Every route-path literal `module` compares a value against, in source
/// order. See the module documentation for the positions read.
pub fn compared_route_literals(module: &Module) -> Vec<ComparedRouteLiteral> {
    let constants = top_level_route_constants(module);
    let mut reader = ComparisonReader {
        constants,
        found: Vec::new(),
    };
    module.visit_with(&mut reader);
    reader.found
}

/// Top-level `const NAME = "<route literal>"` bindings whose name is
/// declared exactly once in the file.
fn top_level_route_constants(module: &Module) -> HashMap<String, String> {
    let mut declarations = DeclarationCounter::default();
    module.visit_with(&mut declarations);
    let mut constants = HashMap::new();
    for item in &module.body {
        let var = match item {
            ModuleItem::Stmt(Stmt::Decl(Decl::Var(var))) => var,
            ModuleItem::ModuleDecl(ModuleDecl::ExportDecl(export)) => match &export.decl {
                Decl::Var(var) => var,
                _ => continue,
            },
            _ => continue,
        };
        if var.kind != VarDeclKind::Const {
            continue;
        }
        for declarator in &var.decls {
            let Pat::Ident(binding) = &declarator.name else {
                continue;
            };
            let name = binding.id.sym.to_string();
            if declarations.counts.get(&name).copied() != Some(1) {
                continue;
            }
            let Some(value) = declarator.init.as_deref().and_then(route_literal) else {
                continue;
            };
            constants.insert(name, value);
        }
    }
    constants
}

/// A string literal (or a template literal with no interpolation) whose value
/// is route-shaped.
fn route_literal(expr: &Expr) -> Option<String> {
    literal_string(expr).filter(|value| is_producer_route_path(value))
}

/// How many times each name is declared anywhere in the file: variables,
/// parameters, destructured bindings, functions, classes and imports.
#[derive(Default)]
struct DeclarationCounter {
    counts: HashMap<String, usize>,
}

impl DeclarationCounter {
    fn count(&mut self, ident: &Ident) {
        *self.counts.entry(ident.sym.to_string()).or_default() += 1;
    }
}

impl Visit for DeclarationCounter {
    fn visit_binding_ident(&mut self, binding: &swc_ecma_ast::BindingIdent) {
        self.count(&binding.id);
        binding.visit_children_with(self);
    }

    fn visit_fn_decl(&mut self, decl: &FnDecl) {
        self.count(&decl.ident);
        decl.visit_children_with(self);
    }

    fn visit_fn_expr(&mut self, expr: &FnExpr) {
        if let Some(ident) = &expr.ident {
            self.count(ident);
        }
        expr.visit_children_with(self);
    }

    fn visit_class_decl(&mut self, decl: &ClassDecl) {
        self.count(&decl.ident);
        decl.visit_children_with(self);
    }

    fn visit_import_named_specifier(&mut self, specifier: &ImportNamedSpecifier) {
        self.count(&specifier.local);
    }

    fn visit_import_default_specifier(&mut self, specifier: &ImportDefaultSpecifier) {
        self.count(&specifier.local);
    }

    fn visit_import_star_as_specifier(&mut self, specifier: &ImportStarAsSpecifier) {
        self.count(&specifier.local);
    }
}

/// The second pass: the comparisons.
struct ComparisonReader {
    constants: HashMap<String, String>,
    found: Vec<ComparedRouteLiteral>,
}

impl ComparisonReader {
    /// The route literal an operand denotes: the literal itself, or the
    /// top-level constant a bare identifier names.
    fn operand(&self, expr: &Expr) -> Option<String> {
        if let Some(value) = route_literal(expr) {
            return Some(value);
        }
        match strip_wrappers(expr) {
            Expr::Ident(ident) => self.constants.get(ident.sym.as_ref()).cloned(),
            _ => None,
        }
    }

    fn push(&mut self, value: String, position: ComparisonPosition, span: Span) {
        self.found.push(ComparedRouteLiteral {
            value,
            position,
            span,
        });
    }
}

/// Parentheses and type assertions change no value.
fn strip_wrappers(expr: &Expr) -> &Expr {
    match expr {
        Expr::Paren(inner) => strip_wrappers(&inner.expr),
        Expr::TsAs(inner) => strip_wrappers(&inner.expr),
        Expr::TsNonNull(inner) => strip_wrappers(&inner.expr),
        Expr::TsSatisfies(inner) => strip_wrappers(&inner.expr),
        Expr::TsConstAssertion(inner) => strip_wrappers(&inner.expr),
        other => other,
    }
}

impl Visit for ComparisonReader {
    fn visit_bin_expr(&mut self, bin: &BinExpr) {
        bin.visit_children_with(self);
        if !matches!(
            bin.op,
            BinaryOp::EqEqEq | BinaryOp::EqEq | BinaryOp::NotEqEq | BinaryOp::NotEq
        ) {
            return;
        }
        for operand in [&bin.left, &bin.right] {
            if let Some(value) = self.operand(operand) {
                self.push(value, ComparisonPosition::Equality, bin.span);
            }
        }
    }

    fn visit_switch_case(&mut self, case: &SwitchCase) {
        case.visit_children_with(self);
        if let Some(value) = case.test.as_deref().and_then(|test| self.operand(test)) {
            self.push(value, ComparisonPosition::Case, case.span);
        }
    }

    fn visit_call_expr(&mut self, call: &CallExpr) {
        call.visit_children_with(self);
        let Callee::Expr(callee) = &call.callee else {
            return;
        };
        let Expr::Member(member) = strip_wrappers(callee) else {
            return;
        };
        if !matches!(&member.prop, MemberProp::Ident(name) if name.sym.as_ref() == "startsWith") {
            return;
        }
        let Some(first) = call.args.first() else {
            return;
        };
        if first.spread.is_some() {
            return;
        }
        if let Some(value) = self.operand(&first.expr) {
            self.push(value, ComparisonPosition::StartsWith, call.span);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    fn read(source: &str) -> Vec<(String, ComparisonPosition)> {
        let (_, module) =
            crate::swc_scanner::parse_standalone_module(Path::new("src/worker.ts"), source)
                .expect("the source parses");
        compared_route_literals(&module)
            .into_iter()
            .map(|found| (found.value, found.position))
            .collect()
    }

    fn one(value: &str, position: ComparisonPosition) -> Vec<(String, ComparisonPosition)> {
        vec![(value.to_string(), position)]
    }

    #[test]
    fn every_equality_operator_reads_either_side() {
        for op in ["===", "==", "!==", "!="] {
            assert_eq!(
                read(&format!("if (p {op} '/a') {{}}")),
                one("/a", ComparisonPosition::Equality),
                "{op}"
            );
            assert_eq!(
                read(&format!("if ('/a' {op} p) {{}}")),
                one("/a", ComparisonPosition::Equality),
                "{op} with the literal on the left"
            );
        }
    }

    #[test]
    fn a_case_test_and_a_starts_with_argument_are_read() {
        assert_eq!(
            read("switch (p) { case `/health`: break; }"),
            one("/health", ComparisonPosition::Case)
        );
        assert_eq!(
            read("if (p.startsWith('/items/')) {}"),
            one("/items/", ComparisonPosition::StartsWith)
        );
    }

    #[test]
    fn a_top_level_constant_is_read_wherever_it_is_declared() {
        assert_eq!(
            read("if (p.startsWith(PREFIX)) {}\nconst PREFIX = '/items/';"),
            one("/items/", ComparisonPosition::StartsWith)
        );
        assert_eq!(
            read("export const HEALTH = '/health';\nif (p === HEALTH) {}"),
            one("/health", ComparisonPosition::Equality)
        );
    }

    #[test]
    fn a_name_declared_twice_or_not_const_is_not_resolved() {
        assert!(read("const P = '/a';\nfunction f(P) { return x === P; }").is_empty());
        assert!(read("let P = '/a';\nif (x === P) {}").is_empty());
        assert!(read("function f() { const P = '/a'; return x === P; }").is_empty());
    }

    #[test]
    fn other_positions_and_shapes_are_not_read() {
        for source in [
            "fetch('/audit');",
            "const routes = { '/items': list };",
            "if (/^\\/items/.test(p)) {}",
            "new URLPattern({ pathname: '/items/:id' });",
            "p.split('/');",
            "if (p === `/items/${id}`) {}",
            "const ROUTES = { items: '/items' };\nif (p === ROUTES.items) {}",
            "if (p === 'items') {}",
            "if (p.endsWith('/items')) {}",
            "if (p < '/items') {}",
        ] {
            assert!(read(source).is_empty(), "{source}");
        }
    }
}
