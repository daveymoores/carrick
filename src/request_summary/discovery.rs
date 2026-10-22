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
//! A call that reached nothing and entered no code says why, on
//! [`Discovery::unfollowed`], and never in `sites`: `value_callee` (an in-repo
//! value that is not a definition: a parameter, a field, a returned
//! function), `outside_service` (a function of this repo outside the
//! service's files), `package_unmatched` (a call declared in a package the
//! store answered with network-role exports, that matched none of them),
//! `unanswered_package` (a call declared in a package the answer holds
//! nothing for), or the hole the call's own row names. A call into an export
//! the store labels with no network role, and a walk into in-repo code that
//! finds no call to follow, stay `no_site`.
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
use crate::library_store::LibraryAnswer;
use crate::library_store::RUNTIME_TYPES;
use crate::position_rows::{
    FLAG_DECLARATION_UNNAMED, FLAG_MODULE_DEFAULT, FLAG_PUBLISHED, Kind, Row,
};
use crate::services::type_sidecar::{
    LibraryCheck, LibraryClaim, LibraryOp, LibraryRole, MakeForm, SemanticsResult, SemanticsVerdict,
};

/// The stop for a walk that reaches a definition the extractor keys and
/// this pass read no body of.
const NO_BODY: &str = "no_body";

/// Why a call that entered no code was not followed ([`Discovery::unfollowed`]).
const VALUE_CALLEE: &str = "value_callee";
const OUTSIDE_SERVICE: &str = "outside_service";
const PACKAGE_UNMATCHED: &str = "package_unmatched";
const UNANSWERED_PACKAGE: &str = "unanswered_package";

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
    /// The binding the labelled call is made on (carrick#2474 R1). Only
    /// with the receiver trace on.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub receiver: Option<ReceiverAt>,
    /// How the labelled call was matched to its export. Only with the
    /// receiver trace on.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reached: Option<ReachedBy>,
}

/// The labelled call a site reaches.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SiteEnd {
    /// The store's key for the package (`lib:lib.dom.d.ts`, `node:child_process`).
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
    /// The call constructs its callee (`new m(…)`).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub new: bool,
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
    /// The calls the walk could not follow, each with its reason as `stop`
    /// (`end` and `chain` empty). Not sites: nothing decided reads them.
    pub unfollowed: Vec<DiscoverySite>,
    /// The calls whose walk ended with nothing missed and no site (`no_site`),
    /// by file and SWC span: the calls the fact layer reads as followed to
    /// the end (carrick#2613).
    pub finished: Vec<(String, u32, u32)>,
    /// The calls whose walk found no site and passed a call it could not
    /// follow on the way (`no_site` all the same): not followed to the end.
    pub missed_inside: Vec<(String, u32, u32)>,
    pub counts: BTreeMap<String, usize>,
}

/// The labelled exports of the store's answer, by the store's package key
/// and the export's name. An export labelled `none` is no network export.
pub struct Labels<'a> {
    exports: &'a [ExportClaims],
    by_name: HashMap<(&'a str, &'a str), &'a ExportClaims>,
    /// With the receiver trace on (carrick#2474 R1), the member
    /// declarations verified op claims resolve to: a call to one is a site
    /// whatever holds its receiver, and every site names its receiver.
    declarations: Option<&'a MemberDeclarations>,
    /// Every package the store answered, with or without an export; `None`
    /// when only the exports are known ([`Labels::of`]), where no package
    /// can be told unanswered.
    answered: Option<HashSet<&'a str>>,
    /// The packages with an export of a network role.
    networked: HashSet<&'a str>,
    /// The exports the store labelled with no network role.
    none: HashSet<(&'a str, &'a str)>,
}

impl<'a> Labels<'a> {
    pub fn of(exports: &'a [ExportClaims]) -> Self {
        let by_name = exports
            .iter()
            .filter(|export| export.role != LibraryRole::None)
            .map(|export| ((export.package.as_str(), export.export.as_str()), export))
            .collect();
        Labels {
            exports,
            by_name,
            declarations: None,
            answered: None,
            networked: exports
                .iter()
                .filter(|export| export.role != LibraryRole::None)
                .map(|export| export.package.as_str())
                .collect(),
            none: exports
                .iter()
                .filter(|export| export.role == LibraryRole::None)
                .map(|export| (export.package.as_str(), export.export.as_str()))
                .collect(),
        }
    }

    /// [`Labels::of`], with the packages the store answered, so a call into
    /// a package it did not answer (still building, skipped, not asked) is
    /// told from one it answered with nothing to label.
    pub fn of_answer(answer: &'a LibraryAnswer) -> Self {
        Labels::of_answered(&answer.exports, &answer.answered_packages)
    }

    /// [`Labels::of_answer`] from the exports and the packages answered, as
    /// the store's first answer to a scan holds them (carrick#2613).
    pub fn of_answered(exports: &'a [ExportClaims], answered: &'a BTreeSet<String>) -> Self {
        Labels {
            answered: Some(answered.iter().map(String::as_str).collect()),
            ..Labels::of(exports)
        }
    }

    /// Whether the store labelled `export` of `package` with no network role.
    fn labelled_none(&self, package: &str, export: &str) -> bool {
        self.none.contains(&(package, export))
    }

