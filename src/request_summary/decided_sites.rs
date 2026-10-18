//! Where a discovered call's name is decided (carrick#2433, part of
//! carrick#2441): one request is one site, at the call that decides its path
//! or name, for every protocol.
//!
//! Discovery (carrick#2431) lists every call that reaches a labelled library
//! export, each with its chain down to the labelled call. Most of those calls
//! are the same request seen one level higher: a page calling a helper that
//! calls `fetch`. The rule is the request summaries' (a row stands at "the
//! site that fills the last hole a wrapper leaves open",
//! [`crate::agents::file_analyzer_agent::ResolutionSource::RequestSummary`]),
//! applied to the name slot of every protocol:
//!
//! - **The slot** is the argument, or one option key of it, the label of the
//!   labelled export names ([`ArgSlot`], the store's op `name`): the path or
//!   URL for HTTP, the event, topic, channel, job or message name otherwise.
//!   A label that names no slot (a channel that sends whatever object it is
//!   handed) leaves the labelled call itself as the site.
//! - **Composed down the chain.** The slot's value at the labelled call is
//!   kept as the summaries keep a URL ([`Piece`]): literal text, the
//!   enclosing function's parameters, and values this pass does not read.
//!   Each hop up fills the parameters with the arguments its call writes.
//! - **The reported site** is the lowest call whose value holds no
//!   parameter piece. A helper called from N sites with N literals gives N
//!   sites. An environment read or a base is an opaque piece, not a
//!   parameter: the environment decides it, not a caller.
//! - **A slot that keeps a parameter up to an entry point** (a function
//!   nothing calls) is a lead, `name_never_bound`. A parameter of a callback
//!   whose own caller is not this code abstains (`callback_parameter`): what
//!   fills it is whatever runs the callback.
//! - **A handler table** registered in a loop (`for (const [name, handler]
//!   of Object.entries(table)) bus.on(name, handler)` over a module constant
//!   object) gives one site per key, at the line of the function that
//!   handles it ([`loop_table`]).
//!
//! Discovery's chain is consumed as it is: the site's call, the call inside
//! each function the chain enters that calls the next, and the labelled
//! call. Only where a chain cannot be placed (a hop into a callback, two
//! calls of the next function in one body) or stops short (its top call
//! still holds a parameter and no listed site calls into its function) does
//! this pass walk up the call graph itself ([`Walk::up`]).
//!
//! Each site also says where its payload is bound ([`PayloadBinding`],
//! carrick#2472 C3), so a row the fact layer adds can be typed at its own
//! call.
//!
//! Record-only: nothing in a scan reads it (carrick#2441 lane 1). It reads
//! the request summaries' inputs and changes none of them, so no summary row
//! moves.

use std::collections::{BTreeSet, HashMap, HashSet};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use swc_common::{SourceMap, Span, Spanned, sync::Lrc};
use swc_ecma_ast::*;
use swc_ecma_visit::{Visit, VisitWith};

use super::{CallIr, FnIr, Piece, RequestSummaryInputs, Value, concat, render_target};

/// The labelled call a discovery site reaches: the store key of the package
/// (`lib:lib.dom.d.ts` for the language's own library), the export and member, the
/// store role and op, and where the call is.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct LabelledEnd {
    pub package: String,
    pub export: String,
    #[serde(default)]
    pub member: Option<String>,
    pub role: String,
    #[serde(default)]
    pub op: Option<String>,
    pub file: String,
    pub line: u32,
    pub span_start: u32,
    pub span_end: u32,
}

impl LabelledEnd {
    /// `file:line`.
    pub fn at(&self) -> String {
        format!("{}:{}", self.file, self.line)
    }
}

/// One function a discovery chain enters: its definition key, file and
/// line. `<callback>` is a function written at the call and run by the
/// callee.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChainHop {
    pub function: String,
    pub file: String,
    pub line: u32,
}

/// One discovery site (carrick#2431's record): a call whose callee reaches
/// a labelled export, with the chain down to it, outermost first. Files are
/// repo-relative; spans are SWC's in the file parsed on its own.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DiscoverySite {
    pub file: String,
    pub line: u32,
    pub span_start: u32,
    pub span_end: u32,
    #[serde(default)]
    pub callee: String,
    /// `None` when the walk ended with no labelled export.
    #[serde(default)]
    pub end: Option<LabelledEnd>,
    #[serde(default)]
    pub chain: Vec<ChainHop>,
    #[serde(default)]
    pub stop: Option<String>,
}

/// Discovery's census file: `{ "sites": [...] }`.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct DiscoveryFile {
    pub sites: Vec<DiscoverySite>,
}

/// An argument of a labelled call, or one option key of it.
#[derive(Debug, Clone, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ArgSlot {
    pub arg: usize,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key: Option<String>,
}

/// Where a label says an HTTP member's method is read (label structure v2,
/// carrick#2434): an argument or one option key of it, the method sent when
/// none is given, or the one method the member always sends.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct MethodLabel {
    #[serde(default)]
    pub from: Option<ArgSlot>,
    #[serde(default)]
    pub default: Option<String>,
    #[serde(default)]
    pub literal: Option<String>,
}

/// One op of a labelled export, as the library store answers it: the
/// export, its role and side, the op and member, and the slots its calls
/// fill.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SiteLabel {
    pub package: String,
    pub export: String,
    #[serde(default)]
    pub member: Option<String>,
    pub role: String,
    #[serde(default)]
    pub op: Option<String>,
    #[serde(default)]
    pub side: Option<String>,
    #[serde(default)]
    pub name: Option<ArgSlot>,
    #[serde(default)]
    pub payload: Option<ArgSlot>,
    #[serde(default)]
    pub handler: Option<ArgSlot>,
    /// Label v2's method fields; a cassette until the store's rebuild.
    #[serde(default)]
    pub method: Option<MethodLabel>,
}

impl SiteLabel {
    /// Every op the store's `/library-claims` answer labels, one label each.
    /// A maker (a construction of the export) labels no op and no name.
    pub fn from_store_answer(answer: &serde_json::Value) -> Vec<SiteLabel> {
        #[derive(Deserialize)]
        struct Op {
            op: String,
            #[serde(default)]
            member: Option<String>,
            #[serde(default)]
            name: Option<ArgSlot>,
            #[serde(default)]
            payload: Option<ArgSlot>,
            #[serde(default)]
            handler: Option<ArgSlot>,
            #[serde(default)]
            method: Option<MethodLabel>,
        }
        #[derive(Deserialize)]
        struct Export {
            export: String,
            role: String,
            #[serde(default)]
            side: Option<String>,
            #[serde(default)]
            ops: Vec<Op>,
        }
        #[derive(Deserialize)]
        struct Package {
            package: String,
            #[serde(default)]
            exports: Vec<Export>,
        }
        let packages: Vec<Package> = answer
            .get("library_claims")
            .cloned()
            .and_then(|value| serde_json::from_value(value).ok())
            .unwrap_or_default();
        let mut labels = Vec::new();
        for package in packages {
            for export in package.exports {
                for op in export.ops {
                    labels.push(SiteLabel {
                        package: package.package.clone(),
                        export: export.export.clone(),
                        member: op.member,
                        role: export.role.clone(),
                        op: Some(op.op),
                        side: export.side.clone(),
                        name: op.name,
                        payload: op.payload,
                        handler: op.handler,
                        method: op.method,
                    });
                }
            }
        }
        labels
    }

    /// Every op the claims the scan settled label (carrick#2439), one label
    /// each, as [`Self::from_store_answer`] reads them off the store's raw
    /// answer: the scan holds the claims, already read and grouped by export.
    /// A maker, a scope and a reserved name label no op.
    pub fn from_exports(exports: &[crate::library_claims::ExportClaims]) -> Vec<SiteLabel> {
        use crate::services::type_sidecar::{LibraryClaim, OpName};
        let word = |value: serde_json::Result<serde_json::Value>| -> Option<String> {
            value.ok()?.as_str().map(str::to_string)
        };
        let slot = |slot: &crate::services::type_sidecar::ClaimSlot| ArgSlot {
            arg: slot.arg as usize,
            key: slot.key.clone(),
        };
        let mut labels = Vec::new();
        for export in exports {
            let role = word(serde_json::to_value(export.role)).unwrap_or_default();
            let side = export
                .side
                .and_then(|side| word(serde_json::to_value(side)));
            for claim in &export.claims {
                let LibraryClaim::Op {
                    op,
                    member,
                    name,
                    payload,
                    handler,
                    method,
                    method_key,
                    method_default,
                    ..
                } = claim
                else {
                    continue;
                };
                let method = (method.is_some() || method_key.is_some() || method_default.is_some())
                    .then(|| MethodLabel {
                        from: method_key.as_ref().map(slot),
                        default: method_default.clone(),
                        literal: method.clone(),
                    });
                labels.push(SiteLabel {
                    package: export.package.clone(),
                    export: export.export.clone(),
                    member: member.clone(),
                    role: role.clone(),
                    op: word(serde_json::to_value(op)),
                    side: side.clone(),
                    name: match name {
                        Some(OpName::Slot(name)) => Some(slot(name)),
                        _ => None,
                    },
                    payload: payload.as_ref().map(slot),
                    handler: handler.as_ref().map(slot),
                    method,
                });
            }
        }
        labels
    }

