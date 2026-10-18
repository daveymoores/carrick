//! Discovery (carrick#2431, step 1 of the fact layer): every call that
//! reaches a library export the store labels with a network role, at any
//! depth, for every protocol.
//!
//! A call is a site when its callee, followed through the compiler's
//! per-position rows (its `call` row, that declaration's definition, the
//! calls in its body, and so on, [`RequestSummaryInputs::reach`]), reaches a
//! call or `new` on a labelled export: the export itself (`fetch(…)`, `new
//! Socket(…)`), or a member of something whose row names a labelled export
//! by its symbol or its type (`this.child.send(…)` on a `ChildProcess`), or
//! a member of an instance one of a labelled export's makers built, as the
//! library-site reader reads it (`const app = pkg(); app.post(…)`, matched
//! on the op the store labels on that receiver, carrick#2506). An
//! export is matched by the name the store keys it by: the name the module
//! publishes it as (`default`, `export =`, a renamed export) for a call
//! through an import binding, and for a global of the language's own library
//! its name under a default-library key, wherever its types declare it. An
//! optional call (`socket?.emit(…)`) is a call. A
//! function handed to an in-repo callee that runs it is followed from the
//! call that hands it over. A function handed to a library export is not:
//! until the store says which exports run what they are handed, none is
//! taken to.
//!
//! There is no hop limit. A cycle ends the walk, and each definition's
//! answer is kept once it does not depend on a definition still being read.
//!
//! A walk that enters code and ends with no labelled export records why:
//! a [`HoleReason`], `interface_member`, `injected_callee`, `cycle` or
//! `no_body` (a definition whose body this pass did not read).
//!
//! Record-only: nothing in a scan calls this. Its tests and the offline
//! census (`discovery_census`) do. Inside request summaries, as ruling D4
//! has it: no second tracer.
//!
//! The record per site is pinned by `tests/fixtures/discovery/` (its
//! `README.md` and `sites.expected.json`), which #2433 and #2435 read.

use serde::{Deserialize, Serialize};

use super::row_calls::Reach;
use super::*;
use crate::library_claims::{ExportClaims, applies, runtime_package_of};
use crate::library_store::RUNTIME_TYPES;
use crate::position_rows::{FLAG_DECLARATION_UNNAMED, Kind, Row};
use crate::services::type_sidecar::{LibraryClaim, LibraryOp, LibraryRole};

/// The stop for a walk that reaches a definition the extractor keys and
/// this pass read no body of.
const NO_BODY: &str = "no_body";

/// One discovered site, or one walk that entered code and stopped.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DiscoverySite {
    /// Repo-relative, `/` separators.
    pub file: String,
    pub line: u32,
    /// The call's SWC span in the file parsed on its own.
    pub span_start: u32,
    pub span_end: u32,
    /// The callee as written.
    pub callee: String,
    /// The labelled call the walk reached; `None` when it stopped.
    pub end: Option<SiteEnd>,
    /// Each in-repo function the walk entered, outermost first.
    pub chain: Vec<ChainHop>,
    /// Why a walk that entered code reached no labelled export.
    pub stop: Option<String>,
}

/// The labelled call a site reaches.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SiteEnd {
    /// The store's key for the package (`lib.dom`, `node:child_process`).
    pub package: String,
    pub export: String,
    /// The member called on the export or its instance; `None` for the
    /// export itself.
    pub member: Option<String>,
    pub role: LibraryRole,
    /// The claim's op for that member; `None` for a construction.
    pub op: Option<LibraryOp>,
    /// Where the labelled call is.
    pub file: String,
    pub line: u32,
    pub span_start: u32,
    pub span_end: u32,
}

/// One function a walk entered.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct ChainHop {
    /// The definition key, or `<callback>` for a function written at the
    /// call and run by the callee.
    pub function: String,
    pub file: String,
    pub line: u32,
}

/// The sites of a service, sorted, and how the walks ended, for the census.
#[derive(Debug, Default)]
pub struct Discovery {
    pub sites: Vec<DiscoverySite>,
    pub counts: BTreeMap<String, usize>,
}

/// The labelled exports of the store's answer, by the store's package key
/// and the export's name. An export labelled `none` is no network export.
pub struct Labels<'a> {
    exports: &'a [ExportClaims],
    by_name: HashMap<(&'a str, &'a str), &'a ExportClaims>,
}

impl<'a> Labels<'a> {
    pub fn of(exports: &'a [ExportClaims]) -> Self {
        let by_name = exports
            .iter()
            .filter(|export| export.role != LibraryRole::None)
            .map(|export| ((export.package.as_str(), export.export.as_str()), export))
            .collect();
        Labels { exports, by_name }
    }

    fn get(&self, package: &str, export: &str) -> Option<&'a ExportClaims> {
        self.by_name.get(&(package, export)).copied()
    }

    /// The labelled global the language's own library declares as `name`,
    /// as the store keys one: under a default-library key (`lib.dom`,
    /// `lib:<file>`), whichever declaration file the compiler read it from.
    /// The first such key, in order, when several label it.
    fn global(&self, name: &str) -> Option<&'a ExportClaims> {
        self.by_name
            .iter()
            .filter(|((package, export), _)| *export == name && is_language_key(package))
            .min_by_key(|((package, _), _)| *package)
            .map(|(_, claims)| *claims)
    }
}

