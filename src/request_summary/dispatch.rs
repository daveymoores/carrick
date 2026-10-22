//! The routes a hand-written dispatch states (carrick#2474 R4a): a request's
//! path compared with a literal (`url.pathname === "/orders"`, `case
//! "/health":`, `ROUTES[pathname]`), each read as one route at the line it is
//! compared on, with the method compared beside it or around it.
//!
//! A path comparison is a route when its chain ([`super::guards`]) reaches
//! the request through one of two arms:
//!
//! - **label**: the first call the comparing function is handed to is a
//!   labelled call discovery found, whose op is `receive`, and the store's
//!   label for it names the handler slot the function sits in, the
//!   parameter that is the request, and the member path the comparison
//!   reads (`request.path`, `request.url`, or the URL's `pathname`). The
//!   route's kind is forced.
//! - **entry point**: the chain ends at a function the module offers that
//!   nothing in the repository calls, and is handed to no call. Its kind is
//!   asked later (R4b): no label names the request's members.
//!
//! The method is a method literal compared, not negated, on the same
//! request: a top-level `&&` operand of the same test, or a guard (an `if`
//! test operand or a `case`) whose branch holds the path comparison. For a
//! predicate the comparison is first looked for in the predicate, then at
//! the caller's call that tests it. A `startsWith` literal states a scope,
//! never a route. Every comparison that states no route is counted by a
//! reason from a closed list.
//!
//! Record-only: a scan with the receiver trace on logs the routes and drops
//! them. Nothing reaches the blob.
//!
//! [`handler_records`] (carrick#2474 R4b, in `dispatch/records.rs`) groups
//! the same comparisons by the request handler they sit in, as the record a
//! model is asked about.

use std::collections::{BTreeMap, HashMap};
use std::path::Path;

use serde::Serialize;

use super::RequestSummaryInputs;
use super::Site;
use super::decided_sites::{ArgSlot, DiscoverySite, LabelledEnd, SiteLabel};
use super::guards::{
    Branch, ChainEnd, ChainFacts, Chains, GuardBranch, GuardChain, GuardIr, GuardPosition,
    GuardShape, Handoff, Step,
};
use crate::services::type_sidecar::RequestAt;
use crate::type_manifest::is_http_method;

mod records;
pub use records::{HoleReach, handler_records};

/// Where a route's method comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum MethodFrom {
    /// A top-level `&&` operand of the same test.
    Conjunct,
    /// A guard whose branch holds the path comparison.
    Enclosing,
    None,
}

/// What makes a comparison a route.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Evidence {
    /// The labelled `receive` call the handler is handed to, at `file:line`.
    Label {
        package: String,
        export: String,
        member: Option<String>,
        at: String,
    },
    /// The function nothing in the repository calls, at `file:line`.
    EntryPoint { function: String, at: String },
}

impl Evidence {
    /// `label` or `entry_point`.
    pub fn kind(&self) -> &'static str {
        match self {
            Self::Label { .. } => "label",
            Self::EntryPoint { .. } => "entry_point",
        }
    }
}

/// Whether the route's kind is settled by its label or asked later.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum KindSlot {
    Forced,
    Asked,
}

/// The request parameter a route reads, and the accesses down from it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RequestRead {
    pub param: usize,
    pub steps: Vec<String>,
}

/// The handed or entry function: what R4b types the response from.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct HandlerAt {
    pub file: String,
    pub line: u32,
    pub function: Option<String>,
}

/// One route a dispatch states.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DispatchRoute {
    pub file: String,
    pub line: u32,
    pub path: String,
    pub method: Option<String>,
    pub method_from: MethodFrom,
    pub position: GuardPosition,
    pub evidence: Evidence,
    pub kind_slot: KindSlot,
    pub request: RequestRead,
    pub handler: HandlerAt,
    pub branch: Vec<Branch>,
}

/// A `startsWith` path literal on the request: a scope, never a route. Its
/// prefix composes onto nothing here.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DispatchScope {
    pub file: String,
    pub line: u32,
    pub prefix: String,
    pub evidence: Evidence,
    /// The calls its branch makes.
    pub delegates: Vec<Branch>,
}

/// A service's dispatch routes, sorted, and what states none.
#[derive(Debug, Default, Serialize)]
pub struct Dispatch {
    pub routes: Vec<DispatchRoute>,
    pub scopes: Vec<DispatchScope>,
    /// Chains (per guard for `unresolved_constant`) that state no route, by
    /// reason.
    pub not_routes: BTreeMap<String, usize>,
    /// What was read: `path_guards`, `table_keys`, `starts_with`, and
    /// `method_conflict` (routes whose qualifying methods disagree).
    pub counts: BTreeMap<String, usize>,
}

/// What a route or a method reaches the request through: the handoff call
/// (the function it is written in, its SWC span) and parameter, or the
/// entry function and parameter.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Anchor {
    Call(usize, u32, u32, usize),
    Entry(usize, usize),
}

/// A chain that reached the request: how, and what it reads.
#[derive(Clone)]
struct Reached {
    anchor: Anchor,
    evidence: Evidence,
    kind_slot: KindSlot,
    request: RequestRead,
    handler: HandlerAt,
    /// The label's method member path, as steps (label arm only).
    method_steps: Option<Vec<Step>>,
}

/// A method literal compared in a function, with its chains.
struct MethodGuard<'g> {
    guard: &'g GuardIr,
    method: String,
    chains: Vec<(GuardChain, ChainFacts)>,
}

/// Where a method is looked for: the node, the comparison's span, the
/// branch of the test it stands in, and whether it is a top-level `&&`
/// operand there.
struct Decision {
    node: usize,
    span: (u32, u32),
    branch: Option<(u32, u32)>,
    conjunct: bool,
}