    /// The label for `end` among `labels`: the same package, export and
    /// member.
    pub fn find<'l>(labels: &'l [SiteLabel], end: &LabelledEnd) -> Option<&'l SiteLabel> {
        labels.iter().find(|label| {
            label.package == end.package && label.export == end.export && label.member == end.member
        })
    }
}

/// Where the payload a decided site sends is bound (carrick#2472 C3): an
/// argument of the deciding call (one option key of it), or a call below it
/// where a wrapper builds the value, `file:line`, with that call's argument.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(untagged)]
pub enum PayloadBinding {
    Arg {
        arg: usize,
        #[serde(skip_serializing_if = "Option::is_none")]
        key: Option<String>,
    },
    BuiltInside {
        /// `file:line` of the call that builds it.
        built_inside: String,
        /// That call's SWC span.
        span_start: u32,
        span_end: u32,
        arg: usize,
        #[serde(skip_serializing_if = "Option::is_none")]
        key: Option<String>,
    },
}

/// The function that handles a receive site or one key of a handler table:
/// its repo-relative file, line and span.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HandlerAt {
    pub file: String,
    pub line: u32,
    pub span_start: u32,
    pub span_end: u32,
}

/// One call between a decided site and its labelled call.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Via {
    /// `file:line`, repo-relative.
    pub at: String,
    /// The callee as written.
    pub callee: String,
}

/// A call where a name is decided.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DecidedSite {
    /// Repo-relative, `/`-separated.
    pub file: String,
    pub line: u32,
    pub span_start: u32,
    pub span_end: u32,
    pub end: LabelledEnd,
    /// The slot's value at this call, composed down the chain. Empty where
    /// the label names no slot.
    pub name: Vec<Piece>,
    /// The key of a handler table this site handles.
    pub key: Option<String>,
    /// The arguments of this call the slot's value is read from: the
    /// label's at the labelled call itself.
    pub name_args: BTreeSet<usize>,
    /// The calls below this one down to the labelled call, outermost
    /// first: each is written inside the function the call above enters.
    pub via: Vec<Via>,
    /// Where the payload is bound: at the label's payload slot, or for a
    /// receive at its handler slot (the payload arrives in the handler's
    /// first parameter).
    pub payload: Option<PayloadBinding>,
    /// The function that handles a receive site, or a handler table's key.
    pub handler: Option<HandlerAt>,
    /// What the label's method slot holds here, when it names one.
    pub method: Option<MethodValue>,
}

impl DecidedSite {
    /// The name as text: every piece literal, or for HTTP a target the
    /// summaries would write (an opaque base before a literal path).
    pub fn name_text(&self) -> Option<String> {
        if let Some(key) = &self.key {
            return Some(key.clone());
        }
        let literal: Option<String> = self
            .name
            .iter()
            .map(|piece| match piece {
                Piece::Lit(text) => Some(text.as_str()),
                _ => None,
            })
            .collect();
        match literal {
            Some(text) if !text.is_empty() => Some(text),
            _ if self.end.role == "http_client" => render_target(&self.name),
            _ => None,
        }
    }
}

/// Why a discovered call names nothing a caller decides.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Undecided {
    /// The slot keeps a parameter up to a function nothing calls.
    NameNeverBound,
    /// The slot holds a parameter of a callback this code does not call.
    CallbackParameter,
    /// The call reaches an export the store labels with a network role and
    /// no op (carrick#2515): the store says nothing about what its members
    /// do, so no slot can be read. The call is listed, not named.
    NoOpClaim,
}

/// A discovered call whose name no call of this code decides.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DecidedLead {
    pub file: String,
    pub line: u32,
    pub span_start: u32,
    pub span_end: u32,
    /// Functions the walk entered from this call (0 when it is at the call
    /// that holds the open name).
    pub depth: u32,
    pub end: LabelledEnd,
    pub reason: Undecided,
}

/// What [`decided_sites`] finds.
#[derive(Debug, Default)]
pub struct Decided {
    pub sites: Vec<DecidedSite>,
    pub leads: Vec<DecidedLead>,
    /// Discovery sites whose labelled call this pass could not place in the
    /// summaries' inputs, by `file:line`.
    pub unplaced: Vec<String>,
    /// Sites whose chain was walked up the call graph rather than read from
    /// discovery's chain (a hop into a callback, an ambiguous hop, or a
    /// chain that stops short), by `file:line`.
    pub walked: Vec<String>,
}

/// One call, where it is written: its file, the definition key of the
/// function whose body holds it (`None` at module level), how many functions
/// deep inside that one it is, and the body that writes it.
#[derive(Clone, Copy)]
struct Placed<'a> {
    file: &'a Path,
    key: Option<&'a str>,
    depth: u8,
    call: &'a CallIr,
}

/// Every call of the service, by file and span, by the function whose body
/// holds it, and by the function of the service the call graph resolves it
/// to.
struct Calls<'a> {
    sites: &'a crate::call_graph::CallSiteTargets,
    by_span: HashMap<(&'a Path, u32, u32), Placed<'a>>,
    by_owner: HashMap<(&'a Path, &'a str), Vec<Placed<'a>>>,
    callers: HashMap<(PathBuf, String), Vec<Placed<'a>>>,
}

impl<'a> Calls<'a> {
    fn index(inputs: &'a RequestSummaryInputs) -> Self {
        fn walk<'a>(
            file: &'a Path,
            key: Option<&'a str>,
            depth: u8,
            ir: &'a FnIr,
            out: &mut Calls<'a>,
        ) {
            for call in ir.calls.iter().chain(&ir.constructions) {
                let placed = Placed {
                    file,
                    key,
                    depth,
                    call,
                };
                out.by_span
                    .entry((file, call.site.span_start, call.site.span_end))
                    .or_insert(placed);
                if let Some(key) = key {
                    out.by_owner.entry((file, key)).or_default().push(placed);
                }
                if let Some(target) = out.sites.target_at(file, call.site.lo, call.site.hi) {
                    out.callers.entry(target.clone()).or_default().push(placed);
                }
            }
            for inner in ir.nested.iter().chain(&ir.detached) {
                walk(file, key, depth.saturating_add(1), inner, out);
            }
        }
        let mut calls = Calls {
            sites: &inputs.sites,
            by_span: HashMap::new(),
            by_owner: HashMap::new(),
            callers: HashMap::new(),
        };
        let mut paths: Vec<&PathBuf> = inputs.files.keys().collect();
        paths.sort();
        for path in paths {
            let file = &inputs.files[path];
            let mut keys: Vec<&String> = file.functions.keys().collect();
            keys.sort();
            for key in keys {
                walk(
                    path,
                    Some(key.as_str()),
                    0,
                    &file.functions[key],
                    &mut calls,
                );
            }
            walk(path, None, 0, &file.module_level, &mut calls);
        }
        calls
    }

    fn at_span(&self, file: &Path, span_start: u32, span_end: u32) -> Option<Placed<'a>> {
        self.by_span.get(&(file, span_start, span_end)).copied()
    }

    fn target(&self, placed: &Placed<'a>) -> Option<&(PathBuf, String)> {
        self.sites
            .target_at(placed.file, placed.call.site.lo, placed.call.site.hi)
    }

    fn callers_of(&self, file: &Path, key: &str) -> &[Placed<'a>] {
        self.callers
            .get(&(file.to_path_buf(), key.to_string()))
            .map_or(&[], Vec::as_slice)
    }
}

/// Whether any piece is a parameter a caller fills.
fn holds_parameter(pieces: &[Piece]) -> bool {
    pieces.iter().any(Piece::is_parameter)
}

/// `pieces`, read where a call at `depth` inside its definition sits, as
/// that definition's own body reads them: every parameter must be the
/// definition's (an enclosing parameter exactly `depth` functions out).
/// `None` when one is a callback's own.
fn to_definition(pieces: &[Piece], depth: u8) -> Option<Vec<Piece>> {
    if depth == 0 {
        return Some(pieces.to_vec());
    }
    pieces
        .iter()
        .map(|piece| match piece {
            Piece::Outer(out, inner) if *out == depth => Some((**inner).clone()),
            piece if piece.is_parameter() => None,
            other => Some(other.clone()),
        })
        .collect()
}

/// The argument `call` writes at `index`, unless a spread before it moves
/// every position.
fn argument(call: &CallIr, index: usize) -> Option<&Value> {
    call.site_args.as_ref()?;
    call.args.get(index)
}

/// `pieces` (the callee's own, its parameters by position) with each
/// parameter filled from the arguments `call` writes. A position the call
/// does not write, or one after a spread, is not known.
fn filled(pieces: &[Piece], call: &CallIr) -> Vec<Piece> {
    concat(pieces.iter().map(|piece| {
        match piece {
            Piece::Param(index, _) => {
                argument(call, *index).map_or(vec![Piece::Unknown], Value::pieces)
            }
            Piece::ParamKey(index, key, name) => match argument(call, *index) {
                Some(Value::Obj(object)) => object
                    .fields
                    .get(key)
                    .map_or(vec![Piece::Unknown], Value::pieces),
                // The caller passes its own parameter on whole: its key waits on
                // the caller's caller.
                Some(Value::Str(pieces)) => match pieces.as_slice() {
                    [Piece::Param(passed, _)] => {
                        vec![Piece::ParamKey(*passed, key.clone(), name.clone())]
                    }
                    _ => vec![Piece::Unknown],
                },
                _ => vec![Piece::Unknown],
            },
            other => vec![other.clone()],
        }
    }))
}