    /// Why a call declared in the package the store keys `key` and matching
    /// no label was not followed, when the answer says nothing for it. The
    /// default library is never `package_unmatched`: its calls are matched
    /// by their global's name, and a call it does not label is no client
    /// call. It is unanswered only when no default-library key is answered.
    fn package_reason(&self, key: &str) -> Option<&'static str> {
        let unanswered = |answered: &HashSet<&str>| {
            if is_language_key(key) {
                !answered.iter().any(|known| is_language_key(known))
            } else {
                !answered.contains(key)
            }
        };
        if !is_language_key(key) && self.networked.contains(key) {
            return Some(PACKAGE_UNMATCHED);
        }
        self.answered
            .as_ref()
            .is_some_and(unanswered)
            .then_some(UNANSWERED_PACKAGE)
    }

    /// [`Self::of`], with the receiver trace on.
    pub fn traced(exports: &'a [ExportClaims], declarations: &'a MemberDeclarations) -> Self {
        Labels {
            declarations: Some(declarations),
            ..Self::of(exports)
        }
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

/// The store's key for a global the runtime's types package declares
/// (`node:web-globals/fetch` for `fetch`), the key the scanner asks
/// ([`runtime_globals`]). None for any other row.
fn runtime_global_key(row: &Row) -> Option<String> {
    (row.decl_package() == Some(RUNTIME_TYPES))
        .then(|| store_key(row, &[]))
        .flatten()
}

/// Whether a call is made on a bare name the file holds no binding for
/// (`fetch(…)`), so it is read as the language's or the runtime's global.
fn is_free_name(ir: &FileIr, call: &CallIr) -> bool {
    call.receiver_spans.is_empty()
        && root_name(call).is_some_and(|root| !ir.bindings.contains_key(root))
}

/// The globals a service's calls use that the runtime's types package
/// declares, by the store's key for the file they are declared in
/// (carrick#2445): a Node program that loads no DOM library has its
/// `fetch` from `web-globals/fetch.d.ts` under the runtime's types, not
/// from a default-library file. Read with the same test discovery reads a
/// global by ([`Walker::library_end`]).
pub fn runtime_globals(inputs: &RequestSummaryInputs) -> BTreeMap<String, BTreeSet<String>> {
    fn walk(
        inputs: &RequestSummaryInputs,
        file: &Path,
        ir: &FileIr,
        body: &FnIr,
        out: &mut BTreeMap<String, BTreeSet<String>>,
    ) {
        let Some(positions) = inputs.positions.as_ref() else {
            return;
        };
        for (call, _) in body.read_calls() {
            if !is_free_name(ir, call) {
                continue;
            }
            let Ok(row) = positions.lookup(
                file,
                ir.swc_start(call.site.lo),
                ir.swc_start(call.site.hi),
                Kind::Call,
            ) else {
                continue;
            };
            if let (Some(symbol), Some(key)) = (row.symbol(), runtime_global_key(row)) {
                out.entry(key).or_default().insert(symbol.to_string());
            }
        }
        for inner in body.nested.iter().chain(&body.detached) {
            walk(inputs, file, ir, inner, out);
        }
    }
    let mut out = BTreeMap::new();
    for (path, ir) in &inputs.files {
        for body in ir.functions.values().chain([&ir.module_level]) {
            walk(inputs, path, ir, body, &mut out);
        }
    }
    out
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

/// The binding a call's callee is read through: the callee itself, or the
/// name its member chain starts at.
fn root_name(call: &CallIr) -> Option<&str> {
    match call.receiver_spans.last() {
        None => Some(call.callee_text.as_str()),
        Some((_, _, false)) => call.callee_text.split(['.', '?']).next().map(str::trim),
        Some(_) => None,
    }
    .filter(|root| super::row_calls::is_identifier(root))
}

/// How the object a callee's member is read on is written.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ObjectForm {
    /// A name (`app` in `app.get(…)`).
    Name,
    /// A member access (`this.app`, `deps.app`).
    Member,
    /// A call's value (`build()` in `build().get(…)`).
    Call,
    /// Anything else (`this`, `new X()`, a literal).
    Other,
}

/// The object the callee's member is read on: its span (discovery
/// numbering) and how it is written. `None` when the callee is no member
/// access.
pub(crate) fn receiver_object(callee: &Expr) -> Option<(u32, u32, ObjectForm)> {
    use crate::graphql_document_sites::unwrap_expression;
    let object = match unwrap_expression(callee) {
        Expr::Member(member) => &member.obj,
        Expr::OptChain(OptChainExpr { base, .. }) => match &**base {
            OptChainBase::Member(member) => &member.obj,
            OptChainBase::Call(_) => return None,
        },
        _ => return None,
    };
    let object = unwrap_expression(object);
    let form = match object {
        Expr::Ident(_) => ObjectForm::Name,
        Expr::Member(_) => ObjectForm::Member,
        Expr::Call(_) => ObjectForm::Call,
        Expr::OptChain(OptChainExpr { base, .. }) => match &**base {
            OptChainBase::Member(_) => ObjectForm::Member,
            OptChainBase::Call(_) => ObjectForm::Call,
        },
        _ => ObjectForm::Other,
    };
    let span = object.span();
    Some((span.lo.0, span.hi.0, form))
}

/// The switch for the receiver trace (carrick#2474 R1): `1` only. Off, and
/// never set by the cloud runner, so with it off every scan is byte-identical
/// to one without it.
pub const RECEIVER_TRACE_SWITCH: &str = "CARRICK_RECEIVER_TRACE";

/// Whether the receiver trace is on in this process.
pub fn receiver_trace_enabled() -> bool {
    std::env::var(RECEIVER_TRACE_SWITCH).ok().as_deref() == Some("1")
}

/// What holds the receiver a labelled call is made on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReceiverKind {
    /// A name a declaration binds that is no parameter (`const app = …`).
    Binding,
    /// A function's parameter, a callback's included.
    Parameter,
    /// A member read off another object (`this.app`).
    Field,
    /// A call's value (`build().get(…)`).
    CallResult,
    /// Anything else (`this`).
    Other,
}

/// The binding a labelled call is made on (carrick#2474 R1): the compiler
/// row at the receiver's span, as it names the receiver's declaration, and
/// what holds it. Recorded only with the receiver trace on; never part of a
/// record's state.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReceiverAt {
    #[serde(default)]
    pub symbol: Option<String>,
    #[serde(default)]
    pub decl_file: Option<String>,
    #[serde(default)]
    pub decl_line: Option<u32>,
    pub kind: ReceiverKind,
}

/// How discovery matched a labelled call to its export.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReachedBy {
    /// A name: the export's published name, an import binding, a global,
    /// or a receiver's symbol or type named like the export.
    Name,
    /// An instance one of the export's makers built, as the library-site
    /// reader holds it.
    Instance,
    /// The declaration the call's member resolves to, which a verified op
    /// claim resolves to on a receiver the claim applies to.
    Declaration,
}

/// One op a member declaration stands for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeclaredOp {
    /// The store's key for the package, as [`Labels`] keys an export.
    pub package: String,
    pub export: String,
    pub member: String,
    pub op: LibraryOp,
}

/// The verify request the receiver trace sends: each labelled op claim whose
/// member a `call` row of that package names, on every receiver the claim
/// applies to, with the export's maker claims the verifier judges an
/// instance op with. `ops` lines up with `checks`: the op each op check
/// stands for, `None` for a maker.
#[derive(Debug, Default, Clone)]
pub struct DeclarationAsk {
    pub checks: Vec<LibraryCheck>,
    pub ops: Vec<Option<DeclaredOp>>,
}

/// The receiver id the instances a maker claim builds are read through, as
/// [`LibrarySite::receiver_ids`] names a site's.
fn make_receiver(form: MakeForm, member: Option<&str>) -> String {
    super::library_sites::maker_receiver_id(form == MakeForm::New, member)
}

/// The ask for the service `inputs` reads, under the store's answer
/// `exports`. Nothing without rows, or with no member any op names.
pub fn declaration_ask(inputs: &RequestSummaryInputs, exports: &[ExportClaims]) -> DeclarationAsk {
    let mut ask = DeclarationAsk::default();
    let Some(positions) = inputs.positions.as_ref() else {
        return ask;
    };
    let called: HashSet<(String, String)> = positions
        .call_rows()
        .filter(|row| !in_repo(row))
        .filter_map(|row| Some((store_key(row, exports)?, row.symbol()?.to_string())))
        .collect();
    for export in exports
        .iter()
        .filter(|export| export.role != LibraryRole::None)
    {
        let claims = export.claims_by_id();
        let wanted: Vec<&(String, &LibraryClaim)> = claims
            .iter()
            .filter(|(_, claim)| match claim {
                LibraryClaim::Op {
                    member: Some(member),
                    ..
                } => called.contains(&(export.package.clone(), member.clone())),
                _ => false,
            })
            .collect();
        if wanted.is_empty() {
            continue;
        }
        let check = |id: &str, receiver: &str, claim: &LibraryClaim| LibraryCheck {
            claim_id: id.to_string(),
            package: export.specifier.clone(),
            export: export.export.clone(),
            role: export.role,
            receiver: receiver.to_string(),
            claim: claim.clone(),
        };
        let mut receivers = vec!["export".to_string()];
        for (id, claim) in &claims {
            if let LibraryClaim::Make { form, member, .. } = claim {
                receivers.push(make_receiver(*form, member.as_deref()));
                ask.checks.push(check(id, "export", claim));
                ask.ops.push(None);
            }
        }
        for (id, claim) in wanted {
            let LibraryClaim::Op {
                op,
                member: Some(member),
                on,
                of,
                ..
            } = claim
            else {
                continue;
            };
            let mut on_receivers = receivers.clone();
            on_receivers.extend(of.iter().cloned());
            on_receivers.sort();
            on_receivers.dedup();
            for receiver in on_receivers
                .iter()
                .filter(|receiver| applies(*on, of.as_deref(), receiver))
            {
                ask.checks.push(check(id, receiver, claim));
                ask.ops.push(Some(DeclaredOp {
                    package: export.package.clone(),
                    export: export.export.clone(),
                    member: member.clone(),
                    op: *op,
                }));
            }
        }
    }
    ask
}

/// Each member declaration a verified op claim resolves to, with the ops it
/// stands for (carrick#2474 R1), and how the ask went.
#[derive(Debug, Default, Clone)]
pub struct MemberDeclarations {
    by_declaration: HashMap<(String, u32), Vec<DeclaredOp>>,
    /// The checks asked.
    pub checks: usize,
    /// Of those, the ones the verifier did not reach in its time limit.
    pub not_reached: usize,
}

