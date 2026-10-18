//! Calls into a same-module request helper that pass a route literal where the
//! helper builds its URL (carrick-cloud#2373, "Signal 4b" of the candidate
//! scanner).
//!
//! A client object whose methods are arrows returning `call('/books', o)`
//! makes every site an un-awaited call into a function the module declares.
//! The awaited-call signal never sees it, and when the helper's shape is one
//! [`crate::local_http_wrapper`] cannot read (a URL built by a call, a method
//! destructured with a default, an options bag forwarded by the site) the site
//! has no deterministic row either. Nothing counts it: the model is never
//! offered it, so no counter reads it as missing.
//!
//! This pass reads only which functions are request helpers and which of their
//! parameters reach the request's URL. It states no method and no path; the
//! sites it returns are handed to the model as candidates.
//!
//! - A *request helper* is a named function declaration, or a `const` bound to
//!   an arrow or function, that directly makes a request: global `fetch`,
//!   `navigator.sendBeacon`, a call that
//!   [`crate::wrapper_request_shape::call_request_shape`] reads as a request,
//!   or a call to another request helper. Calls inside a nested closure count
//!   for the named function that encloses it.
//! - A parameter is a *URL position* when its binding occurs anywhere inside
//!   the request call's first argument (`fetch(withQuery(path, query))` makes
//!   both `path` and `query` URL positions). Through a call to another helper,
//!   a parameter is a URL position when the argument it appears in sits at
//!   that helper's URL position. Resolved to a fixpoint.
//! - A *site* is a call to a helper by its binding, with a string or template
//!   literal starting with `/` at one of its URL positions. A literal that
//!   reaches only a body is not a site.

use std::collections::{BTreeSet, HashMap, HashSet};

use swc_ecma_ast::*;
use swc_ecma_visit::{Visit, VisitWith};

use crate::binding_scope::{BindingKey, ident_key, pat_key};
use crate::local_http_wrapper::{LocalWrapperCall, callee_property};
use crate::wrapper_request_shape::{RequestShapeSignal, call_request_shape};

/// A call to `callee` and, per argument position, the caller parameters that
/// argument mentions.
struct Delegation {
    callee: BindingKey,
    args: Vec<BTreeSet<usize>>,
}

struct HelperFrame {
    key: BindingKey,
    params: Vec<Option<BindingKey>>,
    /// Parameters mentioned in the first argument of a request this function
    /// makes itself.
    url_params: BTreeSet<usize>,
    delegations: Vec<Delegation>,
}

/// The start and end offsets of every call site in `module` that passes a
/// route literal to a same-module request helper, minus the sites
/// `local_wrapper_calls` already states.
pub fn collect_request_helper_sites(
    module: &Module,
    local_wrapper_calls: &[LocalWrapperCall],
) -> HashSet<(u32, u32)> {
    let mut frames = FrameCollector::default();
    module.visit_with(&mut frames);
    let helpers = resolve_url_positions(frames.finished);
    if helpers.is_empty() {
        return HashSet::new();
    }

    let mut sites = SiteCollector {
        helpers,
        sites: HashSet::new(),
    };
    module.visit_with(&mut sites);
    for stated in local_wrapper_calls {
        sites.sites.remove(&(stated.span_start, stated.span_end));
    }
    sites.sites
}

#[derive(Default)]
struct FrameCollector {
    stack: Vec<HelperFrame>,
    finished: Vec<HelperFrame>,
}

impl FrameCollector {
    fn push(&mut self, ident: &Ident, params: Vec<Option<BindingKey>>) {
        self.stack.push(HelperFrame {
            key: ident_key(ident),
            params,
            url_params: BTreeSet::new(),
            delegations: Vec::new(),
        });
    }

    fn pop(&mut self) {
        if let Some(frame) = self.stack.pop() {
            self.finished.push(frame);
        }
    }
}

impl Visit for FrameCollector {
    fn visit_fn_decl(&mut self, node: &FnDecl) {
        self.push(
            &node.ident,
            node.function
                .params
                .iter()
                .map(|p| pat_key(&p.pat))
                .collect(),
        );
        node.visit_children_with(self);
        self.pop();
    }

    fn visit_var_declarator(&mut self, node: &VarDeclarator) {
        let framed = match (&node.name, node.init.as_deref()) {
            (Pat::Ident(binding), Some(Expr::Arrow(arrow))) => {
                self.push(&binding.id, arrow.params.iter().map(pat_key).collect());
                true
            }
            (Pat::Ident(binding), Some(Expr::Fn(function))) => {
                self.push(
                    &binding.id,
                    function
                        .function
                        .params
                        .iter()
                        .map(|p| pat_key(&p.pat))
                        .collect(),
                );
                true
            }
            _ => false,
        };
        node.visit_children_with(self);
        if framed {
            self.pop();
        }
    }

