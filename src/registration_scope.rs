//! Registration scopes: routes registered inside an inline plugin
//! (carrick#2092).
//!
//! `app.register(async (api) => { api.register(items) }, { prefix: '/api/v1' })`
//! hands the callback a fresh instance, `api`, whose routes are served under
//! the prefix the registration states. The model reads the rows inside the
//! callback as hanging from `api`, a name every inline plugin in a repo
//! shares, so two plugins collided on one graph node and the prefix was
//! either lost or joined as raw source text (`opts.prefix ?? '/api/v1'`).
//!
//! This pass gives each such callback parameter an identity of its own, the
//! scope id `api@<file>:<line>`, and states the registration's prefix only
//! where the source states it as a literal:
//!
//! * phase 1 ([`collect_registration_sites`]) reads every call that passes one
//!   inline function, with the value each other argument reads to;
//! * after the join ([`apply_registration_scopes`]) the rows hanging from the
//!   callback parameter are re-pointed to the scope id, and the registration
//!   row is corrected or emitted.
//!
//! A prefix the source does not state as a literal is recorded as unread and
//! never guessed. Nothing here reads a framework, a library or an option name.
//!
//! The scope does not depend on a model row inside the callback
//! (carrick#2303). Phase 1 also reads every call on the callback's parameter
//! that passes an imported or locally declared function first, and once every
//! file's rows are in ([`mount_unstated_registrations`]) each one the model
//! stated no mount for becomes a mount row, where the function it names
//! registers on its own first parameter.
//!
//! A callback that hands its instance to a helper as an argument is read the
//! same way (carrick#2316): `helper(api, deps)` inside the callback mounts
//! `helper` from the scope when the function registers on the parameter that
//! receives the instance, or passes that parameter on to a function that
//! does. The instance is followed by binding from the callback's parameter to
//! each function's parameter, never by name, and a call that hands over
//! anything else (a member, a call's result, a spread) is not followed.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use swc_common::{
    SourceMap, SourceMapper, Spanned,
    errors::{ColorConfig, Handler},
    sync::Lrc,
};
use swc_ecma_ast::*;
use swc_ecma_visit::{Visit, VisitWith};
use tracing::debug;

use crate::agents::file_analyzer_agent::{FileAnalysisResult, MountResult};
use crate::binding_scope::{BindingKey, ident_key};
use crate::graphql_document_sites::unwrap_expression;
use crate::import_bindings::BindingResolver;
use crate::request_summary::literal_specifier;
use crate::workspace_resolver::WorkspaceIndex;

/// How many bindings a prefix read may follow before it gives up.
const MAX_BINDING_DEPTH: usize = 4;

/// How many functions an instance is followed through after the one the
/// callback hands it to (carrick#2316).
const MAX_PASS_ON_DEPTH: usize = 3;

/// What a prefix expression reads to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PrefixRead {
    /// The literal values the expression can take: one, or two for a
    /// conditional whose branches both read.
    Stated(Vec<String>),
    /// The source does not state the value as a literal.
    Unread,
}

/// One non-function argument value of a registration call: the argument
/// itself, or one property value of an object-literal argument.
#[derive(Debug, Clone)]
pub struct OptionValue {
    /// The value's source text, whitespace removed and quotes made one kind,
    /// so the model's copy of it can be found ([`normalise_source_text`]).
    pub text: String,
    pub read: PrefixRead,
}

/// A call `R.m(…)` that passes exactly one inline function, read in phase 1.
#[derive(Debug, Clone)]
pub struct RegistrationSite {
    /// The call's start line.
    pub line: u32,
    /// `R`: the instance the registration is made on.
    pub receiver: String,
    /// `m`, kept for the emitted row's `pattern_matched`.
    pub method: String,
    /// The inline function's first parameter: the instance it is handed.
    pub param: String,
    /// The inline function's first and last lines.
    pub body_start_line: u32,
    pub body_end_line: u32,
    /// Every other argument value, in source order.
    pub option_values: Vec<OptionValue>,
    /// The source text of the first other argument, quoted when the prefix
    /// it may carry is recorded as unread.
    pub options_text: Option<String>,
    /// What the callback registers on the instance it is handed, in source
    /// order (carrick#2303).
    pub inner_registrations: Vec<InnerRegistration>,
    /// What the callback hands its instance to through an option of the
    /// function the registration is written in, in source order
    /// (carrick#2316).
    pub hand_outs: Vec<HandOut>,
}

/// A call `O.k(…, P, …)` inside a registration callback, where `P` is the
/// callback's instance and `O` a parameter of the named function the
/// registration is written in, both by binding (carrick#2316):
///
/// ```ts
/// export async function routesPlugin(app, opts) {
///   await app.register(async (api) => {
///     await opts.registerExtra(api);        // the hand-out
///   }, { prefix: '/api/v1' });
/// }
/// ```
///
/// The function `k` names is supplied by whoever passes `routesPlugin` an
/// options object, often another package: see
/// [`mount_unstated_registrations`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HandOut {
    /// The call's start line.
    pub line: u32,
    /// The name of the function the registration is written in.
    pub function: String,
    /// Which parameter of that function `O` is, counted from 0.
    pub options_position: usize,
    /// `k`, written as a plain name.
    pub key: String,
    /// Which argument of the call is the instance, counted from 0.
    pub position: usize,
    /// `O.k(`, kept for the emitted row's `pattern_matched`.
    pub pattern: String,
}

/// A call inside a registration callback that hands the callback's instance
/// `P` (its first parameter) to a function `F`, an identifier bound to an
/// import or to a function this file declares:
///
/// * `P.m(F, …)`, which calls `F` with the instance first;
/// * `F(…, P, …)`, which calls `F` with the instance at the position `P` is
///   written at (carrick#2316).
///
/// Whether `F` registers anything is not something this file says: see
/// [`mount_unstated_registrations`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InnerRegistration {
    /// The call's start line.
    pub line: u32,
    /// `m` of `P.m(F, …)`, kept for the emitted row's `pattern_matched`;
    /// `None` for the bare call `F(…, P, …)`.
    pub method: Option<String>,
    /// `F`, as this file names it.
    pub identifier: String,
    /// The specifier `F` is imported from, as written; `None` for a function
    /// this file declares.
    pub import_source: Option<String>,
    /// Which parameter of `F` receives the instance, counted from 0.
    pub position: usize,
}

impl InnerRegistration {
    /// The call as a mount row of the file that makes it, hanging from
    /// `instance`, the node the instance handed over is, and mounting
    /// `child`, the node the function called is ([`InstanceTrace::node`]).
    fn mount_row(&self, instance: &str, child: &str) -> MountResult {
        MountResult {
            line_number: i32::try_from(self.line).unwrap_or(i32::MAX),
            parent_node: instance.to_string(),
            child_node: child.to_string(),
            mount_path: String::new(),
            import_source: self.import_source.clone(),
            pattern_matched: match &self.method {
                Some(method) => format!("{instance}.{method}("),
                None => format!("{}(", self.identifier),
            },
        }
    }
}

/// A registration scope whose prefix the source does not state as a literal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnreadPrefix {
    /// `<repo-relative file>:<line>` of the registration call.
    pub site: String,
    pub scope_id: String,
    /// The expression the prefix may be in, as written.
    pub expression: String,
    /// Every distinct route path the registration's other arguments read to
    /// (carrick#2092 slice 1b): the source states each as a literal, but
    /// nothing says which option, if any, is the prefix. The mount graph
    /// accepts one only where the service's own route descriptors agree.
    pub candidates: Vec<String>,
}

impl RegistrationSite {
    /// The distinct non-empty route paths this call's other arguments read
    /// to, in source order.
    fn prefix_candidates(&self) -> Vec<String> {
        let mut candidates: Vec<String> = Vec::new();
        for value in &self.option_values {
            if let PrefixRead::Stated(values) = &value.read {
                for value in values {
                    if !value.is_empty() && !candidates.contains(value) {
                        candidates.push(value.clone());
                    }
                }
            }
        }
        candidates
    }
}

/// The graph node a registration scope stands for. An identifier never holds
/// `@`, so a scope id cannot collide with a binding name.
pub fn scope_id(param: &str, file: &str, line: u32) -> String {
    format!("{param}@{file}:{line}")
}

/// Whether a graph node is a registration scope, so its owner is already
/// resolved and no name-keyed lookup may override it.
pub fn is_scope_id(node: &str) -> bool {
    node.contains('@')
        && node
            .split('@')
            .next()
            .is_some_and(|name| !name.is_empty() && !name.contains(['/', ' ', '.']))
}

/// Source text as the model may copy it: whitespace removed, `"` read as `'`.
pub fn normalise_source_text(text: &str) -> String {
    text.chars()
        .filter(|c| !c.is_whitespace())
        .map(|c| if c == '"' { '\'' } else { c })
        .collect()
}

/// Whether a model's `mount_path` is already a path rather than source text.
fn is_route_literal(text: &str) -> bool {
    text.is_empty() || is_route_path(text) && !text.contains(['\'', '"', '`', '?', '|'])
}

/// A value a route prefix can be: empty, or `/`-led with no whitespace and
/// no scheme.
fn is_route_path(value: &str) -> bool {
    value.is_empty()
        || (value.starts_with('/')
            && !value.contains("://")
            && !value.chars().any(char::is_whitespace))
}

/// A path a route is served at: `/`-led. A row whose path is anything else
/// (a header name read off a request, a content type) registers nothing, so
/// it cannot make a callback a scope.
fn is_served_path(path: &str) -> bool {
    path.starts_with('/')
}

/// Every registration site in a module. `source_map` must be the one the
/// module was parsed with, and the module must have been through the
/// resolver: bindings are read by scope, never by name.
pub fn collect_registration_sites(
    module: &Module,
    source_map: &Lrc<SourceMap>,
) -> Vec<RegistrationSite> {
    // The declarations are collected only for a module that has a site: most
    // files register nothing inline.
    let mut probe = SiteFinder {
        source_map,
        values: None,
        sites: Vec::new(),
        enclosing: Vec::new(),
        naming: None,
    };
    module.visit_with(&mut probe);
    if probe.sites.is_empty() {
        return Vec::new();
    }
    let mut values = DeclaredValues::default();
    module.visit_with(&mut values);
    let mut finder = SiteFinder {
        source_map,
        values: Some(&values),
        sites: Vec::new(),
        enclosing: Vec::new(),
        naming: None,
    };
    module.visit_with(&mut finder);
    finder.sites
}

/// Reads the calls that hand one instance to a function (carrick#2303,
/// carrick#2316): inside a registration callback the instance is the
/// callback's first parameter, and inside a function the instance was passed
/// to it is the parameter that received it. The instance and the function are
/// both read by binding, so a name a block inside declares again is never
/// taken for either.
struct InnerFinder<'s> {
    source_map: &'s Lrc<SourceMap>,
    values: &'s DeclaredValues,
    param: BindingKey,
    found: Vec<InnerRegistration>,
}

