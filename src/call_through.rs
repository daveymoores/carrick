//! A model row at a call through the repository's own code takes the request
//! that call reaches (carrick#1146).
//!
//! A request is one row at the line that writes it. A call that reaches the
//! request through functions the repository declares writes neither a method
//! nor a path, and the model, asked about such a line, makes them up from the
//! names it can see:
//!
//! ```ignore
//! // package `client`, one service
//! export const getRun = (options) =>
//!   transport.get({ url: "/api/v1/runs/{id}", ...options });   // the request row
//!
//! // package `cli`, another service
//! const client = createClient();
//! const run = await client.getRun(id);   // the model's row: `GET /getRun`
//! ```
//!
//! The call graph says which definition each call reaches, and every service's
//! deterministic rows say which requests each definition writes. Where the two
//! together prove ONE operation behind a model row's call, the row takes that
//! operation. Where they prove none, or several, the row is left as the model
//! stated it and counted: it can be the only record of the call.
//!
//! The reached request is usually in another service, and one service's
//! analysis never holds another's rows (each runs on a thread of its own). So
//! the work is in two halves:
//!
//! - [`read_sites`], per service, while its files and its resolved call sites
//!   are in hand: for each model call row, the definitions its call expression
//!   goes through. Never serialised.
//! - [`settle`], once per scan, when every service's rows and definitions are
//!   in hand: the walk and the change.
//!
//! Nothing here names a library, a framework, a file or a symbol.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};

use swc_common::{
    BytePos, SourceMap, Span,
    errors::{ColorConfig, Handler},
    sync::Lrc,
};
use swc_ecma_ast::{CallExpr, Callee, Expr, ExprOrSpread, OptCall, Prop, PropOrSpread, Str, Tpl};
use swc_ecma_visit::{Visit, VisitWith};
use tracing::debug;

use crate::agents::file_analyzer_agent::{DataCallResult, FileAnalysisResult, ResolutionSource};
use crate::agents::file_orchestrator::FileOrchestrator;
use crate::cloud_storage::{CloudRepoData, ManifestRole};
use crate::graphql_document_sites::unwrap_expression;
use crate::mount_graph::{ConsumerRole, DataFetchingCall};
use crate::operation::OperationKey;
use crate::parser::parse_file;
use crate::request_summary::RequestSummaryInputs;
use crate::swc_scanner::SWC_SPAN_BASE;
use crate::type_manifest::normalize_manifest_method;
use crate::wrapper_request_shape::{RequestShapeSignal, call_request_verb};

/// How many call edges the walk follows from the definition a site reaches.
const MAX_EDGE_DEPTH: usize = 4;

/// A definition as every service names it: its repository-relative file and
/// the line it opens on. A definition key is one service's own.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct DefinitionSite {
    pub file: String,
    pub line: u32,
}

/// One model call row and the definitions its call expression goes through.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SiteRow {
    /// Repository-relative file and line, as the row's `file_location` packs
    /// them.
    pub file: String,
    pub line: u32,
    /// The method the row is indexed under.
    pub method: String,
    /// The target the model stated.
    pub target: String,
    /// The definitions reached, in the order they are read. The first step
    /// that reaches a request decides:
    ///
    /// 1. the call's own callee;
    /// 2. the calls whose result is spread into an object-literal argument;
    /// 3. the calls in the body of the one function-valued argument.
    pub steps: Vec<Vec<DefinitionSite>>,
}

/// One service's model call rows at sites that write no request of their own.
#[derive(Debug, Default, Clone)]
pub struct CallThroughSites {
    pub rows: Vec<SiteRow>,
}

