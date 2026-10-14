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
    /// `instance`: the name the file gives the instance handed over.
    fn mount_row(&self, instance: &str) -> MountResult {
        MountResult {
            line_number: i32::try_from(self.line).unwrap_or(i32::MAX),
            parent_node: instance.to_string(),
            child_node: self.identifier.clone(),
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

/// Finds the sites. Without `values` it only finds them (their arguments
/// read as unread); with them it reads each argument value.
struct SiteFinder<'s> {
    source_map: &'s Lrc<SourceMap>,
    values: Option<&'s DeclaredValues>,
    sites: Vec<RegistrationSite>,
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
        });
    }
}

impl Visit for SiteFinder<'_> {
    fn visit_call_expr(&mut self, call: &CallExpr) {
        self.consider(call);
        call.visit_children_with(self);
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
    /// registration without a mount at its line. A file whose every inner
    /// registration the model stated holds nothing to decide.
    pub fn of(
        result: &FileAnalysisResult,
        sites: &[RegistrationSite],
        file_key: &str,
        file: &str,
    ) -> Option<Self> {
        sites
            .iter()
            .flat_map(|site| &site.inner_registrations)
            .any(|inner| !mount_stated_at(result, inner.line))
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
    /// The resolver the mount graph reads a mount's child through, so a
    /// function this decides is one the graph can attribute routes to.
    resolver: BindingResolver,
    heads: FunctionHeads,
    /// The functions on the walk, so one that hands the instance back to a
    /// function already on it ends there.
    walking: Vec<(PathBuf, String, usize)>,
}

impl InstanceTrace<'_> {
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
    /// mount row of the file that makes the call, hanging from `name`, the
    /// function the call runs in: that is the node the row that reached the
    /// function mounts. A function its own file reaches by name is a local
    /// child of that file and is kept as it is, so the hand-on composes under
    /// the scope that made the call. The parameter would resolve file-first to
    /// the binding the file is mounted under, which is not the scope the call
    /// runs in. An imported function resolves through the binding of the
    /// import that reached it. A line that already holds a mount is the
    /// model's and is left to it.
    fn registers(
        &mut self,
        file_results: &HashMap<String, FileAnalysisResult>,
        declaring: &Path,
        name: &str,
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
        // A hand-on this pass wrote hangs from the function's name.
        let mut registers = rows.mounts.iter().any(|mount| {
            (&mount.parent_node == param || mount.parent_node == name) && inside(mount.line_number)
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
            if self.registers(
                file_results,
                &file,
                &function,
                pass.position,
                depth + 1,
                hand_ons,
            ) {
                registers = true;
                hand_ons.push((key.clone(), pass.mount_row(name)));
            }
        }
        self.walking.pop();
        registers
    }
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
/// `file_root` is the directory the `file_results` keys resolve against on
/// disk. No method, option, library or framework name is read.
pub fn mount_unstated_registrations(
    file_results: &mut HashMap<String, FileAnalysisResult>,
    pending: &[UnstatedRegistrations],
    file_root: &Path,
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
        resolver: BindingResolver::new(),
        heads: FunctionHeads::new(),
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
                    if trace.registers(
                        file_results,
                        &declaring,
                        &name,
                        inner.position,
                        0,
                        &mut hand_ons,
                    ) {
                        rows.push(inner.mount_row(&site.param));
                    }
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
}