/// Whether a store key names the language's own library.
fn is_language_key(package: &str) -> bool {
    package.starts_with("lib.") || package.starts_with("lib:")
}

/// The store's keys for the package a specifier imports: the runtime's
/// module (`node:<module>`) for a `node:` specifier, and for a bare one both
/// the runtime's module of that name and the package, as the store asks a
/// specifier ([`crate::library_store::request`]).
fn specifier_keys(specifier: &str) -> Vec<String> {
    if specifier.starts_with("node:") {
        return vec![specifier.to_string()];
    }
    let package = super::library_sites::package_name(specifier);
    let mut keys = vec![package.to_string()];
    if !specifier.starts_with('.') && !specifier.contains('/') {
        keys.push(format!("node:{specifier}"));
    }
    keys
}

/// Each object the callee's member chain is read on, innermost first: its
/// span (discovery numbering) and whether it is a member access. An
/// optional member (`socket?.emit`) is read as a member. The walk stops at
/// anything but a name or a member access (`this`, a call).
pub(crate) fn receiver_spans(callee: &Expr) -> Vec<(u32, u32, bool)> {
    fn member_of(expr: &Expr) -> Option<&MemberExpr> {
        match crate::graphql_document_sites::unwrap_expression(expr) {
            Expr::Member(member) => Some(member),
            Expr::OptChain(OptChainExpr { base, .. }) => match &**base {
                OptChainBase::Member(member) => Some(member),
                OptChainBase::Call(_) => None,
            },
            _ => None,
        }
    }
    let mut out = Vec::new();
    let mut at = member_of(callee);
    while let Some(member) = at {
        let object = crate::graphql_document_sites::unwrap_expression(&member.obj);
        if let Expr::Ident(ident) = object {
            out.push((ident.span.lo.0, ident.span.hi.0, false));
            break;
        }
        at = member_of(object);
        if at.is_some() {
            let span = object.span();
            out.push((span.lo.0, span.hi.0, true));
        }
    }
    out
}

/// The store's key for the package a row's declaration is in: the default
/// library's file (`lib.dom`), a runtime module (`node:<module>`, the file
/// the runtime's types declare it in), or the package the declaring package
/// stands for ([`runtime_package_of`]).
fn store_key(row: &Row, exports: &[ExportClaims]) -> Option<String> {
    let file = row.decl_file()?;
    if let Some(library) = file.strip_prefix("lib:") {
        return Some(library.trim_end_matches(".d.ts").to_string());
    }
    let package = row.decl_package()?;
    if package == RUNTIME_TYPES {
        let (_, module) = file.rsplit_once(&format!("{RUNTIME_TYPES}/"))?;
        let module = module.strip_suffix(".d.ts")?;
        return Some(format!(
            "node:{}",
            module.strip_suffix("/index").unwrap_or(module)
        ));
    }
    Some(runtime_package_of(package, exports))
}

/// Whether a row's declaration is a file of the repository.
fn in_repo(row: &Row) -> bool {
    row.flags() & FLAG_DECLARATION_UNNAMED == 0
        && row.decl_file().is_some_and(|file| {
            !file.starts_with("lib:")
                && !file.starts_with("pkg:")
                && !file.split('/').any(|part| part == "node_modules")
        })
}

/// The name a type's text starts with (`Broker`, `typeof Socket`).
fn type_root(text: &str) -> Option<&str> {
    let text = text.strip_prefix("typeof ").unwrap_or(text);
    let end = text
        .char_indices()
        .find(|(_, c)| !(c.is_alphanumeric() || *c == '_' || *c == '$'))
        .map_or(text.len(), |(at, _)| at);
    let name = &text[..end];
    (!name.is_empty() && !name.starts_with(|c: char| c.is_ascii_digit())).then_some(name)
}

/// One labelled call a walk reached, with the functions it entered below
/// where the walk started.
#[derive(Debug, Clone)]
struct Found {
    chain: Vec<ChainHop>,
    end: SiteEnd,
}

/// What a call or a body reaches.
#[derive(Debug, Clone, Default)]
struct Outcome {
    ends: Vec<Found>,
    /// The first stop, when the walk entered code and found nothing there.
    stop: Option<(Vec<ChainHop>, String)>,
    /// The shallowest definition still being read that the walk came back
    /// to: until it is done, this answer is not final.
    cycle_at: Option<usize>,
}

impl Outcome {
    fn stopped(reason: impl Into<String>) -> Self {
        Outcome {
            stop: Some((Vec::new(), reason.into())),
            ..Outcome::default()
        }
    }

    /// The same, as seen from a call one function further up.
    fn under(mut self, hop: ChainHop) -> Self {
        for found in &mut self.ends {
            found.chain.insert(0, hop.clone());
        }
        if let Some((chain, _)) = &mut self.stop {
            chain.insert(0, hop);
        }
        self
    }