/// The argument positions `pieces` reads, as the callee's own parameters.
fn parameter_positions(pieces: &[Piece]) -> BTreeSet<usize> {
    pieces
        .iter()
        .filter_map(|piece| match piece {
            Piece::Param(index, _) | Piece::ParamKey(index, ..) => Some(*index),
            _ => None,
        })
        .collect()
}

/// The slot's value at the labelled call, as the body that writes it reads
/// it.
fn slot_value(call: &CallIr, slot: &ArgSlot) -> Vec<Piece> {
    let Some(arg) = argument(call, slot.arg) else {
        return vec![Piece::Unknown];
    };
    match (&slot.key, arg) {
        (None, value) => value.pieces(),
        (Some(key), Value::Obj(object)) => object
            .fields
            .get(key)
            .map_or(vec![Piece::Unknown], Value::pieces),
        (Some(key), Value::Str(pieces)) => match pieces.as_slice() {
            [Piece::Param(index, name)] => {
                vec![Piece::ParamKey(*index, key.clone(), name.clone())]
            }
            _ => vec![Piece::Unknown],
        },
        _ => vec![Piece::Unknown],
    }
}

/// Where the payload `slot` names at the labelled call (the last of `down`)
/// is bound, read from the deciding call (the first): carried up while each
/// call hands on a parameter of its own function whole, and built inside the
/// first call that writes anything else there. `None` when a call hands
/// nothing there: it sends no payload.
fn payload_binding(root: &Path, down: &[Placed<'_>], slot: &ArgSlot) -> Option<PayloadBinding> {
    let (at, arg, key) = bound_at(down, slot)?;
    if at == 0 {
        return Some(PayloadBinding::Arg { arg, key });
    }
    let placed = &down[at];
    Some(PayloadBinding::BuiltInside {
        built_inside: format!("{}:{}", relative(root, placed.file), placed.call.site.line),
        span_start: placed.call.site.span_start,
        span_end: placed.call.site.span_end,
        arg,
        key,
    })
}

/// Where the value `slot` names at the labelled call (the last of `down`)
/// is written, as [`payload_binding`] reads it: the index in `down` of the
/// call that writes it, and that call's argument and option key.
fn bound_at(down: &[Placed<'_>], slot: &ArgSlot) -> Option<(usize, usize, Option<String>)> {
    let whole_param = |value: &Value| match value {
        Value::Str(pieces) => match pieces.as_slice() {
            [Piece::Param(index, _)] => Some(*index),
            _ => None,
        },
        _ => None,
    };
    let mut arg = slot.arg;
    let mut key = slot.key.clone();
    for k in (1..down.len()).rev() {
        let placed = &down[k];
        if placed.call.site_args.is_some() && placed.call.args.get(arg).is_none() {
            return None;
        }
        let passed = match (&key, argument(placed.call, arg)) {
            (_, None) => None,
            (None, Some(value)) => whole_param(value).map(|index| (index, None)),
            (Some(field), Some(Value::Obj(object))) => object
                .fields
                .get(field)
                .and_then(whole_param)
                .map(|index| (index, None)),
            (Some(field), Some(value)) => {
                whole_param(value).map(|index| (index, Some(field.clone())))
            }
        };
        match passed.filter(|_| placed.depth == 0) {
            Some((index, field)) => {
                arg = index;
                key = field;
            }
            None => return Some((k, arg, key)),
        }
    }
    let first = down.first()?;
    if first.call.site_args.is_some() && first.call.args.get(arg).is_none() {
        return None;
    }
    Some((0, arg, key))
}

/// What a label's method slot holds at a decided site (label v2,
/// carrick#2434).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MethodValue {
    /// No call down to the labelled one writes the slot (the argument is
    /// absent, or an object with no spread lacks the key): the label's
    /// default is sent.
    Unwritten,
    /// The slot's value as the deciding call's arguments write it.
    Pieces(Vec<Piece>),
}

/// The value `call` writes at argument `arg` (one option key of it).
fn written_at(call: &CallIr, arg: usize, key: Option<&str>) -> MethodValue {
    if call.site_args.is_some() && call.args.get(arg).is_none() {
        return MethodValue::Unwritten;
    }
    let Some(value) = argument(call, arg) else {
        return MethodValue::Pieces(vec![Piece::Unknown]);
    };
    match (key, value) {
        (None, value) => MethodValue::Pieces(value.pieces()),
        (Some(key), Value::Obj(object)) => match object.fields.get(key) {
            Some(value) => MethodValue::Pieces(value.pieces()),
            None if !object.open && object.under_param.is_none() => MethodValue::Unwritten,
            None => MethodValue::Pieces(vec![Piece::Unknown]),
        },
        (Some(key), Value::Str(pieces)) => match pieces.as_slice() {
            [Piece::Param(index, name)] => {
                MethodValue::Pieces(vec![Piece::ParamKey(*index, key.to_string(), name.clone())])
            }
            _ => MethodValue::Pieces(vec![Piece::Unknown]),
        },
        _ => MethodValue::Pieces(vec![Piece::Unknown]),
    }
}

/// The method `slot` reads at the labelled call (the last of `down`),
/// composed up to the deciding call (the first) as the name is: each call
/// fills the parameters of the function below it.
fn method_value(down: &[Placed<'_>], slot: &ArgSlot) -> Option<MethodValue> {
    let last = down.last()?;
    let mut value = written_at(last.call, slot.arg, slot.key.as_deref());
    for k in (0..down.len() - 1).rev() {
        let MethodValue::Pieces(pieces) = &value else {
            break;
        };
        if !holds_parameter(pieces) {
            break;
        }
        let Some(own) = to_definition(pieces, down[k + 1].depth) else {
            return Some(MethodValue::Pieces(vec![Piece::Unknown]));
        };
        value = match own.as_slice() {
            [Piece::Param(index, _)] => written_at(down[k].call, *index, None),
            [Piece::ParamKey(index, key, _)] => written_at(down[k].call, *index, Some(key)),
            _ => MethodValue::Pieces(filled(&own, down[k].call)),
        };
    }
    Some(value)
}

/// The functions a module declares at its top level by name: function
/// declarations, and constants holding an arrow or a function expression.
fn module_functions(module: &Module) -> HashMap<String, Span> {
    let mut functions: HashMap<String, Span> = HashMap::new();
    for item in &module.body {
        let decl = match item {
            ModuleItem::Stmt(Stmt::Decl(decl)) => decl,
            ModuleItem::ModuleDecl(ModuleDecl::ExportDecl(export)) => &export.decl,
            _ => continue,
        };
        match decl {
            Decl::Fn(function) => {
                functions.insert(function.ident.sym.to_string(), function.function.span);
            }
            Decl::Var(var) if var.kind == VarDeclKind::Const => {
                for declarator in &var.decls {
                    let Pat::Ident(name) = &declarator.name else {
                        continue;
                    };
                    match declarator.init.as_deref().map(super::unwrap_value) {
                        Some(Expr::Arrow(arrow)) => {
                            functions.insert(name.id.sym.to_string(), arrow.span);
                        }
                        Some(Expr::Fn(function)) => {
                            functions.insert(name.id.sym.to_string(), function.function.span);
                        }
                        _ => {}
                    }
                }
            }
            _ => {}
        }
    }
    functions
}

/// The function `placed`'s call hands at argument `arg`: one written there,
/// or one the module declares under the name written there; otherwise the
/// argument itself.
fn handler_at(root: &Path, placed: &Placed<'_>, arg: usize) -> Option<HandlerAt> {
    let cm: Lrc<SourceMap> = Default::default();
    let handler = swc_common::errors::Handler::with_tty_emitter(
        swc_common::errors::ColorConfig::Never,
        false,
        false,
        Some(cm.clone()),
    );
    let module = crate::parser::parse_file(placed.file, &cm, &handler)?;
    let mut finder = CallAt {
        target: (placed.call.site.span_start, placed.call.site.span_end),
        args: None,
    };
    module.visit_with(&mut finder);
    let args = finder.args?;
    if args.iter().take(arg + 1).any(|arg| arg.spread.is_some()) {
        return None;
    }
    let written = args.get(arg)?;
    let span = match super::unwrap_value(&written.expr) {
        Expr::Arrow(arrow) => arrow.span,
        Expr::Fn(function) => function.function.span,
        Expr::Ident(name) => module_functions(&module)
            .get(name.sym.as_str())
            .copied()
            .unwrap_or(name.span),
        other => other.span(),
    };
    Some(HandlerAt {
        file: relative(root, placed.file),
        line: cm.lookup_char_pos(span.lo).line as u32,
        span_start: span.lo.0,
        span_end: span.hi.0,
    })
}

/// Finds the arguments of the call or construction at a span.
struct CallAt {
    target: (u32, u32),
    args: Option<Vec<ExprOrSpread>>,
}

impl Visit for CallAt {
    fn visit_call_expr(&mut self, call: &CallExpr) {
        if (call.span.lo.0, call.span.hi.0) == self.target {
            self.args = Some(call.args.clone());
        }
        call.visit_children_with(self);
    }

    fn visit_new_expr(&mut self, new: &NewExpr) {
        if (new.span.lo.0, new.span.hi.0) == self.target {
            self.args = Some(new.args.clone().unwrap_or_default());
        }
        new.visit_children_with(self);
    }
}

/// `file` as the record names it: relative to `root`, `/`-separated.
pub(crate) fn relative(root: &Path, file: &Path) -> String {
    let relative = file.strip_prefix(root).unwrap_or(file);
    relative
        .components()
        .map(|part| part.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/")
}

struct Walk<'a, 'i> {
    calls: &'i Calls<'a>,
    root: &'i Path,
    out: &'i mut Decided,
    seen: HashSet<(String, u32, Option<String>, String)>,
    /// The label of the labelled call being walked.
    label: Option<&'i SiteLabel>,
}

impl<'a> Walk<'a, '_> {
    /// `down` is the calls from the site (first) to the labelled call
    /// (last).
    fn site(
        &mut self,
        down: &[Placed<'a>],
        end: &LabelledEnd,
        name: Vec<Piece>,
        name_args: BTreeSet<usize>,
    ) {
        let via = via_of(self.root, down);
        let placed = &down[0];
        let file = relative(self.root, placed.file);
        let id = (file.clone(), placed.call.site.span_start, None, end.at());
        if !self.seen.insert(id) {
            return;
        }
        let label = self.label;
        let handler_slot = label.and_then(|label| label.handler.as_ref());
        let payload = match label.and_then(|label| label.payload.as_ref()) {
            Some(slot) => payload_binding(self.root, down, slot),
            None => handler_slot.and_then(|slot| payload_binding(self.root, down, slot)),
        };
        let handler = handler_slot
            .and_then(|slot| bound_at(down, slot))
            .and_then(|(at, arg, _)| handler_at(self.root, &down[at], arg));
        let method = label
            .and_then(|label| label.method.as_ref())
            .and_then(|method| method.from.as_ref())
            .and_then(|slot| method_value(down, slot));
        self.out.sites.push(DecidedSite {
            file,
            line: placed.call.site.line,
            span_start: placed.call.site.span_start,
            span_end: placed.call.site.span_end,
            end: end.clone(),
            name,
            key: None,
            name_args,
            via,
            payload,
            handler,
            method,
        });
    }

    fn lead(&mut self, placed: &Placed<'a>, end: &LabelledEnd, why: Undecided) {
        let lead = DecidedLead {
            file: relative(self.root, placed.file),
            line: placed.call.site.line,
            span_start: placed.call.site.span_start,
            span_end: placed.call.site.span_end,
            depth: u32::from(placed.depth),
            end: end.clone(),
            reason: why,
        };
        if !self.out.leads.contains(&lead) {
            self.out.leads.push(lead);
        }
    }

    /// A call on an export the store labels with no op: a lead of its own
    /// reason, at the labelled call.
    fn no_op_claim(&mut self, end: &LabelledEnd) {
        let lead = DecidedLead {
            file: end.file.clone(),
            line: end.line,
            span_start: end.span_start,
            span_end: end.span_end,
            depth: 0,
            end: end.clone(),
            reason: Undecided::NoOpClaim,
        };
        if !self.out.leads.contains(&lead) {
            self.out.leads.push(lead);
        }
    }

    /// Walk up the call graph from `down[0]`, whose slot value `pieces` (as
    /// its own body reads it) still holds a parameter: each caller of its
    /// definition fills it, and the lowest caller left with none is the
    /// site. `path` holds the definitions already passed: a cycle ends.
    fn up(
        &mut self,
        down: &[Placed<'a>],
        pieces: &[Piece],
        end: &LabelledEnd,
        path: &mut Vec<(PathBuf, String)>,
    ) {
        let placed = down[0];
        let Some(own) = to_definition(pieces, placed.depth) else {
            self.lead(&placed, end, Undecided::CallbackParameter);
            return;
        };
        let Some(key) = placed.key else {
            // Module level holds no parameter of its own.
            self.lead(&placed, end, Undecided::NameNeverBound);
            return;
        };
        let callers = self.calls.callers_of(placed.file, key);
        if callers.is_empty() {
            self.lead(&placed, end, Undecided::NameNeverBound);
            return;
        }
        let here = (placed.file.to_path_buf(), key.to_string());
        if path.contains(&here) {
            return;
        }
        path.push(here);
        for caller in callers {
            let value = filled(&own, caller.call);
            let mut longer = vec![*caller];
            longer.extend_from_slice(down);
            if holds_parameter(&value) {
                self.up(&longer, &value, end, path);
            } else {
                self.site(&longer, end, value, parameter_positions(&own));
            }
        }
        path.pop();
    }

    /// One site per key of the handler table the labelled call is handed in
    /// a loop, at the line of the function under each key.
    fn table(&mut self, placed: &Placed<'a>, end: &LabelledEnd, keys: Vec<TableKey>) {
        let file = relative(self.root, placed.file);
        for entry in keys {
            let id = (
                file.clone(),
                entry.handler.line,
                Some(entry.key.clone()),
                end.at(),
            );
            if !self.seen.insert(id) {
                continue;
            }
            self.out.sites.push(DecidedSite {
                file: file.clone(),
                line: entry.handler.line,
                span_start: placed.call.site.span_start,
                span_end: placed.call.site.span_end,
                end: end.clone(),
                name: vec![Piece::Lit(entry.key.clone())],
                key: Some(entry.key),
                name_args: BTreeSet::new(),
                via: Vec::new(),
                payload: None,
                handler: Some(entry.handler),
                method: None,
            });
        }
    }
}

/// Each call below the first in `down`: its `file:line` and its callee as
/// written.
fn via_of(root: &Path, down: &[Placed<'_>]) -> Vec<Via> {
    down.iter()
        .skip(1)
        .map(|placed| Via {
            at: format!("{}:{}", relative(root, placed.file), placed.call.site.line),
            callee: placed.call.callee_text.clone(),
        })
        .collect()
}

/// Every call in `discovered` where its name is decided, and every one
/// whose name no call of this code decides, read with the labels the store
/// answers (`labels`). A labelled call no label answers is skipped, unless
/// its export has no op label at all: that is a lead (carrick#2515). Files
/// are named relative to `root`, the scanned repo's root.
pub fn decided_sites(
    inputs: &RequestSummaryInputs,
    root: &Path,
    discovered: &[DiscoverySite],
    labels: &[SiteLabel],
) -> Decided {
    let calls = Calls::index(inputs);
    let mut out = Decided::default();
    // The functions some listed site's chain enters: a site whose value
    // still holds a parameter is decided by those callers, which discovery
    // lists on their own.
    let entered: HashSet<(PathBuf, String)> = discovered
        .iter()
        .filter_map(|site| site.chain.first())
        .map(|hop| (root.join(&hop.file), hop.function.clone()))
        .collect();
    let mut tables: HashMap<(PathBuf, u32), Option<Vec<TableKey>>> = HashMap::new();
    let mut walked: Vec<String> = Vec::new();
    let mut unplaced: Vec<String> = Vec::new();
    let mut walk = Walk {
        calls: &calls,
        root,
        out: &mut out,
        seen: HashSet::new(),
        label: None,
    };
    for site in discovered {
        let Some(end) = &site.end else {
            continue;
        };
        let Some(label) = SiteLabel::find(labels, end) else {
            // An export the store labels with a role and no op at all
            // (carrick#2515): the call is a lead. An export with ops, and a
            // member of it none labels, is not.
            if !labels
                .iter()
                .any(|label| label.package == end.package && label.export == end.export)
            {
                walk.no_op_claim(end);
            }
            continue;
        };
        walk.label = Some(label);
        let Some(last) = calls.at_span(&root.join(&end.file), end.span_start, end.span_end) else {
            unplaced.push(format!("{}:{}", site.file, site.line));
            continue;
        };
        let Some(slot) = &label.name else {
            // The label names no slot: the labelled call is the site.
            walk.site(&[last], end, Vec::new(), BTreeSet::new());
            continue;
        };
        let table = tables
            .entry((root.join(&end.file), end.span_start))
            .or_insert_with(|| loop_table(&root.join(&end.file), end, slot.arg))
            .clone();
        if let Some(keys) = table {
            walk.table(&last, end, keys);
            continue;
        }
        let at_end = slot_value(last.call, slot);
        if !holds_parameter(&at_end) {
            walk.site(&[last], end, at_end, BTreeSet::from([slot.arg]));
            continue;
        }
        let Some(chain) = place_chain(&calls, root, site, &last) else {
            walked.push(format!("{}:{}", site.file, site.line));
            walk.up(&[last], &at_end, end, &mut Vec::new());
            continue;
        };
        // Compose up the chain: values[k] is the slot at chain[k], as the
        // body that writes chain[k] reads it.
        let n = chain.len();
        let mut values: Vec<Option<Vec<Piece>>> = vec![None; n];
        values[n - 1] = Some(at_end);
        let mut args: Vec<BTreeSet<usize>> = vec![BTreeSet::new(); n];
        args[n - 1] = BTreeSet::from([slot.arg]);
        for k in (0..n - 1).rev() {
            let below = &chain[k + 1];
            values[k] = values[k + 1]
                .as_ref()
                .and_then(|pieces| to_definition(pieces, below.depth))
                .map(|own| {
                    args[k] = parameter_positions(&own);
                    filled(&own, chain[k].call)
                });
        }
        let Some(top) = values[0].clone() else {
            walk.lead(&chain[0], end, Undecided::CallbackParameter);
            continue;
        };
        if !holds_parameter(&top) {
            let below_open = n == 1
                || values[1]
                    .as_ref()
                    .is_none_or(|pieces| holds_parameter(pieces));
            if below_open {
                walk.site(&chain, end, top, args[0].clone());
            }
            continue;
        }
        let covered = chain[0]
            .key
            .filter(|_| chain[0].depth == 0)
            .is_some_and(|key| entered.contains(&(chain[0].file.to_path_buf(), key.to_string())));
        if !covered {
            walked.push(format!("{}:{}", site.file, site.line));
            walk.up(&chain, &top, end, &mut Vec::new());
        }
    }
    walked.sort();
    walked.dedup();
    out.walked = walked;
    out.unplaced = unplaced;
    out.sites.sort_by(|a, b| {
        (&a.file, a.line, &a.key, &a.end.file, a.end.line).cmp(&(
            &b.file,
            b.line,
            &b.key,
            &b.end.file,
            b.end.line,
        ))
    });
    out.leads
        .sort_by(|a, b| (&a.file, a.line).cmp(&(&b.file, b.line)));
    out
}

/// The calls a discovery site's chain names: the site's own first, then in
/// each function the chain enters the one call that calls the next, and
/// the labelled call last. `None` where a hop is a callback (no call of the
/// service enters it), or a body calls the next function more than once or
/// not at all.
fn place_chain<'a>(
    calls: &Calls<'a>,
    root: &Path,
    site: &DiscoverySite,
    last: &Placed<'a>,
) -> Option<Vec<Placed<'a>>> {
    let first = calls.at_span(&root.join(&site.file), site.span_start, site.span_end)?;
    if site.chain.is_empty() {
        return (first.call.site == last.call.site).then(|| vec![first]);
    }
    if site.chain.iter().any(|hop| hop.function.starts_with('<')) {
        return None;
    }
    // Discovery follows calls the call graph may not resolve (a member of a
    // call's result, through the compiler's rows): a call it leaves
    // unresolved enters the hop only when it calls the hop's function by its
    // own name (`openSession(url).notify` for `Session.notify`), so a log
    // line or a global function in the body is never the hop. A call it
    // resolves elsewhere is not this hop.
    let enters = |placed: &Placed<'a>, hop: &ChainHop| match calls.target(placed) {
        Some((file, key)) => file == &root.join(&hop.file) && key == &hop.function,
        None => called_name(&placed.call.callee_text) == called_name(&hop.function),
    };
    if !enters(&first, &site.chain[0]) {
        return None;
    }
    let mut chain = vec![first];
    for pair in site.chain.windows(2) {
        let (from, to) = (&pair[0], &pair[1]);
        let file = root.join(&from.file);
        let inside = calls
            .by_owner
            .get(&(file.as_path(), from.function.as_str()))?;
        let mut next = inside.iter().filter(|placed| enters(placed, to));
        let found = *next.next()?;
        if next.next().is_some() {
            return None;
        }
        chain.push(found);
    }
    let owner = site.chain.last()?;
    if last.key != Some(owner.function.as_str()) || last.file != root.join(&owner.file) {
        return None;
    }
    chain.push(*last);
    Some(chain)
}

/// The name a call reaches its function by, or a definition key names its
/// function by: the last `.` segment (`notify` for
/// `openSession(url).notify` and for `Session.notify`).
fn called_name(text: &str) -> &str {
    text.rsplit('.').next().unwrap_or(text)
}

/// One key of a handler table.
#[derive(Debug, Clone, PartialEq, Eq)]
struct TableKey {
    key: String,
    handler: HandlerAt,
}

/// The keys of the handler table the labelled call at `end` registers in a
/// loop, when its name argument (`arg`) is the key a `for … of` over
/// `Object.entries(T)` or `Object.keys(T)` binds and `T` is a module-scope
/// `const` object literal whose every key is written: each key with the
/// function that handles it (a function the module declares under the
/// value's name, or one written as the value), or the key's own line.
/// `None` for anything else.
fn loop_table(file: &Path, end: &LabelledEnd, arg: usize) -> Option<Vec<TableKey>> {
    let cm: Lrc<SourceMap> = Default::default();
    let handler = swc_common::errors::Handler::with_tty_emitter(
        swc_common::errors::ColorConfig::Never,
        false,
        false,
        Some(cm.clone()),
    );
    let module = crate::parser::parse_file(file, &cm, &handler)?;
    let mut finder = LoopFinder {
        target: (end.span_start, end.span_end),
        arg,
        loops: Vec::new(),
        found: None,
    };
    module.visit_with(&mut finder);
    let table_name = finder.found?;
    let functions = module_functions(&module);
    let mut table: Option<&ObjectLit> = None;
    for item in &module.body {
        let decl = match item {
            ModuleItem::Stmt(Stmt::Decl(decl)) => decl,
            ModuleItem::ModuleDecl(ModuleDecl::ExportDecl(export)) => &export.decl,
            _ => continue,
        };
        if let Decl::Var(var) = decl
            && var.kind == VarDeclKind::Const
        {
            for declarator in &var.decls {
                if let Pat::Ident(name) = &declarator.name
                    && name.id.sym == *table_name
                    && let Some(Expr::Object(object)) =
                        declarator.init.as_deref().map(super::unwrap_value)
                {
                    table = Some(object);
                }
            }
        }
    }
    let line = |span: Span| cm.lookup_char_pos(span.lo).line as u32;
    let at = |span: Span| HandlerAt {
        file: end.file.clone(),
        line: line(span),
        span_start: span.lo.0,
        span_end: span.hi.0,
    };
    let mut keys = Vec::new();
    for prop in &table?.props {
        let PropOrSpread::Prop(prop) = prop else {
            return None;
        };
        let Prop::KeyValue(entry) = &**prop else {
            return None;
        };
        let key = super::prop_name(&entry.key)?;
        let handler = match super::unwrap_value(&entry.value) {
            Expr::Ident(name) => functions
                .get(name.sym.as_str())
                .copied()
                .map_or_else(|| at(entry.span()), at),
            Expr::Arrow(arrow) => at(arrow.span),
            Expr::Fn(function) => at(function.function.span),
            _ => at(entry.span()),
        };
        keys.push(TableKey { key, handler });
    }
    (!keys.is_empty()).then_some(keys)
}

/// Finds the labelled call and the loop binding its name argument reads.
struct LoopFinder {
    target: (u32, u32),
    arg: usize,
    /// Each enclosing `for … of Object.entries(T)`: the name its key is
    /// bound to, and `T`.
    loops: Vec<(String, String)>,
    found: Option<String>,
}

impl Visit for LoopFinder {
    fn visit_for_of_stmt(&mut self, stmt: &ForOfStmt) {
        let bound = loop_key(stmt);
        if let Some(bound) = &bound {
            self.loops.push(bound.clone());
        }
        stmt.visit_children_with(self);
        if bound.is_some() {
            self.loops.pop();
        }
    }

    fn visit_call_expr(&mut self, call: &CallExpr) {
        if (call.span.lo.0, call.span.hi.0) == self.target
            && let Some(Expr::Ident(name)) = call.args.get(self.arg).map(|arg| &*arg.expr)
            && arg_unspread(call, self.arg)
            && let Some((_, table)) = self
                .loops
                .iter()
                .rev()
                .find(|(key, _)| name.sym == *key.as_str())
        {
            self.found = Some(table.clone());
        }
        call.visit_children_with(self);
    }
}

fn arg_unspread(call: &CallExpr, upto: usize) -> bool {
    call.args
        .iter()
        .take(upto + 1)
        .all(|arg| arg.spread.is_none())
}

/// The key name and the table a `for (const [k, …] of Object.entries(T))`
/// or `for (const k of Object.keys(T))` binds.
fn loop_key(stmt: &ForOfStmt) -> Option<(String, String)> {
    let ForHead::VarDecl(var) = &stmt.left else {
        return None;
    };
    if var.kind != VarDeclKind::Const || var.decls.len() != 1 {
        return None;
    }
    let Expr::Call(call) = super::unwrap_value(&stmt.right) else {
        return None;
    };
    let Callee::Expr(callee) = &call.callee else {
        return None;
    };
    let Expr::Member(member) = &**callee else {
        return None;
    };
    let (Expr::Ident(object), MemberProp::Ident(method)) = (&*member.obj, &member.prop) else {
        return None;
    };
    if object.sym != *"Object" || call.args.len() != 1 {
        return None;
    }
    let Expr::Ident(table) = super::unwrap_value(&call.args[0].expr) else {
        return None;
    };
    let key = match (&var.decls[0].name, method.sym.as_str()) {
        (Pat::Array(array), "entries") => match array.elems.first() {
            Some(Some(Pat::Ident(key))) => key.id.sym.to_string(),
            _ => return None,
        },
        (Pat::Ident(key), "keys") => key.id.sym.to_string(),
        _ => return None,
    };
    Some((key, table.sym.to_string()))
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// carrick#2439: the scan's settled claims give one label per op, with
    /// its slots, method fields, role and side, as the store's raw answer
    /// does; a maker labels no op, and a bound name no name slot.
    #[test]
    fn labels_read_from_the_settled_claims() {
        let exports: Vec<crate::library_claims::ExportClaims> =
            serde_json::from_value(serde_json::json!([{
                "package": "@fixture/http",
                "version": "1.0.0",
                "specifier": "@fixture/http",
                "export": "createClient",
                "role": "http_client",
                "side": "client",
                "claims": [
                    { "kind": "op", "op": "request", "member": "post", "name": { "arg": 0 },
                      "payload": { "arg": 1 }, "method": "POST" },
                    { "kind": "op", "op": "request", "member": "send", "name": { "bound": "maker" },
                      "method_key": { "arg": 1, "key": "method" }, "method_default": "GET" },
                    { "kind": "make", "form": "call", "member": null }
                ]
            }]))
            .unwrap();
        let labels = SiteLabel::from_exports(&exports);
        assert_eq!(
            labels,
            vec![
                SiteLabel {
                    package: "@fixture/http".into(),
                    export: "createClient".into(),
                    member: Some("post".into()),
                    role: "http_client".into(),
                    op: Some("request".into()),
                    side: Some("client".into()),
                    name: Some(ArgSlot { arg: 0, key: None }),
                    payload: Some(ArgSlot { arg: 1, key: None }),
                    handler: None,
                    method: Some(MethodLabel {
                        from: None,
                        default: None,
                        literal: Some("POST".into()),
                    }),
                },
                SiteLabel {
                    package: "@fixture/http".into(),
                    export: "createClient".into(),
                    member: Some("send".into()),
                    role: "http_client".into(),
                    op: Some("request".into()),
                    side: Some("client".into()),
                    name: None,
                    payload: None,
                    handler: None,
                    method: Some(MethodLabel {
                        from: Some(ArgSlot {
                            arg: 1,
                            key: Some("method".into()),
                        }),
                        default: Some("GET".into()),
                        literal: None,
                    }),
                },
            ]
        );
    }

    /// carrick#2515: a call on an export the store labels `graphql_client`
    /// with no op is a lead of its own reason and no site; a member the
    /// store left unlabelled on an export that has ops is neither.
    #[test]
    fn a_call_on_an_export_with_no_op_is_a_lead_not_a_vanishing() {
        let root = decided_fixture();
        let inputs = crate::engine::discover_request_inputs(&root);
        let exports: Vec<crate::library_claims::ExportClaims> =
            serde_json::from_value(serde_json::json!([
                { "package": "gql-thing", "version": "1.0.0", "specifier": "gql-thing",
                  "export": "Client", "role": "graphql_client", "side": "client", "claims": [] },
                { "package": "bus-thing", "version": "1.0.0", "specifier": "bus-thing",
                  "export": "Bus", "role": "broker", "side": "client",
                  "claims": [{ "kind": "op", "op": "send", "member": "publish",
                               "name": { "arg": 0 } }] }
            ]))
            .unwrap();
        let labels = SiteLabel::from_exports(&exports);
        let end = |package: &str, export: &str, member: &str, role: &str, line: u32| LabelledEnd {
            package: package.into(),
            export: export.into(),
            member: Some(member.into()),
            role: role.into(),
            op: None,
            file: "src/gql.ts".into(),
            line,
            span_start: line * 100,
            span_end: line * 100 + 20,
        };
        let sites = [
            bottom(
                &end("gql-thing", "Client", "query", "graphql_client", 3),
                "client.query",
            ),
            bottom(
                &end("gql-thing", "Client", "query", "graphql_client", 3),
                "client.query",
            ),
            bottom(&end("bus-thing", "Bus", "close", "broker", 4), "bus.close"),
        ];
        let decided = decided_sites(&inputs, &root, &sites, &labels);
        assert!(decided.sites.is_empty(), "{:?}", decided.sites);
        assert_eq!(
            decided
                .leads
                .iter()
                .map(|lead| (lead.file.as_str(), lead.line, lead.reason))
                .collect::<Vec<_>>(),
            vec![("src/gql.ts", 3, Undecided::NoOpClaim)]
        );
        let found = crate::possible_calls::from_sites(&decided, &[]);
        assert_eq!(found.len(), 1, "{found:?}");
        assert_eq!(
            found[0].unread.reason,
            crate::possible_calls::UnreadReason::NoOpClaim
        );
        assert_eq!(found[0].kind.as_deref(), Some("graphql"));
        assert_eq!(
            serde_json::to_value(found[0].unread.reason).unwrap(),
            "no_op_claim"
        );
    }

    pub(crate) fn manifest() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
    }

    /// The discovery fixture's repo, its service's summary inputs, its
    /// discovery sites and the store's labels.
    pub(crate) fn discovery_fixture() -> (
        PathBuf,
        RequestSummaryInputs,
        Vec<DiscoverySite>,
        Vec<SiteLabel>,
    ) {
        let fixture = manifest().join("tests/fixtures/discovery");
        let repo = std::fs::canonicalize(fixture.join("repo")).unwrap();
        let service = crate::config::Config::load_services(vec![repo.join("carrick.json")])
            .unwrap()
            .remove(0);
        // No walk: every read is the call graph's.
        let inputs = crate::engine::discover_service_inputs(&repo, &service, None);
        let text = std::fs::read_to_string(fixture.join("sites.expected.json")).unwrap();
        let sites = serde_json::from_str::<DiscoveryFile>(&text).unwrap().sites;
        let answer: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(fixture.join("library-claims.json")).unwrap(),
        )
        .unwrap();
        (repo, inputs, sites, SiteLabel::from_store_answer(&answer))
    }

    pub(crate) fn decided() -> Decided {
        let (repo, inputs, sites, labels) = discovery_fixture();
        decided_sites(&inputs, &repo, &sites, &labels)
    }

    const WEB: &str = "services/web/src/";

    fn at(decided: &Decided) -> Vec<(String, Option<String>)> {
        decided
            .sites
            .iter()
            .map(|site| {
                (
                    format!("{}:{}", site.file.trim_start_matches(WEB), site.line),
                    site.name_text(),
                )
            })
            .collect()
    }

    fn has(sites: &[(String, Option<String>)], at: &str, name: Option<&str>) -> bool {
        sites.contains(&(at.to_string(), name.map(str::to_string)))
    }

    /// A helper called from three pages with three literals is three sites,
    /// at the pages, and none at the helper.
    #[test]
    fn a_helper_called_with_literals_is_decided_at_each_caller() {
        let decided = decided();
        assert!(decided.unplaced.is_empty(), "{:?}", decided.unplaced);
        let sites = at(&decided);
        for (file, name) in [
            ("pages/a.ts:4", "/a"),
            ("pages/b.ts:4", "/b"),
            ("pages/c.ts:4", "/c"),
        ] {
            assert!(has(&sites, file, Some(name)), "{file} {name} in {sites:?}");
        }
        for wrapper in ["api.ts:2", "api.ts:6", "api.ts:10"] {
            assert!(
                !sites.iter().any(|(at, _)| at == wrapper),
                "no site at {wrapper}: {sites:?}"
            );
        }
    }

    /// A wrapper that prefixes `/v1` passes the path on: the prefix is
    /// composed into the caller's site.
    #[test]
    fn a_prefixing_wrapper_composes_its_prefix_into_the_site() {
        let decided = decided();
        let sites = at(&decided);
        assert!(
            has(&sites, "pages/things.ts:4", Some("/v1/things")),
            "{sites:?}"
        );
        let things = decided
            .sites
            .iter()
            .find(|site| site.file.ends_with("pages/things.ts"))
            .unwrap();
        assert_eq!(things.name_args, BTreeSet::from([0]));
        assert_eq!(things.end.at(), "services/web/src/api.ts:2");
        assert_eq!(
            things
                .via
                .iter()
                .map(|via| (via.at.as_str(), via.callee.as_str()))
                .collect::<Vec<_>>(),
            vec![
                ("services/web/src/api.ts:6", "api"),
                ("services/web/src/api.ts:2", "fetch")
            ]
        );
    }

    /// A socket send helper called with two constants is two sites, and the
    /// helper is none.
    #[test]
    fn a_socket_helper_called_with_constants_is_two_sites() {
        let sites = at(&decided());
        assert!(has(&sites, "emit.ts:13", Some("user.joined")), "{sites:?}");
        assert!(has(&sites, "emit.ts:17", Some("user.left")), "{sites:?}");
        assert!(!sites.iter().any(|(at, _)| at == "emit.ts:9"), "{sites:?}");
    }

    /// A handler table registered in a loop is one site per key, at the
    /// function that handles it.
    #[test]
    fn a_handler_table_is_one_site_per_key_at_its_handler() {
        let decided = decided();
        let sites = at(&decided);
        for (file, name) in [
            ("job-table.ts:5", "image.resize"),
            ("job-table.ts:9", "image.thumbnail"),
            ("job-table.ts:13", "image.archive"),
        ] {
            assert!(has(&sites, file, Some(name)), "{file} {name} in {sites:?}");
        }
        assert!(
            !sites.iter().any(|(at, _)| at == "job-table.ts:25"),
            "{sites:?}"
        );
        let resize = decided
            .sites
            .iter()
            .find(|site| site.key.as_deref() == Some("image.resize"))
            .unwrap();
        assert_eq!(resize.handler.as_ref().map(|at| at.line), Some(5));
    }

    /// A path parameter filled only at an entry point is a lead.
    #[test]
    fn a_name_left_open_to_an_entry_point_is_a_lead() {
        let decided = decided();
        assert_eq!(
            decided
                .leads
                .iter()
                .map(|lead| (
                    lead.file.trim_start_matches(WEB).to_string(),
                    lead.line,
                    lead.reason
                ))
                .collect::<Vec<_>>(),
            vec![("api.ts".to_string(), 10, Undecided::NameNeverBound)]
        );
    }

    /// A call whose name only an entry point fills is one possible call,
    /// at the call that holds the open name (carrick#2438).
    #[test]
    fn a_name_nothing_binds_is_a_possible_call() {
        let decided = decided();
        let found = crate::possible_calls::from_sites(&decided, &[]);
        assert_eq!(found.len(), 1, "{found:?}");
        let call = &found[0];
        assert_eq!(call.file_path.trim_start_matches(WEB), "api.ts");
        assert_eq!(call.line_number, 10);
        assert!(call.span_end > call.span_start, "{call:?}");
        assert!(!call.file_path.starts_with('/'), "{call:?}");
        assert_eq!(
            call.unread.reason,
            crate::possible_calls::UnreadReason::NameNeverBound
        );
        assert!(call.labelled_end.is_some(), "{call:?}");
    }

    /// Each request is one site at the call that decides it, every protocol:
    /// the full list on the fixture.
    #[test]
    fn every_request_is_one_site_where_its_name_is_decided() {
        let sites = at(&decided());
        let expected: Vec<(&str, Option<&str>)> = vec![
            ("books-api.ts:4", Some("/books")),
            ("bus.ts:7", None),
            // An import from a sibling package the summaries' value flow
            // does not read: decided here, its text is the record's to state
            // from the compiler's rows (carrick#2435).
            ("choices.ts:10", None),
            ("choices.ts:14", None),
            ("choices.ts:18", None),
            ("client.ts:4", Some("/items")),
            ("deep.ts:26", Some("/deep")),
            ("emit.ts:13", Some("user.joined")),
            ("emit.ts:17", Some("user.left")),
            ("events.ts:6", Some("orders.created")),
            ("job-table.ts:5", Some("image.resize")),
            ("job-table.ts:9", Some("image.thumbnail")),
            ("job-table.ts:13", Some("image.archive")),
            ("pages/a.ts:4", Some("/a")),
            ("pages/b.ts:4", Some("/b")),
            ("pages/c.ts:4", Some("/c")),
            ("pages/things.ts:4", Some("/v1/things")),
            ("presence.ts:4", Some("user.joined")),
            ("store.ts:3", Some("/settings")),
            ("traced-load.ts:4", Some("/traced")),
        ];
        let mut got: Vec<(String, Option<String>)> = sites;
        got.sort();
        let mut want: Vec<(String, Option<String>)> = expected
            .into_iter()
            .map(|(at, name)| (at.to_string(), name.map(str::to_string)))
            .collect();
        want.sort();
        assert_eq!(got, want);
    }

    /// Where discovery lists only the labelled calls, the pass walks up the
    /// call graph and decides the same sites, except where only discovery's
    /// chain reaches the caller (a member of a call's result, which the call
    /// graph does not resolve): there the walk ends at a lead.
    #[test]
    fn a_chain_that_stops_short_is_walked_up() {
        let (repo, inputs, sites, labels) = discovery_fixture();
        let full = decided_sites(&inputs, &repo, &sites, &labels);
        let bottoms: Vec<DiscoverySite> = sites
            .into_iter()
            .filter(|site| site.chain.is_empty())
            .collect();
        let walked = decided_sites(&inputs, &repo, &bottoms, &labels);
        let names = |decided: &Decided| {
            let mut names: Vec<(String, u32, Option<String>)> = decided
                .sites
                .iter()
                .map(|site| (site.file.clone(), site.line, site.name_text()))
                .collect();
            names.sort();
            names
        };
        let only_chain = |name: &(String, u32, Option<String>)| name.0.ends_with("presence.ts");
        let mut expected = names(&full);
        expected.retain(|name| !only_chain(name));
        assert_eq!(names(&walked), expected);
        assert!(names(&full).iter().any(only_chain));
        let leads: Vec<(String, u32)> = walked
            .leads
            .iter()
            .map(|lead| (lead.file.trim_start_matches(WEB).to_string(), lead.line))
            .collect();
        assert_eq!(
            leads,
            vec![("api.ts".to_string(), 10), ("session.ts".to_string(), 7)]
        );
        assert!(!walked.walked.is_empty());
    }

    /// The payload's binding at the deciding call: the argument the call
    /// hands the slot through, or the call below where a wrapper builds it.
    #[test]
    fn a_payload_is_bound_at_the_deciding_call_or_where_a_wrapper_builds_it() {
        let decided = decided();
        let payload = |file: &str, line: u32| {
            decided
                .sites
                .iter()
                .find(|site| site.file.ends_with(file) && site.line == line)
                .unwrap_or_else(|| panic!("{file}:{line}"))
                .payload
                .clone()
        };
        assert_eq!(
            payload("emit.ts", 13),
            Some(PayloadBinding::Arg { arg: 1, key: None })
        );
        assert_eq!(
            payload("presence.ts", 4),
            Some(PayloadBinding::Arg { arg: 1, key: None })
        );
        assert_eq!(
            payload("bus.ts", 7),
            Some(PayloadBinding::Arg { arg: 0, key: None })
        );
        // The store labels no payload for `fetch`.
        assert_eq!(payload("pages/b.ts", 4), None);

        // A wrapper that builds the payload itself, beside the caller that
        // decides the name.
        let fixture =
            std::fs::canonicalize(manifest().join("tests/fixtures/decided-sites")).unwrap();
        let inputs = crate::engine::discover_request_inputs(&fixture);
        let session = std::fs::read_to_string(fixture.join("src/realtime/session.ts")).unwrap();
        let call = "wire.emit(name, { at: Date.now() })";
        let start = session.find(call).unwrap() as u32 + crate::swc_scanner::SWC_SPAN_BASE;
        let end = LabelledEnd {
            package: "@fixture/wire".into(),
            export: "connect".into(),
            member: Some("emit".into()),
            role: "socket".into(),
            op: Some("send".into()),
            file: "src/realtime/session.ts".into(),
            line: 7,
            span_start: start,
            span_end: start + call.len() as u32,
        };
        let bottom = DiscoverySite {
            file: end.file.clone(),
            line: end.line,
            span_start: end.span_start,
            span_end: end.span_end,
            callee: "wire.emit".into(),
            end: Some(end),
            chain: Vec::new(),
            stop: None,
        };
        let answer: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(fixture.join("library-claims.json")).unwrap(),
        )
        .unwrap();
        let decided = decided_sites(
            &inputs,
            &fixture,
            &[bottom],
            &SiteLabel::from_store_answer(&answer),
        );
        assert_eq!(
            decided
                .sites
                .iter()
                .map(|site| (
                    site.file.as_str(),
                    site.line,
                    site.name_text(),
                    site.payload.clone()
                ))
                .collect::<Vec<_>>(),
            vec![(
                "src/realtime/rooms.ts",
                4,
                Some("room:open".to_string()),
                Some(PayloadBinding::BuiltInside {
                    built_inside: "src/realtime/session.ts:7".to_string(),
                    span_start: start,
                    span_end: start + call.len() as u32,
                    arg: 1,
                    key: None
                })
            )]
        );
    }

    /// Every row a summary states for a request this pass decides sits on
    /// one of its decided sites' file and line: the rule is the summaries'
    /// rule, generalised, so where both read a request they agree on where
    /// it is decided.
    #[test]
    fn every_decided_http_site_sits_on_its_summary_row() {
        let (repo, inputs, sites, labels) = discovery_fixture();
        let decided = decided_sites(&inputs, &repo, &sites, &labels);
        let summaries = crate::request_summary::summarize(
            &inputs,
            &crate::client_semantics::LibrarySemantics::default(),
        );
        // The request each row reaches (its file and span start) -> the
        // file and line of every row stated for it.
        let mut rows: HashMap<(String, u32), HashSet<(String, u32)>> = HashMap::new();
        for file in inputs.files.keys() {
            for at in summaries.rows(file).into_iter().flat_map(|at| at.values()) {
                for row in at {
                    rows.entry((relative(&repo, &row.request_at.0), row.request_at.1))
                        .or_default()
                        .insert((relative(&repo, file), row.line));
                }
            }
        }
        // The request each decided HTTP site reaches -> the decided sites.
        let mut sites_of: HashMap<(String, u32), HashSet<(String, u32)>> = HashMap::new();
        for site in decided
            .sites
            .iter()
            .filter(|site| site.end.role == "http_client")
        {
            sites_of
                .entry((site.end.file.clone(), site.end.span_start))
                .or_default()
                .insert((site.file.clone(), site.line));
        }
        let mut compared = 0;
        for (request, lines) in &rows {
            let Some(decided) = sites_of.get(request) else {
                continue;
            };
            for line in lines {
                compared += 1;
                assert!(
                    decided.contains(line),
                    "the summary row at {line:?} is not a decided site of {request:?}: {decided:?}"
                );
            }
        }
        assert!(compared > 0, "no summary row compared: {rows:?}");
    }

    // -----------------------------------------------------------------------
    // Hand-built sites on the `decided-sites` fixture.

    pub(crate) fn decided_fixture() -> PathBuf {
        std::fs::canonicalize(manifest().join("tests/fixtures/decided-sites")).unwrap()
    }

    /// The SWC span of the first `call` in `text`.
    pub(crate) fn span_of(text: &str, call: &str) -> (u32, u32) {
        let start = text.find(call).unwrap() as u32 + crate::swc_scanner::SWC_SPAN_BASE;
        (start, start + call.len() as u32)
    }

    /// The 1-based line of the first `needle` in `text`.
    pub(crate) fn line_of(text: &str, needle: &str) -> u32 {
        text[..text.find(needle).unwrap()].matches('\n').count() as u32 + 1
    }

    /// A label of the fixture's `@fixture/wire` `connect` export.
    pub(crate) fn wire_label(
        member: &str,
        op: &str,
        payload: Option<usize>,
        handler: Option<usize>,
    ) -> SiteLabel {
        SiteLabel {
            package: "@fixture/wire".into(),
            export: "connect".into(),
            member: Some(member.into()),
            role: "socket".into(),
            op: Some(op.into()),
            side: Some("client".into()),
            name: Some(ArgSlot { arg: 0, key: None }),
            payload: payload.map(|arg| ArgSlot { arg, key: None }),
            handler: handler.map(|arg| ArgSlot { arg, key: None }),
            method: None,
        }
    }

    /// The language's own `fetch`, with label v2's method fields.
    pub(crate) fn fetch_label() -> SiteLabel {
        SiteLabel {
            package: "lib:lib.dom.d.ts".into(),
            export: "fetch".into(),
            member: None,
            role: "http_client".into(),
            op: Some("request".into()),
            side: Some("client".into()),
            name: Some(ArgSlot { arg: 0, key: None }),
            payload: None,
            handler: None,
            method: Some(MethodLabel {
                from: Some(ArgSlot {
                    arg: 1,
                    key: Some("method".into()),
                }),
                default: Some("GET".into()),
                literal: None,
            }),
        }
    }

    /// A send to a forked child process: its label names a payload and no
    /// name.
    pub(crate) fn child_send_label() -> SiteLabel {
        SiteLabel {
            package: "node:child_process".into(),
            export: "ChildProcess".into(),
            member: Some("send".into()),
            role: "in_process_bus".into(),
            op: Some("send".into()),
            side: Some("client".into()),
            name: None,
            payload: Some(ArgSlot { arg: 0, key: None }),
            handler: None,
            method: None,
        }
    }

    /// The labelled call `call` of `file` (its text `text`) under `label`.
    pub(crate) fn end_for(label: &SiteLabel, file: &str, text: &str, call: &str) -> LabelledEnd {
        let (span_start, span_end) = span_of(text, call);
        LabelledEnd {
            package: label.package.clone(),
            export: label.export.clone(),
            member: label.member.clone(),
            role: label.role.clone(),
            op: label.op.clone(),
            file: file.into(),
            line: line_of(text, call),
            span_start,
            span_end,
        }
    }

    /// A discovery site that is its own labelled call.
    pub(crate) fn bottom(end: &LabelledEnd, callee: &str) -> DiscoverySite {
        DiscoverySite {
            file: end.file.clone(),
            line: end.line,
            span_start: end.span_start,
            span_end: end.span_end,
            callee: callee.into(),
            end: Some(end.clone()),
            chain: Vec::new(),
            stop: None,
        }
    }

    pub(crate) const ROOMS: &str = "src/slots/rooms.ts";

    /// The calls of `slots/rooms.ts` as discovery lists them, each with its
    /// label: `(call, callee, label)`.
    pub(crate) fn rooms_calls() -> Vec<(&'static str, &'static str, SiteLabel)> {
        let send = wire_label("emit", "send", Some(1), None);
        let on = wire_label("on", "receive", None, Some(1));
        vec![
            (
                "wire.emit(\"room:\" + roomId, { at: 1 })",
                "wire.emit",
                send.clone(),
            ),
            ("fetch(\"/items/1\", init)", "fetch", fetch_label()),
            (
                "wire.on(\"user.joined\", (data) => {\n    console.log(data);\n  })",
                "wire.on",
                on.clone(),
            ),
            ("wire.emit(topic, { shipped })", "wire.emit", send),
            (
                "fetch(\"/items\", { method: \"POST\" })",
                "fetch",
                fetch_label(),
            ),
            ("wire.on(\"user.left\", onLeft)", "wire.on", on),
            ("fetch(\"/items/3\", options)", "fetch", fetch_label()),
            ("worker.send(\"ping\")", "worker.send", child_send_label()),
        ]
    }

    /// The discovery sites and labels of `slots/rooms.ts`.
    pub(crate) fn rooms_sites(text: &str) -> (Vec<DiscoverySite>, Vec<SiteLabel>) {
        let mut sites = Vec::new();
        let mut labels: Vec<SiteLabel> = Vec::new();
        for (call, callee, label) in rooms_calls() {
            sites.push(bottom(&end_for(&label, ROOMS, text, call), callee));
            if !labels.contains(&label) {
                labels.push(label);
            }
        }
        (sites, labels)
    }

    /// A call the call graph leaves unresolved (a log line, a global
    /// function) in a hop's body is not the hop: the chain is placed through
    /// the call that enters the next function, and nothing is walked.
    #[test]
    fn an_unrelated_call_in_a_hop_body_is_not_the_hop() {
        let root = decided_fixture();
        let inputs = crate::engine::discover_request_inputs(&root);
        let session = std::fs::read_to_string(root.join("src/inert/session.ts")).unwrap();
        let presence = std::fs::read_to_string(root.join("src/inert/presence.ts")).unwrap();
        assert!(session.contains("console.log(name);"));
        let send = wire_label("emit", "send", Some(1), None);
        let end = end_for(
            &send,
            "src/inert/session.ts",
            &session,
            "wire.emit(name, data)",
        );
        let call = "openSession(\"ws://chat\").notify(\"user.joined\", { at: 1 })";
        let (span_start, span_end) = span_of(&presence, call);
        let site = DiscoverySite {
            file: "src/inert/presence.ts".into(),
            line: line_of(&presence, call),
            span_start,
            span_end,
            callee: "openSession(\"ws://chat\").notify".into(),
            end: Some(end),
            chain: vec![
                ChainHop {
                    function: "Session.notify".into(),
                    file: "src/inert/session.ts".into(),
                    line: line_of(&session, "notify(name"),
                },
                ChainHop {
                    function: "relay".into(),
                    file: "src/inert/session.ts".into(),
                    line: line_of(&session, "function relay"),
                },
            ],
            stop: None,
        };
        let decided = decided_sites(&inputs, &root, &[site], &[send]);
        assert_eq!(
            decided
                .sites
                .iter()
                .map(|s| (s.file.as_str(), s.line, s.name_text()))
                .collect::<Vec<_>>(),
            vec![("src/inert/presence.ts", 4, Some("user.joined".to_string()))]
        );
        assert!(decided.leads.is_empty(), "{:?}", decided.leads);
        assert!(decided.walked.is_empty(), "{:?}", decided.walked);
    }

    /// A receive site names the function that handles it, written at the
    /// call or declared under the name the call hands, and where the
    /// payload arrives: the handler's argument.
    #[test]
    fn a_receive_site_is_anchored_at_its_handler() {
        let root = decided_fixture();
        let inputs = crate::engine::discover_request_inputs(&root);
        let text = std::fs::read_to_string(root.join(ROOMS)).unwrap();
        let (sites, labels) = rooms_sites(&text);
        let decided = decided_sites(&inputs, &root, &sites, &labels);
        let site = |name: &str| {
            decided
                .sites
                .iter()
                .find(|site| site.name_text().as_deref() == Some(name))
                .unwrap_or_else(|| panic!("{name}"))
        };
        let inline = "(data) => {\n    console.log(data);\n  }";
        let (start, end) = span_of(&text, inline);
        let joined = site("user.joined");
        assert_eq!(
            joined.handler,
            Some(HandlerAt {
                file: ROOMS.into(),
                line: line_of(&text, inline),
                span_start: start,
                span_end: end,
            })
        );
        assert_eq!(
            joined.payload,
            Some(PayloadBinding::Arg { arg: 1, key: None })
        );
        let left = site("user.left");
        assert_eq!(
            left.handler.as_ref().map(|at| (at.file.as_str(), at.line)),
            Some((ROOMS, line_of(&text, "function onLeft")))
        );
        assert_eq!(
            left.payload,
            Some(PayloadBinding::Arg { arg: 1, key: None })
        );
        // A send names no handler.
        assert_eq!(site("/items").handler, None);
    }
}
