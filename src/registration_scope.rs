// FROZEN (old path, 2026-10-21): deletions and safety fixes only. Fixes for
// wrong or missing rows go in the compiler rows (receiver identity, call
// targets), library claims (scope, runs, prefix) and request summaries (value
// flow). Delete when routes come from the trace (carrick#2474 R3), with
// discovery on by default and a clean four-repo replay (carrick#2474).

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

/// A call `P.m(F, …)` inside a registration callback, where `P` is the
/// callback's first parameter and `F` an identifier bound to an import or to
/// a function this file declares. Whether `F` registers anything is not
/// something this file says: see [`mount_unstated_registrations`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InnerRegistration {
    /// The call's start line.
    pub line: u32,
    /// `m`, kept for the emitted row's `pattern_matched`.
    pub method: String,
    /// `F`, as this file names it.
    pub identifier: String,
    /// The specifier `F` is imported from, as written; `None` for a function
    /// this file declares.
    pub import_source: Option<String>,
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

/// Reads the calls a registration callback makes on its own first parameter
/// (carrick#2303). The parameter and the function passed are both read by
/// binding, so a name the callback or a block inside it declares again is
/// never taken for either.
struct InnerFinder<'s> {
    source_map: &'s Lrc<SourceMap>,
    values: &'s DeclaredValues,
    param: BindingKey,
    found: Vec<InnerRegistration>,
}

impl Visit for InnerFinder<'_> {
    fn visit_call_expr(&mut self, call: &CallExpr) {
        call.visit_children_with(self);
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
        if ident_key(receiver) != self.param {
            return;
        }
        let Some(first) = call.args.first().filter(|arg| arg.spread.is_none()) else {
            return;
        };
        let Expr::Ident(passed) = unwrap_expression(&first.expr) else {
            return;
        };
        let key = ident_key(passed);
        let import_source = self.values.imports.get(&key).cloned();
        if import_source.is_none() && !self.values.functions.contains(&key) {
            return; // a value this file binds some other way names no function
        }
        self.found.push(InnerRegistration {
            line: u32::try_from(self.source_map.lookup_char_pos(call.span.lo).line).unwrap_or(0),
            method: method.sym.to_string(),
            identifier: passed.sym.to_string(),
            import_source,
        });
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

/// Where a declared function starts and ends, and the name of its first
/// parameter.
struct FunctionHead {
    first_param: String,
    start_line: u32,
    end_line: u32,
}

/// The functions a module declares by name, read on demand: only the modules
/// an unstated inner registration names are parsed.
struct FunctionHeads {
    source_map: Lrc<SourceMap>,
    handler: Handler,
    by_file: HashMap<PathBuf, Vec<(String, Option<FunctionHead>)>>,
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

    /// The one function `file` declares as `name`, when its first parameter
    /// is a plain name. A name the module declares twice is not decided.
    fn head(&mut self, file: &Path, name: &str) -> Option<&FunctionHead> {
        if !self.by_file.contains_key(file) {
            let mut collector = HeadCollector {
                source_map: &self.source_map,
                heads: Vec::new(),
            };
            if let Some(module) = crate::parser::parse_file(file, &self.source_map, &self.handler) {
                module.visit_with(&mut collector);
            }
            let heads = collector.heads;
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
        head.as_ref()
    }
}

struct HeadCollector<'s> {
    source_map: &'s Lrc<SourceMap>,
    heads: Vec<(String, Option<FunctionHead>)>,
}

impl HeadCollector<'_> {
    fn record(&mut self, name: &Ident, first_param: Option<&Pat>, span: swc_common::Span) {
        let line =
            |pos| u32::try_from(self.source_map.lookup_char_pos(pos).line).unwrap_or(u32::MAX);
        let head = match first_param {
            Some(Pat::Ident(param)) => Some(FunctionHead {
                first_param: param.id.sym.to_string(),
                start_line: line(span.lo),
                end_line: line(span.hi),
            }),
            _ => None,
        };
        self.heads.push((name.sym.to_string(), head));
    }
}