impl MemberDeclarations {
    /// The declarations the verdicts for `ask` name. A verdict counts only
    /// for the check at its own position that it names.
    pub fn of(ask: &DeclarationAsk, verdicts: &[SemanticsResult]) -> Self {
        let mut out = MemberDeclarations {
            checks: ask.checks.len(),
            not_reached: crate::services::type_sidecar::not_reached_in_time(verdicts),
            ..MemberDeclarations::default()
        };
        if verdicts.len() != ask.checks.len() {
            return out;
        }
        for ((check, op), verdict) in ask.checks.iter().zip(&ask.ops).zip(verdicts) {
            let Some(op) = op else {
                continue;
            };
            if verdict.verdict != SemanticsVerdict::Verified
                || verdict.claim_id != check.claim_id
                || verdict.receiver != check.receiver
            {
                continue;
            }
            for declared in &verdict.declarations {
                let ops = out
                    .by_declaration
                    .entry((declared.decl_file.clone(), declared.decl_line))
                    .or_default();
                if !ops.contains(op) {
                    ops.push(op.clone());
                }
            }
        }
        out
    }

    /// The ops the declaration at `decl_file:decl_line` stands for.
    fn at(&self, decl_file: &str, decl_line: u32) -> &[DeclaredOp] {
        self.by_declaration
            .get(&(decl_file.to_string(), decl_line))
            .map_or(&[], Vec::as_slice)
    }

    /// How many declarations are indexed.
    pub fn len(&self) -> usize {
        self.by_declaration.len()
    }

    pub fn is_empty(&self) -> bool {
        self.by_declaration.is_empty()
    }
}