/// Read, for each model call row of one service, the definitions its call
/// expression goes through.
///
/// A row is read only when its call expression writes no request of its own:
/// no string or template inside it is shaped like a route or a URL, no call
/// inside it states a verb ([`call_request_verb`]), and no deterministic row
/// sits inside it. A row whose call cannot be placed is not read either.
///
/// `file_results` is the service's joined rows, keyed absolute or relative to
/// `repo_path`.
pub fn read_sites(
    file_results: &HashMap<String, FileAnalysisResult>,
    inputs: &RequestSummaryInputs,
    repo_path: &str,
) -> CallThroughSites {
    let mut keys: Vec<&String> = file_results
        .iter()
        .filter(|(_, result)| result.data_calls.iter().any(is_model_row))
        .map(|(key, _)| key)
        .collect();
    keys.sort();
    if keys.is_empty() {
        return CallThroughSites::default();
    }

    let root = repo_path.trim_end_matches('/');
    // The discovery tables are keyed by the file as it was walked, which is
    // not always the spelling a row's key joins to.
    let mut walked: Option<HashMap<PathBuf, &PathBuf>> = None;
    let mut rows = Vec::new();
    for key in keys {
        let absolute = if Path::new(key).is_absolute() {
            PathBuf::from(key)
        } else {
            Path::new(root).join(key)
        };
        let discovered = match inputs.files.get_key_value(&absolute) {
            Some((path, _)) => Some(path),
            None => {
                let table = walked.get_or_insert_with(|| {
                    inputs
                        .files
                        .keys()
                        .filter_map(|path| Some((path.canonicalize().ok()?, path)))
                        .collect()
                });
                absolute
                    .canonicalize()
                    .ok()
                    .and_then(|canonical| table.get(&canonical).copied())
            }
        };
        let Some(discovered) = discovered else {
            continue;
        };
        let Some(ir) = inputs.files.get(discovered) else {
            continue;
        };
        // Span start and end, in the units a row's span carries -> the
        // definition the call reaches.
        let resolved: HashMap<(u32, u32), DefinitionSite> = inputs
            .sites
            .sites_in(discovered)
            .map(|(lo, hi, file, line)| {
                (
                    (ir.swc_start(lo), ir.swc_start(hi)),
                    DefinitionSite {
                        file: crate::engine::repo_relative(&file.to_string_lossy(), root),
                        line,
                    },
                )
            })
            .collect();
        let relative = crate::engine::repo_relative(key, root);
        rows.extend(read_file_sites(
            &absolute,
            &relative,
            &file_results[key],
            &resolved,
        ));
    }
    CallThroughSites { rows }
}

/// A row this pass may settle: the model's own reading, with nothing linking
/// it to a request already.
fn is_model_row(call: &DataCallResult) -> bool {
    call.resolution_source == Some(ResolutionSource::Model) && call.reaches_request.is_none()
}

fn read_file_sites(
    absolute: &Path,
    relative: &str,
    result: &FileAnalysisResult,
    resolved: &HashMap<(u32, u32), DefinitionSite>,
) -> Vec<SiteRow> {
    let cm: Lrc<SourceMap> = Default::default();
    let handler = Handler::with_tty_emitter(ColorConfig::Never, false, false, Some(cm.clone()));
    let Some(module) = parse_file(absolute, &cm, &handler) else {
        return Vec::new();
    };
    let positions = Positions { cm: &cm };

    let mut every = EveryCall {
        positions: &positions,
        calls: Vec::new(),
    };
    module.visit_with(&mut every);

    // The call each row was recorded at.
    let placed: Vec<(&DataCallResult, CallExtent)> = result
        .data_calls
        .iter()
        .filter(|call| is_model_row(call))
        .filter_map(|call| Some((call, place(&every.calls, call)?)))
        .collect();
    if placed.is_empty() {
        return Vec::new();
    }

    let mut reader = SiteReader {
        positions: &positions,
        wanted: placed.iter().map(|(_, extent)| extent.span()).collect(),
        read: HashMap::new(),
    };
    module.visit_with(&mut reader);

    let mut rows = Vec::new();
    for (call, extent) in placed {
        let Some(site) = reader.read.get(&extent.span()) else {
            continue;
        };
        if site.writes_a_request || holds_a_deterministic_row(result, &extent) {
            continue;
        }
        let Some(method) = FileOrchestrator::normalize_consumer_method(call.method.as_deref())
        else {
            continue;
        };
        let Ok(line) = u32::try_from(call.line_number) else {
            continue;
        };
        let reached = |spans: &[(u32, u32)]| -> Vec<DefinitionSite> {
            let mut definitions: Vec<DefinitionSite> = spans
                .iter()
                .filter_map(|span| resolved.get(span).cloned())
                .collect();
            definitions.sort();
            definitions.dedup();
            definitions
        };
        rows.push(SiteRow {
            file: relative.to_string(),
            line,
            method,
            target: call.target.clone(),
            steps: vec![
                reached(&[extent.span()]),
                reached(&site.spread_calls),
                reached(&site.callback_calls),
            ],
        });
    }
    rows
}

/// Whether a deterministic row of the file sits inside the call expression:
/// the site then holds a request the source states, and the model's row beside
/// it is not this pass's to read.
fn holds_a_deterministic_row(result: &FileAnalysisResult, extent: &CallExtent) -> bool {
    result
        .data_calls
        .iter()
        .filter(|call| {
            call.resolution_source
                .is_some_and(|source| source != ResolutionSource::Model)
        })
        .any(|call| match call.call_expression_span_start {
            Some(start) => extent.lo <= start && start < extent.hi,
            None => u32::try_from(call.line_number)
                .is_ok_and(|line| extent.line <= line && line <= extent.last_line),
        })
}

