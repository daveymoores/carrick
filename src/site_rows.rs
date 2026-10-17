//! Site answers become index rows (carrick#2439, part of carrick#2441), and
//! fold with the file reader's rows at the same site.
//!
//! Every decided site's record ([`SiteRecord`]) either decides each slot
//! itself (bucket `forced`), asks the cloud (`asked`, through
//! [`crate::site_questions`]), or names nothing (`lead`). This module turns
//! the first two into rows on the index's operation keys and folds them with
//! the rows the file reader stated at the same file, line and side.
//!
//! - **Kind to key.** `http` is an HTTP key; `topic` a pub/sub key with no
//!   name scope; `message` and `event` pub/sub keys scoped `global` and
//!   `service`, as library-claim rows are; `socket_event` a socket key whose
//!   direction is read from the label's side: a send on a server's socket
//!   goes server to client, a send on a client's client to server, and a
//!   handler the other way. A side the label does not state (`both`, or one
//!   decided by origin) is `Unknown`.
//! - **Provenance** (decided on carrick#2439, 2026-10-16). A row whose every
//!   slot is forced is a fact: `request_summary` for an HTTP send,
//!   `library_claim` for the rest. A row with any asked slot is `model`, a
//!   candidate, and carries [`Inference`]. No new resolution source.
//! - **The fold** (RULED by David, 2026-10-16). At one site and side: a fact
//!   the scan already states keeps the site and the site row is dropped. A
//!   fact site row replaces the reader's rows. An answer replaces them when
//!   its band slot is at or above [`BAND`] with both option orders agreeing;
//!   otherwise the reader's rows stand and carry the answer as
//!   `inference.alternative`. With no row at the site an answer in the band
//!   stands, and one below it is a possible call
//!   ([`crate::possible_calls`], reason `below_band`).
//! - **The band slot.** The ruling reads the kind slot. A site whose kind
//!   the code forces is read on its name slot instead, when the name is
//!   asked, so a forced kind never lets a low-confidence name through; with
//!   neither asked, nothing in the key was the model's and it is in band.
//! - **Unanswered.** An asked site the cloud did not answer states no row
//!   and withdraws none; it is a possible call (`unanswered`).
//! - **Switch.** Nothing here runs unless `CARRICK_SITE_QUESTIONS=1`
//!   ([`SiteRows::enabled`]); with it off the blob is byte for byte as
//!   before.

use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::agents::file_analyzer_agent::ResolutionSource;
use crate::analyzer::ApiEndpointDetails;
use crate::cloud_storage::CloudRepoData;
use crate::operation::{OperationKey, SocketDirection};
use crate::possible_calls::{PossibleCall, UnreadReason};
use crate::request_summary::decided_sites::{Decided, DecidedSite, SiteLabel};
use crate::request_summary::site_record::{Bucket, SiteRecord, TypeAnchors};
use crate::site_questions::{Answered, SiteAnswers, SiteQuestion, Slot, SlotAnswer, SlotValue};

/// The fold band (RULED by David, 2026-10-16): an answer replaces the
/// reader's row at this confidence or above, with both option orders
/// agreeing. In the concept's run 2 the kind answers in this band were right
/// 43 times of 43.
pub const BAND: f64 = 0.8;

/// Who inferred a row.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InferenceBy {
    /// The site questions (carrick#2437).
    SiteQuestion,
}

/// What a site answer says about the row it stated.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SiteInference {
    pub by: InferenceBy,
    /// The cloud's question-set id the answer was given under.
    pub question_set: Option<String>,
    /// The band slot's confidence ([`BAND`]).
    pub confidence: f64,
    /// Whether both option orders picked the same option for that slot.
    pub orders_agree: bool,
}

/// Which side of the index a row sits on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RowSide {
    /// A call: an HTTP request, a publish, a socket send.
    Call,
    /// An endpoint: an HTTP route, a subscriber, a socket listener.
    Endpoint,
}

/// A site answer a reader row kept its place against: the answer, and the
/// operation it named.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Alternative {
    #[serde(flatten)]
    pub answer: SiteInference,
    pub side: RowSide,
    #[serde(flatten)]
    pub key: OperationKey,
}