    fn absorb(&mut self, other: Outcome) {
        for found in other.ends {
            let at = (&found.end.file, found.end.span_start, found.end.span_end);
            match self
                .ends
                .iter_mut()
                .find(|known| (&known.end.file, known.end.span_start, known.end.span_end) == at)
            {
                Some(known) if known.chain.len() > found.chain.len() => *known = found,
                Some(_) => {}
                None => self.ends.push(found),
            }
        }
        if self.stop.is_none() {
            self.stop = other.stop;
        }
        self.cycle_at = match (self.cycle_at, other.cycle_at) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (a, b) => a.or(b),
        };
    }
}

struct Walker<'a> {
    inputs: &'a RequestSummaryInputs,
    labels: &'a Labels<'a>,
    root: &'a Path,
    /// Each call the library-site reader reads as made through an instance
    /// a package export's maker built (`const app = pkg(); app.post(…)`),
    /// by its file and SWC span ([`super::library_sites`]).
    instances: HashMap<(&'a Path, u32, u32), &'a LibrarySite>,
    memo: HashMap<(PathBuf, String), Outcome>,
    /// The definitions being read, by how deep.
    reading: HashMap<(PathBuf, String), usize>,
}

impl Walker<'_> {
    fn rel(&self, file: &Path) -> String {
        let relative = file.strip_prefix(self.root).unwrap_or(file);
        relative
            .components()
            .map(|part| part.as_os_str().to_string_lossy())
            .collect::<Vec<_>>()
            .join("/")
    }

    fn hop(&self, file: &Path, key: &str) -> ChainHop {
        let line = self
            .inputs
            .files
            .get(file)
            .and_then(|ir| {
                ir.definition_lines
                    .iter()
                    .filter(|(_, keys)| keys.iter().any(|known| known == key))
                    .map(|(line, _)| *line)
                    .min()
            })
            .unwrap_or_default();
        ChainHop {
            function: key.to_string(),
            file: self.rel(file),
            line,
        }
    }

    /// The call row at `call`, when there are rows to read.
    fn row(&self, file: &Path, call: &CallIr) -> Option<&Row> {
        let positions = self.inputs.positions.as_ref()?;
        let ir = self.inputs.files.get(file)?;
        positions
            .lookup(
                file,
                ir.swc_start(call.site.lo),
                ir.swc_start(call.site.hi),
                Kind::Call,
            )
            .ok()
    }

    /// The labelled export `call` is made on, when it is one:
    ///
    /// - the callee's own symbol in the callee's package;
    /// - the name the module publishes the binding the callee is read
    ///   through as (`default`, `io`, `export =`), as the store keys the
    ///   export: the compiler names the declaration (`axios`, `lookup`);
    /// - a global of the language's own library, by its name, wherever its
    ///   types declare it;
    /// - a receiver's symbol or type in the callee's package.
    fn library_end(&self, file: &Path, call: &CallIr, construction: bool) -> Option<SiteEnd> {
        let row = self.row(file, call)?;
        let symbol = row.symbol()?;
        if in_repo(row) {
            return None;
        }
        let end = |package: &str, export: &str, member: Option<&str>, claims: &ExportClaims| {
            let op = (!construction)
                .then(|| {
                    claims.claims.iter().find_map(|claim| match claim {
                        LibraryClaim::Op {
                            op,
                            member: claimed,
                            ..
                        } if claimed.as_deref() == member => Some(*op),
                        _ => None,
                    })
                })
                .flatten();
            SiteEnd {
                package: package.to_string(),
                export: export.to_string(),
                member: member.map(str::to_string),
                role: claims.role,
                op,
                file: self.rel(file),
                line: call.site.line,
                span_start: call.site.span_start,
                span_end: call.site.span_end,
            }
        };
        let package = store_key(row, self.labels.exports);
        if let Some(package) = &package
            && let Some(claims) = self.labels.get(package, symbol)
        {
            return Some(end(package, symbol, None, claims));
        }
        let ir = self.inputs.files.get(file)?;
        // The binding the callee is read through: the callee itself, or the
        // name its member chain starts at.
        let root = match call.receiver_spans.last() {
            None => Some(call.callee_text.as_str()),
            Some((_, _, false)) => call.callee_text.split(['.', '?']).next().map(str::trim),
            Some(_) => None,
        }
        .filter(|root| super::row_calls::is_identifier(root));
        let binding = root.and_then(|root| ir.bindings.get(root));
        if let Some(binding) = binding {
            let member = (!call.receiver_spans.is_empty()).then_some(symbol);
            let (exports, member) = match (binding.namespace, call.receiver_spans.len()) {
                // `ns.fork()`: the member, as written, is the export.
                (true, 1) => (vec![call.callee.as_str()], None),
                (true, _) => (Vec::new(), None),
                (false, _) if binding.export == "default" => (vec!["default", "export ="], member),
                (false, _) => (vec![binding.export.as_str()], member),
            };
            for key in specifier_keys(&binding.specifier) {
                for export in &exports {
                    if let Some(claims) = self.labels.get(&key, export) {
                        return Some(end(&key, export, member, claims));
                    }
                }
            }
        }
        // `fetch(…)`, `new WebSocket(…)`: a name no binding of the file
        // holds, declared by the language's own library or the runtime's
        // types, is the language's global.
        if binding.is_none()
            && call.receiver_spans.is_empty()
            && root.is_some()
            && (row
                .decl_file()
                .is_some_and(|declared| declared.starts_with("lib:"))
                || row.decl_package() == Some(RUNTIME_TYPES))
            && let Some(claims) = self.labels.global(symbol)
        {
            return Some(end(&claims.package, symbol, None, claims));
        }
        // `app.post(…)` where `const app = pkg()`: a member the store labels
        // on the receiver that maker builds (`instance:()`), the instance
        // read by the library-site reader, wherever it is held.
        if let Some(site) = self
            .instances
            .get(&(file, call.site.span_start, call.site.span_end))
            && site.path.is_empty()
            && let Some(member) = site.member.as_deref()
        {
            let receivers = site.receiver_ids();
            let exports = if site.export == "default" {
                vec!["default", "export ="]
            } else {
                vec![site.export.as_str()]
            };
            for key in specifier_keys(&site.specifier) {
                for export in &exports {
                    let Some(claims) = self.labels.get(&key, export) else {
                        continue;
                    };
                    let op = claims.claims.iter().find_map(|claim| match claim {
                        LibraryClaim::Op {
                            op,
                            member: claimed,
                            path,
                            on,
                            of,
                            ..
                        } if claimed.as_deref() == Some(member)
                            && path.is_empty()
                            && receivers
                                .iter()
                                .any(|receiver| applies(*on, of.as_deref(), receiver)) =>
                        {
                            Some(*op)
                        }
                        _ => None,
                    });
                    if let Some(op) = op {
                        return Some(SiteEnd {
                            op: Some(op),
                            ..end(&key, export, Some(member), claims)
                        });
                    }
                }
            }
        }
        let package = package?;
        let positions = self.inputs.positions.as_ref()?;
        for (lo, hi, member) in &call.receiver_spans {
            let kind = if *member { Kind::Member } else { Kind::Ident };
            let Ok(receiver) = positions.lookup(file, ir.swc_start(*lo), ir.swc_start(*hi), kind)
            else {
                break;
            };
            for name in [receiver.symbol(), receiver.type_text().and_then(type_root)]
                .into_iter()
                .flatten()
            {
                if let Some(claims) = self.labels.get(&package, name) {
                    return Some(end(&package, name, Some(symbol), claims));
                }
            }
        }
        None
    }

    /// Whether the call row names a member an interface or type literal of
    /// the service declares: no body here implements it.
    fn interface_member(&self, file: &Path, call: &CallIr) -> bool {
        let Some(row) = self.row(file, call) else {
            return false;
        };
        let (Some(symbol), Some(decl_file), Some(line)) =
            (row.symbol(), row.decl_file(), row.decl_line())
        else {
            return false;
        };
        let Some(positions) = self.inputs.positions.as_ref() else {
            return false;
        };
        in_repo(row)
            && self
                .inputs
                .files
                .get(&positions.path_of(decl_file))
                .and_then(|declared| declared.declaration_lines.signatures.get(&line))
                .is_some_and(|name| name == symbol)
    }

    /// What `call`, written in `body` of `file`, reaches.
    fn call(&mut self, file: &Path, body: &FnIr, call: &CallIr, construction: bool) -> Outcome {
        if call.invokes_param.is_some() {
            return Outcome::stopped("injected_callee");
        }
        if let Some(end) = self.library_end(file, call, construction) {
            return Outcome {
                ends: vec![Found {
                    chain: Vec::new(),
                    end,
                }],
                ..Outcome::default()
            };
        }
        match self.inputs.reach(file, call.site.lo, call.site.hi) {
            Reach::Function(target, key) => {
                let hop = self.hop(&target, &key);
                let id = (target, key);
                let mut out = match self.reading.get(&id) {
                    // The walk came back to a definition it is reading.
                    Some(depth) => Outcome {
                        cycle_at: Some(*depth),
                        ..Outcome::stopped("cycle")
                    },
                    None => self.definition(&id).under(hop.clone()),
                };
                // A function written at the call that the callee runs.
                let runs = self
                    .inputs
                    .files
                    .get(&id.0)
                    .and_then(|ir| ir.functions.get(&id.1))
                    .map(|callee| callee.invoked_params.clone())
                    .unwrap_or_default();
                for (index, arg) in call.args.iter().enumerate() {
                    if let Value::Callback(nested) = arg
                        && runs.contains(&index)
                        && let Some(callback) = body.nested.get(*nested)
                    {
                        let written = ChainHop {
                            function: "<callback>".to_string(),
                            file: self.rel(file),
                            line: callback
                                .site
                                .as_ref()
                                .map_or(call.site.line, |site| site.line),
                        };
                        let inner = self.body(file, callback).under(written).under(hop.clone());
                        out.absorb(inner);
                    }
                }
                if !out.ends.is_empty() {
                    out.stop = None;
                }
                out
            }
            Reach::OutsideService(..) => Outcome::default(),
            Reach::NoFunction if self.interface_member(file, call) => {
                Outcome::stopped("interface_member")
            }
            Reach::NoFunction => Outcome::default(),
            Reach::Unknown(reason) => Outcome::stopped(super::row_calls::hole_key(reason)),
        }
    }

    /// What the definition `id` reaches, read once it is final.
    fn definition(&mut self, id: &(PathBuf, String)) -> Outcome {
        if let Some(known) = self.memo.get(id) {
            return known.clone();
        }
        let inputs = self.inputs;
        // A definition the extractor keys whose body this pass did not read
        // (an object's method, a function written inside another): what it
        // calls is not known, so the walk stops there and says so.
        let Some(ir) = inputs
            .files
            .get(&id.0)
            .and_then(|ir| ir.functions.get(&id.1))
        else {
            return Outcome::stopped(NO_BODY);
        };
        if ir.bodyless {
            return Outcome::stopped("interface_member");
        }
        let depth = self.reading.len();
        self.reading.insert(id.clone(), depth);
        let mut out = self.body(&id.0, ir);
        self.reading.remove(id);
        if out.cycle_at.is_none_or(|at| at >= depth) {
            out.cycle_at = None;
            // A cycle's stop names the functions down to where the walk came
            // back, which depends on where it started: read it again from
            // each start.
            let cycle_stop = out.ends.is_empty()
                && out
                    .stop
                    .as_ref()
                    .is_some_and(|(_, reason)| reason == "cycle");
            if !cycle_stop {
                self.memo.insert(id.clone(), out.clone());
            }
        }
        out
    }

    /// What the calls `body` makes reach.
    fn body(&mut self, file: &Path, body: &FnIr) -> Outcome {
        let mut out = Outcome::default();
        for (call, construction) in body.read_calls() {
            out.absorb(self.call(file, body, call, construction));
        }
        if !out.ends.is_empty() {
            out.stop = None;
        }
        out
    }

    /// The records for every call `body` and the functions written in it
    /// make.
    fn sites(&mut self, file: &Path, body: &FnIr, discovery: &mut Discovery) {
        for (call, construction) in body.read_calls() {
            if call.jsx {
                continue;
            }
            let outcome = self.call(file, body, call, construction);
            let record =
                |end: Option<SiteEnd>, chain: Vec<ChainHop>, stop: Option<String>| DiscoverySite {
                    file: self.rel(file),
                    line: call.site.line,
                    span_start: call.site.span_start,
                    span_end: call.site.span_end,
                    callee: call.callee_text.clone(),
                    end,
                    chain,
                    stop,
                };
            if outcome.ends.is_empty() {
                let Some((chain, stop)) = outcome.stop else {
                    *discovery.counts.entry("no_site".to_string()).or_default() += 1;
                    continue;
                };
                // A call whose own row says nothing is counted, not listed:
                // the walk never entered code.
                if chain.is_empty() && stop != "injected_callee" && stop != "interface_member" {
                    *discovery
                        .counts
                        .entry(format!("unread/{stop}"))
                        .or_default() += 1;
                    continue;
                }
                *discovery.counts.entry(format!("stop/{stop}")).or_default() += 1;
                discovery.sites.push(record(None, chain, Some(stop)));
                continue;
            }
            for found in outcome.ends {
                *discovery
                    .counts
                    .entry(format!("site/{}", role_key(found.end.role)))
                    .or_default() += 1;
                discovery
                    .sites
                    .push(record(Some(found.end), found.chain, None));
            }
        }
        for inner in body.nested.iter().chain(&body.detached) {
            self.sites(file, inner, discovery);
        }
    }
}

