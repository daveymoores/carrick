//! Which class a local binding's value IS, as the file itself states it
//! (carrick#776).
//!
//! `call_graph` resolves `obj.foo()` by resolving `obj`, and a receiver bound
//! to an instance used to resolve to nothing at all:
//!
//! ```ignore
//! import type { ApiClient } from "@scope/core/v3";
//!
//! function readRun(runId: string, client: ApiClient) {
//!   return client.subscribeToRun(runId);
//! }
//! ```
//!
//! Nothing here needs inferring. The file DECLARES what `client` is, and a
//! declared type is a statement in the source exactly as an import is. This
//! pass records those statements: local binding -> the type identifier written
//! for it. Resolving that identifier to a class is `call_graph`'s job, and it
//! uses the same import walk it already uses for a class named directly, so a
//! type that names nothing in this repo simply resolves to nothing.
//!
//! The call graph reads these type expressions per lexical binding in
//! `visitor`, so captured receivers survive ownership attribution and
//! same-named declarations in nested scopes remain distinct. The collector
//! below also supports folding declarations and class fields into one table.
//!
//! What is read, and nothing else:
//!
//! - A binding whose annotation is a plain type reference — `client:
//!   ApiClient`, `client: Client<Run>`. The type ARGUMENTS are ignored: a
//!   generic instantiation names the same class its head does. A qualified
//!   name (`ns.Client`) is not read: the head names a namespace, not the
//!   class, and following it is the namespace walk's job, not this one's.
//! - A `const`/`let`/`var` initialised with `new X()`, where `X` is a plain
//!   identifier. The construction states the class as flatly as an annotation
//!   does.
//! - Parameters wherever they sit — a function, a method, an arrow — because
//!   the shape this pass exists for is a helper that takes the client it calls.
//!
//! Only identifier bindings are read: a destructured or rest pattern binds a
//! part of a value, and which part decides what the value is.
//!
//! A `const` initialised by a call to a function (`const client =
//! createClient()`) states no class at the binding. The declaration of the
//! function called can, and [`factory_tables`] reads that (carrick#1146).
//!
//! Within one scope, a name bound twice to different types is ambiguous and
//! dropped rather than picked between, and so is a name bound once with a type
//! and once without. Mirrors [`crate::receiver_origin`], which answers the
//! same question for a value that traces back to an import rather than to a
//! declared type.
//!
//! A CLASS FIELD is the same statement one level out (carrick#782). `this.x`
//! is not a local binding — it belongs to the class, not to the method — so
//! [`class_field_types`] reads the class body once and the table is keyed by
//! class rather than by function. It reads exactly what a field DECLARES: an
//! annotated instance property, a `#private` one, or a constructor parameter
//! property. A field left unannotated states nothing, whatever its
//! initialiser, and is recorded as contested so a same-named annotation
//! elsewhere in the class cannot answer for it.

use std::collections::HashMap;

use swc_ecma_ast::*;
use swc_ecma_visit::{Visit, VisitWith};

use crate::import_bindings::DEFAULT_EXPORT;

/// Local binding name -> the type identifier declared for it.
pub type ReceiverTypes = HashMap<String, String>;

/// Which class each FIELD of one class is declared to be (carrick#782).
///
/// Instance fields only: `this.x` inside a method names an instance member,
/// and a static field is reached through the class name, which
/// [`crate::call_graph`] already resolves without this table. A private field
/// is keyed `#name`, matching how a `this.#x()` call site is recorded.
pub fn class_field_types(class: &Class) -> ReceiverTypes {
    let mut collector = ReceiverTypeCollector::default();
    for member in &class.body {
        match member {
            ClassMember::ClassProp(prop) if !prop.is_static => {
                let PropName::Ident(key) = &prop.key else {
                    continue;
                };
                let declared = prop
                    .type_ann
                    .as_deref()
                    .and_then(annotated_type_ident)
                    .map(|ident| ident.sym.to_string());
                collector.record(key.sym.to_string(), declared);
            }
            ClassMember::PrivateProp(prop) if !prop.is_static => {
                let declared = prop
                    .type_ann
                    .as_deref()
                    .and_then(annotated_type_ident)
                    .map(|ident| ident.sym.to_string());
                collector.record(format!("#{}", prop.key.name), declared);
            }
            // `constructor(private readonly client: ApiClient)` declares a
            // field in the same breath as the parameter. The defaulted form
            // (`= new ApiClient()`) declares the same field.
            ClassMember::Constructor(constructor) => {
                for param in &constructor.params {
                    let ParamOrTsParamProp::TsParamProp(prop) = param else {
                        continue;
                    };
                    let binding = match &prop.param {
                        TsParamPropParam::Ident(ident) => Some(ident),
                        TsParamPropParam::Assign(assign) => match &*assign.left {
                            Pat::Ident(ident) => Some(ident),
                            _ => None,
                        },
                    };
                    let Some(binding) = binding else {
                        continue;
                    };
                    let declared = binding
                        .type_ann
                        .as_deref()
                        .and_then(annotated_type_ident)
                        .map(|ident| ident.sym.to_string());
                    collector.record(binding.id.sym.to_string(), declared);
                }
            }
            _ => {}
        }
    }
    collector.finish()
}