/// `inference` on an index row (carrick#2439): on a row a site answer
/// stated, the answer; on a reader row an answer below the band disagreed
/// with, only `alternative`. Absent on every other row.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Inference {
    #[serde(flatten)]
    pub answer: Option<SiteInference>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub alternative: Option<Alternative>,
}

/// One row a site states.
#[derive(Debug, Clone, PartialEq)]
pub struct SiteRow {
    /// Repo-relative, `/`-separated.
    pub file: String,
    pub line: u32,
    pub key: OperationKey,
    pub side: RowSide,
    pub source: ResolutionSource,
    /// `None` on a fact.
    pub inference: Option<SiteInference>,
    pub in_band: bool,
    pub name_scope: Option<carrick_match::NameScope>,
    pub anchors: TypeAnchors,
    /// The possible call the site is when it stands nowhere.
    pub lead: PossibleCall,
}

impl SiteRow {
    fn fact(&self) -> bool {
        self.inference.is_none()
    }
}

/// A service's site rows and possible calls, before the fold.
#[derive(Debug, Clone, Default)]
pub struct SiteRows {
    /// Whether the switch was on: off, nothing folds and nothing is written.
    pub enabled: bool,
    pub rows: Vec<SiteRow>,
    /// The sites that are possible calls whatever the fold does: leads,
    /// unanswered sites, and sites whose answer states no row.
    pub leads: Vec<PossibleCall>,
}

/// The id a site is asked under: its file and call span, unique in a run.
pub fn site_id(anchors: &TypeAnchors) -> String {
    format!(
        "{}:{}-{}",
        anchors.call.file, anchors.call.span_start, anchors.call.span_end
    )
}

/// The questions a service's records ask: one per record that asks a slot.
pub fn questions(records: &[SiteRecord]) -> Vec<SiteQuestion> {
    records
        .iter()
        .filter(|record| record.bucket == Bucket::Asked)
        .map(|record| SiteQuestion {
            site_id: site_id(&record.anchors),
            state: record.state.clone(),
        })
        .collect()
}

/// A slot's value: forced (confidence 1, both orders), answered, or `None`
/// when the record neither forces nor asks it.
#[derive(Debug, Clone, PartialEq)]
struct Filled {
    text: Option<String>,
    confidence: f64,
    orders_agree: bool,
    asked: bool,
}

fn filled(record: &SiteRecord, answered: Option<&Answered>, slot: Slot) -> Option<Filled> {
    if let Some(text) = record.state.forced.get(&slot) {
        return Some(Filled {
            text: Some(text.clone()),
            confidence: 1.0,
            orders_agree: true,
            asked: false,
        });
    }
    let answer: &SlotAnswer = match slot {
        Slot::Kind => answered?.kind.as_ref(),
        Slot::Role => answered?.role.as_ref(),
        Slot::Name => answered?.name.as_ref(),
        Slot::Method => answered?.method.as_ref(),
        Slot::Namespace => answered?.namespace.as_ref(),
        Slot::NameArgument => answered?.name_argument.as_ref(),
        Slot::PayloadArgument => answered?.payload_argument.as_ref(),
    }?;
    Some(Filled {
        text: match &answer.value {
            Some(SlotValue::Text(text)) => Some(text.clone()),
            _ => None,
        },
        confidence: answer.confidence,
        orders_agree: answer.orders_agree,
        asked: true,
    })
}

/// The socket direction a call or handler on a socket the label says is a
/// `side` end takes. `None` for a side the label does not state.
fn direction(side: Option<&str>, row: RowSide) -> SocketDirection {
    match (side, row) {
        (Some("server"), RowSide::Call) | (Some("client"), RowSide::Endpoint) => {
            SocketDirection::ServerToClient
        }
        (Some("client"), RowSide::Call) | (Some("server"), RowSide::Endpoint) => {
            SocketDirection::ClientToServer
        }
        _ => SocketDirection::Unknown,
    }
}