/// Every route the guards in `inputs` state, read against discovery's
/// `discovered` sites and the store's `labels`, files relative to `root`.
pub fn dispatch_routes(
    inputs: &RequestSummaryInputs,
    discovered: &[DiscoverySite],
    labels: &[SiteLabel],
    root: &Path,
) -> Dispatch {
    let chains = Chains::new(inputs, root);
    let reader = Reader {
        chains: &chains,
        discovered,
        labels,
    };
    let mut dispatch = Dispatch::default();
    let mut methods: HashMap<usize, Vec<MethodGuard<'_>>> = HashMap::new();
    let mut paths: Vec<(usize, &GuardIr, String)> = Vec::new();
    for (node, at) in chains.nodes.iter().enumerate() {
        for guard in &at.ir.guards {
            let (text, shape, _) = chains.literal(node, &guard.literal);
            match shape {
                GuardShape::Unresolved => count(&mut dispatch.not_routes, "unresolved_constant"),
                GuardShape::Path => paths.push((node, guard, text)),
                GuardShape::Name if is_http_method(&text) => {
                    methods.entry(node).or_default().push(MethodGuard {
                        guard,
                        method: text.trim().to_uppercase(),
                        chains: chains.traced(node, guard),
                    })
                }
                GuardShape::Name => {}
            }
        }
    }
    let mut routes: Vec<DispatchRoute> = Vec::new();
    let mut scopes: Vec<DispatchScope> = Vec::new();
    for (node, guard, text) in paths {
        let read = match guard.position {
            GuardPosition::TableKey => "table_keys",
            GuardPosition::StartsWith => "starts_with",
            GuardPosition::Equality | GuardPosition::Case => "path_guards",
        };
        count(&mut dispatch.counts, read);
        for (chain, facts) in chains.traced(node, guard) {
            if chain.negated {
                count(&mut dispatch.not_routes, "negated");
                continue;
            }
            let reached = match reader.reached(&chain, &facts) {
                Ok(reached) => reached,
                Err(reason) => {
                    count(&mut dispatch.not_routes, &reason);
                    continue;
                }
            };
            if text.contains('?') || text.contains('#') {
                count(&mut dispatch.not_routes, "literal_has_query");
                continue;
            }
            let file = chains.rel(chains.nodes[node].file);
            if guard.position == GuardPosition::StartsWith {
                scopes.push(DispatchScope {
                    file,
                    line: guard.site.line,
                    prefix: text.clone(),
                    evidence: reached.evidence,
                    delegates: chain
                        .branch
                        .iter()
                        .filter(|branch| matches!(branch, Branch::Call { .. }))
                        .cloned()
                        .collect(),
                });
                continue;
            }
            let (stated, from) = method_of(node, guard, &facts, &reached, &methods);
            if matches!(stated, Methods::Conflict) {
                count(&mut dispatch.counts, "method_conflict");
            }
            let line = guard.literal_line.unwrap_or(guard.site.line);
            for method in stated.each() {
                routes.push(DispatchRoute {
                    file: file.clone(),
                    line,
                    path: text.clone(),
                    method_from: if method.is_some() {
                        from
                    } else {
                        MethodFrom::None
                    },
                    method,
                    position: guard.position,
                    evidence: reached.evidence.clone(),
                    kind_slot: reached.kind_slot,
                    request: reached.request.clone(),
                    handler: reached.handler.clone(),
                    branch: chain.branch.clone(),
                });
            }
        }
    }
    // One route per (file, line, path, method): a label's over an entry
    // point's, then the first by its serialised form.
    routes.sort_by_cached_key(|route| {
        (
            (
                route.file.clone(),
                route.line,
                route.path.clone(),
                route.method.clone(),
            ),
            route.evidence.kind() != "label",
            serde_json::to_string(route).unwrap_or_default(),
        )
    });
    routes.dedup_by(|later, kept| {
        (&later.file, later.line, &later.path, &later.method)
            == (&kept.file, kept.line, &kept.path, &kept.method)
    });
    scopes.sort_by_cached_key(|scope| {
        (
            (scope.file.clone(), scope.line, scope.prefix.clone()),
            scope.evidence.kind() != "label",
            serde_json::to_string(scope).unwrap_or_default(),
        )
    });
    scopes.dedup_by(|later, kept| {
        (&later.file, later.line, &later.prefix) == (&kept.file, kept.line, &kept.prefix)
    });
    dispatch.routes = routes;
    dispatch.scopes = scopes;
    dispatch
}

fn count(counts: &mut BTreeMap<String, usize>, key: &str) {
    *counts.entry(key.to_string()).or_default() += 1;
}

/// What the end of a chain is called, for a reason: a hole's reason, or the
/// end's kind.
fn end_kind(end: &ChainEnd) -> String {
    let word = |value: serde_json::Result<serde_json::Value>| {
        value
            .ok()
            .and_then(|value| value.as_str().map(str::to_string))
            .unwrap_or_default()
    };
    match end {
        ChainEnd::Hole { reason, .. } => word(serde_json::to_value(reason)),
        ChainEnd::EntryPoint { .. } => "entry_point".to_string(),
        ChainEnd::Literal { .. } => "literal".to_string(),
        ChainEnd::Env { .. } => "env".to_string(),
        ChainEnd::Global { .. } => "global".to_string(),
        ChainEnd::ExternalImport { .. } => "external_import".to_string(),
    }
}

/// `steps` with the String methods after the last access dropped
/// (`url.toLowerCase()` reads `url`).
fn read_steps(steps: &[Step]) -> &[Step] {
    let mut end = steps.len();
    while end > 0 && matches!(steps[end - 1], Step::Call(_)) {
        end -= 1;
    }
    &steps[..end]
}

/// A label's member path, as the steps a chain reads it with.
fn key_steps(path: &[String]) -> Vec<Step> {
    path.iter().map(|key| Step::Key(key.clone())).collect()
}

/// Whether `steps` read the request's path: its `path`, its `url`, or the
/// `pathname` of a URL made from its `url` (the URL standard's own member).
fn reads_path(steps: &[Step], request: &RequestAt) -> bool {
    let steps = read_steps(steps);
    if let Some(path) = &request.path
        && steps == key_steps(path).as_slice()
    {
        return true;
    }
    let Some(url) = &request.url else {
        return false;
    };
    let url = key_steps(url);
    if steps == url.as_slice() {
        return true;
    }
    let mut parsed = url;
    parsed.push(Step::Url);
    parsed.push(Step::Key("pathname".to_string()));
    steps == parsed.as_slice()
}

/// Whether a label's handler slot is where the function was handed in.
fn slot_is(slot: &ArgSlot, arg: usize, path: &[String]) -> bool {
    slot.arg == arg
        && match &slot.key {
            None => path.is_empty(),
            Some(key) => path.len() == 1 && path[0] == *key,
        }
}