/// Where one call expression sits: its span in the units a row's span
/// carries, and the lines it opens and closes on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct CallExtent {
    lo: u32,
    hi: u32,
    line: u32,
    last_line: u32,
}

impl CallExtent {
    fn span(&self) -> (u32, u32) {
        (self.lo, self.hi)
    }

    fn contains(&self, other: &CallExtent) -> bool {
        self.lo <= other.lo && other.hi <= self.hi && self.span() != other.span()
    }
}

struct Positions<'a> {
    cm: &'a Lrc<SourceMap>,
}

impl Positions<'_> {
    fn offset(&self, pos: BytePos) -> u32 {
        self.cm.lookup_byte_offset(pos).pos.0 + SWC_SPAN_BASE
    }

    fn extent(&self, span: Span) -> CallExtent {
        CallExtent {
            lo: self.offset(span.lo),
            hi: self.offset(span.hi),
            line: self.cm.lookup_char_pos(span.lo).line as u32,
            last_line: self.cm.lookup_char_pos(span.hi).line as u32,
        }
    }
}

/// The call a row was recorded at: the one at its span, or, for a row with no
/// span, the one call that opens on its line and sits inside no other call
/// opening there. A line that opens two such calls states nothing about which
/// the row is, and answers `None`.
fn place(calls: &[CallExtent], row: &DataCallResult) -> Option<CallExtent> {
    if let Some(start) = row.call_expression_span_start {
        let mut at_start = calls.iter().filter(|call| call.lo == start);
        return match row.call_expression_span_end {
            Some(end) => at_start.find(|call| call.hi == end).copied(),
            None => {
                let first = at_start.next()?;
                at_start.next().is_none().then_some(*first)
            }
        };
    }
    let line = u32::try_from(row.line_number).ok()?;
    let on_line: Vec<&CallExtent> = calls.iter().filter(|call| call.line == line).collect();
    let mut outermost = on_line
        .iter()
        .filter(|call| !on_line.iter().any(|other| other.contains(call)));
    let first = outermost.next()?;
    outermost.next().is_none().then_some(**first)
}

/// Every call expression of a file.
struct EveryCall<'a> {
    positions: &'a Positions<'a>,
    calls: Vec<CallExtent>,
}

impl Visit for EveryCall<'_> {
    fn visit_call_expr(&mut self, node: &CallExpr) {
        self.calls.push(self.positions.extent(node.span));
        node.visit_children_with(self);
    }

    fn visit_opt_call(&mut self, node: &OptCall) {
        self.calls.push(self.positions.extent(node.span));
        node.visit_children_with(self);
    }
}

/// What one site's call expression holds.
#[derive(Debug, Default)]
struct SiteReading {
    /// The expression writes a request of its own.
    writes_a_request: bool,
    /// The calls whose result is spread into an object-literal argument:
    /// `useQuery({ ...runOptions(input) })`.
    spread_calls: Vec<(u32, u32)>,
    /// The calls in the body of the function-valued argument, when the
    /// expression holds exactly one, passed directly or as a property of an
    /// object-literal argument. Empty when it holds two or more: nothing says
    /// which of them makes the request.
    callback_calls: Vec<(u32, u32)>,
}

struct SiteReader<'a> {
    positions: &'a Positions<'a>,
    wanted: HashSet<(u32, u32)>,
    read: HashMap<(u32, u32), SiteReading>,
}

impl SiteReader<'_> {
    fn read_site<N: VisitWith<RequestWriter>>(
        &mut self,
        span: Span,
        node: &N,
        args: &[ExprOrSpread],
    ) {
        let key = self.positions.extent(span).span();
        if !self.wanted.contains(&key) {
            return;
        }
        let mut writer = RequestWriter::default();
        node.visit_with(&mut writer);

        let mut spread_calls = Vec::new();
        // The calls in the body of each function-valued argument.
        let mut callbacks: Vec<Vec<(u32, u32)>> = Vec::new();
        for arg in args.iter().filter(|arg| arg.spread.is_none()) {
            match unwrap_expression(&arg.expr) {
                Expr::Object(object) => {
                    for prop in &object.props {
                        match prop {
                            PropOrSpread::Spread(spread) => {
                                if let Some(span) = call_span(&spread.expr) {
                                    spread_calls.push(self.positions.extent(span).span());
                                }
                            }
                            PropOrSpread::Prop(prop) => match &**prop {
                                Prop::KeyValue(pair) => {
                                    callbacks.extend(self.calls_in_function(&pair.value));
                                }
                                // `queryFn() { … }`
                                Prop::Method(method) => {
                                    callbacks.push(self.calls_in(&method.function));
                                }
                                _ => {}
                            },
                        }
                    }
                }
                value => callbacks.extend(self.calls_in_function(value)),
            }
        }
        let callback_calls = match callbacks.len() {
            1 => callbacks.remove(0),
            _ => Vec::new(),
        };
        self.read.insert(
            key,
            SiteReading {
                writes_a_request: writer.found,
                spread_calls,
                callback_calls,
            },
        );
    }

    /// The calls in the body of `expr`, when it is a function.
    fn calls_in_function(&self, expr: &Expr) -> Option<Vec<(u32, u32)>> {
        match unwrap_expression(expr) {
            Expr::Arrow(arrow) => Some(self.calls_in(arrow)),
            Expr::Fn(function) => Some(self.calls_in(&function.function)),
            _ => None,
        }
    }

    fn calls_in<'n, N>(&'n self, node: &N) -> Vec<(u32, u32)>
    where
        N: VisitWith<EveryCall<'n>>,
    {
        let mut inner = EveryCall {
            positions: self.positions,
            calls: Vec::new(),
        };
        node.visit_with(&mut inner);
        inner.calls.iter().map(CallExtent::span).collect()
    }
}