/// Which class each MODULE-SCOPE binding of one file is declared to be
/// (carrick#1147).
///
/// `export const tokens = new TokenService()` is a statement about the value
/// every importer of `tokens` receives, so `tokens.issue()` in another file is
/// `TokenService.issue` as plainly as a call on a local `new TokenService()`
/// is. Only the file's top level is read: a same-named binding inside a
/// function is a different value and must not answer for the export.
///
/// The same two statements a local binding is read for: an annotation that
/// names a class, or a `new X()` initialiser. An anonymous
/// `export default new X()` has no binding name and is keyed by the default
/// export's name (`default`, which is not a valid identifier and so cannot
/// collide). A factory's result (`makeService()`) states nothing here and is
/// not recorded in this table: [`factory_tables`] records the call, and the
/// factory's own declaration decides.
pub fn module_scope_types(module: &Module) -> ReceiverTypes {
    let mut collector = ReceiverTypeCollector::default();
    for item in &module.body {
        match item {
            ModuleItem::Stmt(Stmt::Decl(Decl::Var(var)))
            | ModuleItem::ModuleDecl(ModuleDecl::ExportDecl(ExportDecl {
                decl: Decl::Var(var),
                ..
            })) => {
                for declarator in &var.decls {
                    collector.record_declarator(declarator);
                }
            }
            ModuleItem::ModuleDecl(ModuleDecl::ExportDefaultExpr(export)) => {
                let declared =
                    constructed_type_ident(&export.expr).map(|ident| ident.sym.to_string());
                collector.record(DEFAULT_EXPORT.to_string(), declared);
            }
            _ => {}
        }
    }
    collector.finish()
}

/// The class identifier a type annotation names, or `None` when the
/// annotation is anything but an unqualified type reference.
pub(crate) fn annotated_type_ident(type_ann: &TsTypeAnn) -> Option<&Ident> {
    match &*type_ann.type_ann {
        TsType::TsTypeRef(reference) => match &reference.type_name {
            TsEntityName::Ident(ident) => Some(ident),
            TsEntityName::TsQualifiedName(_) => None,
        },
        _ => None,
    }
}

/// The class identifier a `new X()` initialiser names, through the wrappers
/// that pass a value along without replacing it.
pub(crate) fn constructed_type_ident(expr: &Expr) -> Option<&Ident> {
    match expr {
        Expr::New(new_expr) => match &*new_expr.callee {
            Expr::Ident(ident) => Some(ident),
            _ => None,
        },
        Expr::Await(await_expr) => constructed_type_ident(&await_expr.arg),
        Expr::Paren(paren) => constructed_type_ident(&paren.expr),
        Expr::TsAs(as_expr) => constructed_type_ident(&as_expr.expr),
        Expr::TsNonNull(non_null) => constructed_type_ident(&non_null.expr),
        Expr::TsSatisfies(satisfies) => constructed_type_ident(&satisfies.expr),
        _ => None,
    }
}

/// A binding initialised by a call to a function named by a bare identifier:
/// `const client = createClient()` or `const client = await createClient()`
/// (carrick#1146).
///
/// The call states no class. The DECLARATION of the function it calls can
/// ([`StatedReturn`]), and that is a statement in the source as an annotation
/// on the binding would be.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FactoryCall {
    /// The function called, as the binding's file names it.
    pub callee: String,
    /// Whether the call's value is awaited before it is bound.
    pub awaited: bool,
}