fn role_key(role: LibraryRole) -> String {
    serde_json::to_value(role)
        .ok()
        .and_then(|value| value.as_str().map(str::to_string))
        .unwrap_or_default()
}

/// Every site of the service `inputs` reads, with the store's answer
/// `labels`, paths relative to `root`. Nothing without rows.
pub fn discover(inputs: &RequestSummaryInputs, labels: &Labels<'_>, root: &Path) -> Discovery {
    let mut discovery = Discovery::default();
    if inputs.positions.is_none() {
        return discovery;
    }
    let library = super::library_sites(inputs);
    let instances = library
        .sites
        .iter()
        .filter(|site| site.origin.is_none() && !site.makers().is_empty())
        .map(|site| ((site.file.as_path(), site.span_start, site.span_end), site))
        .collect();
    let mut walker = Walker {
        inputs,
        labels,
        root,
        instances,
        memo: HashMap::new(),
        reading: HashMap::new(),
    };
    let mut paths: Vec<&PathBuf> = inputs.files.keys().collect();
    paths.sort();
    for path in paths {
        let file = &inputs.files[path];
        let mut keys: Vec<&String> = file.functions.keys().collect();
        keys.sort();
        for key in keys {
            walker.sites(path, &file.functions[key], &mut discovery);
        }
        walker.sites(path, &file.module_level, &mut discovery);
    }
    discovery
        .sites
        .sort_by(|a, b| sort_key(a).cmp(&sort_key(b)));
    discovery.sites.dedup();
    discovery
}