impl Visit for SiteReader<'_> {
    fn visit_call_expr(&mut self, node: &CallExpr) {
        self.read_site(node.span, node, &node.args);
        node.visit_children_with(self);
    }

    fn visit_opt_call(&mut self, node: &OptCall) {
        self.read_site(node.span, node, &node.args);
        node.visit_children_with(self);
    }
}

/// The span of the call an expression is, through `await` and the wrappers
/// that pass a value along.
fn call_span(expr: &Expr) -> Option<Span> {
    match unwrap_expression(expr) {
        Expr::Call(call) => Some(call.span),
        Expr::Await(awaited) => call_span(&awaited.arg),
        _ => None,
    }
}

/// Whether an expression writes a request of its own: a string or a template
/// shaped like a route or a URL, or a call that states a verb.
#[derive(Default)]
struct RequestWriter {
    found: bool,
}

impl Visit for RequestWriter {
    fn visit_str(&mut self, node: &Str) {
        self.found |= writes_a_route(&node.value.to_string_lossy());
    }

    fn visit_tpl(&mut self, node: &Tpl) {
        let text = node
            .quasis
            .iter()
            .map(|quasi| quasi.raw.to_string())
            .collect::<Vec<_>>()
            .join(INTERPOLATION);
        self.found |= writes_a_route(&text);
        node.visit_children_with(self);
    }

    fn visit_call_expr(&mut self, node: &CallExpr) {
        if let Callee::Expr(callee) = &node.callee {
            let property = match unwrap_expression(callee) {
                Expr::Member(member) => member.prop.as_ident().map(|ident| ident.sym.to_string()),
                _ => None,
            };
            if matches!(
                call_request_verb(node, property.as_deref()),
                RequestShapeSignal::Known(_)
            ) {
                self.found = true;
            }
        }
        node.visit_children_with(self);
    }
}

/// What an interpolation reads as in a template's text here.
const INTERPOLATION: &str = "${}";

/// A path with something behind the slash, a URL, or an interpolated base
/// with a path behind it. A bare `/` is a separator, not a route.
fn writes_a_route(text: &str) -> bool {
    let text = text.trim();
    let path = |text: &str| text.starts_with('/') && text.len() > 1 && !text.starts_with("//");
    path(text)
        || text.starts_with("http://")
        || text.starts_with("https://")
        || text.strip_prefix(INTERPOLATION).is_some_and(path)
}

/// One service's blob and site facts, as [`settle`] reads and changes them.
pub struct ServiceRows<'a> {
    pub data: &'a mut CloudRepoData,
    pub sites: &'a CallThroughSites,
    /// Analysed in this run, and so written. A service whose stored
    /// generation stands in for it is read for its requests and definitions
    /// and never changed: that generation is what the index serves.
    pub writable: bool,
}

/// What [`settle`] decided, so a scan can state it.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Settled {
    /// Rows whose operation the reached request confirms: kept as the model
    /// stated them, and marked as the call through.
    pub relabelled: usize,
    /// Rows that took the reached request's operation.
    pub corrected: usize,
    /// Rows left alone because their call reaches no request row.
    pub reaches_none: usize,
    /// Rows left alone because their call reaches more than one operation.
    pub reaches_several: usize,
}