/// The bare identifier an initialiser calls, and whether the result is
/// awaited, through the wrappers that pass a value along without restating its
/// type. `f() as T` is not read: the assertion states a type of its own.
///
/// A member callee (`factory.create()`), an optional call and a call of a
/// call state nothing here: which function runs is not named by one
/// identifier.
pub(crate) fn factory_call(expr: &Expr) -> Option<(&Ident, bool)> {
    match expr {
        Expr::Call(call) => match &call.callee {
            Callee::Expr(callee) => match &**callee {
                Expr::Ident(ident) => Some((ident, false)),
                _ => None,
            },
            _ => None,
        },
        Expr::Await(await_expr) => factory_call(&await_expr.arg).map(|(ident, _)| (ident, true)),
        Expr::Paren(paren) => factory_call(&paren.expr),
        Expr::TsNonNull(non_null) => factory_call(&non_null.expr),
        Expr::TsSatisfies(satisfies) => factory_call(&satisfies.expr),
        _ => None,
    }
}

/// What a function's declaration states its call returns (carrick#1146).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StatedReturn {
    /// An instance of the class this identifier names in the function's own
    /// module. `promised` when the caller receives a promise of it.
    Class { name: String, promised: bool },
    /// Whatever a call to this function returns: the body's only statement
    /// returns that call. `promised` when the function is `async`.
    Call { callee: String, promised: bool },
}

/// The two tables one module contributes to reading a receiver bound from a
/// factory call (carrick#1146).
#[derive(Debug, Default, Clone)]
pub struct FactoryTables {
    /// Module-scope function -> what its declaration states it returns.
    pub returns: HashMap<String, StatedReturn>,
    /// Module-scope `const` -> the factory call that initialises it, for a
    /// binding the module states no class for by annotation or `new`.
    pub bindings: HashMap<String, FactoryCall>,
    /// Module-scope type alias of a function type -> the class a call of a
    /// value of that type returns (carrick#1561). Always
    /// [`StatedReturn::Class`]: a type states no body to follow.
    pub aliases: HashMap<String, StatedReturn>,
}

/// What a binding's declared type says a call of it returns, when the binding
/// is a function value rather than a function declaration: a parameter or a
/// local whose annotation is a function type (carrick#1561).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CalledType {
    /// `make: (id: string) => ApiClient`: the class the inline function type
    /// names, written in the binding's own module.
    Returns { name: String, promised: bool },
    /// `make: MakeClient`: the type the annotation names, which states a
    /// return only if it is an alias of a function type
    /// ([`FactoryTables::aliases`]).
    Alias(String),
}

/// A binding initialised by calling a function VALUE whose declared type is a
/// function type: `const client = make("p1")` where `make: MakeClient`
/// (carrick#1561). The value called is handed in, so its name says nothing;
/// its type is the statement.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TypedCall {
    pub called: CalledType,
    /// Whether the call's value is awaited before it is bound.
    pub awaited: bool,
}

/// What a binding's type annotation says a call of the binding returns:
/// an inline function type that names one class (or `Promise` of one), or a
/// plain type reference whose declaration may be such an alias. Anything else
/// states nothing, under the same rules as a function's return annotation.
pub fn called_type(type_ann: &TsTypeAnn) -> Option<CalledType> {
    match unparenthesized(&type_ann.type_ann) {
        TsType::TsTypeRef(reference) => match &reference.type_name {
            TsEntityName::Ident(ident) => Some(CalledType::Alias(ident.sym.to_string())),
            TsEntityName::TsQualifiedName(_) => None,
        },
        ty => match function_type_return(ty, None)? {
            StatedReturn::Class { name, promised } => Some(CalledType::Returns { name, promised }),
            StatedReturn::Call { .. } => None,
        },
    }
}

fn unparenthesized(ty: &TsType) -> &TsType {
    match ty {
        TsType::TsParenthesizedType(paren) => unparenthesized(&paren.type_ann),
        ty => ty,
    }
}

/// The class a function type states its call returns. `outer` is the type
/// parameters of the alias the function type is declared by, which name no
/// class either.
fn function_type_return(ty: &TsType, outer: Option<&TsTypeParamDecl>) -> Option<StatedReturn> {
    let TsType::TsFnOrConstructorType(TsFnOrConstructorType::TsFnType(function)) =
        unparenthesized(ty)
    else {
        return None;
    };
    annotated_return(
        &function.type_ann,
        &[outer, function.type_params.as_deref()],
    )
}