impl InnerFinder<'_> {
    /// The function an identifier names: `Some(import specifier)` for an
    /// import, `Some(None)` for a function this file declares, `None` for a
    /// value this file binds some other way, which names no function.
    fn function_named(&self, ident: &Ident) -> Option<Option<String>> {
        let key = ident_key(ident);
        match self.values.imports.get(&key) {
            Some(specifier) => Some(Some(specifier.clone())),
            None => self.values.functions.contains(&key).then_some(None),
        }
    }

    fn line(&self, call: &CallExpr) -> u32 {
        u32::try_from(self.source_map.lookup_char_pos(call.span.lo).line).unwrap_or(0)
    }

    /// `P.m(F, …)`: `F` is called with the instance first.
    fn passed_to_the_instance(&mut self, call: &CallExpr, member: &MemberExpr) {
        let Expr::Ident(receiver) = unwrap_expression(&member.obj) else {
            return;
        };
        let MemberProp::Ident(method) = &member.prop else {
            return;
        };
        if ident_key(receiver) != self.param {
            return;
        }
        let Some(first) = call.args.first().filter(|arg| arg.spread.is_none()) else {
            return;
        };
        let Expr::Ident(passed) = unwrap_expression(&first.expr) else {
            return;
        };
        let Some(import_source) = self.function_named(passed) else {
            return;
        };
        self.found.push(InnerRegistration {
            line: self.line(call),
            method: Some(method.sym.to_string()),
            identifier: passed.sym.to_string(),
            import_source,
            position: 0,
        });
    }

    /// `F(…, P, …)`: `F` is called with the instance at the position it is
    /// written at. A spread argument leaves every position after it unknown,
    /// and an instance written twice names no one parameter, so neither is
    /// recorded. An argument that only holds the instance (`P.inner`,
    /// `wrap(P)`, `{ P }`) is another value.
    fn handed_the_instance(&mut self, call: &CallExpr, function: &Ident) {
        if call.args.iter().any(|arg| arg.spread.is_some()) {
            return;
        }
        let mut positions = call.args.iter().enumerate().filter(|(_, arg)| {
            matches!(unwrap_expression(&arg.expr), Expr::Ident(passed) if ident_key(passed) == self.param)
        });
        let (Some((position, _)), None) = (positions.next(), positions.next()) else {
            return;
        };
        let Some(import_source) = self.function_named(function) else {
            return;
        };
        self.found.push(InnerRegistration {
            line: self.line(call),
            method: None,
            identifier: function.sym.to_string(),
            import_source,
            position,
        });
    }
}

impl Visit for InnerFinder<'_> {
    fn visit_call_expr(&mut self, call: &CallExpr) {
        call.visit_children_with(self);
        let Callee::Expr(callee) = &call.callee else {
            return;
        };
        match unwrap_expression(callee) {
            Expr::Member(member) => self.passed_to_the_instance(call, member),
            Expr::Ident(function) => self.handed_the_instance(call, function),
            _ => {}
        }
    }
}

/// The named function a registration is written in: its name and each
/// parameter's binding, `None` where the parameter is not one name.
#[derive(Debug, Clone)]
struct Enclosing {
    name: String,
    params: Vec<Option<BindingKey>>,
}

impl Enclosing {
    fn of<'p>(name: String, params: impl Iterator<Item = &'p Pat>) -> Self {
        let params = params
            .map(|param| match param {
                Pat::Ident(ident) => Some(ident_key(&ident.id)),
                // `opts = {}`: the default changes nothing about the binding.
                Pat::Assign(assign) => match assign.left.as_ref() {
                    Pat::Ident(ident) => Some(ident_key(&ident.id)),
                    _ => None,
                },
                _ => None,
            })
            .collect();
        Self { name, params }
    }
}

/// Reads the hand-outs inside one registration callback (carrick#2316): calls
/// `O.k(…, P, …)` and `O.k?.(…, P, …)`, `O` a parameter of the enclosing
/// function and `P` the callback's instance, both by binding. A computed key,
/// a spread argument or an instance written twice is not recorded.
struct HandOutFinder<'s> {
    source_map: &'s Lrc<SourceMap>,
    enclosing: &'s Enclosing,
    param: BindingKey,
    found: Vec<HandOut>,
}

impl HandOutFinder<'_> {
    fn consider(&mut self, span: swc_common::Span, callee: &Expr, args: &[ExprOrSpread]) {
        let member = match unwrap_expression(callee) {
            Expr::Member(member) => member,
            Expr::OptChain(chain) => match chain.base.as_ref() {
                OptChainBase::Member(member) => member,
                OptChainBase::Call(_) => return,
            },
            _ => return,
        };
        let Expr::Ident(options) = unwrap_expression(&member.obj) else {
            return;
        };
        let MemberProp::Ident(key) = &member.prop else {
            return;
        };
        let options_key = ident_key(options);
        let Some(options_position) = self
            .enclosing
            .params
            .iter()
            .position(|param| param.as_ref() == Some(&options_key))
        else {
            return;
        };
        if args.iter().any(|arg| arg.spread.is_some()) {
            return;
        }
        let mut positions = args.iter().enumerate().filter(|(_, arg)| {
            matches!(unwrap_expression(&arg.expr), Expr::Ident(passed) if ident_key(passed) == self.param)
        });
        let (Some((position, _)), None) = (positions.next(), positions.next()) else {
            return;
        };
        self.found.push(HandOut {
            line: u32::try_from(self.source_map.lookup_char_pos(span.lo).line).unwrap_or(0),
            function: self.enclosing.name.clone(),
            options_position,
            key: key.sym.to_string(),
            position,
            pattern: format!("{}.{}(", options.sym, key.sym),
        });
    }
}

impl Visit for HandOutFinder<'_> {
    fn visit_call_expr(&mut self, call: &CallExpr) {
        call.visit_children_with(self);
        if let Callee::Expr(callee) = &call.callee {
            self.consider(call.span, callee, &call.args);
        }
    }

    fn visit_opt_chain_expr(&mut self, chain: &OptChainExpr) {
        chain.visit_children_with(self);
        if let OptChainBase::Call(call) = chain.base.as_ref() {
            self.consider(call.span, &call.callee, &call.args);
        }
    }
}

/// Finds the sites. Without `values` it only finds them (their arguments
/// read as unread); with them it reads each argument value.
struct SiteFinder<'s> {
    source_map: &'s Lrc<SourceMap>,
    values: Option<&'s DeclaredValues>,
    sites: Vec<RegistrationSite>,
    /// The functions around the call being read, innermost last; `None` for
    /// one with no name of its own (carrick#2316).
    enclosing: Vec<Option<Enclosing>>,
    /// The name a declarator gives the function it is initialised with,
    /// until that function is entered.
    naming: Option<String>,
}

impl SiteFinder<'_> {
    fn line(&self, pos: swc_common::BytePos) -> u32 {
        u32::try_from(self.source_map.lookup_char_pos(pos).line).unwrap_or(0)
    }

    fn snippet(&self, expr: &Expr) -> String {
        self.source_map
            .span_to_snippet(expr.span())
            .unwrap_or_default()
    }

    fn option_value(&self, expr: &Expr) -> OptionValue {
        OptionValue {
            text: normalise_source_text(&self.snippet(expr)),
            read: match self.values {
                Some(values) => read_prefix(expr, values, 0),
                None => PrefixRead::Unread,
            },
        }
    }

    fn consider(&mut self, call: &CallExpr) {
        let Callee::Expr(callee) = &call.callee else {
            return;
        };
        let Expr::Member(member) = unwrap_expression(callee) else {
            return;
        };
        let Expr::Ident(receiver) = unwrap_expression(&member.obj) else {
            return;
        };
        let MemberProp::Ident(method) = &member.prop else {
            return;
        };
        let mut inline: Vec<(&Expr, &BindingIdent)> = Vec::new();
        let mut others: Vec<&Expr> = Vec::new();
        for arg in &call.args {
            if arg.spread.is_some() {
                continue;
            }
            let first_param = match unwrap_expression(&arg.expr) {
                Expr::Arrow(arrow) => Some(arrow.params.first()),
                Expr::Fn(function) => Some(function.function.params.first().map(|p| &p.pat)),
                _ => None,
            };
            match first_param {
                // An inline function with no plain first parameter hands out
                // no instance this pass can name.
                Some(Some(Pat::Ident(param))) => inline.push((&arg.expr, param)),
                Some(_) => return,
                None => others.push(&arg.expr),
            }
        }
        let [(callback, param)] = inline.as_slice() else {
            return;
        };
        let span = callback.span();
        let mut inner_registrations = Vec::new();
        let mut hand_outs = Vec::new();
        if let Some(values) = self.values {
            let mut inner = InnerFinder {
                source_map: self.source_map,
                values,
                param: ident_key(&param.id),
                found: Vec::new(),
            };
            callback.visit_with(&mut inner);
            inner.found.sort_by_key(|found| found.line);
            inner_registrations = inner.found;
            // The call is read before the callback is entered, so the
            // innermost function around it is the one it is written in.
            if let Some(Some(enclosing)) = self.enclosing.last() {
                let mut finder = HandOutFinder {
                    source_map: self.source_map,
                    enclosing,
                    param: ident_key(&param.id),
                    found: Vec::new(),
                };
                callback.visit_with(&mut finder);
                finder.found.sort_by_key(|found| found.line);
                hand_outs = finder.found;
            }
        }
        let mut option_values = Vec::new();
        for expr in &others {
            match unwrap_expression(expr) {
                Expr::Object(object) => {
                    for prop in &object.props {
                        let PropOrSpread::Prop(prop) = prop else {
                            continue; // a spread names no value of its own
                        };
                        match prop.as_ref() {
                            Prop::KeyValue(kv) => option_values.push(self.option_value(&kv.value)),
                            Prop::Shorthand(ident) => {
                                option_values.push(self.option_value(&Expr::Ident(ident.clone())))
                            }
                            _ => {}
                        }
                    }
                }
                other => option_values.push(self.option_value(other)),
            }
        }
        self.sites.push(RegistrationSite {
            line: self.line(call.span.lo),
            receiver: receiver.sym.to_string(),
            method: method.sym.to_string(),
            param: param.id.sym.to_string(),
            body_start_line: self.line(span.lo),
            body_end_line: self.line(span.hi),
            option_values,
            options_text: others.first().map(|expr| self.snippet(expr)),
            inner_registrations,
            hand_outs,
        });
    }
}

