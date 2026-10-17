//! Calls the scan can see but cannot name (carrick#2438, part of
//! carrick#2441; the cloud reader is carrick-cloud#2372): a call where a
//! library sends or receives data whose path, event name or method the code
//! does not state. It goes on the index blob as `possible_calls`, labelled,
//! for the agent to read. It is never a row.
//!
//! - **Never an input.** Nothing here feeds findings, the type check, the
//!   matcher or the PR result. The cloud reader shows them in a section of
//!   their own.
//! - **Switch.** Written only when `CARRICK_SITE_QUESTIONS=1`
//!   ([`crate::site_questions::enabled`]). With the switch off the blob has
//!   neither key, byte for byte as before.
//! - **Cap.** At most [`MAX_PER_SERVICE`] rows per service, in a fixed
//!   order; the count past that is `possible_calls_omitted`, so the blob
//!   stays inside the ingest ceilings. One cap, here: every producer of a
//!   possible call (the decided-site leads below, and the site answers that
//!   fall below the fold band, carrick#2439) goes through [`cap`].
//! - **Names.** The keys, the closed list of reasons and the kind and role
//!   words are what the reader reads (`lambdas/mcp-server/src/types.ts`
//!   `PossibleCall`, `utils/possible-calls.ts`). Files are repo-relative.
//!
//! Producers here: a decided site whose name is unstated (a record in
//! [`Bucket::Lead`](crate::request_summary::site_record::Bucket)), and a
//! discovered call whose name no call of this code decides
//! ([`DecidedLead`]).

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

use crate::cloud_storage::CloudRepoData;
use crate::request_summary::decided_sites::{Decided, DecidedLead, LabelledEnd, Undecided};
use crate::request_summary::site_record::{Bucket, SiteRecord};

/// The most possible calls one service's blob carries.
pub const MAX_PER_SERVICE: usize = 2000;

/// Why a call is a possible call: a closed list, one phrase for each in the
/// reader.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UnreadReason {
    /// The name is only known when the program runs.
    NameNeverBound,
    /// The call could go to more than one implementation.
    SeveralImplementations,
    /// Too many implementations to follow.
    ImplementationsOverCap,
    /// The name comes from an import the scan could not read.
    UnresolvedImport,
    /// The function is passed in from elsewhere and nothing says which.
    InjectedCallee,
    /// The receiver's type is not known.
    UntypedReceiver,
    /// An answer was given and fell below the band that states a row.
    BelowBand,
    /// The call was not checked on this scan.
    Unanswered,
}

/// The slot the code does not state, and why.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Unread {
    /// `name` (an event, topic or message name) or `path` (an HTTP path).
    pub slot: String,
    pub reason: UnreadReason,
}

/// The labelled library call a possible call reaches.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LabelledEndRef {
    pub package: String,
    pub export: String,
    pub member: Option<String>,
}

/// One possible call, as the blob carries it (`possible_calls[]`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PossibleCall {
    /// Repo-relative, `/`-separated.
    pub file_path: String,
    pub line_number: u32,
    pub span_start: u32,
    pub span_end: u32,
    /// The protocol (`http`, `graphql`, `pubsub`, `socket`), when the label
    /// says one.
    pub kind: Option<String>,
    /// `producer` or `consumer`, when the label's op says one.
    pub role: Option<String>,
    pub labelled_end: Option<LabelledEndRef>,
    pub unread: Unread,
    /// How many functions the walk entered from the call to the labelled
    /// call.
    pub trace_depth: u32,
}

impl PossibleCall {
    fn sort_key(&self) -> (&str, u32, u32, u32) {
        (
            &self.file_path,
            self.line_number,
            self.span_start,
            self.span_end,
        )
    }
}

/// The protocol word for a store role.
fn kind_of(role: &str) -> Option<&'static str> {
    match role {
        "http_client" | "server_framework" => Some("http"),
        "graphql_client" => Some("graphql"),
        "broker" | "in_process_bus" => Some("pubsub"),
        "socket" => Some("socket"),
        _ => None,
    }
}

/// `producer` or `consumer` from the label's role and op. An HTTP client
/// that requests consumes a route; a server that receives produces one;
/// elsewhere a send produces and a receive consumes.
fn role_of(role: &str, op: Option<&str>) -> Option<&'static str> {
    let http = matches!(role, "http_client" | "graphql_client" | "server_framework");
    match (op?, http) {
        ("request" | "send", true) => Some("consumer"),
        ("receive", true) => Some("producer"),
        ("send", false) => Some("producer"),
        ("receive", false) => Some("consumer"),
        _ => None,
    }
}