/// The store's key for the package a row's declaration is in: the default
/// library's file, as the store keys it (`lib:lib.dom.d.ts`), a runtime module (`node:<module>`, the file
/// the runtime's types declare it in), or the package the declaring package
/// stands for ([`runtime_package_of`]).
fn store_key(row: &Row, exports: &[ExportClaims]) -> Option<String> {
    let file = row.decl_file()?;
    if file.starts_with("lib:") {
        return Some(file.to_string());
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

/// The names a row's declaration is published as, as the store keys an
/// export: `default` and `export =` for its module's whole-module target,
/// and its own name when its module exports it by that name or it is a
/// global. None for a member of a class or interface, whatever its name,
/// and none for a row that does not say (carrick#2508).
fn published_names(row: &Row) -> Vec<&str> {
    let mut names = Vec::new();
    if row.flags() & FLAG_MODULE_DEFAULT != 0 {
        names.extend(["default", "export ="]);
    }
    if row.flags() & FLAG_PUBLISHED != 0
        && let Some(symbol) = row.symbol()
        && !names.contains(&symbol)
    {
        names.push(symbol);
    }
    names
}

/// Whether a row's declaration is a file of the repository.
pub(super) fn in_repo(row: &Row) -> bool {
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
    /// With the receiver trace on: the labelled call's receiver and how it
    /// was matched.
    receiver: Option<ReceiverAt>,
    reached: Option<ReachedBy>,
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
    /// Somewhere in the code the walk read, a call could not be followed
    /// (one of [`Discovery::unfollowed`]'s reasons): a walk that found no
    /// site has not read everything the call runs (carrick#2613).
    missed: bool,
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
        self.missed |= other.missed;
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
    /// Calls whose member declaration stands for more than one op
    /// (carrick#2474 R1): no site.
    ambiguous: std::cell::Cell<usize>,
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

    /// The binding `call` is made on (carrick#2474 R1): the row at the
    /// object its member is read on, `ident` for a name, `member` for a
    /// member access, `call` for a call's value, and what holds it. A name
    /// is a parameter when a function of its declaring file binds that name
    /// at the line the row names. `None` for a call with no receiver.
    fn receiver_of(&self, file: &Path, call: &CallIr) -> Option<ReceiverAt> {
        let (lo, hi, form) = call.receiver_object?;
        let (kind, row_kind) = match form {
            ObjectForm::Name => (ReceiverKind::Binding, Some(Kind::Ident)),
            ObjectForm::Member => (ReceiverKind::Field, Some(Kind::Member)),
            ObjectForm::Call => (ReceiverKind::CallResult, Some(Kind::Call)),
            ObjectForm::Other => (ReceiverKind::Other, None),
        };
        let row = row_kind.and_then(|row_kind| {
            let positions = self.inputs.positions.as_ref()?;
            let ir = self.inputs.files.get(file)?;
            positions
                .lookup(file, ir.swc_start(lo), ir.swc_start(hi), row_kind)
                .ok()
        });
        let parameter = form == ObjectForm::Name
            && row.is_some_and(|row| {
                if in_repo(row)
                    && let (Some(symbol), Some(decl_file), Some(line)) =
                        (row.symbol(), row.decl_file(), row.decl_line())
                    && let Some(positions) = self.inputs.positions.as_ref()
                    && let Some(declared) = self.inputs.files.get(&positions.path_of(decl_file))
                {
                    declared
                        .declaration_lines
                        .params
                        .contains(&(line, symbol.to_string()))
                } else {
                    false
                }
            });
        Some(ReceiverAt {
            symbol: row.and_then(|row| row.symbol()).map(str::to_string),
            decl_file: row.and_then(|row| row.decl_file()).map(str::to_string),
            decl_line: row.and_then(|row| row.decl_line()),
            kind: if parameter {
                ReceiverKind::Parameter
            } else {
                kind
            },
        })
    }

    /// The labelled export `call` is made on, when it is one, and how it
    /// was matched:
    ///
    /// - the name the callee's package publishes the callee's declaration
    ///   as, when the row says: its own name, or `default` / `export =` for
    ///   the module's whole-module target; never a member's name;
    /// - the name the module publishes the binding the callee is read
    ///   through as (`default`, `io`, `export =`), as the store keys the
    ///   export: the compiler names the declaration (`axios`, `lookup`);
    /// - a global of the language's own library, by its name, wherever its
    ///   types declare it;
    /// - a receiver's symbol or type in the callee's package;
    /// - with the receiver trace on, a member whose declaration a verified
    ///   op claim resolves to, whatever holds the receiver
    ///   ([`MemberDeclarations`]). A declaration that stands for more than
    ///   one op is no site, and is counted.
    fn library_end(
        &self,
        file: &Path,
        call: &CallIr,
        construction: bool,
    ) -> Option<(SiteEnd, ReachedBy)> {
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
                new: construction,
            }
        };
        let package = store_key(row, self.labels.exports);
        if let Some(package) = &package {
            for export in published_names(row) {
                if let Some(claims) = self.labels.get(package, export) {
                    return Some((end(package, export, None, claims), ReachedBy::Name));
                }
            }
        }
        let ir = self.inputs.files.get(file)?;
        // The binding the callee is read through: the callee itself, or the
        // name its member chain starts at.
        let root = root_name(call);
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
                        return Some((end(&key, export, member, claims), ReachedBy::Name));
                    }
                }
            }
        }
        // `fetch(…)`, `new WebSocket(…)`: a name no binding of the file
        // holds, declared by the language's own library or the runtime's
        // types, is the language's global.
        if is_free_name(ir, call)
            && (row
                .decl_file()
                .is_some_and(|declared| declared.starts_with("lib:"))
                || row.decl_package() == Some(RUNTIME_TYPES))
            && let Some(claims) = self.labels.global(symbol)
        {
            return Some((end(&claims.package, symbol, None, claims), ReachedBy::Name));
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
                        return Some((
                            SiteEnd {
                                op: Some(op),
                                ..end(&key, export, Some(member), claims)
                            },
                            ReachedBy::Instance,
                        ));
                    }
                }
            }
        }
        if let Some(package) = &package
            && let Some(positions) = self.inputs.positions.as_ref()
        {
            for (lo, hi, member) in &call.receiver_spans {
                let kind = if *member { Kind::Member } else { Kind::Ident };
                let Ok(receiver) =
                    positions.lookup(file, ir.swc_start(*lo), ir.swc_start(*hi), kind)
                else {
                    break;
                };
                for name in [receiver.symbol(), receiver.type_text().and_then(type_root)]
                    .into_iter()
                    .flatten()
                {
                    if let Some(claims) = self.labels.get(package, name) {
                        return Some((end(package, name, Some(symbol), claims), ReachedBy::Name));
                    }
                }
            }
        }
        // `app.get(…)` on an app handed in as a parameter, held in a field
        // or returned by a call: the member's declaration is one a verified
        // op claim resolves to (carrick#2474 R1).
        let declarations = self.labels.declarations.filter(|_| !construction)?;
        let ops = declarations.at(row.decl_file()?, row.decl_line()?);
        match ops {
            [] => None,
            [one] => {
                let claims = self.labels.get(&one.package, &one.export)?;
                Some((
                    SiteEnd {
                        op: Some(one.op),
                        ..end(&one.package, &one.export, Some(&one.member), claims)
                    },
                    ReachedBy::Declaration,
                ))
            }
            _ => {
                self.ambiguous.set(self.ambiguous.get() + 1);
                None
            }
        }
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

    /// Why `call`, which reached no labelled export and entered no code, was
    /// not followed: its callee is an in-repo value that is no definition, a
    /// function outside the service, or a member of a package the answer does
    /// not match or does not hold. `None` when nothing was missed.
    fn unfollowed(&self, file: &Path, call: &CallIr, construction: bool) -> Option<&'static str> {
        match self.inputs.reach(file, call.site.lo, call.site.hi) {
            Reach::OutsideService(..) => Some(OUTSIDE_SERVICE),
            Reach::NoFunction => {
                let row = self.row(file, call)?;
                if in_repo(row) {
                    // A class built here has a constructor with no body to
                    // read; a value that is no definition is the gap.
                    return (!construction).then_some(VALUE_CALLEE);
                }
                let key = store_key(row, self.labels.exports)?;
                if self.labelled_none_export(file, call, row, &key) {
                    return None;
                }
                self.labels.package_reason(&key)
            }
            _ => None,
        }
    }

    /// Whether the store labelled the export `call` is made on with no
    /// network role: by the name its package publishes the callee's
    /// declaration as, or the name the file's binding imports it by.
    fn labelled_none_export(&self, file: &Path, call: &CallIr, row: &Row, key: &str) -> bool {
        if published_names(row)
            .into_iter()
            .any(|export| self.labels.labelled_none(key, export))
        {
            return true;
        }
        let binding =
            root_name(call).and_then(|root| self.inputs.files.get(file)?.bindings.get(root));
        binding.is_some_and(|binding| {
            specifier_keys(&binding.specifier).iter().any(|package| {
                [binding.export.as_str(), "default", "export ="]
                    .into_iter()
                    .any(|export| self.labels.labelled_none(package, export))
            })
        })
    }

    /// What `call`, written in `body` of `file`, reaches.
    fn call(&mut self, file: &Path, body: &FnIr, call: &CallIr, construction: bool) -> Outcome {
        if call.invokes_param.is_some() {
            return Outcome::stopped("injected_callee");
        }
        if let Some((end, reached)) = self.library_end(file, call, construction) {
            let traced = self.labels.declarations.is_some();
            return Outcome {
                ends: vec![Found {
                    chain: Vec::new(),
                    end,
                    receiver: traced.then(|| self.receiver_of(file, call)).flatten(),
                    reached: traced.then_some(reached),
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
            let mut inner = self.call(file, body, call, construction);
            if self.misses(file, body, call, construction, &inner) {
                inner.missed = true;
            }
            out.absorb(inner);
        }
        if !out.ends.is_empty() {
            out.stop = None;
        }
        out
    }

    /// Whether `call`, whose walk gave `outcome` and found no site, passed
    /// something it did not read (carrick#2613): a call it could not follow,
    /// or a function written at the call that no walk entered from it (a
    /// callback handed to a package's function) and that itself reaches a
    /// labelled export or stops short.
    fn misses(
        &mut self,
        file: &Path,
        body: &FnIr,
        call: &CallIr,
        construction: bool,
        outcome: &Outcome,
    ) -> bool {
        if !outcome.ends.is_empty() || outcome.stop.is_some() {
            return false;
        }
        if outcome.missed || self.unfollowed(file, call, construction).is_some() {
            return true;
        }
        call.args.iter().any(|arg| {
            let Value::Callback(nested) = arg else {
                return false;
            };
            let Some(callback) = body.nested.get(*nested) else {
                return false;
            };
            let read = self.body(file, callback);
            !read.ends.is_empty() || read.stop.is_some() || read.missed
        })
    }

    /// The records for every call `body` and the functions written in it
    /// make.
    fn sites(&mut self, file: &Path, body: &FnIr, discovery: &mut Discovery) {
        for (call, construction) in body.read_calls() {
            if call.jsx {
                continue;
            }
            let outcome = self.call(file, body, call, construction);
            let missed = self.misses(file, body, call, construction, &outcome);
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
                    receiver: None,
                    reached: None,
                };
            if outcome.ends.is_empty() {
                let Some((chain, stop)) = outcome.stop else {
                    match self.unfollowed(file, call, construction) {
                        Some(reason) => {
                            *discovery
                                .counts
                                .entry(format!("unfollowed/{reason}"))
                                .or_default() += 1;
                            discovery.unfollowed.push(record(
                                None,
                                Vec::new(),
                                Some(reason.to_string()),
                            ));
                        }
                        None => {
                            *discovery.counts.entry("no_site".to_string()).or_default() += 1;
                            let at = (self.rel(file), call.site.span_start, call.site.span_end);
                            if missed {
                                discovery.missed_inside.push(at);
                            } else {
                                discovery.finished.push(at);
                            }
                        }
                    }
                    continue;
                };
                // A call whose own row says nothing: the walk never entered
                // code. Listed with the hole the row names.
                if chain.is_empty() && stop != "injected_callee" && stop != "interface_member" {
                    *discovery
                        .counts
                        .entry(format!("unread/{stop}"))
                        .or_default() += 1;
                    discovery.unfollowed.push(record(None, chain, Some(stop)));
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
                discovery.sites.push(DiscoverySite {
                    receiver: found.receiver,
                    reached: found.reached,
                    ..record(Some(found.end), found.chain, None)
                });
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
        ambiguous: std::cell::Cell::new(0),
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
    let ambiguous = walker.ambiguous.get();
    if ambiguous > 0 {
        discovery
            .counts
            .insert("declaration_ambiguous".to_string(), ambiguous);
    }
    discovery
        .sites
        .sort_by(|a, b| sort_key(a).cmp(&sort_key(b)));
    discovery.sites.dedup();
    discovery
        .unfollowed
        .sort_by(|a, b| sort_key(a).cmp(&sort_key(b)));
    discovery.unfollowed.dedup();
    discovery.finished.sort();
    discovery.finished.dedup();
    discovery.missed_inside.sort();
    discovery.missed_inside.dedup();
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

    /// carrick#2504: a declaration in a default-library file is keyed as the
    /// store keys the language's library, `lib:<file>`, not by the file's
    /// stem.
    #[test]
    fn a_default_library_declaration_is_keyed_lib_colon_file() {
        let row = |decl: &str| {
            Row(
                crate::position_rows::Kind::Call,
                0,
                5,
                Some("fetch".into()),
                Some(decl.into()),
                Some(1),
                None,
                None,
                0,
            )
        };
        assert_eq!(
            store_key(&row("lib:lib.dom.d.ts"), &[]),
            Some("lib:lib.dom.d.ts".to_string())
        );
        assert_eq!(
            store_key(&row("lib:lib.es2015.promise.d.ts"), &[]),
            Some("lib:lib.es2015.promise.d.ts".to_string())
        );
    }

    fn fixture(name: &str) -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/discovery")
            .join(name)
    }

    fn answer() -> LibraryAnswer {
        let text = std::fs::read_to_string(fixture("library-claims.json")).unwrap();
        let answer = crate::library_store::parse_answer(&text);
        assert_eq!(answer.dropped, 0, "every fixture claim parses");
        answer
    }

    fn claims() -> Vec<ExportClaims> {
        answer().exports
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
        if got["sites"] != expected["sites"] {
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

    /// carrick#2613 (T2): a call that reached nothing and entered no code says
    /// why, on `unfollowed`: a function a factory returns, an in-repo
    /// function outside the service, a member of a package the store answered
    /// that none of its labels matched, and a call into a package the answer
    /// holds nothing for. A body that calls only local helpers and the
    /// language's own library is `no_site`, and no record of any kind. The
    /// answer key is the fixture's `unfollowed` list.
    #[test]
    fn every_call_it_cannot_follow_says_why() {
        let Some(on) = walked("discovery", "services/web", "web", &[], true) else {
            return;
        };
        let answer = answer();
        let discovery = discover(&on.inputs, &Labels::of_answer(&answer), &on.repo);
        let expected: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(fixture("sites.expected.json")).unwrap())
                .unwrap();
        assert_eq!(
            serde_json::to_value(&discovery.unfollowed).unwrap(),
            expected["unfollowed"],
            "{}",
            serde_json::to_string(&discovery.unfollowed).unwrap()
        );
        for file in ["unfollowed.ts", "passed-in.ts"] {
            let in_file = |records: &[DiscoverySite]| {
                records
                    .iter()
                    .filter(|site| site.file.ends_with(file))
                    .map(|site| format!("{} {}", site.line, site.callee))
                    .collect::<Vec<_>>()
            };
            let sited = in_file(&discovery.sites);
            let unlisted = in_file(&discovery.unfollowed);
            assert!(
                sited.iter().all(|record| !unlisted.contains(record)),
                "no call is both a site and unfollowed: {sited:?} {unlisted:?}"
            );
        }
        assert!(
            !discovery
                .unfollowed
                .iter()
                .chain(&discovery.sites)
                .any(|site| site.file.ends_with("unfollowed.ts") && site.callee == "double"),
            "a call to a local helper that finds nothing is no_site"
        );
        assert!(
            discovery.counts.get("no_site").is_some_and(|n| *n > 0),
            "{:?}",
            discovery.counts
        );
        // Labels that know only the exports cannot tell a package the store
        // did not answer from one it answered with nothing to label.
        let blind = discover(&on.inputs, &Labels::of(&answer.exports), &on.repo);
        assert!(
            blind
                .unfollowed
                .iter()
                .all(|site| site.stop.as_deref() != Some(UNANSWERED_PACKAGE))
        );
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

    /// The store's answer for the export-shape cases: a package with a
    /// top-level `json` export, and an `export =` factory's default.
    const EXPORT_SHAPE_CLAIMS: &str = r#"{
  "library_claims": [
    {
      "package": "fixture-rest", "version": "1.0.0", "status": "answered",
      "exports": [
        { "specifier": "fixture-rest", "export": "json", "role": "server_framework", "side": "server",
          "makes": [], "scopes": [], "ops": [], "reserved": [], "calls": [], "patterns": [] }
      ]
    },
    {
      "package": "fixture-make", "version": "1.0.0", "status": "answered",
      "exports": [
        { "specifier": "fixture-make", "export": "default", "role": "server_framework", "side": "server",
          "makes": [ { "form": "call", "member": null, "picker": "fixture-picker" } ], "scopes": [],
          "ops": [], "reserved": [], "calls": [], "patterns": [] }
      ]
    }
  ]
}"#;

    /// carrick#2508: a declaration is matched by name only when its package
    /// publishes it under that name. An interface's `json` method is not the
    /// package's top-level `json` export, however the call is written; the
    /// top-level export is, imported directly or through a local barrel. An
    /// `export =` factory re-exported by a local barrel as a named binding,
    /// so that no import names the package, is the package's `default`.
    #[test]
    fn a_member_never_matches_an_export_by_name_and_a_barrel_default_is_the_default() {
        let edit = [
            (
                "node_modules/fixture-rest/package.json",
                r#"{ "name": "fixture-rest", "version": "1.0.0", "types": "index.d.ts" }"#,
            ),
            (
                "node_modules/fixture-rest/index.d.ts",
                "export interface Res {\n  json(body: unknown): void;\n}\nexport declare function json(): (req: unknown, res: Res) => void;\n",
            ),
            (
                "node_modules/fixture-make/package.json",
                r#"{ "name": "fixture-make", "version": "1.0.0", "types": "index.d.ts" }"#,
            ),
            (
                "node_modules/fixture-make/index.d.ts",
                "interface App {\n  post(path: string, handler: () => void): void;\n}\ndeclare function make(): App;\nexport = make;\n",
            ),
            (
                "src/rest-barrel.ts",
                "export { json as parse } from \"fixture-rest\";\nexport { default as makeApp } from \"fixture-make\";\n",
            ),
            (
                "src/rest.ts",
                "import { json, type Res } from \"fixture-rest\";\nimport { makeApp, parse } from \"./rest-barrel\";\n\nexport function reply(res: Res): void {\n  res.json({});\n}\n\nexport function parsers(): void {\n  json();\n  parse();\n}\n\nexport function boot(): void {\n  makeApp();\n}\n",
            ),
        ];
        let Some(on) = walked("discovery", "services/web", "web", &edit, true) else {
            return;
        };
        let mut exports = claims();
        let added = crate::library_store::parse_answer(EXPORT_SHAPE_CLAIMS);
        assert_eq!(added.dropped, 0, "every added claim parses");
        exports.extend(added.exports);
        let discovery = discover(&on.inputs, &Labels::of(&exports), &on.repo);
        assert_eq!(
            records_in(&discovery, "services/web/src/rest.ts"),
            texts(&[
                "9 json -> fixture-rest json -",
                "10 parse -> fixture-rest json -",
                "14 makeApp -> fixture-make default -",
            ])
        );
    }

    /// carrick#2508: the publication bits are flag bits. A row with neither
    /// reads as a declaration nothing says is published: no name to match it
    /// by.
    #[test]
    fn a_row_without_the_publication_bits_reads_and_names_nothing() {
        let text = concat!(
            r#"{"v":4,"typescript":"5.9.3","service":"api","files":1}"#,
            "\n",
            r#"{"f":"src/a.ts","h":"00","r":[["call",0,7,"greet","node_modules/p/index.d.ts",1,"p","string",0],["call",9,16,"make","node_modules/p/index.d.ts",3,"p","App",96]]}"#,
            "\n"
        );
        let rows = crate::position_rows::read_rows(text.as_bytes()).expect("an old row reads");
        let [old, new] = rows.files[0].r.as_slice() else {
            panic!("two rows");
        };
        assert!(published_names(old).is_empty());
        assert_eq!(published_names(new), vec!["default", "export =", "make"]);
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

    /// The receiver-trace fixture (carrick#2474 R1), read with rows, its
    /// store answer, and the member declarations the real verifier returns
    /// for the trace's ask.
    fn traced_fixture() -> Option<(
        super::super::fixture_walk::Walked,
        Vec<ExportClaims>,
        MemberDeclarations,
    )> {
        let on = walked("receiver-trace", ".", "web", &[], true)?;
        let text = std::fs::read_to_string(
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("tests/fixtures/receiver-trace/library-claims.json"),
        )
        .unwrap();
        let answer = crate::library_store::parse_answer(&text);
        assert_eq!(answer.dropped, 0, "every fixture claim parses");
        let exports = answer.exports;
        let ask = declaration_ask(&on.inputs, &exports);
        let script = super::super::fixture_walk::sidecar()?;
        let sidecar = crate::services::TypeSidecar::spawn(&script).expect("spawn the sidecar");
        sidecar.start_init(&on.repo, None);
        sidecar
            .wait_ready(std::time::Duration::from_secs(120))
            .expect("the sidecar is ready");
        let verdicts = sidecar
            .verify_library_claims(&on.repo, &ask.checks)
            .expect("the verifier answers")
            .verdicts;
        let declarations = MemberDeclarations::of(&ask, &verdicts);
        Some((on, exports, declarations))
    }

    /// Each site as `file:line callee -> export member op reached receiver`.
    fn traced_records(discovery: &Discovery) -> Vec<String> {
        discovery
            .sites
            .iter()
            .filter_map(|site| {
                let end = site.end.as_ref()?;
                let op = end
                    .op
                    .map(|op| {
                        serde_json::to_value(op)
                            .unwrap()
                            .as_str()
                            .unwrap()
                            .to_string()
                    })
                    .unwrap_or_else(|| "-".to_string());
                let receiver = site.receiver.as_ref().map_or("-".to_string(), |at| {
                    format!(
                        "{}:{}@{}:{}",
                        serde_json::to_value(at.kind).unwrap().as_str().unwrap(),
                        at.symbol.as_deref().unwrap_or("-"),
                        at.decl_file.as_deref().unwrap_or("-"),
                        at.decl_line.unwrap_or_default()
                    )
                });
                let reached = site.reached.map_or("-".to_string(), |by| {
                    serde_json::to_value(by)
                        .unwrap()
                        .as_str()
                        .unwrap()
                        .to_string()
                });
                Some(format!(
                    "{}:{} {} -> {} {} {} {} {}",
                    site.file,
                    site.line,
                    site.callee,
                    end.export,
                    end.member.as_deref().unwrap_or("-"),
                    op,
                    reached,
                    receiver
                ))
            })
            .collect()
    }

    /// carrick#2474 R1: with the receiver trace on, a call whose member's
    /// declaration a verified op claim resolves to is a site with that op,
    /// whatever holds the receiver: a parameter (b), a callback's parameter
    /// (c), a field set from a constructor parameter (d), a call's value
    /// (e), and an HTTP client handed in as a parameter. The control (a)
    /// is found as before, by its instance. The request's own `get` (f), an
    /// in-repo class (g), an untyped receiver (h) and a local structural
    /// copy of the app's type (i) are no site. Each site names its
    /// receiver.
    #[test]
    fn a_call_on_a_labelled_member_s_declaration_is_a_site_whatever_holds_it() {
        let Some((on, exports, declarations)) = traced_fixture() else {
            return;
        };
        assert!(!declarations.is_empty(), "the verifier names declarations");
        let discovery = discover(
            &on.inputs,
            &Labels::traced(&exports, &declarations),
            &on.repo,
        );
        let got = traced_records(&discovery);
        // A call into a function that reaches a labelled call is a site too
        // (`users(app)`), naming the labelled call's receiver. The factory's
        // result (e) is an instance the library-site reader already holds.
        let want = texts(&[
            "src/app.ts:12 make -> default - - name -",
            "src/app.ts:13 app.get -> default get receive instance binding:app@src/app.ts:12",
            "src/app.ts:16 users -> default get receive declaration parameter:app@src/users.ts:7",
            "src/app.ts:19 withApp -> default post receive declaration parameter:inner@src/app.ts:19",
            "src/app.ts:19 withApp -> default - - name -",
            "src/app.ts:19 inner.post -> default post receive declaration parameter:inner@src/app.ts:19",
            "src/app.ts:22 new Routes(app).mount -> default get receive declaration field:app@src/routes.ts:8",
            "src/app.ts:25 build -> default - - name -",
            "src/app.ts:25 build().get -> default get receive instance call_result:build@src/build.ts:3",
            "src/build.ts:4 make -> default - - name -",
            "src/client.ts:5 client.get -> default get request declaration parameter:client@src/client.ts:4",
            // carrick#2474 R2a's instance shapes.
            "src/helpers.ts:10 app.get -> default get receive declaration parameter:app@src/helpers.ts:9",
            "src/helpers.ts:15 app.get -> default get receive declaration parameter:app@src/helpers.ts:14",
            "src/helpers.ts:18 three -> default get receive declaration parameter:app@src/helpers.ts:14",
            "src/helpers.ts:21 two -> default get receive declaration parameter:app@src/helpers.ts:14",
            "src/helpers.ts:27 pong -> default get receive declaration parameter:app@src/helpers.ts:25",
            "src/helpers.ts:29 app.get -> default get receive declaration parameter:app@src/helpers.ts:25",
            "src/helpers.ts:32 ping -> default get receive declaration parameter:app@src/helpers.ts:25",
            "src/helpers.ts:37 app.get -> default get receive declaration parameter:app@src/helpers.ts:36",
            "src/helpers.ts:42 deps.app.get -> default get receive declaration field:app@src/helpers.ts:41",
            "src/helpers.ts:47 scoped.get -> default get receive declaration parameter:scoped@src/helpers.ts:46",
            "src/helpers.ts:51 make -> default - - name -",
            "src/helpers.ts:52 make -> default - - name -",
            "src/helpers.ts:53 health -> default get receive declaration parameter:app@src/helpers.ts:9",
            "src/helpers.ts:54 health -> default get receive declaration parameter:app@src/helpers.ts:9",
            "src/helpers.ts:55 health -> default get receive declaration parameter:app@src/helpers.ts:9",
            "src/helpers.ts:55 build -> default - - name -",
            "src/helpers.ts:56 health -> default get receive declaration parameter:app@src/helpers.ts:9",
            "src/helpers.ts:57 one -> default get receive declaration parameter:app@src/helpers.ts:14",
            "src/helpers.ts:58 ping -> default get receive declaration parameter:app@src/helpers.ts:25",
            "src/helpers.ts:59 keyed -> default get receive declaration field:app@src/helpers.ts:41",
            "src/helpers.ts:61 inner.get -> default get receive declaration parameter:inner@src/helpers.ts:61",
            "src/helpers.ts:62 health -> default get receive declaration parameter:app@src/helpers.ts:9",
            "src/helpers.ts:62 build -> default - - name -",
            "src/routes.ts:11 this.app.get -> default get receive declaration field:app@src/routes.ts:8",
            "src/users.ts:8 app.get -> default get receive declaration parameter:app@src/users.ts:7",
            "src/with-app.ts:4 make -> default - - name -",
        ]);
        assert_eq!(got, want, "{got:#?}");
        for line in [
            "negatives.ts:5",
            "negatives.ts:15",
            "negatives.ts:20",
            "negatives.ts:28",
        ] {
            assert!(
                !got.iter().any(|record| record.contains(line)),
                "{line} is no site"
            );
        }
        assert!(
            discovery
                .counts
                .keys()
                .any(|key| key.ends_with("untyped_receiver")),
            "the untyped receiver is a hole: {:?}",
            discovery.counts
        );
    }

    /// carrick#2474 R1: with the trace off, discovery on the fixture is what
    /// it was: the trace's own records add only sites and fields, and every
    /// site found without it is the same record, with no receiver. A walk
    /// that stopped with nothing found (a cycle through a helper whose
    /// receiver is a parameter) may find the labelled call with it on.
    #[test]
    fn with_the_trace_off_discovery_is_unchanged() {
        let Some((on, exports, declarations)) = traced_fixture() else {
            return;
        };
        let off = discover(&on.inputs, &Labels::of(&exports), &on.repo);
        let traced = discover(
            &on.inputs,
            &Labels::traced(&exports, &declarations),
            &on.repo,
        );
        let text = serde_json::to_string(&off.sites).unwrap();
        assert!(!text.contains("\"receiver\"") && !text.contains("\"reached\""));
        let plain: Vec<DiscoverySite> = traced
            .sites
            .iter()
            .cloned()
            .map(|site| DiscoverySite {
                receiver: None,
                reached: None,
                ..site
            })
            .collect();
        for site in off.sites.iter().filter(|site| site.end.is_some()) {
            assert!(plain.contains(site), "{site:?} is kept with the trace on");
        }
        assert_eq!(
            off.sites
                .iter()
                .filter(|site| site.end.is_some())
                .map(|site| format!("{}:{}", site.file, site.line))
                .collect::<Vec<_>>(),
            texts(&[
                "src/app.ts:12",
                "src/app.ts:13",
                "src/app.ts:19",
                "src/app.ts:25",
                "src/app.ts:25",
                "src/build.ts:4",
                "src/helpers.ts:51",
                "src/helpers.ts:52",
                "src/helpers.ts:55",
                "src/helpers.ts:62",
                "src/with-app.ts:4"
            ]),
            "the trace's sites are not found without it"
        );
    }

    /// The decided sites and records of `discovery`.
    fn records_of(
        on: &super::super::fixture_walk::Walked,
        exports: &[ExportClaims],
        discovery: &Discovery,
    ) -> (
        super::super::decided_sites::Decided,
        Vec<super::super::site_record::SiteRecord>,
    ) {
        let discovered: Vec<super::super::decided_sites::DiscoverySite> = discovery
            .sites
            .iter()
            .map(|site| serde_json::from_value(serde_json::to_value(site).unwrap()).unwrap())
            .collect();
        let labels = super::super::decided_sites::SiteLabel::from_exports(exports);
        let decided =
            super::super::decided_sites::decided_sites(&on.inputs, &on.repo, &discovered, &labels);
        let records =
            super::super::site_record::site_records(&on.inputs, &on.repo, &decided, &labels);
        (decided, records)
    }

    /// carrick#2474 R1: each decided site and record names the binding its
    /// labelled call is made on, wherever its name is decided; with the
    /// trace off none does. Every record found without the trace keeps its
    /// state byte for byte with it on, so no site's sha moves.
    #[test]
    fn decided_sites_and_records_name_their_receiver_and_keep_today_s_state() {
        let Some((on, exports, declarations)) = traced_fixture() else {
            return;
        };
        let off = discover(&on.inputs, &Labels::of(&exports), &on.repo);
        let traced = discover(
            &on.inputs,
            &Labels::traced(&exports, &declarations),
            &on.repo,
        );
        let (decided_off, records_off) = records_of(&on, &exports, &off);
        let (decided_on, records_on) = records_of(&on, &exports, &traced);
        assert!(decided_off.sites.iter().all(|site| site.receiver.is_none()));
        assert!(records_off.iter().all(|record| record.receiver.is_none()));
        let kind_at = |file: &str, line: u32| {
            records_on
                .iter()
                .find(|record| record.anchors.call.file == file && record.anchors.call.line == line)
                .and_then(|record| record.receiver.as_ref())
                .map(|receiver| receiver.kind)
        };
        assert_eq!(kind_at("src/users.ts", 8), Some(ReceiverKind::Parameter));
        assert_eq!(kind_at("src/routes.ts", 11), Some(ReceiverKind::Field));
        assert_eq!(kind_at("src/app.ts", 19), Some(ReceiverKind::Parameter));
        assert_eq!(kind_at("src/app.ts", 25), Some(ReceiverKind::CallResult));
        assert_eq!(kind_at("src/app.ts", 13), Some(ReceiverKind::Binding));
        assert!(
            decided_on
                .sites
                .iter()
                .filter(|site| site.end.op.as_deref() == Some("receive"))
                .all(|site| site.receiver.is_some())
        );
        assert!(!records_off.is_empty());
        for record in &records_off {
            let same = records_on
                .iter()
                .find(|other| other.anchors == record.anchors)
                .expect("a record found without the trace is found with it");
            assert_eq!(
                serde_json::to_string(&same.state).unwrap(),
                serde_json::to_string(&record.state).unwrap()
            );
        }
    }

    /// Each site of `decided`, with the receiver trace's instance
    /// walk, as `file:line <- at root|hole …`, one line per instance end.
    fn instance_ends(decided: &super::super::decided_sites::Decided) -> Vec<String> {
        use super::super::decided_sites::InstanceAt;
        let mut out = Vec::new();
        for site in &decided.sites {
            for end in &site.instances {
                let what = match &end.end {
                    InstanceAt::Root { file, line, .. } => format!("root {file}:{line}"),
                    InstanceAt::Hole { reason, .. } => serde_json::to_value(reason)
                        .unwrap()
                        .as_str()
                        .unwrap()
                        .to_string(),
                };
                out.push(format!("{}:{} <- {} {what}", site.file, site.line, end.at));
            }
        }
        out.sort();
        out
    }

    /// carrick#2474 R2a: each decided site names the instance its labelled
    /// call is made on, one end per caller that hands one in. A receiver
    /// that is a parameter, or a key of one, is filled at each caller and
    /// walked up through helpers to the labelled maker whose result it is:
    /// held in a binding (a module's or a function's), built by an own
    /// factory in another module (called twice, two ends), or one helper
    /// handed two apps (two roots). A cycle ends, giving the root its other
    /// caller hands in.
    /// The holes: a function nothing calls (`entry_point`), a callback
    /// this code runs (`callback_parameter`), a function a library call
    /// runs, by name or written in place (`runs_by_library`), an app a
    /// package declares (`outside_service`), and a field (`unread`).
    #[test]
    fn each_deciding_caller_gives_its_own_instance_end() {
        let Some((on, exports, declarations)) = traced_fixture() else {
            return;
        };
        let discovery = discover(
            &on.inputs,
            &Labels::traced(&exports, &declarations),
            &on.repo,
        );
        use super::super::decided_sites::{MakerLabel, SiteLabel, decided_sites_traced};
        let discovered: Vec<super::super::decided_sites::DiscoverySite> = discovery
            .sites
            .iter()
            .map(|site| serde_json::from_value(serde_json::to_value(site).unwrap()).unwrap())
            .collect();
        let labels = SiteLabel::from_exports(&exports);
        let decided = decided_sites_traced(
            &on.inputs,
            &on.repo,
            &discovered,
            &labels,
            &MakerLabel::from_exports(&exports),
        );
        let got = instance_ends(&decided);
        let want = texts(&[
            "src/app.ts:13 <- src/app.ts:13 root src/app.ts:12",
            "src/app.ts:19 <- src/app.ts:19 callback_parameter",
            "src/app.ts:25 <- src/app.ts:25 root src/build.ts:4",
            "src/client.ts:5 <- src/client.ts:5 entry_point",
            "src/helpers.ts:10 <- src/helpers.ts:53 root src/helpers.ts:51",
            "src/helpers.ts:10 <- src/helpers.ts:54 root src/helpers.ts:52",
            "src/helpers.ts:10 <- src/helpers.ts:55 root src/build.ts:4",
            "src/helpers.ts:10 <- src/helpers.ts:56 outside_service",
            "src/helpers.ts:10 <- src/helpers.ts:62 root src/build.ts:4",
            "src/helpers.ts:15 <- src/helpers.ts:57 root src/helpers.ts:51",
            "src/helpers.ts:29 <- src/helpers.ts:58 root src/helpers.ts:51",
            "src/helpers.ts:37 <- src/helpers.ts:37 entry_point",
            "src/helpers.ts:42 <- src/helpers.ts:59 root src/helpers.ts:51",
            "src/helpers.ts:47 <- src/helpers.ts:47 runs_by_library",
            "src/helpers.ts:61 <- src/helpers.ts:61 runs_by_library",
            "src/routes.ts:11 <- src/routes.ts:11 unread",
            "src/users.ts:8 <- src/app.ts:16 root src/app.ts:12",
        ]);
        assert_eq!(got, want, "{got:#?}");
        // Without the makers, no site names an instance; with them, every
        // record keeps its state byte for byte, so no site's sha moves.
        let (untraced, records) = records_of(&on, &exports, &discovery);
        assert!(untraced.sites.iter().all(|site| site.instances.is_empty()));
        let traced =
            super::super::site_record::site_records(&on.inputs, &on.repo, &decided, &labels);
        assert_eq!(
            serde_json::to_string(
                &traced
                    .iter()
                    .map(|record| &record.state)
                    .collect::<Vec<_>>()
            )
            .unwrap(),
            serde_json::to_string(
                &records
                    .iter()
                    .map(|record| &record.state)
                    .collect::<Vec<_>>()
            )
            .unwrap()
        );
    }

    /// carrick#2474 R1: the ask holds each export's maker on `export` and
    /// each op whose member a call row names, on every receiver its `of`
    /// or `on` applies to, whether or not a library site uses it.
    #[test]
    fn the_declaration_ask_names_each_called_op_on_its_receivers() {
        let Some(on) = walked("receiver-trace", ".", "web", &[], true) else {
            return;
        };
        let text = std::fs::read_to_string(
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("tests/fixtures/receiver-trace/library-claims.json"),
        )
        .unwrap();
        let exports = crate::library_store::parse_answer(&text).exports;
        let ask = declaration_ask(&on.inputs, &exports);
        let got: Vec<String> = ask
            .checks
            .iter()
            .map(|check| format!("{} {}", check.claim_id, check.receiver))
            .collect();
        assert_eq!(
            got,
            texts(&[
                "fixture-server@1:default:make:call:() export",
                "fixture-server@1:default:op:receive:get:of:instance:() instance:()",
                "fixture-server@1:default:op:receive:post:of:instance:() instance:()",
                "fixture-fetcher@1:default:make:call:() export",
                "fixture-fetcher@1:default:op:request:get:of:instance:() instance:()",
            ]),
            "{got:#?}"
        );
        assert_eq!(ask.ops.iter().filter(|op| op.is_some()).count(), 3);
    }

    /// The offline census over flag-on rows: every site per repo, by role,
    /// with the stops, written as JSON.
    ///
    /// `CARRICK_DISCOVERY_CENSUS='<label>=<repo>=<positions dir>=<store
    /// answer>[;…]' [CARRICK_DISCOVERY_CENSUS_OUT=<file>] cargo test --lib
    /// discovery_census -- --ignored --nocapture`
    ///
    /// With `CARRICK_RECEIVER_TRACE=1` the sites are discovered with the
    /// receiver trace (carrick#2474 R1): the built sidecar verifies each
    /// service's declaration ask from the service's root, as a scan does,
    /// and every site names its receiver and how it was reached.
    #[test]
    #[ignore]
    fn discovery_census() {
        let Ok(spec) = std::env::var("CARRICK_DISCOVERY_CENSUS") else {
            return;
        };
        let script = receiver_trace_enabled()
            .then(|| super::super::fixture_walk::sidecar().expect("the built sidecar"));
        let mut all: BTreeMap<String, serde_json::Value> = BTreeMap::new();
        for entry in spec.split(';').filter(|entry| !entry.is_empty()) {
            let parts: Vec<&str> = entry.splitn(4, '=').collect();
            let [label, root, positions, answer] = parts.as_slice() else {
                panic!("<label>=<repo>=<positions dir>=<store answer>: {entry}");
            };
            let answer = crate::library_store::parse_answer(
                &std::fs::read_to_string(answer).expect("the store answer reads"),
            );
            let labels = Labels::of_answer(&answer);
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
            let mut unfollowed: Vec<DiscoverySite> = Vec::new();
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
                let declarations = script.as_ref().map(|script| {
                    let service_root = root.join(service.directory.as_deref().unwrap_or("."));
                    let sidecar =
                        crate::services::TypeSidecar::spawn(script).expect("spawn the sidecar");
                    sidecar.set_scan_root(&root);
                    sidecar.start_init(&service_root, None);
                    sidecar
                        .wait_ready(std::time::Duration::from_secs(900))
                        .expect("the sidecar is ready");
                    let ask = declaration_ask(&inputs, &answer.exports);
                    let verdicts = sidecar
                        .verify_library_claims(&service_root, &ask.checks)
                        .map(|answer| answer.verdicts)
                        .unwrap_or_default();
                    let declarations = MemberDeclarations::of(&ask, &verdicts);
                    crate::outln!(
                        "  {name}: {} check(s), {} not reached, {} declaration(s)",
                        declarations.checks,
                        declarations.not_reached,
                        declarations.len()
                    );
                    declarations
                });
                let traced;
                let labels = match &declarations {
                    Some(declarations) => {
                        traced = Labels {
                            declarations: Some(declarations),
                            ..Labels::of_answer(&answer)
                        };
                        &traced
                    }
                    None => &labels,
                };
                let discovery = discover(&inputs, labels, &root);
                for (key, count) in discovery.counts {
                    *counts.entry(key).or_default() += count;
                }
                sites.extend(discovery.sites);
                unfollowed.extend(discovery.unfollowed);
            }
            crate::outln!("== {label}: {} record(s)", sites.len());
            for (key, count) in &counts {
                crate::outln!("  {key}: {count}");
            }
            all.insert(
                label.to_string(),
                serde_json::json!({ "counts": counts, "sites": sites, "unfollowed": unfollowed }),
            );
        }
        if let Ok(out) = std::env::var("CARRICK_DISCOVERY_CENSUS_OUT") {
            std::fs::write(&out, serde_json::to_string(&all).unwrap()).expect("the census writes");
        }
    }

    /// The library-store request each service of a repo sends with site
    /// questions on, as a scan builds it: the packages its library sites
    /// import, at their installed versions, and the default-library files
    /// the rows name. Offline, for answering the census from a store copy.
    ///
    /// `CARRICK_LIBRARY_ASK_CENSUS='<label>=<repo>=<positions dir>[;…]'
    /// CARRICK_LIBRARY_ASK_CENSUS_OUT=<file> cargo test --lib
    /// library_ask_census -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn library_ask_census() {
        let Ok(spec) = std::env::var("CARRICK_LIBRARY_ASK_CENSUS") else {
            return;
        };
        let mut all: BTreeMap<String, BTreeMap<String, serde_json::Value>> = BTreeMap::new();
        for entry in spec.split(';').filter(|entry| !entry.is_empty()) {
            let parts: Vec<&str> = entry.splitn(3, '=').collect();
            let [label, root, positions] = parts.as_slice() else {
                panic!("<label>=<repo>=<positions dir>: {entry}");
            };
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
                let service_root = root.join(service.directory.as_deref().unwrap_or("."));
                let specifiers: BTreeSet<String> = super::super::library_sites(&inputs)
                    .packages()
                    .into_values()
                    .flatten()
                    .collect();
                let asked = crate::library_store::request(
                    &specifiers,
                    &crate::library_store::Install {
                        service_root: &service_root,
                        repo_root: &root,
                        home: None,
                        yarn_registry_env: None,
                        npm_registry_env: None,
                    },
                );
                let library = inputs
                    .positions
                    .as_ref()
                    .and_then(|positions| positions.default_library_globals())
                    .map(|(typescript, globals)| {
                        crate::library_store::default_library_packages(typescript, globals)
                    })
                    .unwrap_or_default();
                let request = crate::library_store::Asked::with_default_library(asked, library)
                    .map(|asked| {
                        asked
                            .with_roles(crate::library_store::roles_for(true))
                            .request
                    });
                all.entry(label.to_string())
                    .or_default()
                    .insert(name, serde_json::to_value(request).unwrap());
            }
        }
        let out = std::env::var("CARRICK_LIBRARY_ASK_CENSUS_OUT").expect("an out file");
        std::fs::write(out, serde_json::to_string_pretty(&all).unwrap()).expect("the asks write");
    }
}