impl Visit for SiteFinder<'_> {
    fn visit_call_expr(&mut self, call: &CallExpr) {
        self.consider(call);
        call.visit_children_with(self);
    }

    fn visit_fn_decl(&mut self, decl: &FnDecl) {
        self.naming = Some(decl.ident.sym.to_string());
        decl.visit_children_with(self);
    }

    fn visit_fn_expr(&mut self, function: &FnExpr) {
        if let Some(name) = &function.ident {
            self.naming = Some(name.sym.to_string());
        }
        function.visit_children_with(self);
    }

    fn visit_var_declarator(&mut self, declarator: &VarDeclarator) {
        let named = match (&declarator.name, declarator.init.as_deref()) {
            (Pat::Ident(name), Some(init))
                if matches!(unwrap_expression(init), Expr::Arrow(_) | Expr::Fn(_)) =>
            {
                Some(name.id.sym.to_string())
            }
            _ => None,
        };
        declarator.name.visit_with(self);
        self.naming = named;
        declarator.init.visit_with(self);
        self.naming = None;
    }

    /// A function body, a method's included: named only when a declaration,
    /// a named expression or a declarator just named it.
    fn visit_function(&mut self, function: &Function) {
        let name = self.naming.take();
        self.enclosing.push(
            name.map(|name| Enclosing::of(name, function.params.iter().map(|param| &param.pat))),
        );
        function.visit_children_with(self);
        self.enclosing.pop();
    }

    fn visit_arrow_expr(&mut self, arrow: &ArrowExpr) {
        let name = self.naming.take();
        self.enclosing
            .push(name.map(|name| Enclosing::of(name, arrow.params.iter())));
        arrow.visit_children_with(self);
        self.enclosing.pop();
    }
}

/// What each binding a prefix can be read through holds: a `const`
/// initialiser, a parameter default, or a destructuring default. Keyed by the
/// binding's scope, never its name.
#[derive(Default)]
struct DeclaredValues {
    values: HashMap<BindingKey, Expr>,
    /// The value imports of the module, by the binding each declares.
    imports: HashMap<BindingKey, String>,
    /// The bindings that name a function this module declares: a function
    /// declaration, or a declarator initialised with a function.
    functions: HashSet<BindingKey>,
}

impl DeclaredValues {
    /// Record the written defaults inside a pattern. `init` is the value the
    /// whole pattern is bound to, read only when the pattern is one name.
    fn bind(&mut self, pat: &Pat, init: Option<&Expr>) {
        match pat {
            Pat::Ident(ident) => {
                if let Some(init) = init {
                    self.values.insert(ident_key(&ident.id), init.clone());
                }
            }
            // `name = '/x'` or `{ name = '/x' } = {}`: the right side is the
            // written default.
            Pat::Assign(assign) => self.bind(&assign.left, Some(&assign.right)),
            Pat::Object(object) => {
                for prop in &object.props {
                    match prop {
                        ObjectPatProp::Assign(assign) => {
                            if let Some(default) = &assign.value {
                                self.values
                                    .insert(ident_key(&assign.key.id), (**default).clone());
                            }
                        }
                        ObjectPatProp::KeyValue(kv) => self.bind(&kv.value, None),
                        ObjectPatProp::Rest(_) => {}
                    }
                }
            }
            _ => {}
        }
    }
}

impl Visit for DeclaredValues {
    fn visit_var_decl(&mut self, decl: &VarDecl) {
        if decl.kind == VarDeclKind::Const {
            for declarator in &decl.decls {
                self.bind(&declarator.name, declarator.init.as_deref());
            }
        }
        for declarator in &decl.decls {
            if let (Pat::Ident(name), Some(init)) = (&declarator.name, &declarator.init)
                && matches!(unwrap_expression(init), Expr::Arrow(_) | Expr::Fn(_))
            {
                self.functions.insert(ident_key(&name.id));
            }
        }
        decl.visit_children_with(self);
    }

    fn visit_fn_decl(&mut self, decl: &FnDecl) {
        self.functions.insert(ident_key(&decl.ident));
        decl.visit_children_with(self);
    }

    fn visit_import_decl(&mut self, import: &ImportDecl) {
        if import.type_only {
            return;
        }
        let specifier = import.src.value.to_string_lossy().into_owned();
        for named in &import.specifiers {
            let local = match named {
                ImportSpecifier::Named(named) if !named.is_type_only => &named.local,
                ImportSpecifier::Default(default) => &default.local,
                // A namespace binds a module, never one function of it.
                _ => continue,
            };
            self.imports.insert(ident_key(local), specifier.clone());
        }
    }

    fn visit_param(&mut self, param: &Param) {
        self.bind_param(&param.pat);
        param.visit_children_with(self);
    }

    fn visit_arrow_expr(&mut self, arrow: &ArrowExpr) {
        for param in &arrow.params {
            self.bind_param(param);
        }
        arrow.visit_children_with(self);
    }
}

impl DeclaredValues {
    /// A parameter states a value only through a default: a plain name holds
    /// whatever the caller passes.
    fn bind_param(&mut self, pat: &Pat) {
        if !matches!(pat, Pat::Ident(_)) {
            self.bind(pat, None);
        }
    }
}

/// Read a prefix expression to the literal values the source states for it.
///
/// * a string literal, or a template with no substitution, is its value;
/// * `a ?? b` and `a || b` read `a`, else the written default `b`;
/// * `c ? a : b` reads both values when both branches read;
/// * an identifier reads through the declaration its scope names (a `const`
///   initialiser or a written default), at most [`MAX_BINDING_DEPTH`] deep;
/// * anything else, and any value that is not a route path, is unread.
fn read_prefix(expr: &Expr, values: &DeclaredValues, depth: usize) -> PrefixRead {
    let expr = unwrap_expression(expr);
    if let Some(value) = literal_specifier(expr) {
        return if is_route_path(&value) {
            PrefixRead::Stated(vec![value])
        } else {
            PrefixRead::Unread
        };
    }
    match expr {
        Expr::Bin(bin) if matches!(bin.op, BinaryOp::NullishCoalescing | BinaryOp::LogicalOr) => {
            match read_prefix(&bin.left, values, depth) {
                PrefixRead::Unread => read_prefix(&bin.right, values, depth),
                stated => stated,
            }
        }
        Expr::Cond(cond) => match (
            read_prefix(&cond.cons, values, depth),
            read_prefix(&cond.alt, values, depth),
        ) {
            (PrefixRead::Stated(mut cons), PrefixRead::Stated(alt)) => {
                for value in alt {
                    if !cons.contains(&value) {
                        cons.push(value);
                    }
                }
                if cons.len() <= 2 {
                    PrefixRead::Stated(cons)
                } else {
                    PrefixRead::Unread
                }
            }
            _ => PrefixRead::Unread,
        },
        Expr::Ident(ident) if depth < MAX_BINDING_DEPTH => {
            match values.values.get(&ident_key(ident)) {
                Some(init) => read_prefix(init, values, depth + 1),
                None => PrefixRead::Unread,
            }
        }
        _ => PrefixRead::Unread,
    }
}

/// Give every registration scope in one file's joined rows its own node, and
/// state its prefix where the source does. Returns the scopes whose prefix
/// the source does not state, each logged once.
///
/// `file` is the repo-relative path the index stores, never an absolute one:
/// the scope id is part of the graph every reader sees.
pub fn apply_registration_scopes(
    result: &mut FileAnalysisResult,
    sites: &[RegistrationSite],
    file: &str,
) -> Vec<UnreadPrefix> {
    let mut unread = Vec::new();
    if sites.is_empty() {
        return unread;
    }
    // Innermost first, so a nested scope that rebinds the parameter's name
    // claims its own rows before the scope around it.
    let mut order: Vec<&RegistrationSite> = sites.iter().collect();
    order.sort_by_key(|site| (site.body_end_line - site.body_start_line, site.line));

    let mut mount_claimed = vec![false; result.mounts.len()];
    let mut registration_rows = vec![false; result.mounts.len()];
    let mut endpoint_claimed = vec![false; result.endpoints.len()];
    // Rows to append once every scope is read, and rows to replace.
    let mut emitted: Vec<MountResult> = Vec::new();
    let mut fanned: Vec<(usize, Vec<String>)> = Vec::new();

    for site in order {
        let inside = |line: i32| {
            u32::try_from(line)
                .is_ok_and(|line| line >= site.body_start_line && line <= site.body_end_line)
        };
        if registers_a_route(result, site) {
            continue;
        }
        let registrations: Vec<usize> = result
            .mounts
            .iter()
            .enumerate()
            .filter(|(index, mount)| {
                !registration_rows[*index]
                    && u32::try_from(mount.line_number) == Ok(site.line)
                    && mount.parent_node == site.receiver
            })
            .map(|(index, _)| index)
            .collect();
        let hanging_mounts: Vec<usize> = result
            .mounts
            .iter()
            .enumerate()
            .filter(|(index, mount)| {
                !mount_claimed[*index]
                    && !registrations.contains(index)
                    && mount.parent_node == site.param
                    && inside(mount.line_number)
            })
            .map(|(index, _)| index)
            .collect();
        let hanging_endpoints: Vec<usize> = result
            .endpoints
            .iter()
            .enumerate()
            .filter(|(index, endpoint)| {
                !endpoint_claimed[*index]
                    && endpoint.owner_node == site.param
                    && is_served_path(&endpoint.path)
                    && inside(endpoint.line_number)
            })
            .map(|(index, _)| index)
            .collect();
        if hanging_mounts.is_empty() && hanging_endpoints.is_empty() {
            continue; // a callback that heads no rows is not a scope
        }

        let id = scope_id(&site.param, file, site.line);
        for index in hanging_mounts {
            mount_claimed[index] = true;
            result.mounts[index].parent_node = id.clone();
        }
        for index in hanging_endpoints {
            endpoint_claimed[index] = true;
            result.endpoints[index].owner_node = id.clone();
        }

        let site_text = format!("{file}:{}", site.line);
        if registrations.is_empty() {
            // The model stated no row for the registration: the scope still
            // hangs from the instance it was registered on.
            emitted.push(MountResult {
                line_number: i32::try_from(site.line).unwrap_or(i32::MAX),
                parent_node: site.receiver.clone(),
                child_node: id.clone(),
                mount_path: String::new(),
                import_source: None,
                pattern_matched: format!("{}.{}(", site.receiver, site.method),
            });
            if let Some(text) = &site.options_text {
                unread.push(UnreadPrefix {
                    site: site_text.clone(),
                    scope_id: id.clone(),
                    expression: text.clone(),
                    candidates: site.prefix_candidates(),
                });
            }
            continue;
        }
        for index in registrations {
            registration_rows[index] = true;
            let mount = &mut result.mounts[index];
            mount.child_node = id.clone();
            if is_route_literal(&mount.mount_path) {
                continue;
            }
            // The model wrote the prefix as source text: read the argument
            // value it names.
            let wanted = normalise_source_text(&mount.mount_path);
            let read = site
                .option_values
                .iter()
                .find(|value| value.text == wanted)
                .map(|value| value.read.clone())
                .unwrap_or(PrefixRead::Unread);
            match read {
                PrefixRead::Stated(values) => fanned.push((index, values)),
                PrefixRead::Unread => {
                    unread.push(UnreadPrefix {
                        site: site_text.clone(),
                        scope_id: id.clone(),
                        expression: mount.mount_path.clone(),
                        candidates: site.prefix_candidates(),
                    });
                    mount.mount_path = String::new();
                }
            }
        }
    }

    // A prefix read to two values serves the scope under each of them.
    for (index, values) in fanned {
        let mut values = values.into_iter();
        let Some(first) = values.next() else {
            continue;
        };
        for value in values {
            let mut copy = result.mounts[index].clone();
            copy.mount_path = value;
            emitted.push(copy);
        }
        result.mounts[index].mount_path = first;
    }
    result.mounts.extend(emitted);

    for record in &unread {
        debug!(
            "Mount prefix unread at {}: '{}' is not a literal the source states; scope {} keeps \
             no prefix",
            record.site, record.expression, record.scope_id
        );
    }
    unread
}