/// Read a module's top level for [`FactoryTables`].
///
/// A function states a class in two ways, and no other:
///
/// - a return type annotation that names one type, or `Promise` of one;
/// - with no annotation, a body whose only statement returns `new C(…)`, or
///   returns a call to a function named by a bare identifier.
///
/// An annotation that is anything else (a union, a qualified name, the
/// function's own type parameter) states no class, and the body is then not
/// read in its place: the annotation is what the function declares. A body
/// with a second statement states nothing, because nothing here says which
/// `return` runs.
///
/// A name declared twice at the top level with different statements is
/// dropped, as [`ReceiverTypeCollector`] drops a binding declared twice.
pub fn factory_tables(module: &Module) -> FactoryTables {
    let mut returns: HashMap<String, Option<StatedReturn>> = HashMap::new();
    let mut bindings: HashMap<String, Option<FactoryCall>> = HashMap::new();
    let mut aliases: HashMap<String, Option<StatedReturn>> = HashMap::new();
    fn record<T: PartialEq>(
        table: &mut HashMap<String, Option<T>>,
        name: String,
        value: Option<T>,
    ) {
        match table.get(&name) {
            Some(existing) if *existing == value => {}
            Some(_) => {
                table.insert(name, None);
            }
            None => {
                table.insert(name, value);
            }
        }
    }
    for item in &module.body {
        let decl = match item {
            ModuleItem::Stmt(Stmt::Decl(decl)) => decl,
            ModuleItem::ModuleDecl(ModuleDecl::ExportDecl(export)) => &export.decl,
            _ => continue,
        };
        match decl {
            Decl::TsTypeAlias(alias) => record(
                &mut aliases,
                alias.id.sym.to_string(),
                function_type_return(&alias.type_ann, alias.type_params.as_deref()),
            ),
            Decl::Fn(function) => record(
                &mut returns,
                function.ident.sym.to_string(),
                function_stated_return(&function.function),
            ),
            Decl::Var(var) => {
                for declarator in &var.decls {
                    let Pat::Ident(ident) = &declarator.name else {
                        continue;
                    };
                    let name = ident.id.sym.to_string();
                    match declarator.init.as_deref() {
                        Some(Expr::Arrow(arrow)) => {
                            record(&mut returns, name, arrow_stated_return(arrow));
                        }
                        Some(Expr::Fn(function)) => {
                            record(
                                &mut returns,
                                name,
                                function_stated_return(&function.function),
                            );
                        }
                        init => {
                            let stated = (var.kind == VarDeclKind::Const
                                && ident.type_ann.is_none())
                            .then(|| init.and_then(factory_call))
                            .flatten()
                            .map(|(callee, awaited)| FactoryCall {
                                callee: callee.sym.to_string(),
                                awaited,
                            });
                            record(&mut bindings, name, stated);
                        }
                    }
                }
            }
            _ => {}
        }
    }
    FactoryTables {
        returns: returns
            .into_iter()
            .filter_map(|(name, stated)| stated.map(|stated| (name, stated)))
            .collect(),
        bindings: bindings
            .into_iter()
            .filter_map(|(name, stated)| stated.map(|stated| (name, stated)))
            .collect(),
        aliases: aliases
            .into_iter()
            .filter_map(|(name, stated)| stated.map(|stated| (name, stated)))
            .collect(),
    }
}

fn function_stated_return(function: &Function) -> Option<StatedReturn> {
    let returned = function
        .body
        .as_ref()
        .and_then(|body| match body.stmts.as_slice() {
            [Stmt::Return(statement)] => statement.arg.as_deref(),
            _ => None,
        });
    stated_return(
        function.return_type.as_deref(),
        function.type_params.as_deref(),
        function.is_async,
        returned,
    )
}

fn arrow_stated_return(arrow: &ArrowExpr) -> Option<StatedReturn> {
    let returned = match &*arrow.body {
        ArrowFunctionBody::Expr(expr) => Some(&**expr),
        ArrowFunctionBody::FunctionBody(body) => match body.stmts.as_slice() {
            [Stmt::Return(statement)] => statement.arg.as_deref(),
            _ => None,
        },
    };
    stated_return(
        arrow.return_type.as_deref(),
        arrow.type_params.as_deref(),
        arrow.is_async,
        returned,
    )
}