impl Visit for HeadCollector<'_> {
    fn visit_fn_decl(&mut self, decl: &FnDecl) {
        let function = &decl.function;
        self.record(
            &decl.ident,
            function.params.first().map(|param| &param.pat),
            function.span,
        );
        decl.visit_children_with(self);
    }

    fn visit_fn_expr(&mut self, function: &FnExpr) {
        // `export default function name(app) {}` and any other named function
        // expression: the name is a declaration only where the module exports
        // it, which is what the caller's resolver has already decided.
        if let Some(name) = &function.ident {
            let inner = &function.function;
            self.record(
                name,
                inner.params.first().map(|param| &param.pat),
                inner.span,
            );
        }
        function.visit_children_with(self);
    }

    fn visit_var_declarator(&mut self, declarator: &VarDeclarator) {
        if let (Pat::Ident(name), Some(init)) = (&declarator.name, &declarator.init) {
            match unwrap_expression(init) {
                Expr::Arrow(arrow) => self.record(&name.id, arrow.params.first(), arrow.span),
                Expr::Fn(function) if function.ident.is_none() => self.record(
                    &name.id,
                    function.function.params.first().map(|param| &param.pat),
                    function.function.span,
                ),
                _ => {}
            }
        }
        declarator.visit_children_with(self);
    }
}

/// State the mounts a registration callback makes and the model did not
/// (carrick#2303). Returns how many mount rows the files gained, and the
/// scopes this made whose prefix the source does not state as a literal.
///
/// An inner registration `P.m(F)` becomes a mount row of its file when both
/// hold:
///
/// * the file's rows hold no mount at that line, so a model row always wins
///   and nothing is stated twice;
/// * `F` resolves to a function whose first parameter is a plain name, and at
///   least one stated route or mount inside that function's span hangs from
///   that parameter. A function that registers nothing on its first parameter
///   (an error handler, a hook) is not mounted.
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

    // The resolver the mount graph reads a mount's child through, so a
    // function this decides is one the graph can attribute routes to.
    let mut resolver = BindingResolver::new();
    let mut heads = FunctionHeads::new();
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
            for site in &file.sites {
                if registers_a_route(result, site) {
                    continue;
                }
                for inner in &site.inner_registrations {
                    if mount_stated_at(result, inner.line) {
                        continue;
                    }
                    let (declaring, name) = match &inner.import_source {
                        // By the name this file gives it, as the graph reads
                        // the emitted row's child: an import the graph cannot
                        // follow (one renamed on the way in) is not decided.
                        Some(specifier) => {
                            let Some(resolved) =
                                resolver.resolve(&importer, specifier, &inner.identifier)
                            else {
                                continue;
                            };
                            let Some(name) = resolved.local_name else {
                                continue; // an export with no name to find
                            };
                            (resolved.file, name)
                        }
                        None => (importer.clone(), inner.identifier.clone()),
                    };
                    let Some(declaring_rows) = key_by_canonical
                        .get(&declaring)
                        .and_then(|key| file_results.get(key))
                    else {
                        continue;
                    };
                    let Some(head) = heads.head(&declaring, &name) else {
                        continue;
                    };
                    let inside = |line: i32| {
                        u32::try_from(line)
                            .is_ok_and(|line| line >= head.start_line && line <= head.end_line)
                    };
                    let registers = declaring_rows.mounts.iter().any(|mount| {
                        mount.parent_node == head.first_param && inside(mount.line_number)
                    }) || declaring_rows.endpoints.iter().any(|endpoint| {
                        endpoint.owner_node == head.first_param
                            && is_served_path(&endpoint.path)
                            && inside(endpoint.line_number)
                    });
                    if !registers {
                        continue;
                    }
                    rows.push(MountResult {
                        line_number: i32::try_from(inner.line).unwrap_or(i32::MAX),
                        parent_node: site.param.clone(),
                        child_node: inner.identifier.clone(),
                        mount_path: String::new(),
                        import_source: inner.import_source.clone(),
                        pattern_matched: format!("{}.{}(", site.param, inner.method),
                    });
                }
            }
            if rows.is_empty() {
                continue;
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
            method: method.to_string(),
            identifier: identifier.to_string(),
            import_source: import.map(str::to_string),
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