/// A call that itself registers a route hands its callback a request, not an
/// instance: `router.get('/me', async (c) => c.get('user'))`.
fn registers_a_route(result: &FileAnalysisResult, site: &RegistrationSite) -> bool {
    result.endpoints.iter().any(|endpoint| {
        u32::try_from(endpoint.line_number) == Ok(site.line)
            && endpoint.owner_node == site.receiver
            && is_served_path(&endpoint.path)
    })
}

/// One file's registration sites, kept until every file's rows are in
/// (carrick#2303): whether a function passed inside a callback registers
/// anything is read from the rows of the file that declares it.
#[derive(Debug, Clone)]
pub struct UnstatedRegistrations {
    /// The `file_results` key of the registering file.
    file_key: String,
    /// Its repo-relative path, as a scope id writes it.
    file: String,
    sites: Vec<RegistrationSite>,
}

impl UnstatedRegistrations {
    /// The sites of one file, when its joined rows leave at least one inner
    /// registration or hand-out without a mount at its line. A file whose
    /// every one the model stated holds nothing to decide.
    pub fn of(
        result: &FileAnalysisResult,
        sites: &[RegistrationSite],
        file_key: &str,
        file: &str,
    ) -> Option<Self> {
        sites
            .iter()
            .flat_map(|site| {
                site.inner_registrations
                    .iter()
                    .map(|inner| inner.line)
                    .chain(site.hand_outs.iter().map(|hand_out| hand_out.line))
            })
            .any(|line| !mount_stated_at(result, line))
            .then(|| Self {
                file_key: file_key.to_string(),
                file: file.to_string(),
                sites: sites.to_vec(),
            })
    }
}

/// Whether the file's rows hold a mount on `line`: the model's own, or one
/// this pass has already emitted.
fn mount_stated_at(result: &FileAnalysisResult, line: u32) -> bool {
    result
        .mounts
        .iter()
        .any(|mount| u32::try_from(mount.line_number) == Ok(line))
}

/// Where a declared function starts and ends, the name of each parameter, and
/// what its body hands each parameter on to.
struct FunctionHead {
    /// The parameters by position: the name of a plain one, `None` for a
    /// destructured, defaulted or rest parameter, which holds no instance
    /// this pass can name.
    params: Vec<Option<String>>,
    start_line: u32,
    end_line: u32,
    /// The bare calls in the body that hand a plain parameter to a function,
    /// each with the position of the parameter handed over (carrick#2316).
    passes_on: Vec<(usize, InnerRegistration)>,
}

/// The functions a module declares by name, read on demand: only the modules
/// an unstated inner registration names are parsed.
struct FunctionHeads {
    source_map: Lrc<SourceMap>,
    handler: Handler,
    by_file: HashMap<PathBuf, Vec<(String, FunctionHead)>>,
}

impl FunctionHeads {
    fn new() -> Self {
        let source_map: Lrc<SourceMap> = Default::default();
        // Quiet: a module that does not parse declares nothing this reads.
        let handler =
            Handler::with_tty_emitter(ColorConfig::Never, false, false, Some(source_map.clone()));
        Self {
            source_map,
            handler,
            by_file: HashMap::new(),
        }
    }

    /// The one function `file` declares as `name`. A name the module declares
    /// twice is not decided.
    fn head(&mut self, file: &Path, name: &str) -> Option<&FunctionHead> {
        if !self.by_file.contains_key(file) {
            let mut heads = Vec::new();
            if let Some(module) = crate::parser::parse_file(file, &self.source_map, &self.handler) {
                let mut values = DeclaredValues::default();
                module.visit_with(&mut values);
                let mut collector = HeadCollector {
                    source_map: &self.source_map,
                    values: &values,
                    heads: Vec::new(),
                };
                module.visit_with(&mut collector);
                heads = collector.heads;
            }
            self.by_file.insert(file.to_path_buf(), heads);
        }
        let mut named = self
            .by_file
            .get(file)?
            .iter()
            .filter(|(declared, _)| declared == name);
        let (_, head) = named.next()?;
        if named.next().is_some() {
            return None;
        }
        Some(head)
    }
}

struct HeadCollector<'s> {
    source_map: &'s Lrc<SourceMap>,
    values: &'s DeclaredValues,
    heads: Vec<(String, FunctionHead)>,
}

impl<'s> HeadCollector<'s> {
    /// Record one named function. `read_body` runs a finder over the
    /// function's body.
    fn record<'p>(
        &mut self,
        name: &Ident,
        params: impl Iterator<Item = &'p Pat>,
        span: swc_common::Span,
        read_body: impl Fn(&mut InnerFinder<'s>),
    ) {
        let line =
            |pos| u32::try_from(self.source_map.lookup_char_pos(pos).line).unwrap_or(u32::MAX);
        let mut names = Vec::new();
        let mut passes_on = Vec::new();
        for (position, param) in params.enumerate() {
            let Pat::Ident(param) = param else {
                names.push(None);
                continue;
            };
            names.push(Some(param.id.sym.to_string()));
            let mut finder = InnerFinder {
                source_map: self.source_map,
                values: self.values,
                param: ident_key(&param.id),
                found: Vec::new(),
            };
            read_body(&mut finder);
            // `p.m(F)` inside a plain function is a mount the model states
            // or nothing: only the hand-on of the instance itself is read.
            passes_on.extend(
                finder
                    .found
                    .into_iter()
                    .filter(|found| found.method.is_none())
                    .map(|found| (position, found)),
            );
        }
        passes_on.sort_by_key(|(_, found)| found.line);
        self.heads.push((
            name.sym.to_string(),
            FunctionHead {
                params: names,
                start_line: line(span.lo),
                end_line: line(span.hi),
                passes_on,
            },
        ));
    }

    fn record_function(&mut self, name: &Ident, function: &Function) {
        self.record(
            name,
            function.params.iter().map(|param| &param.pat),
            function.span,
            |finder| function.body.visit_with(finder),
        );
    }
}

impl Visit for HeadCollector<'_> {
    fn visit_fn_decl(&mut self, decl: &FnDecl) {
        self.record_function(&decl.ident, &decl.function);
        decl.visit_children_with(self);
    }

    fn visit_fn_expr(&mut self, function: &FnExpr) {
        // `export default function name(app) {}` and any other named function
        // expression: the name is a declaration only where the module exports
        // it, which is what the caller's resolver has already decided.
        if let Some(name) = &function.ident {
            self.record_function(name, &function.function);
        }
        function.visit_children_with(self);
    }

    fn visit_var_declarator(&mut self, declarator: &VarDeclarator) {
        if let (Pat::Ident(name), Some(init)) = (&declarator.name, &declarator.init) {
            match unwrap_expression(init) {
                Expr::Arrow(arrow) => {
                    self.record(&name.id, arrow.params.iter(), arrow.span, |finder| {
                        arrow.body.visit_with(finder)
                    })
                }
                Expr::Fn(function) if function.ident.is_none() => {
                    self.record_function(&name.id, &function.function)
                }
                _ => {}
            }
        }
        declarator.visit_children_with(self);
    }
}

/// Follows an instance from the function it is handed to through the
/// functions that function hands it on to (carrick#2316).
struct InstanceTrace<'a> {
    key_by_canonical: &'a HashMap<PathBuf, String>,
    /// The canonical directory the keys resolve against, so a node of one
    /// file is named by its repo-relative path.
    root: Option<PathBuf>,
    /// The resolver the mount graph reads a mount's child through, so a
    /// function this decides is one the graph can attribute routes to.
    resolver: BindingResolver,
    heads: FunctionHeads,
    /// The functions on the walk, so one that hands the instance back to a
    /// function already on it ends there.
    walking: Vec<(PathBuf, String, usize)>,
}

impl InstanceTrace<'_> {
    /// The node `registration` mounts, for the function `function` that
    /// `declaring` declares and the call reaches. The graph keys mounts by
    /// name, so a function the file declares and that hands the instance on
    /// is named by its file and line ([`scope_id`]): two files each handing
    /// their own scope to a local function of the same name stay two nodes,
    /// and neither's hand-ons take the other's prefix. An imported function
    /// keeps the name the file gives it, which the graph resolves through
    /// the import's binding.
    fn node(
        &mut self,
        declaring: &Path,
        registration: &InnerRegistration,
        function: &str,
    ) -> String {
        if registration.import_source.is_some() {
            return registration.identifier.clone();
        }
        let file = self
            .root
            .as_deref()
            .and_then(|root| declaring.strip_prefix(root).ok())
            .map(|relative| relative.to_string_lossy().into_owned())
            .or_else(|| self.key_by_canonical.get(declaring).cloned());
        match (file, self.heads.head(declaring, function)) {
            (Some(file), Some(head)) if !head.passes_on.is_empty() => {
                scope_id(function, &file, head.start_line)
            }
            _ => registration.identifier.clone(),
        }
    }

    /// The function a file names `identifier`: the module that declares it
    /// and its name there. By the name the file gives it, as the graph reads
    /// the emitted row's child: an import the graph cannot follow (one
    /// renamed on the way in) is not decided.
    fn declaration(
        &mut self,
        file: &Path,
        registration: &InnerRegistration,
    ) -> Option<(PathBuf, String)> {
        match &registration.import_source {
            Some(specifier) => {
                let resolved = self
                    .resolver
                    .resolve(file, specifier, &registration.identifier)?;
                // An export with no name to find is not decided.
                Some((resolved.file, resolved.local_name?))
            }
            None => Some((file.to_path_buf(), registration.identifier.clone())),
        }
    }

    /// Whether the function `declaring` declares as `name` registers on the
    /// parameter at `position`: a stated route or mount inside its span hangs
    /// from that parameter, or its body hands that parameter to a function
    /// for which the same holds, at most [`MAX_PASS_ON_DEPTH`] functions on.
    ///
    /// Each hand-on that reaches a registration is pushed to `hand_ons` as a
    /// mount row of the file that makes the call, hanging from `node`, the
    /// node the row that reached the function mounts ([`Self::node`]). A
    /// function its own file reaches is a local child of that file, named by
    /// file and line, and kept as it is, so the hand-on composes under the
    /// scope that made the call and under no other file's. The parameter
    /// would resolve file-first to the binding the file is mounted under,
    /// which is not the scope the call runs in. An imported function
    /// resolves through the binding of the import that reached it. A line
    /// that already holds a mount is the model's and is left to it.
    #[allow(clippy::too_many_arguments)]
    fn registers(
        &mut self,
        file_results: &HashMap<String, FileAnalysisResult>,
        declaring: &Path,
        name: &str,
        node: &str,
        position: usize,
        depth: usize,
        hand_ons: &mut Vec<(String, MountResult)>,
    ) -> bool {
        let Some(key) = self.key_by_canonical.get(declaring) else {
            return false;
        };
        let Some(rows) = file_results.get(key) else {
            return false;
        };
        let Some(head) = self.heads.head(declaring, name) else {
            return false;
        };
        let Some(Some(param)) = head.params.get(position) else {
            return false;
        };
        let inside = |line: i32| {
            u32::try_from(line).is_ok_and(|line| line >= head.start_line && line <= head.end_line)
        };
        // A hand-on this pass wrote hangs from the function's node.
        let mut registers = rows.mounts.iter().any(|mount| {
            (&mount.parent_node == param || mount.parent_node == node) && inside(mount.line_number)
        }) || rows.endpoints.iter().any(|endpoint| {
            &endpoint.owner_node == param
                && is_served_path(&endpoint.path)
                && inside(endpoint.line_number)
        });
        if depth >= MAX_PASS_ON_DEPTH {
            return registers;
        }
        let walked = (declaring.to_path_buf(), name.to_string(), position);
        if self.walking.contains(&walked) {
            return registers;
        }
        let passes_on: Vec<InnerRegistration> = head
            .passes_on
            .iter()
            .filter(|(from, pass)| *from == position && !mount_stated_at(rows, pass.line))
            .map(|(_, pass)| pass.clone())
            .collect();
        self.walking.push(walked);
        for pass in passes_on {
            let Some((file, function)) = self.declaration(declaring, &pass) else {
                continue;
            };
            let child = self.node(&file, &pass, &function);
            if self.registers(
                file_results,
                &file,
                &function,
                &child,
                pass.position,
                depth + 1,
                hand_ons,
            ) {
                registers = true;
                hand_ons.push((key.clone(), pass.mount_row(node, &child)));
            }
        }
        self.walking.pop();
        registers
    }
}