fn labelled_end(end: &LabelledEnd) -> LabelledEndRef {
    LabelledEndRef {
        package: end.package.clone(),
        export: end.export.clone(),
        member: end.member.clone(),
    }
}

fn slot_of(role: &str) -> &'static str {
    if matches!(role, "http_client" | "server_framework") {
        "path"
    } else {
        "name"
    }
}

/// Why a decided site's name is unstated: an identifier the compiler reads
/// as `any` with no declaration is an import the scan could not read.
/// Otherwise nothing says, and the call was not checked on this scan.
fn reason_of(record: &SiteRecord) -> UnreadReason {
    let unreadable = record.state.compiler.names.iter().any(|name| {
        name.declaration.is_none()
            && name.package.is_none()
            && name.type_text.as_deref() == Some("any")
    });
    if unreadable {
        UnreadReason::UnresolvedImport
    } else {
        UnreadReason::Unanswered
    }
}

fn reason_of_undecided(reason: Undecided) -> UnreadReason {
    match reason {
        // A parameter of a callback this code does not call is, like a
        // parameter nothing fills, a name only the running program knows.
        Undecided::NameNeverBound | Undecided::CallbackParameter => UnreadReason::NameNeverBound,
    }
}

fn from_undecided(lead: &DecidedLead) -> PossibleCall {
    PossibleCall {
        file_path: lead.file.clone(),
        line_number: lead.line,
        span_start: lead.span_start,
        span_end: lead.span_end,
        kind: kind_of(&lead.end.role).map(str::to_string),
        role: role_of(&lead.end.role, lead.end.op.as_deref()).map(str::to_string),
        labelled_end: Some(labelled_end(&lead.end)),
        unread: Unread {
            slot: slot_of(&lead.end.role).to_string(),
            reason: reason_of_undecided(lead.reason),
        },
        trace_depth: lead.depth,
    }
}

/// The possible calls a service's decided sites hold: the records in the
/// lead bucket, and the discovered calls no call of this code decides.
/// Unordered and uncapped; [`cap`] orders and bounds them.
pub fn from_sites(decided: &Decided, records: &[SiteRecord]) -> Vec<PossibleCall> {
    let mut out: Vec<PossibleCall> = Vec::new();
    for record in records.iter().filter(|r| r.bucket == Bucket::Lead) {
        let at = &record.anchors.call;
        // The record's own decided site carries the labelled end and the
        // depth the record does not.
        let site = decided.sites.iter().find(|site| {
            site.file == at.file && site.span_start == at.span_start && site.span_end == at.span_end
        });
        out.push(PossibleCall {
            file_path: at.file.clone(),
            line_number: at.line,
            span_start: at.span_start,
            span_end: at.span_end,
            kind: site.and_then(|s| kind_of(&s.end.role)).map(str::to_string),
            role: site
                .and_then(|s| role_of(&s.end.role, s.end.op.as_deref()))
                .map(str::to_string),
            labelled_end: site.map(|s| labelled_end(&s.end)),
            unread: Unread {
                slot: site.map_or("name", |s| slot_of(&s.end.role)).to_string(),
                reason: reason_of(record),
            },
            trace_depth: site.map_or(0, |s| s.via.len() as u32),
        });
    }
    out.extend(decided.leads.iter().map(from_undecided));
    out
}

/// Order the rows (file, line, span), drop a repeat of one call, and keep
/// [`MAX_PER_SERVICE`]. Returns the rows kept and how many were left out.
pub fn cap(mut rows: Vec<PossibleCall>) -> (Vec<PossibleCall>, usize) {
    rows.sort_by(|a, b| a.sort_key().cmp(&b.sort_key()));
    let mut seen: BTreeSet<(String, u32, u32)> = BTreeSet::new();
    rows.retain(|row| seen.insert((row.file_path.clone(), row.span_start, row.span_end)));
    let omitted = rows.len().saturating_sub(MAX_PER_SERVICE);
    rows.truncate(MAX_PER_SERVICE);
    (rows, omitted)
}

/// Write a service's possible calls on its blob when the site-question
/// switch is on; with it off the blob is left as it was.
pub fn attach(data: &mut CloudRepoData, rows: Vec<PossibleCall>) {
    attach_when(crate::site_questions::enabled(), data, rows);
}

/// [`attach`] with the switch given.
pub fn attach_when(on: bool, data: &mut CloudRepoData, rows: Vec<PossibleCall>) {
    if !on {
        return;
    }
    let (rows, omitted) = cap(rows);
    data.possible_calls = Some(rows);
    data.possible_calls_omitted = (omitted > 0).then_some(omitted as u32);
}