struct Reader<'r, 'a> {
    chains: &'r Chains<'a>,
    discovered: &'r [DiscoverySite],
    labels: &'r [SiteLabel],
}

impl Reader<'_, '_> {
    /// How `chain` reaches the request, or why it states no route.
    fn reached(&self, chain: &GuardChain, facts: &ChainFacts) -> Result<Reached, String> {
        if let Some(handoff) = &facts.handoff {
            return self.labelled(chain, handoff);
        }
        match (&chain.end, &facts.entry) {
            (ChainEnd::EntryPoint { function, .. }, Some(entry)) => {
                let node = &self.chains.nodes[entry.node];
                let file = self.chains.rel(node.file);
                let line = self.chains.line_of(entry.node);
                Ok(Reached {
                    anchor: Anchor::Entry(entry.node, entry.param),
                    evidence: Evidence::EntryPoint {
                        function: function.clone(),
                        at: format!("{file}:{line}"),
                    },
                    kind_slot: KindSlot::Asked,
                    request: RequestRead {
                        param: entry.param,
                        steps: entry.steps.iter().map(Step::render).collect(),
                    },
                    handler: HandlerAt {
                        file,
                        line,
                        function: node.key.clone(),
                    },
                    method_steps: None,
                })
            }
            (ChainEnd::Hole { .. }, _) => Err(format!("hole:{}", end_kind(&chain.end))),
            (end, _) => Err(format!("end:{}", end_kind(end))),
        }
    }

    /// The labelled end discovery found at the handoff call.
    fn end_at(&self, file: &str, handoff: &Handoff) -> Option<&LabelledEnd> {
        self.end_at_site(file, &handoff.site)
    }

    /// The labelled end discovery found at the call `site` in `file`.
    fn end_at_site(&self, file: &str, site: &Site) -> Option<&LabelledEnd> {
        self.discovered.iter().find_map(|found| {
            found.end.as_ref().filter(|end| {
                end.file == file
                    && end.span_start == site.span_start
                    && end.span_end == site.span_end
            })
        })
    }

    /// The labelled `receive` call written in `caller` at `site` whose
    /// label's handler slot is `arg` and `path`, the slot a function is
    /// handed in (carrick#2474 R4b): the function is that label's handler,
    /// whether or not the label names the request's members.
    fn receive_at(
        &self,
        caller: usize,
        site: &Site,
        arg: usize,
        path: &[String],
    ) -> Option<&LabelledEnd> {
        let file = self.chains.rel(self.chains.nodes[caller].file);
        let end = self.end_at_site(&file, site)?;
        if end.op.as_deref() != Some("receive") {
            return None;
        }
        self.labels
            .iter()
            .any(|label| {
                label.op.as_deref() == Some("receive")
                    && label.package == end.package
                    && label.export == end.export
                    && label.member == end.member
                    && label
                        .handler
                        .as_ref()
                        .is_some_and(|slot| slot_is(slot, arg, path))
            })
            .then_some(end)
    }

    /// The label arm: the handoff call is a labelled `receive` whose label
    /// names the handler slot, the request parameter and the path read.
    fn labelled(&self, chain: &GuardChain, handoff: &Handoff) -> Result<Reached, String> {
        let chains = self.chains;
        let file = chains.rel(chains.nodes[handoff.caller].file);
        let Some(end) = self.end_at(&file, handoff) else {
            return Err(format!("handoff_unlabelled:{}", end_kind(&chain.end)));
        };
        if end.op.as_deref() != Some("receive") {
            return Err("handoff_not_receive".to_string());
        }
        let mut agreed: Vec<(&Option<ArgSlot>, &Option<RequestAt>)> = Vec::new();
        for label in self.labels.iter().filter(|label| {
            label.op.as_deref() == Some("receive")
                && label.package == end.package
                && label.export == end.export
                && label.member == end.member
        }) {
            let read = (&label.handler, &label.request);
            if !agreed.contains(&read) {
                agreed.push(read);
            }
        }
        if agreed.len() > 1 {
            return Err("label_ambiguous".to_string());
        }
        let Some((handler, Some(request))) = agreed.first().copied() else {
            return Err("label_without_request".to_string());
        };
        if !handler
            .as_ref()
            .is_some_and(|slot| slot_is(slot, handoff.arg, &handoff.path))
        {
            return Err("handler_slot_mismatch".to_string());
        }
        if request.param as usize != handoff.param {
            return Err("request_param_mismatch".to_string());
        }
        if !reads_path(&handoff.steps, request) {
            return Err("steps_not_path".to_string());
        }
        let handed = &chains.nodes[handoff.handed];
        Ok(Reached {
            anchor: Anchor::Call(
                handoff.caller,
                handoff.site.span_start,
                handoff.site.span_end,
                handoff.param,
            ),
            evidence: Evidence::Label {
                package: end.package.clone(),
                export: end.export.clone(),
                member: end.member.clone(),
                at: end.at(),
            },
            kind_slot: KindSlot::Forced,
            request: RequestRead {
                param: handoff.param,
                steps: handoff.steps.iter().map(Step::render).collect(),
            },
            handler: HandlerAt {
                file: chains.rel(handed.file),
                line: chains.line_of(handoff.handed),
                function: handed.key.clone(),
            },
            method_steps: request.method.as_deref().map(key_steps),
        })
    }
}

/// The methods a route states.
enum Methods {
    None,
    One(String),
    /// `case "GET": case "HEAD":` sharing one branch.
    Each(Vec<String>),
    /// Qualifying methods that disagree otherwise.
    Conflict,
}

impl Methods {
    fn each(&self) -> Vec<Option<String>> {
        match self {
            Self::None | Self::Conflict => vec![None],
            Self::One(method) => vec![Some(method.clone())],
            Self::Each(methods) => methods.iter().cloned().map(Some).collect(),
        }
    }
}