/// The one function the scanned repository supplies under a hand-out's key,
/// read from the supplying file's source (carrick#2316).
#[derive(Debug, Clone)]
struct Supplier {
    /// The supplying file, canonical.
    file: PathBuf,
    /// The supplying call's start line.
    line: u32,
    /// What the supplied function hands the instance it receives to.
    hands_on: Vec<InnerRegistration>,
}

/// What one call that passes a hand-out's enclosing function says about the
/// hand-out's key.
enum Supplied {
    /// The call's options do not hold the key.
    Nothing,
    /// An inline function, with what it hands the instance to.
    Inline(u32, Vec<InnerRegistration>),
    /// A function the supplying file declares, by name.
    Declared(u32, String),
    /// The call may supply the key and its source does not say what.
    Unknown(&'static str),
}

/// A property's key as written: a plain name, or one the source computes.
enum PropKey {
    Named(String),
    Computed,
}

fn prop_key(prop: &Prop) -> PropKey {
    let name = match prop {
        Prop::Shorthand(ident) => return PropKey::Named(ident.sym.to_string()),
        Prop::KeyValue(kv) => &kv.key,
        Prop::Method(method) => &method.key,
        Prop::Getter(getter) => &getter.key,
        Prop::Setter(setter) => &setter.key,
        Prop::Assign(assign) => return PropKey::Named(assign.key.sym.to_string()),
    };
    match name {
        PropName::Ident(ident) => PropKey::Named(ident.sym.to_string()),
        PropName::Str(text) => PropKey::Named(text.value.to_string_lossy().into_owned()),
        PropName::Num(number) => PropKey::Named(number.value.to_string()),
        _ => PropKey::Computed,
    }
}

/// Reads one file for the calls that pass a hand-out's enclosing function
/// with options, and what each supplies under the key.
struct SupplierFinder<'s> {
    source_map: &'s Lrc<SourceMap>,
    values: &'s DeclaredValues,
    resolver: &'s mut BindingResolver,
    /// The file read, canonical.
    file: &'s Path,
    /// The hand-out's file, canonical.
    target: &'s Path,
    hand_out: &'s HandOut,
    /// What each call that passes the function supplies, by its line.
    found: Vec<(u32, Supplied)>,
}

impl SupplierFinder<'_> {
    /// Whether `ident` is the hand-out's enclosing function, by binding: an
    /// import that resolves to its declaration, or the declaration itself.
    fn names_the_function(&mut self, ident: &Ident) -> bool {
        let key = ident_key(ident);
        if let Some(specifier) = self.values.imports.get(&key) {
            return self
                .resolver
                .resolve(self.file, specifier, &ident.sym)
                .is_some_and(|resolved| {
                    resolved.file.canonicalize().ok().as_deref() == Some(self.target)
                        && resolved.local_name.as_deref() == Some(self.hand_out.function.as_str())
                });
        }
        self.file == self.target
            && self.values.functions.contains(&key)
            && *ident.sym == *self.hand_out.function
    }

    fn passes_the_function(&mut self, expr: &Expr) -> bool {
        matches!(unwrap_expression(expr), Expr::Ident(ident) if self.names_the_function(ident))
    }

    fn line(&self, span: swc_common::Span) -> u32 {
        u32::try_from(self.source_map.lookup_char_pos(span.lo).line).unwrap_or(0)
    }

    /// What an options object supplies under the key. A spread or a computed
    /// key written after the key's last mention may replace it, so the value
    /// is decided only when nothing follows it that could.
    fn read_options(&self, line: u32, object: &ObjectLit) -> Supplied {
        let mut value: Option<(usize, &Prop)> = None;
        let mut unknown_at: Option<usize> = None;
        for (index, prop) in object.props.iter().enumerate() {
            match prop {
                PropOrSpread::Spread(_) => unknown_at = Some(index),
                PropOrSpread::Prop(prop) => match prop_key(prop) {
                    PropKey::Named(name) if name == self.hand_out.key => {
                        value = Some((index, prop.as_ref()))
                    }
                    PropKey::Named(_) => {}
                    PropKey::Computed => unknown_at = Some(index),
                },
            }
        }
        // A literal that never writes the key supplies nothing this call can
        // show, whatever a spread or a computed key in it holds: a function
        // supplied that way only adds to the scope, and cannot make the rows
        // another call's written key reaches wrong.
        match value {
            Some((index, prop)) if unknown_at.is_none_or(|at| at < index) => {
                self.read_value(line, prop)
            }
            Some(_) => Supplied::Unknown("a spread or a computed key after it may replace it"),
            None => Supplied::Nothing,
        }
    }

    fn read_value(&self, line: u32, prop: &Prop) -> Supplied {
        match prop {
            Prop::KeyValue(kv) => match unwrap_expression(&kv.value) {
                Expr::Arrow(arrow) => self.inline(line, arrow.params.iter(), |finder| {
                    arrow.body.visit_with(finder)
                }),
                Expr::Fn(function) => self.inline(
                    line,
                    function.function.params.iter().map(|param| &param.pat),
                    |finder| function.function.body.visit_with(finder),
                ),
                Expr::Ident(ident) => self.declared(line, ident),
                _ => Supplied::Unknown("the value is not a function the file declares"),
            },
            Prop::Shorthand(ident) => self.declared(line, ident),
            Prop::Method(method) => self.inline(
                line,
                method.function.params.iter().map(|param| &param.pat),
                |finder| method.function.body.visit_with(finder),
            ),
            _ => Supplied::Unknown("the value is not a function the file declares"),
        }
    }

    fn declared(&self, line: u32, ident: &Ident) -> Supplied {
        if self.values.functions.contains(&ident_key(ident)) {
            Supplied::Declared(line, ident.sym.to_string())
        } else {
            Supplied::Unknown("the value is not a function the file declares")
        }
    }

    /// An inline function: what it hands the parameter at the hand-out's
    /// position to. A function with no plain parameter there hands nothing.
    fn inline<'p>(
        &self,
        line: u32,
        mut params: impl Iterator<Item = &'p Pat>,
        read_body: impl Fn(&mut InnerFinder<'_>),
    ) -> Supplied {
        let Some(Pat::Ident(param)) = params.nth(self.hand_out.position) else {
            return Supplied::Inline(line, Vec::new());
        };
        let mut finder = InnerFinder {
            source_map: self.source_map,
            values: self.values,
            param: ident_key(&param.id),
            found: Vec::new(),
        };
        read_body(&mut finder);
        finder.found.sort_by_key(|found| found.line);
        Supplied::Inline(line, finder.found)
    }

    /// What one argument that is not the function supplies, when it may be
    /// the options.
    fn read_argument(&self, line: u32, expr: &Expr) -> Supplied {
        match unwrap_expression(expr) {
            Expr::Object(object) => self.read_options(line, object),
            Expr::Lit(_) | Expr::Tpl(_) | Expr::Arrow(_) | Expr::Fn(_) => Supplied::Nothing,
            _ => Supplied::Unknown("the options are built elsewhere"),
        }
    }
}

impl Visit for SupplierFinder<'_> {
    fn visit_call_expr(&mut self, call: &CallExpr) {
        call.visit_children_with(self);
        let Callee::Expr(callee) = &call.callee else {
            return;
        };
        let line = self.line(call.span);
        let supplied = self.supplied_by(line, callee, call);
        self.found
            .extend(supplied.into_iter().map(|supplied| (line, supplied)));
    }
}

impl SupplierFinder<'_> {
    /// What one call supplies under the key: nothing when it does not pass
    /// the enclosing function.
    fn supplied_by(&mut self, line: u32, callee: &Expr, call: &CallExpr) -> Vec<Supplied> {
        // `routesPlugin(app, opts)`: the options are the argument at the
        // enclosing function's own position.
        if self.passes_the_function(callee) {
            let position = self.hand_out.options_position;
            if call
                .args
                .iter()
                .take(position + 1)
                .any(|arg| arg.spread.is_some())
            {
                return vec![Supplied::Unknown("a spread argument may hold the options")];
            }
            return vec![match call.args.get(position) {
                Some(arg) => self.read_argument(line, &arg.expr),
                None => Supplied::Nothing,
            }];
        }
        // `app.register(routesPlugin, opts)`: whatever calls the function
        // passes it the options. An object literal among the other arguments
        // is read; with none, any other value may be them.
        let mut passes = false;
        let mut others: Vec<&Expr> = Vec::new();
        for arg in &call.args {
            if arg.spread.is_none() && self.passes_the_function(&arg.expr) {
                passes = true;
            } else {
                others.push(&arg.expr);
            }
        }
        if !passes {
            return Vec::new();
        }
        if call.args.iter().any(|arg| arg.spread.is_some()) {
            return vec![Supplied::Unknown("a spread argument may hold the options")];
        }
        let literals: Vec<&ObjectLit> = others
            .iter()
            .filter_map(|expr| match unwrap_expression(expr) {
                Expr::Object(object) => Some(object),
                _ => None,
            })
            .collect();
        if literals.is_empty() {
            return others
                .into_iter()
                .map(|expr| self.read_argument(line, expr))
                .find(|supplied| !matches!(supplied, Supplied::Nothing))
                .into_iter()
                .collect();
        }
        literals
            .into_iter()
            .map(|object| self.read_options(line, object))
            .collect()
    }
}