    fn visit_call_expr(&mut self, call: &CallExpr) {
        if let Some(frame) = self.stack.last_mut() {
            if is_request_call(call) {
                if let Some(url) = call.args.first().filter(|a| a.spread.is_none()) {
                    let mentioned = mentioned_params(&url.expr, &frame.params);
                    frame.url_params.extend(mentioned);
                }
            } else if let Callee::Expr(callee) = &call.callee
                && let Expr::Ident(ident) = &**callee
            {
                let args = call
                    .args
                    .iter()
                    .map(|arg| match arg.spread {
                        Some(_) => BTreeSet::new(),
                        None => mentioned_params(&arg.expr, &frame.params),
                    })
                    .collect();
                frame.delegations.push(Delegation {
                    callee: ident_key(ident),
                    args,
                });
            }
        }
        call.visit_children_with(self);
    }
}

/// Whether `call` is a request where it stands: global `fetch`,
/// `navigator.sendBeacon`, or a call the wrapper-shape reader takes for one.
fn is_request_call(call: &CallExpr) -> bool {
    if let Callee::Expr(callee) = &call.callee {
        match &**callee {
            Expr::Ident(ident) if ident.sym.as_ref() == "fetch" => return true,
            Expr::Member(member) => {
                if let (Expr::Ident(obj), MemberProp::Ident(prop)) = (&*member.obj, &member.prop)
                    && obj.sym.as_ref() == "navigator"
                    && prop.sym.as_ref() == "sendBeacon"
                {
                    return true;
                }
            }
            _ => {}
        }
    }
    call_request_shape(call, callee_property(call).as_deref()) != RequestShapeSignal::NotARequest
}

/// The positions of `params` whose binding occurs anywhere in `expr`.
fn mentioned_params(expr: &Expr, params: &[Option<BindingKey>]) -> BTreeSet<usize> {
    struct Mentions<'a> {
        params: &'a [Option<BindingKey>],
        found: BTreeSet<usize>,
    }
    impl Visit for Mentions<'_> {
        fn visit_ident(&mut self, ident: &Ident) {
            let key = ident_key(ident);
            if let Some(position) = self.params.iter().position(|p| p.as_ref() == Some(&key)) {
                self.found.insert(position);
            }
        }
        // `a.b`: the property name is not a binding.
        fn visit_member_prop(&mut self, prop: &MemberProp) {
            if let MemberProp::Computed(computed) = prop {
                computed.visit_with(self);
            }
        }
    }
    let mut mentions = Mentions {
        params,
        found: BTreeSet::new(),
    };
    expr.visit_with(&mut mentions);
    mentions.found
}

/// Every function with at least one URL position, by binding. A binding two
/// functions share (possible only where the resolver has not run) names no
/// single function and is dropped.
fn resolve_url_positions(frames: Vec<HelperFrame>) -> HashMap<BindingKey, BTreeSet<usize>> {
    let mut counts: HashMap<&BindingKey, usize> = HashMap::new();
    for frame in &frames {
        *counts.entry(&frame.key).or_default() += 1;
    }
    let frames: Vec<&HelperFrame> = frames
        .iter()
        .filter(|frame| counts.get(&frame.key) == Some(&1))
        .collect();

    let mut positions: HashMap<BindingKey, BTreeSet<usize>> = frames
        .iter()
        .filter(|frame| !frame.url_params.is_empty())
        .map(|frame| (frame.key.clone(), frame.url_params.clone()))
        .collect();
    // Positions only grow and a function has finitely many, so this ends.
    loop {
        let mut grew = false;
        for frame in &frames {
            for delegation in &frame.delegations {
                if delegation.callee == frame.key {
                    continue;
                }
                let Some(callee_positions) = positions.get(&delegation.callee).cloned() else {
                    continue;
                };
                let reached: BTreeSet<usize> = callee_positions
                    .iter()
                    .filter_map(|j| delegation.args.get(*j))
                    .flatten()
                    .copied()
                    .collect();
                if reached.is_empty() {
                    continue;
                }
                let held = positions.entry(frame.key.clone()).or_default();
                for position in reached {
                    grew |= held.insert(position);
                }
            }
        }
        if !grew {
            break;
        }
    }
    positions
}

struct SiteCollector {
    helpers: HashMap<BindingKey, BTreeSet<usize>>,
    sites: HashSet<(u32, u32)>,
}

impl Visit for SiteCollector {
    fn visit_call_expr(&mut self, call: &CallExpr) {
        if let Callee::Expr(callee) = &call.callee
            && let Expr::Ident(ident) = &**callee
            && let Some(positions) = self.helpers.get(&ident_key(ident))
            && positions.iter().any(|position| {
                call.args
                    .get(*position)
                    .filter(|arg| arg.spread.is_none())
                    .is_some_and(|arg| is_route_literal(&arg.expr))
            })
        {
            self.sites.insert((call.span.lo.0, call.span.hi.0));
        }
        call.visit_children_with(self);
    }
}

/// A string or template literal that starts with `/`.
fn is_route_literal(expr: &Expr) -> bool {
    match expr {
        Expr::Lit(Lit::Str(literal)) => literal.value.to_string_lossy().starts_with('/'),
        Expr::Tpl(tpl) => tpl
            .quasis
            .first()
            .is_some_and(|quasi| quasi.raw.starts_with('/')),
        _ => false,
    }
}