/// The key a kind names for `name`, and its name scope.
fn key_of(
    kind: &str,
    name: String,
    method: Option<&str>,
    namespace: Option<String>,
    side: Option<&str>,
    row: RowSide,
) -> Option<(OperationKey, Option<carrick_match::NameScope>)> {
    let scoped = |scope: &str| carrick_match::NameScope {
        scope: scope.to_string(),
        namespace: namespace.clone(),
    };
    Some(match kind {
        "http" => (OperationKey::http(method.unwrap_or_default(), name), None),
        "topic" => (OperationKey::pubsub(name), None),
        "message" => (
            OperationKey::pubsub(name),
            Some(scoped(carrick_match::GLOBAL_SCOPE)),
        ),
        "event" => (
            OperationKey::pubsub(name),
            Some(scoped(carrick_match::SERVICE_SCOPE)),
        ),
        "socket_event" => (OperationKey::socket(name, direction(side, row)), None),
        _ => return None,
    })
}

/// The decided site a record was built for.
fn site_of<'d>(decided: &'d Decided, anchors: &TypeAnchors) -> Option<&'d DecidedSite> {
    decided.sites.iter().find(|site| {
        site.file == anchors.call.file
            && site.span_start == anchors.call.span_start
            && site.span_end == anchors.call.span_end
    })
}

/// The rows and possible calls a service's records come to, read with the
/// labels the store answered and the cloud's answers. The lead-bucket
/// records and the undecided calls are [`crate::possible_calls::from_sites`]'s.
pub fn of(
    decided: &Decided,
    records: &[SiteRecord],
    labels: &[SiteLabel],
    answers: &SiteAnswers,
) -> SiteRows {
    let mut out = SiteRows {
        enabled: true,
        rows: Vec::new(),
        leads: crate::possible_calls::from_sites(decided, records),
    };
    for record in records {
        if record.bucket == Bucket::Lead {
            continue;
        }
        let Some(site) = site_of(decided, &record.anchors) else {
            continue;
        };
        let lead = |reason| crate::possible_calls::from_site(site, reason);
        let answered = answers.answered.get(&site_id(&record.anchors));
        if record.bucket == Bucket::Asked && answered.is_none() {
            out.leads.push(lead(UnreadReason::Unanswered));
            continue;
        }
        let slot = |slot| filled(record, answered, slot);
        let (Some(kind), Some(role), Some(name)) =
            (slot(Slot::Kind), slot(Slot::Role), slot(Slot::Name))
        else {
            out.leads.push(lead(UnreadReason::BelowBand));
            continue;
        };
        let (Some(kind_text), Some(role_text), Some(name_text)) =
            (kind.text.clone(), role.text.clone(), name.text.clone())
        else {
            // The model chose none of the options for a slot the key needs.
            out.leads.push(lead(UnreadReason::BelowBand));
            continue;
        };
        let side = match role_text.as_str() {
            "sends" => RowSide::Call,
            "handles" => RowSide::Endpoint,
            // `neither` is no network call, and a scope site names a
            // prefix, not an operation.
            _ => continue,
        };
        let label = SiteLabel::find(labels, &site.end);
        let method = slot(Slot::Method).and_then(|method| method.text);
        let namespace = slot(Slot::Namespace).and_then(|namespace| namespace.text);
        let Some((key, name_scope)) = key_of(
            &kind_text,
            name_text,
            method.as_deref(),
            namespace,
            label.and_then(|label| label.side.as_deref()),
            side,
        ) else {
            out.leads.push(lead(UnreadReason::BelowBand));
            continue;
        };
        let fact = record.bucket == Bucket::Forced;
        let band = if kind.asked {
            Some(&kind)
        } else if name.asked {
            Some(&name)
        } else {
            None
        };
        let (confidence, orders_agree) =
            band.map_or((1.0, true), |slot| (slot.confidence, slot.orders_agree));
        let source = match (fact, &key) {
            (true, OperationKey::Http { .. }) if side == RowSide::Call => {
                ResolutionSource::RequestSummary
            }
            (true, _) => ResolutionSource::LibraryClaim,
            (false, _) => ResolutionSource::Model,
        };
        out.rows.push(SiteRow {
            file: site.file.clone(),
            line: site.line,
            key,
            side,
            source,
            inference: (!fact).then(|| SiteInference {
                by: InferenceBy::SiteQuestion,
                question_set: answers.question_set.clone(),
                confidence,
                orders_agree,
            }),
            in_band: fact || (confidence >= BAND && orders_agree),
            name_scope,
            anchors: record.anchors.clone(),
            lead: lead(UnreadReason::BelowBand),
        });
    }
    out
}