/// The relative specifier from `from`'s directory to `to`, both canonical:
/// `./notes.ts`, `../routes/notes.ts`. The file's own extension is kept, so
/// the specifier names that file and no sibling.
fn relative_specifier(from: &Path, to: &Path) -> Option<String> {
    let from_dir = from.parent()?;
    let from_parts: Vec<_> = from_dir.components().collect();
    let to_parts: Vec<_> = to.components().collect();
    let shared = from_parts
        .iter()
        .zip(&to_parts)
        .take_while(|(a, b)| a == b)
        .count();
    if shared == 0 {
        return None;
    }
    let mut parts: Vec<String> = Vec::new();
    for _ in shared..from_parts.len() {
        parts.push("..".to_string());
    }
    for part in &to_parts[shared..] {
        parts.push(part.as_os_str().to_str()?.to_string());
    }
    let joined = parts.join("/");
    Some(if joined.starts_with("..") {
        joined
    } else {
        format!("./{joined}")
    })
}

/// Follows a hand-out to the function the scanned repository supplies under
/// its key, and from there to the functions of this service the instance
/// reaches (carrick#2316).
struct CallbackTrace<'w> {
    modules: &'w WorkspaceIndex,
    /// Resolves through workspace package names and aliases: the supplying
    /// package names this one by its package name.
    packages: BindingResolver,
    /// Each hand-out's one supplier, or why there is none, by (file,
    /// enclosing function, key).
    suppliers: HashMap<(PathBuf, String, String), Result<Supplier, String>>,
    /// The files [`CallbackTrace::candidate_files`] read for a package, by
    /// its directory (`None` outside any package).
    walked: HashMap<Option<PathBuf>, Vec<PathBuf>>,
    walking: Vec<(PathBuf, String, usize)>,
}

impl CallbackTrace<'_> {
    /// The files a supplier can be written in: those of the package holding
    /// the hand-out, and of every workspace package that depends on it, as a
    /// scan of each would read them (no tests, no build output).
    /// Walked once per package: every hand-out of one package searches the
    /// same files.
    fn candidate_files(&mut self, hand_out_file: &Path) -> Vec<PathBuf> {
        let package = self.modules.package_holding(hand_out_file);
        let walk_key = package.as_ref().map(|(_, dir)| dir.clone());
        if let Some(files) = self.walked.get(&walk_key) {
            return files.clone();
        }
        let root = self.modules.repo_root();
        let mut dirs: Vec<PathBuf> = match package {
            Some((name, dir)) => std::iter::once(dir)
                .chain(self.modules.dependents_of(&name))
                .collect(),
            None => vec![PathBuf::new()],
        };
        dirs.sort();
        dirs.dedup();
        // A package nested in another one is read with it.
        let outer: Vec<PathBuf> = dirs
            .iter()
            .filter(|dir| {
                !dirs
                    .iter()
                    .any(|other| other != *dir && dir.starts_with(other))
            })
            .cloned()
            .collect();
        let mut files: Vec<PathBuf> = outer
            .iter()
            .flat_map(|dir| {
                crate::file_finder::find_files(
                    &root.join(dir).to_string_lossy(),
                    &crate::packages::MANIFEST_SKIP_DIRS,
                )
                .0
            })
            .filter_map(|file| file.canonicalize().ok())
            .collect();
        files.sort();
        files.dedup();
        self.walked.insert(walk_key, files.clone());
        files
    }

    /// The one call in the scanned repository that supplies the hand-out's
    /// key, or why none is decided, logged once at `at`.
    fn supplier(
        &mut self,
        heads: &mut FunctionHeads,
        hand_out_file: &Path,
        at: &str,
        hand_out: &HandOut,
    ) -> Result<Supplier, String> {
        let cache_key = (
            hand_out_file.to_path_buf(),
            hand_out.function.clone(),
            hand_out.key.clone(),
        );
        if let Some(known) = self.suppliers.get(&cache_key) {
            return known.clone();
        }
        let found = self.find_supplier(heads, hand_out_file, hand_out);
        if let Err(why) = &found {
            debug!(
                "Hand-out at {at} not followed: {} '{}' ({why}); the routes it reaches keep their \
                 paths",
                hand_out.function, hand_out.key
            );
        }
        self.suppliers.insert(cache_key, found.clone());
        found
    }

    fn find_supplier(
        &mut self,
        heads: &mut FunctionHeads,
        hand_out_file: &Path,
        hand_out: &HandOut,
    ) -> Result<Supplier, String> {
        let mut supplied: Vec<(PathBuf, u32, Supplied)> = Vec::new();
        for file in self.candidate_files(hand_out_file) {
            // A cost gate only: the decision below is by binding.
            let Ok(text) = std::fs::read_to_string(&file) else {
                continue;
            };
            if !text.contains(hand_out.function.as_str()) {
                continue;
            }
            let Some(module) = crate::parser::parse_file(&file, &heads.source_map, &heads.handler)
            else {
                continue;
            };
            let mut values = DeclaredValues::default();
            module.visit_with(&mut values);
            let mut finder = SupplierFinder {
                source_map: &heads.source_map,
                values: &values,
                resolver: &mut self.packages,
                file: &file,
                target: hand_out_file,
                hand_out,
                found: Vec::new(),
            };
            module.visit_with(&mut finder);
            supplied.extend(
                finder
                    .found
                    .into_iter()
                    .map(|(line, found)| (file.clone(), line, found)),
            );
        }
        let mut one: Option<(PathBuf, Supplied)> = None;
        let mut count = 0usize;
        for (file, line, found) in supplied {
            match found {
                Supplied::Nothing => {}
                Supplied::Unknown(why) => {
                    return Err(format!("{}:{line}: {why}", file.display()));
                }
                other => {
                    count += 1;
                    one = Some((file, other));
                }
            }
        }
        let (file, found) = match (one, count) {
            (Some(one), 1) => one,
            (_, 0) => return Err("no call in the scanned repository supplies it".to_string()),
            (_, count) => return Err(format!("{count} calls supply it")),
        };
        match found {
            Supplied::Inline(line, hands_on) => Ok(Supplier {
                file,
                line,
                hands_on,
            }),
            Supplied::Declared(line, name) => {
                let hands_on = heads
                    .head(&file, &name)
                    .map(|head| {
                        head.passes_on
                            .iter()
                            .filter(|(from, _)| *from == hand_out.position)
                            .map(|(_, pass)| pass.clone())
                            .collect()
                    })
                    .unwrap_or_default();
                Ok(Supplier {
                    file,
                    line,
                    hands_on,
                })
            }
            Supplied::Nothing | Supplied::Unknown(_) => {
                Err("no call in the scanned repository supplies it".to_string())
            }
        }
    }

    /// The function `file` names as `registration` hands the instance to,
    /// resolved through workspace packages: its declaring module, canonical,
    /// and its name there.
    fn declaration(
        &mut self,
        file: &Path,
        registration: &InnerRegistration,
    ) -> Option<(PathBuf, String)> {
        match &registration.import_source {
            Some(specifier) => {
                let resolved = self
                    .packages
                    .resolve(file, specifier, &registration.identifier)?;
                Some((resolved.file.canonicalize().ok()?, resolved.local_name?))
            }
            None => Some((file.to_path_buf(), registration.identifier.clone())),
        }
    }

    /// The functions of this service the instance reaches from `file`'s call
    /// `registration`, each one that registers on the parameter it receives
    /// ([`InstanceTrace::registers`]). A function outside this service is
    /// read from its source only, for what it hands the instance on to, at
    /// most [`MAX_PASS_ON_DEPTH`] functions on.
    #[allow(clippy::too_many_arguments)]
    fn reach(
        &mut self,
        trace: &mut InstanceTrace<'_>,
        file_results: &HashMap<String, FileAnalysisResult>,
        file: &Path,
        registration: &InnerRegistration,
        depth: usize,
        reached: &mut Vec<Reached>,
    ) {
        let Some((declaring, name)) = self.declaration(file, registration) else {
            return;
        };
        if trace.key_by_canonical.contains_key(&declaring) {
            let mut hand_ons = Vec::new();
            if trace.registers(
                file_results,
                &declaring,
                &name,
                // The hand-out's row mounts the function by this name, so
                // its hand-ons hang from it.
                &name,
                registration.position,
                depth,
                &mut hand_ons,
            ) && !reached
                .iter()
                .any(|known| known.file == declaring && known.name == name)
            {
                reached.push(Reached {
                    file: declaring,
                    name,
                    hand_ons,
                });
            }
            return;
        }
        if depth >= MAX_PASS_ON_DEPTH {
            return;
        }
        let walked = (declaring.clone(), name.clone(), registration.position);
        if self.walking.contains(&walked) {
            return;
        }
        let passes_on: Vec<InnerRegistration> = match trace.heads.head(&declaring, &name) {
            Some(head) => head
                .passes_on
                .iter()
                .filter(|(from, _)| *from == registration.position)
                .map(|(_, pass)| pass.clone())
                .collect(),
            None => return,
        };
        self.walking.push(walked);
        for pass in passes_on {
            self.reach(trace, file_results, &declaring, &pass, depth + 1, reached);
        }
        self.walking.pop();
    }

    /// The mount rows one hand-out makes in its file (see
    /// [`mount_unstated_registrations`]), and the hand-ons the functions it
    /// mounts make in theirs, pushed to `hand_ons`.
    #[allow(clippy::too_many_arguments)]
    fn mount_rows(
        &mut self,
        trace: &mut InstanceTrace<'_>,
        file_results: &HashMap<String, FileAnalysisResult>,
        file_root: &Path,
        importer: &Path,
        file: &str,
        site: &RegistrationSite,
        hand_out: &HandOut,
        hand_ons: &mut Vec<(String, MountResult)>,
    ) -> Vec<MountResult> {
        let at = format!("{file}:{}", hand_out.line);
        let Ok(supplier) = self.supplier(&mut trace.heads, importer, &at, hand_out) else {
            return Vec::new();
        };
        let mut reached: Vec<Reached> = Vec::new();
        for registration in &supplier.hands_on {
            self.reach(
                trace,
                file_results,
                &supplier.file,
                registration,
                0,
                &mut reached,
            );
        }
        if reached.is_empty() {
            return Vec::new();
        }
        let bindings = crate::agents::file_orchestrator::FileOrchestrator::resolve_mount_bindings(
            file_results,
            file_root,
        );
        let mut rows = Vec::new();
        for function in reached {
            let Some(key) = trace.key_by_canonical.get(&function.file) else {
                continue;
            };
            let Some(declared_rows) = file_results.get(key) else {
                continue;
            };
            let Some((start, end)) = trace
                .heads
                .head(&function.file, &function.name)
                .map(|head| (head.start_line, head.end_line))
            else {
                continue;
            };
            if !holds_every_row_of_its_file(declared_rows, start, end) {
                debug!(
                    "Hand-out at {at} reaches {} in {key}, which shares its file with rows it does \
                     not hold; not mounted",
                    function.name
                );
                continue;
            }
            if bindings.get(key).is_some_and(|known| {
                known
                    .iter()
                    .any(|binding| binding.child_node != function.name)
            }) {
                debug!(
                    "Hand-out at {at} reaches {} in {key}, which another mount already resolves \
                     to; not mounted",
                    function.name
                );
                continue;
            }
            let Some(specifier) = relative_specifier(importer, &function.file) else {
                continue;
            };
            debug!(
                "Mount read from the source at {at}: {} hands {} to {} supplied at {}:{}, and the \
                 instance reaches {}",
                hand_out.function,
                site.param,
                hand_out.key,
                supplier.file.display(),
                supplier.line,
                function.name
            );
            hand_ons.extend(function.hand_ons);
            rows.push(MountResult {
                line_number: i32::try_from(hand_out.line).unwrap_or(i32::MAX),
                parent_node: site.param.clone(),
                child_node: function.name,
                mount_path: String::new(),
                import_source: Some(specifier),
                pattern_matched: hand_out.pattern.clone(),
            });
        }
        rows
    }
}