/// `returned` is the expression the body's only statement returns, when the
/// body is that one statement.
fn stated_return(
    annotation: Option<&TsTypeAnn>,
    type_params: Option<&TsTypeParamDecl>,
    is_async: bool,
    returned: Option<&Expr>,
) -> Option<StatedReturn> {
    if let Some(annotation) = annotation {
        return annotated_return(annotation, &[type_params]);
    }
    let returned = returned?;
    if let Some(class) = constructed_type_ident(returned) {
        return Some(StatedReturn::Class {
            name: class.sym.to_string(),
            promised: is_async,
        });
    }
    factory_call(returned).map(|(callee, _)| StatedReturn::Call {
        callee: callee.sym.to_string(),
        promised: is_async,
    })
}

/// The class a return annotation names: one type, or `Promise` of one. A
/// union, a qualified name or a type parameter `own` declares states nothing.
fn annotated_return(
    annotation: &TsTypeAnn,
    own: &[Option<&TsTypeParamDecl>],
) -> Option<StatedReturn> {
    let TsType::TsTypeRef(reference) = &*annotation.type_ann else {
        return None;
    };
    let TsEntityName::Ident(head) = &reference.type_name else {
        return None;
    };
    let (named, promised) = match reference.type_params.as_deref() {
        None => (head, false),
        // `Promise<C>`. Any other generic head names the class its head
        // does, as a binding's annotation is read.
        Some(arguments) if &*head.sym == "Promise" => match arguments.params.as_slice() {
            [argument] => match &**argument {
                TsType::TsTypeRef(inner) => match &inner.type_name {
                    TsEntityName::Ident(inner) => (inner, true),
                    TsEntityName::TsQualifiedName(_) => return None,
                },
                _ => return None,
            },
            _ => return None,
        },
        Some(_) => (head, false),
    };
    let own_parameter = own
        .iter()
        .flatten()
        .any(|declared| declared.params.iter().any(|p| p.name.sym == named.sym));
    if own_parameter || &*named.sym == "Promise" {
        return None;
    }
    Some(StatedReturn::Class {
        name: named.sym.to_string(),
        promised,
    })
}

/// Reads one function's declared receiver types. Drive it over the function's
/// own parameters with [`record_pat`](Self::record_pat) and over its body with
/// `visit_with`, then take the table with [`finish`](Self::finish).
///
/// `None` against a name marks it contested: declared twice differently within
/// the scope, or declared once and left untyped once.
#[derive(Default)]
pub struct ReceiverTypeCollector {
    types: HashMap<String, Option<String>>,
}

impl ReceiverTypeCollector {
    /// Record what one binding pattern declares. Non-identifier patterns
    /// declare nothing and are skipped rather than recorded as contested: a
    /// destructured parameter binds names this pass has no statement about.
    pub fn record_pat(&mut self, pat: &Pat) {
        if let Pat::Ident(ident) = pat {
            let declared = ident
                .type_ann
                .as_deref()
                .and_then(annotated_type_ident)
                .map(|ident| ident.sym.to_string());
            self.record(ident.id.sym.to_string(), declared);
        }
    }

    /// The names this scope declares a class for.
    pub fn finish(self) -> ReceiverTypes {
        self.types
            .into_iter()
            .filter_map(|(name, declared)| declared.map(|type_name| (name, type_name)))
            .collect()
    }

    /// Record what one `const`/`let`/`var` declarator states: an annotation
    /// naming a class wins, else a `new X()` initialiser. A destructured
    /// declarator binds a part of a value and states nothing.
    fn record_declarator(&mut self, declarator: &VarDeclarator) {
        if let Pat::Ident(ident) = &declarator.name {
            let declared = ident
                .type_ann
                .as_deref()
                .and_then(annotated_type_ident)
                .or_else(|| declarator.init.as_deref().and_then(constructed_type_ident))
                .map(|ident| ident.sym.to_string());
            self.record(ident.id.sym.to_string(), declared);
        }
    }

    fn record(&mut self, name: String, type_name: Option<String>) {
        match self.types.get(&name) {
            Some(existing) if existing.as_deref() == type_name.as_deref() => {}
            Some(_) => {
                self.types.insert(name, None);
            }
            None => {
                self.types.insert(name, type_name);
            }
        }
    }
}

impl Visit for ReceiverTypeCollector {
    fn visit_param(&mut self, param: &Param) {
        self.record_pat(&param.pat);
        param.visit_children_with(self);
    }

    /// Arrow parameters are `Pat`s with no `Param` wrapper around them.
    fn visit_arrow_expr(&mut self, arrow: &ArrowExpr) {
        for pat in &arrow.params {
            self.record_pat(pat);
        }
        arrow.visit_children_with(self);
    }