fn sort_key(site: &DiscoverySite) -> (&str, u32, u32, Option<(&str, u32, u32)>) {
    (
        &site.file,
        site.span_start,
        site.span_end,
        site.end
            .as_ref()
            .map(|end| (end.file.as_str(), end.span_start, end.span_end)),
    )
}

#[cfg(test)]
mod tests {
    use super::super::fixture_walk::walked;
    use super::*;

    fn fixture(name: &str) -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/discovery")
            .join(name)
    }

    fn claims() -> Vec<ExportClaims> {
        let text = std::fs::read_to_string(fixture("library-claims.json")).unwrap();
        let answer = crate::library_store::parse_answer(&text);
        assert_eq!(answer.dropped, 0, "every fixture claim parses");
        answer.exports
    }

    fn sites_of(discovery: &Discovery) -> serde_json::Value {
        serde_json::json!({ "sites": discovery.sites })
    }

    /// carrick#2431: every call that reaches a labelled export, for every
    /// protocol, at any depth, is a site with its end and its chain: an HTTP
    /// helper three files deep, a seven-hop chain, an arrow method, a message
    /// through an in-repo bus over a child-process channel, a socket send
    /// through a session class, a broker publish behind a wrapper, a
    /// callback an in-repo tracer runs. A closure a library's store
    /// initialiser is handed is not followed, a cycle ends, and an interface
    /// member and an injected callee are stops. The answer key is the shared
    /// fixture #2433 and #2435 read.
    #[test]
    fn every_call_that_reaches_a_labelled_export_is_a_site() {
        let Some(on) = walked("discovery", "services/web", "web", &[], true) else {
            return;
        };
        let exports = claims();
        let discovery = discover(&on.inputs, &Labels::of(&exports), &on.repo);
        let expected: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(fixture("sites.expected.json")).unwrap())
                .unwrap();
        let got = sites_of(&discovery);
        if got != expected {
            let got: Vec<String> = discovery
                .sites
                .iter()
                .map(|site| serde_json::to_string(site).unwrap())
                .collect();
            let want: Vec<String> = expected["sites"]
                .as_array()
                .unwrap()
                .iter()
                .map(|site| {
                    serde_json::to_string(
                        &serde_json::from_value::<DiscoverySite>(site.clone()).unwrap(),
                    )
                    .unwrap()
                })
                .collect();
            let extra: Vec<&String> = got.iter().filter(|s| !want.contains(s)).collect();
            let missing: Vec<&String> = want.iter().filter(|s| !got.contains(s)).collect();
            panic!("extra: {extra:#?}\nmissing: {missing:#?}");
        }
    }

    /// The seven-hop site has seven hops, and no site sits at the store
    /// initialiser's call.
    #[test]
    fn the_seven_hop_site_and_no_site_at_an_initialiser() {
        let Some(on) = walked("discovery", "services/web", "web", &[], true) else {
            return;
        };
        let exports = claims();
        let discovery = discover(&on.inputs, &Labels::of(&exports), &on.repo);
        let start = discovery
            .sites
            .iter()
            .find(|site| site.file.ends_with("deep.ts") && site.line == 30)
            .expect("the seven-hop site");
        assert_eq!(start.chain.len(), 7);
        assert!(
            !discovery
                .sites
                .iter()
                .any(|site| site.callee == "createStore"),
            "a closure a library only stores is not followed"
        );
    }

    /// The store's answer for the packages the published-name and global
    /// cases add, beside the fixture's own.
    const PUBLISHED_CLAIMS: &str = r#"{
  "library_claims": [
    {
      "package": "fixture-http", "version": "1.0.0", "status": "answered",
      "exports": [
        { "specifier": "fixture-http", "export": "default", "role": "http_client", "side": "client",
          "makes": [], "scopes": [],
          "ops": [ { "op": "request", "member": "get", "on": "export", "name": { "arg": 0 }, "picker": "fixture-picker" } ],
          "reserved": [], "calls": [], "patterns": [] }
      ]
    },
    {
      "package": "fixture-io", "version": "1.0.0", "status": "answered",
      "exports": [
        { "specifier": "fixture-io", "export": "io", "role": "socket", "side": "client",
          "makes": [ { "form": "call", "member": null, "picker": "fixture-picker" } ], "scopes": [],
          "ops": [], "reserved": [], "calls": [], "patterns": [] }
      ]
    },
    {
      "package": "fixture-pipe", "version": "1.0.0", "status": "answered",
      "exports": [
        { "specifier": "fixture-pipe", "export": "export =", "role": "socket", "side": "client",
          "makes": [ { "form": "new", "member": null, "picker": "fixture-picker" } ], "scopes": [],
          "ops": [], "reserved": [], "calls": [], "patterns": [] }
      ]
    }
  ]
}"#;

    /// The fixture's store answer and [`PUBLISHED_CLAIMS`].
    fn all_claims() -> Vec<ExportClaims> {
        let mut exports = claims();
        let added = crate::library_store::parse_answer(PUBLISHED_CLAIMS);
        assert_eq!(added.dropped, 0, "every added claim parses");
        exports.extend(added.exports);
        exports
    }

    /// The records discovery writes for `file` (repo-relative), each as
    /// `line callee -> package export member` or `line callee stop <reason>`.
    fn records_in(discovery: &Discovery, file: &str) -> Vec<String> {
        discovery
            .sites
            .iter()
            .filter(|site| site.file == file)
            .map(|site| match &site.end {
                Some(end) => format!(
                    "{} {} -> {} {} {}",
                    site.line,
                    site.callee,
                    end.package,
                    end.export,
                    end.member.as_deref().unwrap_or("-")
                ),
                None => format!(
                    "{} {} stop {}",
                    site.line,
                    site.callee,
                    site.stop.as_deref().unwrap_or("-")
                ),
            })
            .collect()
    }

    fn texts(items: &[&str]) -> Vec<String> {
        items.iter().map(|item| item.to_string()).collect()
    }

    /// carrick#2431: the store keys an export by the name the module
    /// publishes it as (`default`, a renamed `io`, `export =`), and the rows
    /// name the declaration (`http`, `lookup`, `Pipe`). A call through an
    /// import binding is matched on what the binding imports.
    #[test]
    fn a_call_through_a_binding_matches_the_published_export() {
        let edit = [
            (
                "node_modules/fixture-http/package.json",
                r#"{ "name": "fixture-http", "version": "1.0.0", "types": "index.d.ts" }"#,
            ),
            (
                "node_modules/fixture-http/index.d.ts",
                "export interface HttpStatic {\n  get(url: string): Promise<unknown>;\n}\ndeclare const http: HttpStatic;\nexport default http;\n",
            ),
            (
                "node_modules/fixture-io/package.json",
                r#"{ "name": "fixture-io", "version": "1.0.0", "types": "index.d.ts" }"#,
            ),
            (
                "node_modules/fixture-io/index.d.ts",
                "export interface Channel {\n  emit(name: string, data: unknown): void;\n}\ndeclare function lookup(url: string): Channel;\nexport { lookup as io };\n",
            ),
            (
                "node_modules/fixture-pipe/package.json",
                r#"{ "name": "fixture-pipe", "version": "1.0.0", "types": "index.d.ts" }"#,
            ),
            (
                "node_modules/fixture-pipe/index.d.ts",
                "declare class Pipe {\n  constructor(url: string);\n  write(data: unknown): void;\n}\nexport = Pipe;\n",
            ),
            (
                "src/published.ts",
                "import http from \"fixture-http\";\nimport { io } from \"fixture-io\";\nimport Pipe from \"fixture-pipe\";\n\nexport function readPublished(): Promise<unknown> {\n  return http.get(\"/published\");\n}\n\nexport function openChannel(): void {\n  io(\"wss://published\");\n}\n\nexport function openPipe(): Pipe {\n  return new Pipe(\"wss://pipe\");\n}\n",
            ),
            (
                "src/published-user.ts",
                "import { openPipe, readPublished } from \"./published\";\n\nexport function readBoth(): void {\n  readPublished();\n  openPipe();\n}\n",
            ),
        ];
        let Some(on) = walked("discovery", "services/web", "web", &edit, true) else {
            return;
        };
        let exports = all_claims();
        let discovery = discover(&on.inputs, &Labels::of(&exports), &on.repo);
        assert_eq!(
            records_in(&discovery, "services/web/src/published.ts"),
            texts(&[
                "6 http.get -> fixture-http default get",
                "10 io -> fixture-io io -",
                "14 Pipe -> fixture-pipe export = -",
            ])
        );
        assert_eq!(
            records_in(&discovery, "services/web/src/published-user.ts"),
            texts(&[
                "4 readPublished -> fixture-http default get",
                "5 openPipe -> fixture-pipe export = -",
            ])
        );
    }

    /// carrick#2431: a global of the language's own library is keyed as the
    /// store keys the language's globals, wherever the compiler read its
    /// declaration: without the DOM library, the runtime's types declare
    /// `fetch` in a file of their own, and the call is the same site.
    #[test]
    fn a_global_the_runtime_declares_is_the_language_s() {
        let edit = [
            (
                "tsconfig.json",
                r#"{
  "compilerOptions": {
    "target": "es2022",
    "module": "esnext",
    "moduleResolution": "bundler",
    "strict": true,
    "skipLibCheck": true,
    "lib": ["es2022"],
    "types": ["node"]
  },
  "include": ["src/**/*"]
}
"#,
            ),
            (
                "node_modules/@types/node/index.d.ts",
                "/// <reference path=\"child_process.d.ts\" />\n/// <reference path=\"web-globals/fetch.d.ts\" />\n",
            ),
            (
                "node_modules/@types/node/web-globals/fetch.d.ts",
                "export {};\ndeclare global {\n  function fetch(input: string, init?: unknown): Promise<unknown>;\n}\n",
            ),
            (
                "src/node-fetch.ts",
                "export function readNodeGlobal(): Promise<unknown> {\n  return fetch(\"/node-global\");\n}\n",
            ),
        ];
        let Some(on) = walked("discovery", "services/web", "web", &edit, true) else {
            return;
        };
        let exports = claims();
        let discovery = discover(&on.inputs, &Labels::of(&exports), &on.repo);
        assert_eq!(
            records_in(&discovery, "services/web/src/node-fetch.ts"),
            texts(&["2 fetch -> lib.dom fetch -"])
        );
    }

    /// carrick#2431: a walk that reaches a definition whose body this pass
    /// did not read stops there and says so; an optional call is a call.
    #[test]
    fn an_unread_body_is_a_stop_and_an_optional_call_is_a_site() {
        let edit = [
            (
                "src/table.ts",
                "export function readTable(): Promise<Response> {\n  function loadInner(): Promise<Response> {\n    return fetch(\"/table\");\n  }\n  return loadInner();\n}\n\nexport const table = {\n  load(): Promise<Response> {\n    return fetch(\"/entry\");\n  },\n};\n\nexport function readEntry(): Promise<Response> {\n  return table.load();\n}\n",
            ),
            (
                "src/maybe.ts",
                "import { Socket } from \"fixture-socket\";\n\nexport function maybeSend(socket: Socket | undefined): void {\n  socket?.send(\"ping\", {});\n}\n",
            ),
        ];
        let Some(on) = walked("discovery", "services/web", "web", &edit, true) else {
            return;
        };
        let exports = claims();
        let discovery = discover(&on.inputs, &Labels::of(&exports), &on.repo);
        assert_eq!(
            records_in(&discovery, "services/web/src/table.ts"),
            texts(&[
                "3 fetch -> lib.dom fetch -",
                "5 loadInner stop no_body",
                "10 fetch -> lib.dom fetch -",
                "15 table.load stop no_body",
            ])
        );
        assert_eq!(
            records_in(&discovery, "services/web/src/maybe.ts"),
            texts(&["4 socket?.send -> fixture-socket Socket send"])
        );
    }

    /// Without rows there is nothing to walk: no site.
    #[test]
    fn without_rows_nothing_is_discovered() {
        let Some(off) = walked("discovery", "services/web", "web", &[], false) else {
            return;
        };
        let exports = claims();
        assert!(
            discover(&off.inputs, &Labels::of(&exports), &off.repo)
                .sites
                .is_empty()
        );
    }

    /// The offline census over flag-on rows: every site per repo, by role,
    /// with the stops, written as JSON.
    ///
    /// `CARRICK_DISCOVERY_CENSUS='<label>=<repo>=<positions dir>=<store
    /// answer>[;…]' [CARRICK_DISCOVERY_CENSUS_OUT=<file>] cargo test --lib
    /// discovery_census -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn discovery_census() {
        let Ok(spec) = std::env::var("CARRICK_DISCOVERY_CENSUS") else {
            return;
        };
        let mut all: BTreeMap<String, serde_json::Value> = BTreeMap::new();
        for entry in spec.split(';').filter(|entry| !entry.is_empty()) {
            let parts: Vec<&str> = entry.splitn(4, '=').collect();
            let [label, root, positions, answer] = parts.as_slice() else {
                panic!("<label>=<repo>=<positions dir>=<store answer>: {entry}");
            };
            let exports = crate::library_store::parse_answer(
                &std::fs::read_to_string(answer).expect("the store answer reads"),
            )
            .exports;
            let labels = Labels::of(&exports);
            let root = std::fs::canonicalize(root).expect("the repo exists");
            let config = root.join("carrick.json");
            let services = if config.exists() {
                crate::config::Config::load_services(vec![config]).expect("carrick.json loads")
            } else {
                vec![crate::config::Config::default()]
            };
            let mut by_service: BTreeMap<String, PathBuf> = BTreeMap::new();
            for dir_entry in std::fs::read_dir(positions).expect("the positions dir reads") {
                let path = dir_entry.expect("dir entry").path();
                if path.to_string_lossy().ends_with(".jsonl.gz")
                    && let Ok(rows) = crate::position_rows::read_rows_file(&path)
                {
                    by_service.insert(rows.header.service.clone(), path);
                }
            }
            let mut sites: Vec<DiscoverySite> = Vec::new();
            let mut counts: BTreeMap<String, usize> = BTreeMap::new();
            for service in &services {
                let name = service
                    .service_name
                    .clone()
                    .unwrap_or_else(|| "(root)".to_string());
                let index = match by_service.get(&name) {
                    Some(path) => crate::position_rows::PositionIndex::of(
                        &root,
                        crate::position_rows::read_rows_file(path).expect("rows read"),
                    ),
                    None => crate::position_rows::PositionIndex::missing(&root),
                };
                let inputs = crate::engine::discover_service_inputs(&root, service, Some(index));
                let discovery = discover(&inputs, &labels, &root);
                for (key, count) in discovery.counts {
                    *counts.entry(key).or_default() += count;
                }
                sites.extend(discovery.sites);
            }
            crate::outln!("== {label}: {} record(s)", sites.len());
            for (key, count) in &counts {
                crate::outln!("  {key}: {count}");
            }
            all.insert(
                label.to_string(),
                serde_json::json!({ "counts": counts, "sites": sites }),
            );
        }
        if let Ok(out) = std::env::var("CARRICK_DISCOVERY_CENSUS_OUT") {
            std::fs::write(&out, serde_json::to_string(&all).unwrap()).expect("the census writes");
        }
    }
}