/// A function of this service an instance reaches through a supplied
/// callback, with the hand-ons its own body makes.
struct Reached {
    /// Its declaring file, canonical.
    file: PathBuf,
    name: String,
    hand_ons: Vec<(String, MountResult)>,
}

/// Whether a function holds every row its file states: the graph gives a
/// mounted file's routes to the binding that resolves to the file, whole
/// (`resolve_endpoint_owner`), so a function that shares its file with
/// routes registered elsewhere cannot be mounted alone (carrick#2334).
fn holds_every_row_of_its_file(rows: &FileAnalysisResult, start_line: u32, end_line: u32) -> bool {
    let inside =
        |line: i32| u32::try_from(line).is_ok_and(|line| line >= start_line && line <= end_line);
    rows.endpoints
        .iter()
        .filter(|endpoint| is_served_path(&endpoint.path))
        .all(|endpoint| inside(endpoint.line_number))
        && rows.mounts.iter().all(|mount| inside(mount.line_number))
}

/// State the mounts a registration callback makes and the model did not
/// (carrick#2303). Returns how many mount rows the files gained, and the
/// scopes this made whose prefix the source does not state as a literal.
///
/// An inner registration, `P.m(F)` or `F(…, P, …)`, becomes a mount row of
/// its file when both hold:
///
/// * the file's rows hold no mount at that line, so a model row always wins
///   and nothing is stated twice;
/// * `F` resolves to a function whose parameter at the position the instance
///   is handed to is a plain name, and that function registers on it
///   ([`InstanceTrace::registers`]): at least one stated route or mount
///   inside the function's span hangs from that parameter, or the function
///   hands the parameter on to one for which that holds (carrick#2316). A
///   function that registers nothing on the instance (an error handler, a
///   hook) is not mounted.
///
/// The second reads another file's rows, which is why this runs once every
/// file's rows are in, and to a fixed point: a function whose only
/// registration is itself an unstated scope heads a row after its own file is
/// read. Each file that gained a row then goes through
/// [`apply_registration_scopes`] again, which is what makes the callback a
/// scope and corrects or emits its outer registration. The second run only
/// acts on the new rows: a scope the first run made has its registration row
/// at the site's line with a literal path, so that row is found, kept and
/// records nothing, and the rows already hanging from the scope id no longer
/// hang from the parameter's name.
///
/// A hand-out, `O.k(P)`, becomes one mount row of its file for each function
/// the instance reaches through the function the scanned repository supplies
/// under `k` (carrick#2316), when:
///
/// * the file's rows hold no mount at that line;
/// * exactly one call in the hand-out's package, or in a workspace package
///   that depends on it, passes the enclosing function with an object
///   literal that writes `k` as an inline function or a function that file
///   declares, nothing written after `k` in that literal (a spread, a
///   computed key) may replace it, and no such call passes options built
///   elsewhere: [`CallbackTrace::supplier`]. A literal that never writes `k`
///   supplies nothing, whatever a spread in it holds;
/// * the function is declared in this service's own files, registers on the
///   parameter it receives, holds every row its file states, and no other
///   mount already resolves to its file.
///
/// The row names the function as its file declares it, with a relative
/// specifier to that file, so the graph attributes the file's routes to the
/// scope the hand-out is written in. The supplying package's own rows are
/// never read or changed.
///
/// `file_root` is the directory the `file_results` keys resolve against on
/// disk, and `modules` the service's module index, which names the workspace
/// packages. No method, option, library or framework name is read.
pub fn mount_unstated_registrations(
    file_results: &mut HashMap<String, FileAnalysisResult>,
    pending: &[UnstatedRegistrations],
    file_root: &Path,
    modules: &WorkspaceIndex,
) -> (usize, Vec<UnreadPrefix>) {
    let mut unread = Vec::new();
    if pending.is_empty() {
        return (0, unread);
    }
    let key_by_canonical: HashMap<PathBuf, String> = file_results
        .keys()
        .filter_map(|key| Some((file_root.join(key).canonicalize().ok()?, key.clone())))
        .collect();
    let mut order: Vec<&UnstatedRegistrations> = pending.iter().collect();
    order.sort_by(|a, b| a.file_key.cmp(&b.file_key));

    let mut trace = InstanceTrace {
        key_by_canonical: &key_by_canonical,
        root: file_root.canonicalize().ok(),
        resolver: BindingResolver::new(),
        heads: FunctionHeads::new(),
        walking: Vec::new(),
    };
    let mut callbacks = CallbackTrace {
        modules,
        packages: BindingResolver::with_workspace(modules.clone()),
        suppliers: HashMap::new(),
        walked: HashMap::new(),
        walking: Vec::new(),
    };
    let mut emitted = 0usize;
    loop {
        let mut progressed = false;
        for file in &order {
            let Some(result) = file_results.get(&file.file_key) else {
                continue;
            };
            let Ok(importer) = file_root.join(&file.file_key).canonicalize() else {
                continue;
            };
            let mut rows: Vec<MountResult> = Vec::new();
            let mut hand_ons: Vec<(String, MountResult)> = Vec::new();
            for site in &file.sites {
                if registers_a_route(result, site) {
                    continue;
                }
                for inner in &site.inner_registrations {
                    if mount_stated_at(result, inner.line) {
                        continue;
                    }
                    let Some((declaring, name)) = trace.declaration(&importer, inner) else {
                        continue;
                    };
                    let node = trace.node(&declaring, inner, &name);
                    if trace.registers(
                        file_results,
                        &declaring,
                        &name,
                        &node,
                        inner.position,
                        0,
                        &mut hand_ons,
                    ) {
                        rows.push(inner.mount_row(&site.param, &node));
                    }
                }
                for hand_out in &site.hand_outs {
                    if mount_stated_at(result, hand_out.line) {
                        continue;
                    }
                    rows.extend(callbacks.mount_rows(
                        &mut trace,
                        file_results,
                        file_root,
                        &importer,
                        &file.file,
                        site,
                        hand_out,
                        &mut hand_ons,
                    ));
                }
            }
            if rows.is_empty() {
                continue;
            }
            // The hand-ons first: one in this file is then among the rows the
            // scope pass reads below.
            for (key, row) in hand_ons {
                let Some(target) = file_results.get_mut(&key) else {
                    continue;
                };
                // One function reached from two scopes hands on once. Kept
                // apart by the call, not the line: `a(app); b(app);` is two
                // hand-ons. A line the model stated a mount on never reaches
                // here, as `registers` skips its calls.
                if target.mounts.iter().any(|mount| {
                    mount.line_number == row.line_number
                        && mount.parent_node == row.parent_node
                        && mount.child_node == row.child_node
                }) {
                    continue;
                }
                debug!(
                    "Mount read from the source at {}:{}: {} is handed on to {} and the model \
                     stated no mount there",
                    key, row.line_number, row.parent_node, row.child_node
                );
                target.mounts.push(row);
                emitted += 1;
            }
            let Some(result) = file_results.get_mut(&file.file_key) else {
                continue;
            };
            for row in &rows {
                debug!(
                    "Mount read from the source at {}:{}: {} registers {} and the model stated \
                     no mount there",
                    file.file, row.line_number, row.parent_node, row.child_node
                );
            }
            let before = result.mounts.len();
            result.mounts.extend(rows);
            unread.extend(apply_registration_scopes(result, &file.sites, &file.file));
            // The inner mounts, and the outer registration when the second
            // run had to emit it.
            emitted += result.mounts.len() - before;
            progressed = true;
        }
        if !progressed {
            break;
        }
    }
    (emitted, unread)
}

#[cfg(test)]
mod tests {
    use super::*;
    use swc_common::{
        FileName, GLOBALS, Globals, Mark,
        errors::{ColorConfig, Handler},
    };
    use swc_ecma_parser::{Parser, StringInput, Syntax, TsSyntax, lexer::Lexer};
    use swc_ecma_transforms_base::resolver;
    use swc_ecma_visit::VisitMutWith;

    fn sites(source: &str) -> Vec<RegistrationSite> {
        GLOBALS.set(&Globals::new(), || {
            let cm: Lrc<SourceMap> = Default::default();
            let _handler =
                Handler::with_tty_emitter(ColorConfig::Never, true, false, Some(cm.clone()));
            let fm = cm.new_source_file(
                Lrc::new(FileName::Custom("t.ts".into())),
                source.to_string(),
            );
            let lexer = Lexer::new(
                Syntax::Typescript(TsSyntax::default()),
                Default::default(),
                StringInput::from(&*fm),
                None,
            );
            let mut module = Parser::new_from(lexer).parse_module().expect("parses");
            module.visit_mut_with(&mut resolver(Mark::new(), Mark::new(), true));
            collect_registration_sites(&module, &cm)
        })
    }

    fn read_of(source: &str) -> PrefixRead {
        let found = sites(source);
        let [site] = found.as_slice() else {
            panic!("one site, got {found:?}");
        };
        site.option_values
            .iter()
            .find(|value| value.text.contains("prefix") || value.read != PrefixRead::Unread)
            .map(|value| value.read.clone())
            .unwrap_or(PrefixRead::Unread)
    }

    fn stated(values: &[&str]) -> PrefixRead {
        PrefixRead::Stated(values.iter().map(|v| v.to_string()).collect())
    }

    #[test]
    fn a_nullish_default_reads_the_written_default() {
        assert_eq!(
            read_of("app.register(async (api) => {}, { prefix: opts.prefix ?? '/api/v1' })"),
            stated(&["/api/v1"])
        );
    }

    #[test]
    fn a_logical_or_default_reads_the_written_default() {
        assert_eq!(
            read_of("app.register(async (api) => {}, { prefix: opts.prefix || `/api/v2` })"),
            stated(&["/api/v2"])
        );
    }