/// Settle every model call row whose call reaches exactly one request
/// operation, over every service of the scan.
///
/// | Reached | The row |
/// |---|---|
/// | One operation, the row's own | stays the model's and keeps its type anchors; gains `role: wrapper_call` and `reaches_request` |
/// | One operation, another | takes the reached row's method, target, base and dispatch value; its source becomes `request_summary`; its consumer type entries go |
/// | None, or several | left as it is, and counted |
pub fn settle(services: &mut [ServiceRows<'_>]) -> Settled {
    let graph = ReachGraph::of(services.iter().map(|service| &*service.data));
    let mut settled = Settled::default();
    for service in services.iter_mut().filter(|service| service.writable) {
        let mut corrected = 0;
        for row in &service.sites.rows {
            match graph.decide(row) {
                Reach::None => settled.reaches_none += 1,
                Reach::Several => settled.reaches_several += 1,
                Reach::One(reached) => match apply(service.data, row, reached) {
                    Some(Change::Relabelled) => settled.relabelled += 1,
                    Some(Change::Corrected) => corrected += 1,
                    None => {}
                },
            }
        }
        settled.corrected += corrected;
        if corrected > 0
            && let Some(mut boundary) = service.data.boundary.take()
        {
            boundary.fold_settled_calls(service.data, corrected);
            service.data.boundary = Some(boundary);
        }
    }
    debug!(
        "Call-through rows settled: {} confirmed, {} corrected; left alone: {} reach no request row, {} reach several operations (carrick#1146)",
        settled.relabelled, settled.corrected, settled.reaches_none, settled.reaches_several
    );
    settled
}

/// A deterministic request row, as much of it as a corrected row takes.
#[derive(Debug, Clone)]
struct Reached {
    method: String,
    target_url: String,
    canonical_path: String,
    host: Option<String>,
    base: Option<crate::call_base::CallBaseResolution>,
    dispatch: Option<crate::dispatch::Dispatch>,
    /// `"<file>:<line>"`, the row's own `file_location`.
    site: String,
}

enum Reach<'g> {
    None,
    Several,
    One(&'g Reached),
}

/// Every service's definitions and the requests each one writes.
#[derive(Default)]
struct ReachGraph {
    /// Definition -> the definitions it calls, and the ones the definitions
    /// written inside it call.
    calls: HashMap<DefinitionSite, Vec<DefinitionSite>>,
    /// Definition -> the request rows written inside its lines.
    requests: HashMap<DefinitionSite, Vec<Reached>>,
}

impl ReachGraph {
    fn of<'d>(services: impl Iterator<Item = &'d CloudRepoData> + Clone) -> Self {
        // A definition's body holds the definitions written inside it: a
        // callback handed to a call is indexed as a definition of its own,
        // and what it calls and requests is what the function around it does
        // (`return this.send(() => getRun(options))`). So a definition takes
        // the calls of every definition inside its lines, and the request
        // rows inside its lines whichever definition is innermost.
        let mut graph = Self::default();
        // File -> each definition's first and last line, and what it calls.
        let mut by_file: HashMap<String, Vec<(u32, u32, Vec<DefinitionSite>)>> = HashMap::new();
        for data in services.clone() {
            // The file-level owner of module-scope calls is no function: no
            // call reaches it, and its lines are the whole file.
            let functions = data
                .function_definitions
                .values()
                .filter(|definition| definition.name != crate::visitor::MODULE_SCOPE_KEY);
            for definition in functions {
                by_file
                    .entry(definition.file_path.to_string_lossy().into_owned())
                    .or_default()
                    .push((
                        definition.line_number,
                        definition.end_line.max(definition.line_number),
                        definition
                            .calls
                            .iter()
                            .map(|call| DefinitionSite {
                                file: call.file_path.clone(),
                                line: call.line_number,
                            })
                            .collect(),
                    ));
            }
        }
        for (file, definitions) in &by_file {
            for (first, last, _) in definitions {
                let callees = graph
                    .calls
                    .entry(DefinitionSite {
                        file: file.clone(),
                        line: *first,
                    })
                    .or_default();
                for (inner_first, inner_last, inner_calls) in definitions {
                    if first <= inner_first && inner_last <= last {
                        callees.extend(inner_calls.iter().cloned());
                    }
                }
                callees.sort();
                callees.dedup();
            }
        }
        for data in services {
            let Some(mount_graph) = data.mount_graph.as_ref() else {
                continue;
            };
            for call in mount_graph
                .data_calls
                .iter()
                .filter(|call| is_request_row(call))
            {
                let Some((file, line)) = site_of(call) else {
                    continue;
                };
                let holding = by_file
                    .get(file)
                    .into_iter()
                    .flatten()
                    .filter(|(first, last, _)| *first <= line && line <= *last);
                for (first, _, _) in holding {
                    graph
                        .requests
                        .entry(DefinitionSite {
                            file: file.to_string(),
                            line: *first,
                        })
                        .or_default()
                        .push(Reached {
                            method: call.method.clone(),
                            target_url: call.target_url.clone(),
                            canonical_path: call.canonical_path.clone(),
                            host: call.host.clone(),
                            base: call.base.clone(),
                            dispatch: call.dispatch.clone(),
                            site: call.file_location.clone(),
                        });
                }
            }
        }
        graph
    }

    /// The request the first step of `row` that reaches any decides.
    fn decide(&self, row: &SiteRow) -> Reach<'_> {
        for step in &row.steps {
            // By site, so a request two paths reach is one request.
            let mut reached: BTreeMap<&str, &Reached> = BTreeMap::new();
            for definition in step {
                self.walk(definition, &mut reached);
            }
            let Some(first) = reached.values().next() else {
                continue;
            };
            let operations: BTreeSet<(String, &str, String)> = reached
                .values()
                .map(|request| {
                    (
                        request.method.to_uppercase(),
                        request.canonical_path.as_str(),
                        crate::dispatch::Dispatch::key_suffix(request.dispatch.as_ref()),
                    )
                })
                .collect();
            return match operations.len() {
                1 => Reach::One(first),
                _ => Reach::Several,
            };
        }
        Reach::None
    }

    /// Collect the requests written in `start` and in the definitions it
    /// reaches within [`MAX_EDGE_DEPTH`] call edges, each definition once.
    fn walk<'g>(&'g self, start: &DefinitionSite, reached: &mut BTreeMap<&'g str, &'g Reached>) {
        let mut seen: HashSet<&DefinitionSite> = HashSet::new();
        let mut queue: VecDeque<(&DefinitionSite, usize)> = VecDeque::new();
        let Some((start, _)) = self.calls.get_key_value(start) else {
            return;
        };
        seen.insert(start);
        queue.push_back((start, 0));
        while let Some((definition, depth)) = queue.pop_front() {
            for request in self.requests.get(definition).into_iter().flatten() {
                reached.insert(request.site.as_str(), request);
            }
            if depth == MAX_EDGE_DEPTH {
                continue;
            }
            for callee in self.calls.get(definition).into_iter().flatten() {
                // A callee no service indexes is a definition outside the
                // scan: nothing is known of what it writes.
                let Some((callee, _)) = self.calls.get_key_value(callee) else {
                    continue;
                };
                if seen.insert(callee) {
                    queue.push_back((callee, depth + 1));
                }
            }
        }
    }
}