    fn visit_var_declarator(&mut self, declarator: &VarDeclarator) {
        self.record_declarator(declarator);
        declarator.visit_children_with(self);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::swc_scanner::parse_standalone_module;
    use std::path::Path;

    /// Walks the whole module as one scope. Production keys the table per
    /// function (see `FunctionDefinitionExtractor`); these cases exercise the
    /// rules that hold within any one scope.
    fn types(source: &str) -> ReceiverTypes {
        let (_, module) = parse_standalone_module(Path::new("types.ts"), source).expect("parses");
        let mut collector = ReceiverTypeCollector::default();
        module.visit_with(&mut collector);
        collector.finish()
    }

    #[test]
    fn a_typed_parameter_states_its_class() {
        let types = types(
            r#"
            import type { ApiClient } from "@scope/core/v3";
            function readRun(runId: string, client: ApiClient) {
              return client.subscribeToRun(runId);
            }
            "#,
        );
        assert_eq!(types.get("client").map(String::as_str), Some("ApiClient"));
        // A keyword type is not a type reference and names no class.
        assert_eq!(types.get("runId"), None);
    }

    #[test]
    fn a_generic_instantiation_names_the_class_its_head_names() {
        let types = types("function run(client: ApiClient<Run>) { return client.list(); }");
        assert_eq!(types.get("client").map(String::as_str), Some("ApiClient"));
    }

    #[test]
    fn an_arrow_parameter_is_read_too() {
        let types = types("export const run = (client: ApiClient) => client.list();");
        assert_eq!(types.get("client").map(String::as_str), Some("ApiClient"));
    }

    #[test]
    fn a_constructed_local_states_its_class() {
        let types = types(r#"const client = new ApiClient("http://x");"#);
        assert_eq!(types.get("client").map(String::as_str), Some("ApiClient"));
    }

    fn factories(source: &str) -> FactoryTables {
        let (_, module) =
            parse_standalone_module(Path::new("factories.ts"), source).expect("parses");
        factory_tables(&module)
    }

    fn class(name: &str, promised: bool) -> StatedReturn {
        StatedReturn::Class {
            name: name.to_string(),
            promised,
        }
    }

    #[test]
    fn a_return_annotation_states_the_class_a_function_returns() {
        let tables = factories(
            r#"
            export function createClient(): ApiClient { return build(); }
            export async function openClient(): Promise<ApiClient> { return build(); }
            export const makeClient = (): ApiClient<Run> => build();
            "#,
        );
        assert_eq!(
            tables.returns.get("createClient"),
            Some(&class("ApiClient", false))
        );
        assert_eq!(
            tables.returns.get("openClient"),
            Some(&class("ApiClient", true))
        );
        assert_eq!(
            tables.returns.get("makeClient"),
            Some(&class("ApiClient", false))
        );
    }

    /// A type alias of a function type states the class a call of a value of
    /// that type returns, by the rules a return annotation is read by
    /// (carrick#1561).
    #[test]
    fn a_function_type_alias_states_the_class_its_call_returns() {
        let tables = factories(
            r#"
            export type MakeClient = (projectId: string) => ApiClient;
            type OpenClient = () => Promise<ApiClient>;
            type Wrapped = (() => ApiClient);
            type Either = () => ApiClient | BatchClient;
            type Qualified = () => core.ApiClient;
            type Own<T> = () => T;
            type Inner = <T>() => T;
            type Shape = { (projectId: string): ApiClient };
            type NotAFunction = ApiClient;
            "#,
        );
        assert_eq!(
            tables.aliases.get("MakeClient"),
            Some(&class("ApiClient", false))
        );
        assert_eq!(
            tables.aliases.get("OpenClient"),
            Some(&class("ApiClient", true))
        );
        assert_eq!(
            tables.aliases.get("Wrapped"),
            Some(&class("ApiClient", false))
        );
        assert_eq!(tables.aliases.len(), 3, "{:?}", tables.aliases);
        assert!(tables.returns.is_empty(), "{:?}", tables.returns);
    }

    fn annotation(source: &str) -> Option<CalledType> {
        let (_, module) =
            parse_standalone_module(Path::new("annotation.ts"), source).expect("parses");
        let ModuleItem::Stmt(Stmt::Decl(Decl::Var(var))) = &module.body[0] else {
            panic!("one declaration");
        };
        let Pat::Ident(ident) = &var.decls[0].name else {
            panic!("an identifier binding");
        };
        called_type(ident.type_ann.as_deref().expect("annotated"))
    }

    #[test]
    fn a_function_typed_binding_states_what_its_call_returns() {
        assert_eq!(
            annotation("let make: (id: string) => ApiClient;"),
            Some(CalledType::Returns {
                name: "ApiClient".to_string(),
                promised: false
            })
        );
        assert_eq!(
            annotation("let make: () => Promise<ApiClient>;"),
            Some(CalledType::Returns {
                name: "ApiClient".to_string(),
                promised: true
            })
        );
        assert_eq!(
            annotation("let make: MakeClient;"),
            Some(CalledType::Alias("MakeClient".to_string()))
        );
        assert_eq!(annotation("let make: () => ApiClient | BatchClient;"), None);
        assert_eq!(annotation("let make: <T>() => T;"), None);
        assert_eq!(annotation("let make: core.MakeClient;"), None);
    }

    #[test]
    fn a_sole_return_states_what_an_unannotated_function_returns() {
        let tables = factories(
            r#"
            export function createClient() { return new ApiClient("http://x"); }
            export const makeClient = () => new ApiClient("http://x");
            export async function openClient() { return new ApiClient("http://x"); }
            export function getClient() { return createClient(); }
            export const sharedClient = async () => await openClient();
            "#,
        );
        assert_eq!(
            tables.returns.get("createClient"),
            Some(&class("ApiClient", false))
        );
        assert_eq!(
            tables.returns.get("makeClient"),
            Some(&class("ApiClient", false))
        );
        assert_eq!(
            tables.returns.get("openClient"),
            Some(&class("ApiClient", true))
        );
        assert_eq!(
            tables.returns.get("getClient"),
            Some(&StatedReturn::Call {
                callee: "createClient".to_string(),
                promised: false,
            })
        );
        assert_eq!(
            tables.returns.get("sharedClient"),
            Some(&StatedReturn::Call {
                callee: "openClient".to_string(),
                promised: true,
            })
        );
    }

    #[test]
    fn an_annotation_that_names_no_one_type_states_nothing_and_hides_the_body() {
        let tables = factories(
            r#"
            export function either(): ApiClient | BatchClient { return new ApiClient(); }
            export function qualified(): core.ApiClient { return new ApiClient(); }
            export function generic<T>(): T { return new ApiClient() as T; }
            export function many(): Promise<ApiClient[]> { return load(); }
            export function bare(): Promise { return load(); }
            "#,
        );
        assert!(tables.returns.is_empty(), "{:?}", tables.returns);
    }

    #[test]
    fn a_body_with_a_second_statement_states_nothing() {
        let tables = factories(
            r#"
            export function byMode(batch: boolean) {
              if (batch) { return new BatchClient(); }
              return new ApiClient();
            }
            export function logged() {
              log("creating");
              return new ApiClient();
            }
            export const viaMember = () => registry.create();
            export const viaLocal = () => { const c = new ApiClient(); return c; };
            "#,
        );
        assert!(tables.returns.is_empty(), "{:?}", tables.returns);
    }

    #[test]
    fn a_function_declared_twice_with_different_returns_is_dropped() {
        let tables = factories(
            r#"
            export function create(): ApiClient;
            export function create(mode: string): BatchClient;
            export function create(mode?: string) { return pick(mode); }
            "#,
        );
        assert_eq!(tables.returns.get("create"), None);
    }

    #[test]
    fn a_module_scope_const_bound_from_a_factory_call_is_recorded() {
        let tables = factories(
            r#"
            export const client = createClient();
            export const remote = await openClient();
            export let rebound = createClient();
            export const typed: ApiClient = createClient();
            export const viaMember = registry.create();
            export const asserted = createClient() as BatchClient;
            export const { part } = createClient();
            function inner() { const hidden = createClient(); return hidden; }
            "#,
        );
        assert_eq!(
            tables.bindings.get("client"),
            Some(&FactoryCall {
                callee: "createClient".to_string(),
                awaited: false,
            })
        );
        assert_eq!(
            tables.bindings.get("remote"),
            Some(&FactoryCall {
                callee: "openClient".to_string(),
                awaited: true,
            })
        );
        assert_eq!(tables.bindings.len(), 2, "{:?}", tables.bindings);
    }

    #[test]
    fn an_annotation_wins_over_the_initialiser() {
        let types = types("const client: ApiClient = new CachingClient();");
        assert_eq!(types.get("client").map(String::as_str), Some("ApiClient"));
    }

    #[test]
    fn a_qualified_type_name_states_nothing() {
        let types = types("function run(client: core.ApiClient) { return client.list(); }");
        assert_eq!(types.get("client"), None);
    }

    #[test]
    fn a_name_declared_two_different_ways_in_one_scope_is_dropped() {
        let types = types(
            r#"
            function run(client: ApiClient) {
              const inner = (client: BatchClient) => client.list();
              return inner;
            }
            "#,
        );
        assert_eq!(types.get("client"), None);
    }

    #[test]
    fn a_name_typed_once_and_untyped_once_in_one_scope_is_dropped() {
        let types = types(
            r#"
            function run(client: ApiClient) {
              const client = useClient();
              return client.list();
            }
            "#,
        );
        assert_eq!(types.get("client"), None);
    }

    #[test]
    fn a_destructured_binding_states_nothing() {
        let types = types("function run({ client }: Deps) { return client.list(); }");
        assert_eq!(types.get("client"), None);
    }

    /// Walk the module and read the fields of its first class declaration.
    fn fields(source: &str) -> ReceiverTypes {
        let (_, module) = parse_standalone_module(Path::new("fields.ts"), source).expect("parses");
        let class = module
            .body
            .iter()
            .find_map(|item| match item {
                ModuleItem::Stmt(Stmt::Decl(Decl::Class(class))) => Some(&class.class),
                ModuleItem::ModuleDecl(ModuleDecl::ExportDecl(export)) => match &export.decl {
                    Decl::Class(class) => Some(&class.class),
                    _ => None,
                },
                _ => None,
            })
            .expect("a class declaration");
        class_field_types(class)
    }

    #[test]
    fn an_annotated_field_states_its_class() {
        let fields = fields(
            r#"
            export class Manager {
              private readonly client: ApiClient;
              #cache: CacheClient;
              static shared: SharedClient;
            }
            "#,
        );
        assert_eq!(fields.get("client").map(String::as_str), Some("ApiClient"));
        assert_eq!(
            fields.get("#cache").map(String::as_str),
            Some("CacheClient")
        );
        // A static field is reached through the class name, not `this`.
        assert_eq!(fields.get("shared"), None);
    }

    #[test]
    fn a_constructor_parameter_property_states_its_class() {
        let fields = fields(
            r#"
            export class Manager {
              constructor(
                private readonly client: ApiClient,
                public cache: CacheClient = new CacheClient(),
                plain: ApiClient,
              ) {}
            }
            "#,
        );
        assert_eq!(fields.get("client").map(String::as_str), Some("ApiClient"));
        assert_eq!(fields.get("cache").map(String::as_str), Some("CacheClient"));
        // A plain parameter is not a field: no modifier, so nothing is
        // assigned to `this`.
        assert_eq!(fields.get("plain"), None);
    }

    #[test]
    fn an_unannotated_field_states_nothing() {
        let fields = fields(
            r#"
            export class Manager {
              client = new ApiClient();
              other;
              constructor(private readonly deps) {}
            }
            "#,
        );
        assert_eq!(fields.get("client"), None);
        assert_eq!(fields.get("other"), None);
        assert_eq!(fields.get("deps"), None);
    }

    #[test]
    fn a_field_declared_two_different_ways_is_dropped() {
        let fields = fields(
            r#"
            export class Manager {
              client: ApiClient;
              constructor(private readonly client: BatchClient) {}
            }
            "#,
        );
        assert_eq!(fields.get("client"), None);
    }

    #[test]
    fn a_field_annotated_once_and_left_bare_once_is_dropped() {
        let fields = fields(
            r#"
            export class Manager {
              client: ApiClient;
              constructor(public client = makeClient()) {}
            }
            "#,
        );
        assert_eq!(fields.get("client"), None);
    }

    #[test]
    fn the_same_type_declared_twice_survives() {
        let types = types(
            r#"
            function run(client: ApiClient) {
              const again: ApiClient = client;
              return again.list();
            }
            "#,
        );
        assert_eq!(types.get("client").map(String::as_str), Some("ApiClient"));
        assert_eq!(types.get("again").map(String::as_str), Some("ApiClient"));
    }
}