    #[test]
    fn a_ternary_of_literals_reads_both_branches() {
        assert_eq!(
            read_of("app.register(async (api) => {}, { prefix: o.legacy ? '/legacy' : '/v3' })"),
            stated(&["/legacy", "/v3"])
        );
        assert_eq!(
            read_of("app.register(async (api) => {}, { prefix: o.legacy ? o.p : '/v3' })"),
            PrefixRead::Unread
        );
    }

    #[test]
    fn a_const_and_a_parameter_default_are_read_by_scope() {
        assert_eq!(
            read_of(
                "const prefix = '/api/v5';\napp.register(async (api) => {}, { prefix: prefix })"
            ),
            stated(&["/api/v5"])
        );
        assert_eq!(
            read_of(
                "export function setup(app, { prefix = '/api/v4' } = {}) {\n  app.register(async (api) => {}, { prefix });\n}"
            ),
            stated(&["/api/v4"])
        );
        assert_eq!(
            read_of(
                "export function setup(app, prefix = '/api/v6') {\n  app.register(async (api) => {}, { prefix });\n}"
            ),
            stated(&["/api/v6"])
        );
    }

    #[test]
    fn a_nested_const_does_not_answer_for_the_outer_binding() {
        let source = "export function setup(app, prefix) {\n  { const prefix = '/inner'; }\n  app.register(async (api) => {}, { prefix });\n}";
        assert_eq!(read_of(source), PrefixRead::Unread);
    }

    #[test]
    fn a_member_read_a_call_and_a_substitution_are_unread() {
        for value in ["opts.prefix", "makePrefix()", "`/api/${version}`"] {
            let source = format!("app.register(async (api) => {{}}, {{ prefix: {value} }})");
            assert_eq!(read_of(&source), PrefixRead::Unread, "{value}");
        }
    }

    #[test]
    fn a_value_that_is_not_a_route_path_is_unread() {
        for value in ["'https://example.test/api'", "'/a b'", "'api'"] {
            let source = format!("app.register(async (api) => {{}}, {{ prefix: {value} }})");
            assert_eq!(read_of(&source), PrefixRead::Unread, "{value}");
        }
    }

    #[test]
    fn the_binding_depth_is_capped() {
        let source = "const a = '/deep';\nconst b = a;\nconst c = b;\nconst d = c;\nconst e = d;\napp.register(async (api) => {}, { prefix: e })";
        assert_eq!(read_of(source), PrefixRead::Unread);
        let source = "const a = '/deep';\nconst b = a;\nconst c = b;\nconst d = c;\napp.register(async (api) => {}, { prefix: d })";
        assert_eq!(read_of(source), stated(&["/deep"]));
    }

    #[test]
    fn a_call_with_two_inline_functions_or_none_is_not_a_site() {
        assert!(sites("app.addHook('onRequest', async (req) => {}, async (res) => {})").is_empty());
        assert!(sites("app.register(plugin, { prefix: '/x' })").is_empty());
        assert!(sites("run(async (api) => {})").is_empty());
    }

    #[test]
    fn a_site_records_its_lines_and_names() {
        let found = sites(
            "app.register(\n  async (api) => {\n    api.get('/x', h);\n  },\n  { prefix: '/p' },\n);",
        );
        let [site] = found.as_slice() else {
            panic!("one site");
        };
        assert_eq!(
            (site.line, site.receiver.as_str(), site.param.as_str()),
            (1, "app", "api")
        );
        assert_eq!((site.body_start_line, site.body_end_line), (2, 4));
        assert_eq!(site.options_text.as_deref(), Some("{ prefix: '/p' }"));
    }

    /// The inner registrations of the one site in `source`.
    fn inner_of(source: &str) -> Vec<InnerRegistration> {
        let found = sites(source);
        let [site] = found.as_slice() else {
            panic!("one site, got {found:?}");
        };
        site.inner_registrations.clone()
    }

    fn inner(line: u32, method: &str, identifier: &str, import: Option<&str>) -> InnerRegistration {
        InnerRegistration {
            line,
            method: Some(method.to_string()),
            identifier: identifier.to_string(),
            import_source: import.map(str::to_string),
            position: 0,
        }
    }

    #[test]
    fn a_callback_records_the_functions_it_passes_to_its_parameter() {
        let source = "import { items as itemRoutes } from './items';\nimport orders from './orders';\nfunction local(app) {}\nconst arrow = async (app) => {};\napp.register(async (api) => {\n  await api.register(itemRoutes);\n  api.use(orders, { audit: true });\n  api.register(local);\n  api.register(arrow);\n});";
        assert_eq!(
            inner_of(source),
            vec![
                inner(6, "register", "itemRoutes", Some("./items")),
                inner(7, "use", "orders", Some("./orders")),
                inner(8, "register", "local", None),
                inner(9, "register", "arrow", None),
            ]
        );
    }

    fn handed(
        line: u32,
        identifier: &str,
        import: Option<&str>,
        position: usize,
    ) -> InnerRegistration {
        InnerRegistration {
            line,
            method: None,
            identifier: identifier.to_string(),
            import_source: import.map(str::to_string),
            position,
        }
    }

    #[test]
    fn a_callback_records_the_functions_it_hands_its_parameter_to() {
        // First, second, awaited, through a cast, and to a function this
        // file declares: each with the position the instance is written at.
        let source = "import { first, second } from './routes';\nimport third from './third';\nfunction local(deps, app) {}\napp.register(async (api) => {\n  first(api, deps);\n  second(deps, api);\n  await third(api as Server);\n  local(deps, api);\n});";
        assert_eq!(
            inner_of(source),
            vec![
                handed(5, "first", Some("./routes"), 0),
                handed(6, "second", Some("./routes"), 1),
                handed(7, "third", Some("./third"), 0),
                handed(8, "local", None, 1),
            ]
        );
    }

    #[test]
    fn a_call_that_does_not_hand_over_the_instance_itself_is_not_recorded() {
        // A member of it, a value made from it, an object holding it, a
        // spread before it, the instance twice, a callee that names no
        // function, and a method of some other object.
        let source = "import { routes } from './routes';\nconst table = { a: 1 };\napp.register(async (api) => {\n  routes(api.inner);\n  routes(wrap(api));\n  routes({ api });\n  routes(...rest, api);\n  routes(...[api]);\n  routes(api, api);\n  table(api);\n  undeclared(api);\n  registry.add(api);\n  routes(deps);\n});";
        assert_eq!(inner_of(source), vec![]);
    }

    #[test]
    fn another_binding_of_the_parameters_name_is_not_handed_on() {
        let source = "import { routes } from './routes';\napp.register(async (api) => {\n  {\n    const api = other();\n    routes(api);\n  }\n  list.forEach((api) => routes(api));\n});";
        let found = sites(source);
        let outer = found.iter().find(|site| site.line == 2).expect("outer");
        assert!(outer.inner_registrations.is_empty(), "{outer:?}");
    }

    #[test]
    fn a_value_that_names_no_function_is_not_recorded() {
        // A string, a call's result, a const holding neither a function nor
        // an import, a type-only import and a namespace: none names a
        // function this file can point at.
        let source = "import type { T } from './t';\nimport * as all from './all';\nconst options = { a: 1 };\napp.register(async (api) => {\n  api.addHook('onRequest', handler);\n  api.register(make());\n  api.decorate(options);\n  api.register(T);\n  api.register(all);\n  api.register(undeclared);\n});";
        assert_eq!(inner_of(source), vec![]);
    }

    #[test]
    fn a_call_on_another_binding_of_the_parameters_name_is_not_recorded() {
        // The inner callback declares `api` again: its calls are its own. The
        // call on some other instance is no one's.
        let source = "import { items } from './items';\napp.register(async (api) => {\n  other.register(items);\n  api.group(async (api) => {\n    api.register(items);\n  });\n});";
        let found = sites(source);
        let outer = found.iter().find(|site| site.line == 2).expect("outer");
        let inner = found.iter().find(|site| site.line == 4).expect("inner");
        assert!(outer.inner_registrations.is_empty(), "{outer:?}");
        assert_eq!(inner.inner_registrations.len(), 1);
        assert_eq!(inner.inner_registrations[0].line, 5);
    }

    #[test]
    fn a_shadowed_import_name_is_not_the_import() {
        let source = "import { items } from './items';\napp.register(async (api) => {\n  const items = load();\n  api.register(items);\n});";
        assert_eq!(inner_of(source), vec![]);
    }

    #[test]
    fn scope_ids_are_told_from_binding_names() {
        assert!(is_scope_id(&scope_id("api", "src/a.ts", 3)));
        assert!(!is_scope_id("api"));
        assert!(!is_scope_id("@scope/pkg"));
    }

    fn hand_outs_of(source: &str) -> Vec<(u32, String, usize, String, usize)> {
        sites(source)
            .iter()
            .flat_map(|site| site.hand_outs.clone())
            .map(|hand_out| {
                (
                    hand_out.line,
                    hand_out.function,
                    hand_out.options_position,
                    hand_out.key,
                    hand_out.position,
                )
            })
            .collect()
    }

    #[test]
    fn a_callback_records_the_option_it_hands_its_instance_to() {
        // A declaration, a declarator, an optional call, a defaulted
        // parameter and the instance written second.
        let source = "export async function plugin(app, opts) {\n  app.register(async (api) => {\n    await opts.registerExtra(api);\n    opts.onReady?.(api);\n  }, { prefix: '/v1' });\n}\nexport const other = async (app, deps, opts = {}) => {\n  app.register(async (api) => { opts.extend(deps, api); });\n};";
        assert_eq!(
            hand_outs_of(source),
            vec![
                (3, "plugin".to_string(), 1, "registerExtra".to_string(), 0),
                (4, "plugin".to_string(), 1, "onReady".to_string(), 0),
                (8, "other".to_string(), 2, "extend".to_string(), 1),
            ]
        );
    }

    #[test]
    fn a_hand_out_that_names_no_option_of_the_enclosing_function_is_not_recorded() {
        // A computed key, a receiver that is not a parameter, the plugin's
        // own instance, a spread, and a registration in an unnamed function.
        let source = "export async function plugin(app, opts) {\n  const local = opts;\n  app.register(async (api) => {\n    opts[key](api);\n    local.registerExtra(api);\n    opts.registerExtra(app);\n    opts.registerExtra(...[api]);\n  });\n}\nexport default async (app, opts) => {\n  app.register(async (api) => { opts.registerExtra(api); });\n};";
        assert_eq!(hand_outs_of(source), vec![]);
    }

    #[test]
    fn a_relative_specifier_names_the_declaring_file() {
        assert_eq!(
            relative_specifier(
                Path::new("/r/a/src/index.ts"),
                Path::new("/r/a/src/notes.ts")
            ),
            Some("./notes.ts".to_string())
        );
        assert_eq!(
            relative_specifier(
                Path::new("/r/a/src/index.ts"),
                Path::new("/r/b/lib/notes.ts")
            ),
            Some("../../b/lib/notes.ts".to_string())
        );
    }
}