/// What the fold did, for the type manifest and the possible calls.
#[derive(Debug, Clone, Default)]
pub struct SiteFold {
    /// The site rows the index now holds, which the type manifest anchors
    /// at their records' anchors.
    pub stated: Vec<SiteRow>,
    /// The reader rows the fold replaced, as `(file, line, key, side)`:
    /// their manifest entries would anchor an operation no row holds.
    pub withdrawn: Vec<(String, u32, OperationKey, RowSide)>,
    /// Every possible call: the leads, and the answers below the band at a
    /// site with no row.
    pub leads: Vec<PossibleCall>,
}

/// `file` relative to `root`, `/`-separated, whichever form it is held in
/// ([`crate::utils::repo_relative_source_path`]).
pub fn relative(root: &Path, file: &str) -> String {
    crate::utils::repo_relative_source_path(file, &root.to_string_lossy()).replace('\\', "/")
}

/// A row's `file:line`, relative to `root`.
fn place(root: &Path, row: &ApiEndpointDetails) -> Option<(String, u32)> {
    let text = row.file_path.to_string_lossy();
    let (file, line) = text.rsplit_once(':')?;
    Some((relative(root, file), line.parse().ok()?))
}

/// Fold `site`'s rows into `data` (module docs). `make` builds a row the
/// way the index builds a protocol row of that side. Off, nothing changes.
pub fn fold(
    data: &mut CloudRepoData,
    site: &SiteRows,
    root: &Path,
    make: impl Fn(OperationKey, &Path, u32, RowSide) -> ApiEndpointDetails,
) -> SiteFold {
    fold_rows(&mut data.calls, &mut data.endpoints, site, root, make)
}

/// [`fold`] over a blob's calls and endpoints.
pub fn fold_rows(
    calls: &mut Vec<ApiEndpointDetails>,
    endpoints: &mut Vec<ApiEndpointDetails>,
    site: &SiteRows,
    root: &Path,
    make: impl Fn(OperationKey, &Path, u32, RowSide) -> ApiEndpointDetails,
) -> SiteFold {
    let mut out = SiteFold::default();
    if !site.enabled {
        return out;
    }
    out.leads = site.leads.clone();
    for row in &site.rows {
        let rows = match row.side {
            RowSide::Call => &mut *calls,
            RowSide::Endpoint => &mut *endpoints,
        };
        let here: Vec<usize> = rows
            .iter()
            .enumerate()
            .filter(|(_, existing)| {
                place(root, existing)
                    .is_some_and(|(file, line)| file == row.file && line == row.line)
            })
            .map(|(index, _)| index)
            .collect();
        let reader = |existing: &ApiEndpointDetails| {
            existing.resolution_source == Some(ResolutionSource::Model)
                && existing.inference.is_none()
        };
        // A fact the scan states keeps the site.
        if here.iter().any(|&index| !reader(&rows[index])) {
            continue;
        }
        let readers: Vec<usize> = here;
        let replaces = row.fact() || row.in_band;
        if !readers.is_empty() && !replaces {
            // The reader's rows stand, carrying the answer.
            let answer = row
                .inference
                .clone()
                .expect("a candidate carries its answer");
            for index in readers {
                rows[index].inference = Some(Inference {
                    answer: None,
                    alternative: Some(Alternative {
                        answer: answer.clone(),
                        side: row.side,
                        key: row.key.clone(),
                    }),
                });
            }
            continue;
        }
        if readers.is_empty() && !replaces {
            out.leads.push(row.lead.clone());
            continue;
        }
        // Replaced: the reader's rows go, and the site row stands.
        let mut withdrawn: Vec<usize> = readers;
        withdrawn.sort_unstable();
        for index in withdrawn.into_iter().rev() {
            let gone = rows.remove(index);
            out.withdrawn
                .push((row.file.clone(), row.line, gone.key, row.side));
        }
        let mut stated = make(row.key.clone(), Path::new(&row.file), row.line, row.side);
        stated.resolution_source = Some(row.source);
        stated.name_scope = row.name_scope.clone();
        stated.inference = row.inference.clone().map(|answer| Inference {
            answer: Some(answer),
            alternative: None,
        });
        rows.push(stated);
        out.stated.push(row.clone());
    }
    out
}

#[cfg(test)]
mod tests;