/// A request the source states: a fact, and not itself a call through.
fn is_request_row(call: &DataFetchingCall) -> bool {
    call.resolution_source
        .is_some_and(|source| source != ResolutionSource::Model)
        && call.role != Some(ConsumerRole::WrapperCall)
}

/// The file and line a row's `file_location` packs.
fn site_of(call: &DataFetchingCall) -> Option<(&str, u32)> {
    let (file, line) = call.file_location.rsplit_once(':')?;
    Some((file, call.line.or_else(|| line.parse().ok())?))
}

enum Change {
    Relabelled,
    Corrected,
}

/// Change the one row of `data` that `row` names to what `reached` states.
/// `None` when the blob holds no such row: the graph build keeps fewer rows
/// than the analysis stated.
fn apply(data: &mut CloudRepoData, row: &SiteRow, reached: &Reached) -> Option<Change> {
    let location = format!("{}:{}", row.file, row.line);
    let graph = data.mount_graph.as_mut()?;
    let index = graph.data_calls.iter().position(|call| {
        call.file_location == location
            && call.method == row.method
            && call.target_url == row.target
            && call.resolution_source == Some(ResolutionSource::Model)
            && call.reaches_request.is_none()
    })?;

    let call = &graph.data_calls[index];
    let own = call.method.eq_ignore_ascii_case(&reached.method)
        && call.dispatch == reached.dispatch
        && carrick_match::paths_match(&reached.canonical_path, &call.canonical_path);
    if own {
        let call = &mut graph.data_calls[index];
        call.role = Some(ConsumerRole::WrapperCall);
        call.reaches_request = Some(reached.site.clone());
        debug!(
            "  - {location}: the model's {} {} is the call through to {}",
            call.method, call.canonical_path, reached.site
        );
        return Some(Change::Relabelled);
    }

    let before = OperationKey::http(&call.method, call.canonical_path.clone());
    let before_dispatch = call.dispatch.clone();
    debug!(
        "  - {location}: the model's {} {} becomes {} {}, the request written at {}",
        call.method, call.target_url, reached.method, reached.target_url, reached.site
    );
    // The site may already hold a row for the reached operation, stated
    // twice by the model. One row is kept.
    let twin = graph.data_calls.iter().enumerate().any(|(other, call)| {
        other != index
            && call.file_location == location
            && call.method.eq_ignore_ascii_case(&reached.method)
            && call.canonical_path == reached.canonical_path
            && call.dispatch == reached.dispatch
    });
    let own_route = graph
        .find_matching_endpoints(&reached.canonical_path, &reached.method)
        .iter()
        .any(|endpoint| {
            endpoint.evidence == carrick_match::MatchEvidence::RouteDefinition
                && carrick_match::match_agreement(&endpoint.full_path, &reached.canonical_path)
                    .unwrap_or(0)
                    > 0
        });
    if twin {
        graph.data_calls.remove(index);
    } else {
        let call = &mut graph.data_calls[index];
        call.method = reached.method.clone();
        call.target_url = reached.target_url.clone();
        call.canonical_path = reached.canonical_path.clone();
        call.host = reached.host.clone();
        call.base = reached.base.clone();
        call.dispatch = reached.dispatch.clone();
        call.resolution_source = Some(ResolutionSource::RequestSummary);
        call.role = Some(ConsumerRole::WrapperCall);
        call.reaches_request = Some(reached.site.clone());
        call.own_route = own_route;
    }

    // The projection of the same row.
    let projected = data.calls.iter().position(|operation| {
        operation.key == before
            && operation.dispatch == before_dispatch
            && operation.resolution_source == Some(ResolutionSource::Model)
            && operation.file_path == Path::new(&location)
    });
    if let Some(projected) = projected {
        if twin {
            data.calls.remove(projected);
        } else {
            let operation = &mut data.calls[projected];
            operation.key = OperationKey::http(&reached.method, reached.canonical_path.clone());
            operation.dispatch = reached.dispatch.clone();
            operation.resolution_source = Some(ResolutionSource::RequestSummary);
            operation.request_type = None;
            operation.response_type = None;
        }
    }

    // The types the model anchored at the site were anchored for a request
    // the site does not make. No type is asked for at a call through, as at
    // a row the request summaries restate at a caller.
    if let (Some(manifest), Some((method, path))) = (data.type_manifest.as_mut(), before.as_http())
    {
        let method = normalize_manifest_method(method);
        manifest.retain(|entry| {
            let at_the_row = entry.role == ManifestRole::Consumer
                && entry.file_path == row.file
                && entry.line_number == row.line
                && entry
                    .key
                    .as_http()
                    .is_some_and(|(entry_method, entry_path)| {
                        entry_method == method && entry_path == path
                    });
            !at_the_row
        });
    }
    Some(Change::Corrected)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SOURCE: &str = r#"export async function load(id: string, target: string) {
  const run = await client.getRun(id);
  const page = useQuery({ ...runOptions({ id }) });
  const list = useQuery({ queryKey: ["runs"], queryFn: () => listRuns() });
  await retry(() => vendor.send(id), { onRetry: () => audit(id) });
  const orders = await request("/orders");
  const made = await timed(http.post(target, id));
  const saved = await save(id, await loadToken());
  const one = first(id); const two = second(id);
  const built = await request(`${target}/orders/${id}`);
  const split = await tokens(id.split("/"));
}
"#;

    /// The span of the call written as `needle`, in the units a row carries.
    fn span(needle: &str) -> (u32, u32) {
        let offset = SOURCE.find(needle).expect("needle is in the source") as u32;
        (
            offset + SWC_SPAN_BASE,
            offset + SWC_SPAN_BASE + needle.len() as u32,
        )
    }

    fn definition(file: &str, line: u32) -> DefinitionSite {
        DefinitionSite {
            file: file.to_string(),
            line,
        }
    }

    /// A model row on `line` that no candidate joined: it has no span.
    fn model_row(line: i32) -> DataCallResult {
        DataCallResult {
            candidate_id: format!("model:{line}"),
            line_number: line,
            target: format!("/stated/{line}"),
            method: Some("GET".to_string()),
            call_kind: None,
            pattern_matched: String::new(),
            call_expression_span_start: None,
            call_expression_span_end: None,
            call_expression_text: None,
            call_expression_line: Some(line),
            payload_expression_text: None,
            payload_expression_line: None,
            primary_type_symbol: None,
            type_import_source: None,
            loopback_default_url: None,
            base: None,
            consumers_not_resolved: None,
            dispatch: None,
            resolution_source: Some(ResolutionSource::Model),
            reaches_request: None,
            body_literals: Default::default(),
            library_semantics: Vec::new(),
            at_caller: false,
            call_body: None,
        }
    }

    /// Every call the call graph resolved in [`SOURCE`], by span.
    fn resolved() -> HashMap<(u32, u32), DefinitionSite> {
        [
            ("client.getRun(id)", definition("client.ts", 10)),
            ("runOptions({ id })", definition("options.ts", 3)),
            ("listRuns()", definition("client.ts", 20)),
            ("audit(id)", definition("audit.ts", 1)),
            (r#"request("/orders")"#, definition("request.ts", 1)),
            ("timed(http.post(target, id))", definition("timed.ts", 1)),
            ("save(id, await loadToken())", definition("save.ts", 1)),
            ("loadToken()", definition("token.ts", 1)),
            (
                "request(`${target}/orders/${id}`)",
                definition("request.ts", 1),
            ),
            (r#"tokens(id.split("/"))"#, definition("tokens.ts", 1)),
        ]
        .into_iter()
        .map(|(needle, reached)| (span(needle), reached))
        .collect()
    }

    /// The rows [`read_file_sites`] reads for `rows` over [`SOURCE`], as
    /// (line, steps).
    fn read(rows: Vec<DataCallResult>) -> Vec<(u32, Vec<Vec<DefinitionSite>>)> {
        let dir = tempfile::tempdir().expect("temp dir");
        let file = dir.path().join("load.ts");
        std::fs::write(&file, SOURCE).expect("the source is written");
        let result = FileAnalysisResult {
            data_calls: rows,
            ..Default::default()
        };
        read_file_sites(&file, "src/load.ts", &result, &resolved())
            .into_iter()
            .map(|row| (row.line, row.steps))
            .collect()
    }

    #[test]
    fn the_calls_own_callee_is_the_first_step() {
        assert_eq!(
            read(vec![model_row(2)]),
            [(2, vec![vec![definition("client.ts", 10)], vec![], vec![]])]
        );
    }

    #[test]
    fn a_row_with_a_span_is_placed_by_it() {
        let (start, end) = span("client.getRun(id)");
        let row = DataCallResult {
            call_expression_span_start: Some(start),
            call_expression_span_end: Some(end),
            ..model_row(2)
        };
        assert_eq!(
            read(vec![row]),
            [(2, vec![vec![definition("client.ts", 10)], vec![], vec![]])]
        );
    }

    #[test]
    fn a_call_spread_into_an_object_argument_is_the_second_step() {
        assert_eq!(
            read(vec![model_row(3)]),
            [(3, vec![vec![], vec![definition("options.ts", 3)], vec![]])]
        );
    }

    #[test]
    fn the_calls_in_the_one_callback_are_the_third_step() {
        assert_eq!(
            read(vec![model_row(4)]),
            [(4, vec![vec![], vec![], vec![definition("client.ts", 20)]])]
        );
    }

    /// Nothing says which of two callbacks makes the request.
    #[test]
    fn two_callbacks_state_no_step() {
        assert_eq!(
            read(vec![model_row(5)]),
            [(5, vec![vec![], vec![], vec![]])]
        );
    }

    /// `save(id, await loadToken())`: `loadToken` reaches a definition too,
    /// and a request made while an argument is computed has a site of its own.
    #[test]
    fn a_plain_argument_is_never_read() {
        assert_eq!(
            read(vec![model_row(8)]),
            [(8, vec![vec![definition("save.ts", 1)], vec![], vec![]])]
        );
    }

    #[test]
    fn a_site_that_writes_a_route_or_states_a_verb_is_not_read() {
        // A literal path, a call that states its verb, and a template with a
        // path behind its base.
        assert_eq!(read(vec![model_row(6), model_row(7), model_row(10)]), []);
        // A bare separator is no route.
        assert_eq!(
            read(vec![model_row(11)]),
            [(11, vec![vec![definition("tokens.ts", 1)], vec![], vec![]])]
        );
    }

    /// A line that opens two calls, neither inside the other, says nothing
    /// about which the row is.
    #[test]
    fn a_row_on_a_line_with_two_calls_is_not_placed() {
        assert_eq!(read(vec![model_row(9)]), []);
    }

    #[test]
    fn a_site_that_holds_a_deterministic_row_is_not_read() {
        let (start, end) = span("listRuns()");
        let stated = DataCallResult {
            resolution_source: Some(ResolutionSource::RequestSummary),
            call_expression_span_start: Some(start),
            call_expression_span_end: Some(end),
            ..model_row(4)
        };
        assert_eq!(read(vec![model_row(4), stated]), []);
    }

    #[test]
    fn a_route_is_a_path_a_url_or_a_base_with_a_path_behind_it() {
        for route in [
            "/orders",
            "http://api.test/x",
            "https://api.test",
            "${}/orders",
        ] {
            assert!(writes_a_route(route), "{route}");
        }
        for other in ["/", "orders", "//", "${}", "${}orders", "a/b", ""] {
            assert!(!writes_a_route(other), "{other}");
        }
    }
}