/// The method a path guard's route states: looked for at the comparison,
/// then, for a predicate's comparison, at the caller's call that tests it.
fn method_of(
    node: usize,
    guard: &GuardIr,
    facts: &ChainFacts,
    reached: &Reached,
    methods: &HashMap<usize, Vec<MethodGuard<'_>>>,
) -> (Methods, MethodFrom) {
    method_reaching(node, guard, facts, methods, &|candidate| {
        candidate
            .chains
            .iter()
            .any(|(_, facts)| reaches_same(facts, reached))
    })
}

/// [`method_of`], with `reaches` saying whether a method guard reaches the
/// request the path comparison does (carrick#2474 R4b: a hole's comparison
/// has no anchor, only the handler both attach to).
fn method_reaching(
    node: usize,
    guard: &GuardIr,
    facts: &ChainFacts,
    methods: &HashMap<usize, Vec<MethodGuard<'_>>>,
    reaches: &dyn Fn(&MethodGuard<'_>) -> bool,
) -> (Methods, MethodFrom) {
    let own = Decision {
        node,
        span: (guard.site.lo, guard.site.hi),
        branch: match guard.branch {
            GuardBranch::Span(lo, hi) => Some((lo, hi)),
            _ => None,
        },
        conjunct: guard.conjunct,
    };
    let found = qualifying(&own, reaches, methods);
    let found = match (found.is_empty(), &guard.branch, facts.decided_at) {
        (true, GuardBranch::Returned, Some(at)) => qualifying(
            &Decision {
                node: at.node,
                span: at.span,
                branch: Some(at.branch),
                conjunct: at.conjunct,
            },
            reaches,
            methods,
        ),
        _ => found,
    };
    if found.is_empty() {
        return (Methods::None, MethodFrom::None);
    }
    let from = if found
        .iter()
        .any(|(_, from, _)| *from == MethodFrom::Conjunct)
    {
        MethodFrom::Conjunct
    } else {
        MethodFrom::Enclosing
    };
    let mut each: Vec<String> = found.iter().map(|(method, _, _)| method.clone()).collect();
    each.sort();
    each.dedup();
    if each.len() == 1 {
        return (Methods::One(each.remove(0)), from);
    }
    let first = &found[0].2;
    let one_case_branch = found
        .iter()
        .all(|(_, _, guard)| guard.position == GuardPosition::Case && guard.branch == first.branch);
    if one_case_branch {
        (Methods::Each(each), from)
    } else {
        (Methods::Conflict, MethodFrom::None)
    }
}

/// The method guards of `at`'s node that qualify for it, each with its
/// method, how it qualifies and the guard.
fn qualifying<'g>(
    at: &Decision,
    reaches: &dyn Fn(&MethodGuard<'_>) -> bool,
    methods: &'g HashMap<usize, Vec<MethodGuard<'g>>>,
) -> Vec<(String, MethodFrom, &'g GuardIr)> {
    let mut found = Vec::new();
    for candidate in methods.get(&at.node).into_iter().flatten() {
        let guard = candidate.guard;
        if guard.negated {
            continue;
        }
        let same_test = guard.conjunct
            && at.conjunct
            && at
                .branch
                .is_some_and(|(lo, hi)| guard.branch == GuardBranch::Span(lo, hi));
        let encloses = (guard.conjunct || guard.position == GuardPosition::Case)
            && matches!(guard.branch, GuardBranch::Span(lo, hi)
                if lo <= at.span.0 && at.span.1 <= hi);
        let from = if same_test {
            MethodFrom::Conjunct
        } else if encloses {
            MethodFrom::Enclosing
        } else {
            continue;
        };
        if !reaches(candidate) {
            continue;
        }
        found.push((candidate.method.clone(), from, guard));
    }
    found
}

/// Whether a method guard's chain reaches the request the route does: the
/// same handoff call and parameter, reading the label's method member; or
/// the same entry function and parameter.
fn reaches_same(facts: &ChainFacts, reached: &Reached) -> bool {
    match &reached.anchor {
        Anchor::Call(caller, start, end, param) => {
            let (Some(handoff), Some(method)) = (&facts.handoff, &reached.method_steps) else {
                return false;
            };
            handoff.caller == *caller
                && handoff.param == *param
                && handoff.site.span_start == *start
                && handoff.site.span_end == *end
                && read_steps(&handoff.steps) == method.as_slice()
        }
        Anchor::Entry(node, param) => {
            facts.handoff.is_none()
                && facts
                    .entry
                    .as_ref()
                    .is_some_and(|entry| entry.node == *node && entry.param == *param)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::discovery::{Labels, discover};
    use super::super::fixture_walk::{Walked, walked};
    use super::*;
    use crate::library_claims::ExportClaims;

    fn fixture(name: &str) -> std::path::PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/dispatch-routes")
            .join(name)
    }

    /// The fixture's store answer, with `edit` applied to its raw JSON.
    fn claims(edit: impl FnOnce(&mut serde_json::Value)) -> Vec<ExportClaims> {
        let mut answer: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(fixture("library-claims.json")).unwrap())
                .unwrap();
        edit(&mut answer);
        let answer = crate::library_store::parse_answer(&answer.to_string());
        assert_eq!(answer.dropped, 0, "every fixture claim parses");
        answer.exports
    }

    /// Discovery's sites on `walked` under `exports`, read as the engine
    /// hands them on.
    fn discovered(walked: &Walked, exports: &[ExportClaims]) -> Vec<DiscoverySite> {
        discover(&walked.inputs, &Labels::of(exports), &walked.repo)
            .sites
            .iter()
            .map(|site| serde_json::from_value(serde_json::to_value(site).unwrap()).unwrap())
            .collect()
    }

    fn dispatch_of(walked: &Walked, exports: &[ExportClaims]) -> serde_json::Value {
        let labels = SiteLabel::from_exports(exports);
        let dispatch = dispatch_routes(
            &walked.inputs,
            &discovered(walked, exports),
            &labels,
            &walked.repo,
        );
        serde_json::to_value(&dispatch).unwrap()
    }

    fn the_fixture() -> Option<Walked> {
        walked("dispatch-routes", ".", "web", &[], true)
    }

    fn key() -> serde_json::Value {
        serde_json::from_str(
            &std::fs::read_to_string(fixture("dispatch.expected.json")).expect("the key reads"),
        )
        .expect("the key is JSON")
    }

    /// `file:line METHOD path method_from evidence` for each route.
    fn route_lines(dispatch: &serde_json::Value) -> Vec<String> {
        dispatch["routes"]
            .as_array()
            .unwrap()
            .iter()
            .map(|route| {
                format!(
                    "{}:{} {} {} {} {}",
                    route["file"].as_str().unwrap(),
                    route["line"],
                    route["method"].as_str().unwrap_or("-"),
                    route["path"].as_str().unwrap(),
                    route["method_from"].as_str().unwrap(),
                    route["evidence"]["kind"].as_str().unwrap()
                )
            })
            .collect()
    }

    /// carrick#2474 R4a: each route a dispatch states sits on the line its
    /// literal is compared on (a table key at its key's line, a predicate's
    /// in the predicate's file), reached through the labelled `receive` call
    /// its handler is handed to or through the entry point nothing calls;
    /// every comparison that states no route is counted by its reason. Two
    /// routes in one closure sit on their own lines (carrick#2139).
    #[test]
    fn each_route_is_stated_at_its_comparison() {
        let Some(on) = the_fixture() else {
            return;
        };
        let got = dispatch_of(&on, &claims(|_| {}));
        assert_eq!(
            got,
            key(),
            "dispatch:\n{}",
            serde_json::to_string_pretty(&got).unwrap()
        );
    }

    /// The method is a method literal on the same request: an operand of
    /// the same `&&` test, or a guard whose branch holds the comparison (an
    /// `if` on the method, a `case` of a switch on it, falling through to
    /// one route per method). A predicate's comparison takes the method
    /// beside its caller's call. A method on another value, or under `||`,
    /// states none.
    #[test]
    fn a_method_comes_from_its_test_or_an_enclosing_guard() {
        let Some(on) = the_fixture() else {
            return;
        };
        let got = route_lines(&dispatch_of(&on, &claims(|_| {})));
        for want in [
            "src/server.ts:11 GET /orders conjunct label",
            "src/server.ts:13 POST /orders conjunct label",
            "src/server.ts:35 DELETE /cache enclosing label",
            "src/server.ts:42 GET /feed enclosing label",
            "src/server.ts:42 HEAD /feed enclosing label",
            "src/routes.ts:1 POST /orders/export conjunct label",
            "src/api.ts:5 GET /api/items conjunct label",
            "src/worker.ts:13 GET /home conjunct entry_point",
            "src/middleware.ts:12 GET /api/one conjunct entry_point",
            "src/middleware.ts:20 POST /api/two conjunct entry_point",
            "src/negatives.ts:20 - /mixed none label",
            "src/negatives.ts:23 - /both none label",
            "src/negatives.ts:26 - /either none label",
        ] {
            assert!(got.contains(&want.to_string()), "{want} in {got:#?}");
        }
    }

    /// A label that names no `request` states no route: every label route
    /// goes and is counted `label_without_request`; entry routes stay.
    #[test]
    fn a_label_that_names_no_request_states_no_route() {
        let Some(on) = the_fixture() else {
            return;
        };
        let without = dispatch_of(
            &on,
            &claims(|answer| {
                for package in answer["library_claims"].as_array_mut().unwrap() {
                    for export in package["exports"].as_array_mut().unwrap() {
                        for op in export["ops"].as_array_mut().unwrap() {
                            op.as_object_mut().unwrap().remove("request");
                        }
                    }
                }
            }),
        );
        let with = key();
        let entry = |dispatch: &serde_json::Value| -> Vec<String> {
            route_lines(dispatch)
                .into_iter()
                .filter(|line| line.ends_with(" entry_point"))
                .collect()
        };
        assert!(
            route_lines(&without)
                .iter()
                .all(|line| line.ends_with(" entry_point")),
            "no label route: {:#?}",
            route_lines(&without)
        );
        assert_eq!(entry(&without), entry(&with), "entry routes stay");
        assert!(without["scopes"].as_array().unwrap().is_empty());
        assert!(
            without["not_routes"]["label_without_request"].as_u64()
                > with["not_routes"]["label_without_request"].as_u64()
        );
    }

    /// Two reads of one tree state the same routes, in `(file, line, path,
    /// method)` order.
    #[test]
    fn two_reads_of_one_tree_state_the_same_routes() {
        let (Some(first), Some(second)) = (the_fixture(), the_fixture()) else {
            return;
        };
        let exports = claims(|_| {});
        let a = serde_json::to_string(&dispatch_of(&first, &exports)).unwrap();
        let b = serde_json::to_string(&dispatch_of(&second, &exports)).unwrap();
        assert_eq!(a, b);
        let labels = SiteLabel::from_exports(&exports);
        let dispatch = dispatch_routes(
            &first.inputs,
            &discovered(&first, &exports),
            &labels,
            &first.repo,
        );
        let keys: Vec<_> = dispatch
            .routes
            .iter()
            .map(|r| (r.file.clone(), r.line, r.path.clone(), r.method.clone()))
            .collect();
        let mut sorted = keys.clone();
        sorted.sort();
        sorted.dedup();
        assert_eq!(keys, sorted, "routes are sorted, one per key");
    }

    fn records_of(
        walked: &Walked,
        exports: &[ExportClaims],
        reach: HoleReach,
    ) -> serde_json::Value {
        let labels = SiteLabel::from_exports(exports);
        let records = handler_records(
            &walked.inputs,
            &discovered(walked, exports),
            &labels,
            &walked.repo,
            reach,
        );
        serde_json::to_value(&records).unwrap()
    }

    fn records_key() -> serde_json::Value {
        serde_json::from_str(
            &std::fs::read_to_string(fixture("records.expected.json")).expect("the key reads"),
        )
        .expect("the key is JSON")
    }

    /// carrick#2474 R4b: one record per request handler, holding every
    /// comparison only it reaches (a hole in it or in a function written
    /// inside it among them), and one shared record per function for a
    /// comparison several handlers reach. What the code proves is forced;
    /// the rest is asked.
    #[test]
    fn each_handler_asks_what_it_does_not_state() {
        let Some(on) = the_fixture() else {
            return;
        };
        let got = records_of(&on, &claims(|_| {}), HoleReach::Nested);
        assert_eq!(
            got,
            records_key(),
            "records:\n{}",
            serde_json::to_string_pretty(&got).unwrap()
        );
        // Every record's asked ids name its comparisons; the wire type reads
        // each record back unchanged.
        for record in got["records"].as_array().unwrap() {
            let typed: crate::site_questions::HandlerRecord =
                serde_json::from_value(record.clone()).unwrap();
            assert_eq!(&serde_json::to_value(&typed).unwrap(), record);
            assert!(
                typed
                    .asked
                    .iter()
                    .all(|id| typed.comparisons.iter().any(|c| c.id == *id))
            );
        }
    }

    /// Every route R4a states is a comparison of a record, at its line: a
    /// label route's path forced, an entry route's asked.
    #[test]
    fn every_dispatch_route_is_a_comparison_of_a_record() {
        let Some(on) = the_fixture() else {
            return;
        };
        let exports = claims(|_| {});
        let routes = dispatch_of(&on, &exports);
        let records = records_of(&on, &exports, HoleReach::Nested);
        for route in routes["routes"].as_array().unwrap() {
            let at = format!("{}:{}", route["file"].as_str().unwrap(), route["line"]);
            let path = route["path"].as_str().unwrap();
            let found = records["records"]
                .as_array()
                .unwrap()
                .iter()
                .find_map(|record| {
                    let comparison = record["comparisons"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .find(|c| c["at"] == at.as_str() && c["literal"] == path)?;
                    Some((record, comparison["id"].to_string()))
                });
            let Some((record, id)) = found else {
                panic!("{at} {path} is in no record");
            };
            let forced_path = record["forced"][&id]["path"].as_str();
            match route["evidence"]["kind"].as_str().unwrap() {
                "label" => assert_eq!(forced_path, Some(path), "{at} {path}"),
                _ => assert_eq!(forced_path, None, "{at} {path}"),
            }
        }
    }

    /// A record as the cloud keys it (`withoutPositions` in the
    /// site-questions lambda): each `at` without its line, each `code`
    /// string without its line numbers.
    fn without_positions(value: &serde_json::Value, key: Option<&str>) -> serde_json::Value {
        use serde_json::Value;
        let line = regex::Regex::new(r"^(.+?):\d+(?::\d+)?$").unwrap();
        let number = regex::Regex::new(r"(?m)^\d+:( |$)").unwrap();
        match value {
            Value::Array(items) => Value::Array(
                items
                    .iter()
                    .map(|item| without_positions(item, None))
                    .collect(),
            ),
            Value::Object(map) => Value::Object(
                map.iter()
                    .map(|(k, v)| (k.clone(), without_positions(v, Some(k))))
                    .collect(),
            ),
            Value::String(text) if matches!(key, Some("at" | "declaration")) => {
                Value::String(line.replace(text, "$1").into_owned())
            }
            Value::String(text) if key == Some("code") => {
                Value::String(number.replace_all(text, "").into_owned())
            }
            other => other.clone(),
        }
    }

    /// Lines added above a handler, and above a function its comparison is
    /// written in, move no record's key: every position is under `at`.
    #[test]
    fn a_record_moves_no_key_with_an_edit_above_it() {
        let read = |name: &str| {
            std::fs::read_to_string(fixture("repo").join(name)).expect("the fixture reads")
        };
        let edge = format!("\n\n{}", read("src/edge.ts"));
        let report = format!("// a note\n\n{}", read("src/report.ts"));
        let (Some(before), Some(after)) = (
            the_fixture(),
            walked(
                "dispatch-routes",
                ".",
                "web",
                &[
                    ("src/edge.ts", edge.as_str()),
                    ("src/report.ts", report.as_str()),
                ],
                true,
            ),
        ) else {
            return;
        };
        let exports = claims(|_| {});
        let before = records_of(&before, &exports, HoleReach::Nested);
        let after = records_of(&after, &exports, HoleReach::Nested);
        assert_ne!(before, after, "the edit moves positions");
        assert_eq!(
            without_positions(&before["records"], None),
            without_positions(&after["records"], None)
        );
    }

    /// What the answer key is written against: today's chains and
    /// discovery's sites on the fixture, and this pass's output.
    #[test]
    #[ignore = "prints the fixture's chains, sites and routes"]
    fn probe() {
        let Some(on) = the_fixture() else {
            return;
        };
        let exports = claims(|_| {});
        let chains = super::super::guards::guard_chains(&on.inputs, &on.repo);
        crate::outln!("{}", serde_json::to_string_pretty(&chains).unwrap());
        crate::outln!(
            "{}",
            serde_json::to_string_pretty(&discovered(&on, &exports)).unwrap()
        );
        crate::outln!(
            "{}",
            serde_json::to_string_pretty(&dispatch_of(&on, &exports)).unwrap()
        );
        crate::outln!(
            "{}",
            serde_json::to_string_pretty(&records_of(&on, &exports, HoleReach::Nested)).unwrap()
        );
    }
}

/// The dispatch routes on real repos (carrick#2474 R4a): offline and
/// model-free. Each repo needs a flag-on scan's rows (`<blobs>/positions`),
/// discovery's sites for it written by `discovery_census` with
/// `CARRICK_RECEIVER_TRACE=1`, the store's answer for its packages, and the
/// scan's blobs:
///
/// `CARRICK_DISPATCH_CENSUS='<label>=<repo>=<positions dir>=<sites.json>=<store answer>=<blobs dir>[;…]'
/// [CARRICK_DISPATCH_CENSUS_OUT=<file>] [CARRICK_DISPATCH_CENSUS_RECORDS_DIR=<dir>]
/// cargo test --lib dispatch_census -- --ignored --nocapture`
///
/// With a records dir, each repository's asked handler records (carrick#2474
/// R4b) are written to `<dir>/<label>.json` as the site-questions pack runner
/// reads them, and the what-if arm's added records to
/// `<dir>/<label>.what-if.json`. The out file carries other repositories'
/// paths and the records their code: both stay private.
#[cfg(test)]
mod census {
    use std::collections::{BTreeMap, HashSet};
    use std::path::Path;

    use serde::Serialize;

    use super::super::decided_sites::{DiscoveryFile, SiteLabel, relative};
    use super::{Dispatch, HoleReach, dispatch_routes, handler_records};
    use crate::config::Config;
    use crate::position_rows::PositionIndex;

    #[derive(Debug, Default, Serialize)]
    struct RepoCensus {
        path_guards: usize,
        table_keys: usize,
        starts_with: usize,
        routes: usize,
        routes_by_evidence: BTreeMap<String, usize>,
        routes_by_method_from: BTreeMap<String, usize>,
        /// Routes whose path is `/`, apart: most are separator checks.
        slash_routes: usize,
        scopes: usize,
        not_routes: BTreeMap<String, usize>,
        method_conflicts: usize,
        /// Per file, each route as `line METHOD path evidence`.
        per_file: BTreeMap<String, Vec<String>>,
        /// The blob's HTTP endpoint rows in files that hold a route.
        endpoint_rows: usize,
        /// Rows a route states: same method, the row's path ends with the
        /// route's, same line.
        same_line: Vec<String>,
        /// Same method and path, another line (the #2139 shape).
        other_line: Vec<String>,
        /// Routes no row states.
        extras: Vec<String>,
        /// Rows in those files no route states.
        unstated_rows: Vec<String>,
        ms: u128,
        /// The handler records (carrick#2474 R4b): their counts, what
        /// attaches to no handler, and per record `at kind key asked/all`.
        records: BTreeMap<String, usize>,
        records_not_attached: BTreeMap<String, usize>,
        per_record: Vec<String>,
        /// The what-if arm: holes one call below a handler attached too.
        what_if: BTreeMap<String, usize>,
        records_ms: u128,
    }

    /// One service's asked records, as the site-questions request's `sites`.
    fn sites_of(
        service: &str,
        records: &[crate::site_questions::HandlerRecord],
    ) -> Vec<serde_json::Value> {
        records
            .iter()
            .filter(|record| !record.asked.is_empty())
            .enumerate()
            .map(|(index, record)| {
                serde_json::json!({
                    "site_id": format!("{service}#{index:04}"),
                    "handler": record,
                })
            })
            .collect()
    }

    /// The blob's HTTP endpoint rows: (method, path, file, line).
    fn endpoint_rows(blobs: &Path) -> Vec<(String, String, String, u32)> {
        let mut rows = Vec::new();
        for entry in std::fs::read_dir(blobs).expect("the blobs dir reads") {
            let path = entry.expect("dir entry").path();
            if path.extension().is_none_or(|ext| ext != "json") {
                continue;
            }
            let Ok(blob) =
                serde_json::from_str::<serde_json::Value>(&std::fs::read_to_string(&path).unwrap())
            else {
                continue;
            };
            for endpoint in blob["endpoints"].as_array().into_iter().flatten() {
                let key = &endpoint["key"];
                if key["protocol"] != "http" {
                    continue;
                }
                let (Some(method), Some(at), Some(file_line)) = (
                    key["method"].as_str(),
                    key["path"].as_str(),
                    endpoint["file_path"].as_str(),
                ) else {
                    continue;
                };
                let Some((file, line)) = file_line.rsplit_once(':') else {
                    continue;
                };
                rows.push((
                    method.to_uppercase(),
                    at.to_string(),
                    file.to_string(),
                    line.parse().unwrap_or_default(),
                ));
            }
        }
        rows.sort();
        rows.dedup();
        rows
    }

    fn census_repo(
        root: &Path,
        positions: &Path,
        sites: &Path,
        claims: &Path,
        blobs: &Path,
    ) -> (RepoCensus, Vec<serde_json::Value>, Vec<serde_json::Value>) {
        let root = std::fs::canonicalize(root).expect("the repo exists");
        let config = root.join("carrick.json");
        let services = if config.exists() {
            Config::load_services(vec![config]).expect("carrick.json loads")
        } else {
            vec![Config::default()]
        };
        let discovered = serde_json::from_str::<DiscoveryFile>(
            &std::fs::read_to_string(sites).expect("the sites file reads"),
        )
        .expect("the sites file parses")
        .sites;
        let exports = crate::library_store::parse_answer(
            &std::fs::read_to_string(claims).expect("the claims read"),
        )
        .exports;
        let labels = SiteLabel::from_exports(&exports);
        let mut by_service = BTreeMap::new();
        for entry in std::fs::read_dir(positions).expect("the positions dir reads") {
            let path = entry.expect("dir entry").path();
            if path.to_string_lossy().ends_with(".jsonl.gz")
                && let Ok(rows) = crate::position_rows::read_rows_file(&path)
            {
                by_service.insert(rows.header.service.clone(), path);
            }
        }
        let mut census = RepoCensus::default();
        let mut all = Dispatch::default();
        let mut asked_sites: Vec<serde_json::Value> = Vec::new();
        let mut what_if_sites: Vec<serde_json::Value> = Vec::new();
        for service in &services {
            let label = service
                .service_name
                .clone()
                .unwrap_or_else(|| "(root)".to_string());
            let index = match by_service.get(&label) {
                Some(path) => PositionIndex::of(
                    &root,
                    crate::position_rows::read_rows_file(path).expect("rows read"),
                ),
                None => PositionIndex::missing(&root),
            };
            let inputs = crate::engine::discover_service_inputs(&root, service, Some(index));
            let files: HashSet<String> = inputs
                .files
                .keys()
                .map(|file| relative(&root, file))
                .collect();
            let mine: Vec<_> = discovered
                .iter()
                .filter(|site| files.contains(&site.file))
                .cloned()
                .collect();
            let started = std::time::Instant::now();
            let dispatch = dispatch_routes(&inputs, &mine, &labels, &root);
            census.ms += started.elapsed().as_millis();
            let again = dispatch_routes(&inputs, &mine, &labels, &root);
            assert_eq!(
                serde_json::to_string(&dispatch).unwrap(),
                serde_json::to_string(&again).unwrap(),
                "two reads of {label} state the same routes"
            );
            let started = std::time::Instant::now();
            let records = handler_records(&inputs, &mine, &labels, &root, HoleReach::Nested);
            census.records_ms += started.elapsed().as_millis();
            let again = handler_records(&inputs, &mine, &labels, &root, HoleReach::Nested);
            assert_eq!(
                serde_json::to_string(&records).unwrap(),
                serde_json::to_string(&again).unwrap(),
                "two reads of {label} state the same records"
            );
            let what_if = handler_records(&inputs, &mine, &labels, &root, HoleReach::OneCallBelow);
            for (what, n) in &records.counts {
                *census.records.entry(what.clone()).or_default() += n;
            }
            for (what, n) in &records.not_attached {
                *census.records_not_attached.entry(what.clone()).or_default() += n;
            }
            for (what, n) in &what_if.counts {
                *census.what_if.entry(what.clone()).or_default() += n;
            }
            for record in &records.records {
                census.per_record.push(format!(
                    "{} {:?} {} asked {}/{}",
                    record.subject.at,
                    record.subject.kind,
                    record.subject.key,
                    record.asked.len(),
                    record.comparisons.len()
                ));
            }
            asked_sites.extend(sites_of(&label, &records.records));
            let base: Vec<serde_json::Value> = records
                .records
                .iter()
                .map(|record| serde_json::to_value(record).unwrap())
                .collect();
            let added: Vec<_> = what_if
                .records
                .into_iter()
                .filter(|record| !base.contains(&serde_json::to_value(record).unwrap()))
                .collect();
            what_if_sites.extend(sites_of(&format!("{label}+w"), &added));
            all.routes.extend(dispatch.routes);
            all.scopes.extend(dispatch.scopes);
            for (reason, n) in dispatch.not_routes {
                *all.not_routes.entry(reason).or_default() += n;
            }
            for (what, n) in dispatch.counts {
                *all.counts.entry(what).or_default() += n;
            }
        }
        let read = |what: &str| all.counts.get(what).copied().unwrap_or_default();
        census.path_guards = read("path_guards");
        census.table_keys = read("table_keys");
        census.starts_with = read("starts_with");
        census.method_conflicts = read("method_conflict");
        census.routes = all.routes.len();
        census.scopes = all.scopes.len();
        census.not_routes = all.not_routes;
        let word = |value: serde_json::Value| value.as_str().unwrap_or_default().to_string();
        for route in &all.routes {
            *census
                .routes_by_evidence
                .entry(route.evidence.kind().to_string())
                .or_default() += 1;
            *census
                .routes_by_method_from
                .entry(word(serde_json::to_value(route.method_from).unwrap()))
                .or_default() += 1;
            if route.path == "/" {
                census.slash_routes += 1;
            }
            census
                .per_file
                .entry(route.file.clone())
                .or_default()
                .push(format!(
                    "{} {} {} {}",
                    route.line,
                    route.method.as_deref().unwrap_or("-"),
                    route.path,
                    route.evidence.kind()
                ));
        }
        let rows = endpoint_rows(blobs);
        let route_files: HashSet<&str> = all.routes.iter().map(|r| r.file.as_str()).collect();
        let in_files: Vec<_> = rows
            .iter()
            .filter(|(_, _, file, _)| route_files.contains(file.as_str()))
            .collect();
        census.endpoint_rows = in_files.len();
        let same = |method: &Option<String>, row_method: &str| {
            method.as_deref().is_none_or(|method| method == row_method)
        };
        let mut stated: HashSet<usize> = HashSet::new();
        for route in &all.routes {
            let at = format!(
                "{}:{} {} {}",
                route.file,
                route.line,
                route.method.as_deref().unwrap_or("-"),
                route.path
            );
            let matching: Vec<(usize, bool)> = in_files
                .iter()
                .enumerate()
                .filter(|(_, (method, path, file, _))| {
                    *file == route.file
                        && same(&route.method, method)
                        && path.ends_with(&route.path)
                })
                .map(|(index, (_, _, _, line))| (index, *line == route.line))
                .collect();
            if let Some((index, _)) = matching.iter().find(|(_, on_line)| *on_line) {
                stated.insert(*index);
                census.same_line.push(at);
            } else if let Some((index, _)) = matching.first() {
                stated.insert(*index);
                census.other_line.push(at);
            } else {
                census.extras.push(at);
            }
        }
        for (index, (method, path, file, line)) in in_files.iter().enumerate() {
            if !stated.contains(&index) {
                census
                    .unstated_rows
                    .push(format!("{file}:{line} {method} {path}"));
            }
        }
        (census, asked_sites, what_if_sites)
    }

    /// `sites` as a records file the site-questions pack runner reads.
    fn write_records(path: &Path, label: &str, sites: Vec<serde_json::Value>) {
        let file = serde_json::json!({
            "repo": label,
            "service": null,
            "question_set": null,
            "sites": sites,
        });
        std::fs::write(path, serde_json::to_string_pretty(&file).unwrap())
            .expect("the records write");
    }

    #[test]
    #[ignore = "a census over real repos: set CARRICK_DISPATCH_CENSUS"]
    fn dispatch_census() {
        let Ok(spec) = std::env::var("CARRICK_DISPATCH_CENSUS") else {
            crate::errln!("CARRICK_DISPATCH_CENSUS is not set");
            return;
        };
        let mut out: BTreeMap<String, RepoCensus> = BTreeMap::new();
        for repo in spec.split(';').filter(|part| !part.is_empty()) {
            let parts: Vec<&str> = repo.split('=').collect();
            let [label, root, positions, sites, claims, blobs] = parts.as_slice() else {
                panic!("expected <label>=<repo>=<positions>=<sites>=<answer>=<blobs>, got {repo}");
            };
            let (census, asked, what_if) = census_repo(
                Path::new(root),
                Path::new(positions),
                Path::new(sites),
                Path::new(claims),
                Path::new(blobs),
            );
            // The records are the repository's code: private files only.
            if let Ok(dir) = std::env::var("CARRICK_DISPATCH_CENSUS_RECORDS_DIR") {
                let dir = Path::new(&dir);
                std::fs::create_dir_all(dir).expect("the records dir");
                write_records(&dir.join(format!("{label}.json")), label, asked);
                write_records(&dir.join(format!("{label}.what-if.json")), label, what_if);
            }
            out.insert(label.to_string(), census);
        }
        let text = serde_json::to_string_pretty(&out).expect("the census serialises");
        match std::env::var("CARRICK_DISPATCH_CENSUS_OUT") {
            Ok(path) => std::fs::write(path, &text).expect("the census writes"),
            Err(_) => crate::outln!("{text}"),
        }
    }
}
