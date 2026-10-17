//! Site questions on the wire (carrick#2437, part of #2441): the scanner sends
//! each discovered site's record and reads back the model's answers.
//!
//! The model runs in the cloud, because its key is a cloud secret
//! (carrick-cloud#2380 is the lambda). This module is the contract and the
//! client. The record is built by #2435 and the answers become rows in #2439;
//! neither is here.
//!
//! - **Switch.** Nothing is sent unless `CARRICK_SITE_QUESTIONS=1`
//!   ([`enabled`]). With it unset the scanner sends nothing new, and every
//!   site is unanswered.
//! - **Route.** `POST /site-questions`, through the agent service's one path
//!   ([`crate::agent_service::AgentService::post_to_lambda`]): the scanner's
//!   credential, the `X-Carrick-Scanner-Version` gate, the pacer, and the
//!   request-level retry on the envelope's `retriable`. A 2xx body is the
//!   answer itself ([`crate::agent_service::SuccessBody::of`]).
//! - **Request.** `{ repo, service, question_set, sites: [{ site_id, state }] }`,
//!   at most [`MAX_SITES_PER_REQUEST`] sites and [`MAX_REQUEST_BYTES`] per
//!   request ([`batches`]). `question_set` is the id this run last received,
//!   or null.
//! - **State.** The site record ([`SiteState`]) with no static question text:
//!   the stated values travel as places, and the cloud reads them out. Which
//!   slots are asked is part of the record, so it is part of the cloud's cache
//!   key.
//! - **Response.** `{ question_set, answers }`, one per site: answered, with
//!   one [`SlotAnswer`] per asked slot, or failed with a code and whether
//!   asking again can help ([`SiteAnswer`]). Each answer is read on its own,
//!   so one unreadable answer costs only its site.
//! - **Failure.** A failed request, a failed site, a budget refusal or a
//!   timeout leaves that site unanswered ([`SiteAnswers::unanswered`]): it
//!   states no row and withdraws no reader row. No scan-health entry and no
//!   customer text. Only a site the cloud called retriable is asked again,
//!   once, in one more round of requests ([`ask`]).
//! - **Cache.** The cloud's, keyed by workspace, question set and the
//!   record's sha. The scanner stores no answer in the blob, so no
//!   `CACHE_VERSION` pairing: a question change moves `question_set` and the
//!   next scan asks again by itself.
//!
//! Both repos test against one example: `tests/fixtures/site_questions/contract.json`
//! here, vendored byte for byte as carrick-cloud's
//! `lambdas/site-questions/fixtures/contract.json`.

use std::collections::{BTreeMap, BTreeSet};
use std::future::Future;
use std::time::Duration;

use futures::stream::{self, StreamExt};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tracing::debug;

use crate::agent_service::{AgentCallError, AgentService};

/// The route.
pub const ROUTE: &str = "/site-questions";

/// The switch (#2439): on only when set to `1`.
pub const SWITCH: &str = "CARRICK_SITE_QUESTIONS";

/// The offline answer's key: `<CARRICK_MOCK_FIXTURE_DIR>/site-questions/default.json`.
pub const MOCK_SEED: &str = "default";

/// The most sites one request carries (the cloud refuses more).
pub const MAX_SITES_PER_REQUEST: usize = 50;

/// The most bytes one request's body holds (the cloud refuses more). The
/// Lambda sync payload cap is 6 MB and API Gateway allows 30 s.
pub const MAX_REQUEST_BYTES: usize = 2 * 1024 * 1024;

/// How long one request may take, its transport retries included, before its
/// sites are left unanswered.
pub const ASK_TIMEOUT: Duration = Duration::from_secs(60);

/// Requests in flight at once.
const IN_FLIGHT: usize = 4;

/// The agent service site questions are sent through: its retries sleep on
/// the run's retry budget ([`crate::retry_budget`]), never a budget of
/// their own.
pub fn agent() -> AgentService {
    AgentService::new().with_retry_policy(crate::agent_service::RetryPolicy::RUN_BUDGETED)
}

/// Whether the switch is on in this process.
pub fn enabled() -> bool {
    enabled_from(std::env::var(SWITCH).ok().as_deref())
}

/// Whether a value of [`SWITCH`] turns site questions on: `1` only.
pub fn enabled_from(value: Option<&str>) -> bool {
    value == Some("1")
}

/// A slot the record can ask or state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Slot {
    Kind,
    Role,
    Name,
    NameArgument,
    PayloadArgument,
    Method,
    Namespace,
}

/// Where the site is: `at` is a repo-relative `file:line`, `code` the source
/// around it with line numbers.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Point {
    pub at: String,
    pub code: String,
}

/// The call at the site, for the argument questions: its source and its
/// arguments' source texts in order.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SiteCall {
    pub text: String,
    pub at: String,
    pub args: Vec<String>,
}

/// Where a value is stated.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PlaceFrom {
    /// The literal the checker gives a name (`name`), at the site or, with
    /// `via`, in a function the site's calls reach.
    Checker,
    /// A string literal, at the site or (with `via`) in a reached function.
    StringLiteral,
    TemplateLiteral,
    ObjectKey,
    /// The request method a library label says `export.member` sends.
    LabelMethod,
    /// The method that label says it sends when none is given.
    LabelMethodDefault,
}

/// One place a value is stated.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Place {
    pub from: PlaceFrom,
    pub at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub via: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub export: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub member: Option<String>,
}

/// One value the code states, with every place it is stated (at most four);
/// the first place is the one the option names.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StatedValue {
    pub text: String,
    pub places: Vec<Place>,
}

/// The stated values: at most 24 each, each text at most 120 characters and
/// unique in its list. A request-method word is offered only as a method.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Stated {
    pub names: Vec<StatedValue>,
    pub methods: Vec<StatedValue>,
}

/// The site record (#2435) as the wire carries it.
///
/// `compiler`, `syntax`, `libraries` and `package_network_roles` are the
/// record's own; the wire carries them as they are. `package_network_roles` is
/// absent when there are none, never null; `call` is null when the site has
/// none, never absent. `asked` is never empty: a record with nothing to ask is
/// not sent. A slot is in `asked` or in `forced`, never both, and no path in
/// any field is absolute. A forced `role` is `neither`, `sends`, `handles`, or
/// `scope` for a scope site (#2474), whose open prefix is asked in `name`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SiteState {
    pub point: Point,
    pub compiler: Value,
    pub syntax: Value,
    pub libraries: Value,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub package_network_roles: Option<Value>,
    pub call: Option<SiteCall>,
    pub stated: Stated,
    pub asked: Vec<Slot>,
    pub forced: BTreeMap<Slot, String>,
}

/// One site to ask about. `site_id` is the scanner's own handle for the
/// site, unique in a run; the cloud reads nothing into it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SiteQuestion {
    pub site_id: String,
    pub state: SiteState,
}

/// One request's body.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SiteQuestionsRequest {
    pub repo: String,
    pub service: Option<String>,
    pub question_set: Option<String>,
    pub sites: Vec<SiteQuestion>,
}

/// An answered slot's value: a stated value's text (name, method,
/// namespace), an option's id (kind, role), or an argument's index from 0
/// (name_argument, payload_argument).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum SlotValue {
    Index(u32),
    Text(String),
}

/// One answered slot. `value` is `None` when the model chose none of the
/// options; `orders_agree` says whether both option orders picked the same
/// one.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SlotAnswer {
    pub value: Option<SlotValue>,
    pub confidence: f64,
    pub orders_agree: bool,
}

/// An answered site: one slot per asked slot the model was asked, `None` for
/// a slot not asked.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Answered {
    pub kind: Option<SlotAnswer>,
    pub role: Option<SlotAnswer>,
    pub name: Option<SlotAnswer>,
    pub method: Option<SlotAnswer>,
    pub namespace: Option<SlotAnswer>,
    pub name_argument: Option<SlotAnswer>,
    pub payload_argument: Option<SlotAnswer>,
}

/// A site with no answer this scan, and why.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Unanswered {
    pub code: String,
    pub retriable: bool,
}

/// One element of a response's `answers`. The answered side is boxed: it is
/// seven slots against a failure's code and flag.
#[derive(Debug, Clone, PartialEq)]
pub enum SiteAnswer {
    Answered(Box<Answered>),
    Failed(Unanswered),
}

/// What a scan's site questions came to.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct SiteAnswers {
    /// The cloud's question-set id, from the last response read.
    pub question_set: Option<String>,
    pub answered: BTreeMap<String, Answered>,
    pub unanswered: BTreeMap<String, Unanswered>,
    /// Requests sent.
    pub requests: usize,
}

/// A site the scanner did not send: its own request would be over
/// [`MAX_REQUEST_BYTES`].
pub const TOO_LARGE_CODE: &str = "too_large";
/// A response that held no readable answer for a site it was asked about.
pub const NO_ANSWER_CODE: &str = "no_answer";
/// A request that did not answer within [`ASK_TIMEOUT`].
pub const TIMEOUT_CODE: &str = "timeout";

/// A response, each answer kept for [`parse_answer`] to read on its own.
#[derive(Deserialize)]
struct RawResponse {
    #[serde(default)]
    question_set: Option<String>,
    #[serde(default)]
    answers: Vec<Value>,
}

/// The answers a response body holds, by site id, and its question-set id.
/// An element that cannot be read is skipped, and its site reads as
/// unanswered. A body that is not the response's shape holds none.
pub fn parse_response(text: &str) -> (Option<String>, BTreeMap<String, SiteAnswer>) {
    let Ok(raw) = serde_json::from_str::<RawResponse>(text) else {
        return (None, BTreeMap::new());
    };
    let answers = raw
        .answers
        .iter()
        .filter_map(|element| {
            let site_id = element.get("site_id")?.as_str()?.to_string();
            Some((site_id, parse_answer(element)?))
        })
        .collect();
    (raw.question_set, answers)
}

/// One element of `answers`, or `None` when it is neither shape.
pub fn parse_answer(element: &Value) -> Option<SiteAnswer> {
    match element.get("ok")?.as_bool()? {
        true => serde_json::from_value::<Answered>(element.clone())
            .ok()
            .map(|answered| SiteAnswer::Answered(Box::new(answered))),
        false => Some(SiteAnswer::Failed(Unanswered {
            code: element.get("code")?.as_str()?.to_string(),
            retriable: element.get("retriable")?.as_bool()?,
        })),
    }
}

/// The bytes `site` adds to a request's body.
fn site_bytes(site: &SiteQuestion) -> usize {
    serde_json::to_vec(site)
        .map(|b| b.len())
        .unwrap_or(usize::MAX)
}

/// Split `sites` into requests of at most [`MAX_SITES_PER_REQUEST`] sites and
/// [`MAX_REQUEST_BYTES`] of body, in order. A site whose own request would be
/// over the byte ceiling is returned apart and never sent.
pub fn batches(
    repo: &str,
    service: Option<&str>,
    question_set: Option<&str>,
    sites: Vec<SiteQuestion>,
) -> (Vec<SiteQuestionsRequest>, Vec<SiteQuestion>) {
    let empty = SiteQuestionsRequest {
        repo: repo.to_string(),
        service: service.map(str::to_string),
        question_set: question_set.map(str::to_string),
        sites: Vec::new(),
    };
    let envelope = serde_json::to_vec(&empty).map(|b| b.len()).unwrap_or(0);
    let mut out: Vec<SiteQuestionsRequest> = Vec::new();
    let mut too_large = Vec::new();
    let mut current = empty.clone();
    let mut bytes = envelope;
    for site in sites {
        let size = site_bytes(&site);
        if envelope.saturating_add(size) > MAX_REQUEST_BYTES {
            too_large.push(site);
            continue;
        }
        // One comma between sites.
        let added = size + usize::from(!current.sites.is_empty());
        if current.sites.len() == MAX_SITES_PER_REQUEST || bytes + added > MAX_REQUEST_BYTES {
            out.push(std::mem::replace(&mut current, empty.clone()));
            bytes = envelope;
        }
        bytes += size + usize::from(!current.sites.is_empty());
        current.sites.push(site);
    }
    if !current.sites.is_empty() {
        out.push(current);
    }
    (out, too_large)
}

/// One round: every request through `send`, [`IN_FLIGHT`] at a time. The
/// sites each request carried, with what came back for each.
async fn round<S, Fut>(
    requests: Vec<SiteQuestionsRequest>,
    send: &S,
) -> Vec<(
    Vec<SiteQuestion>,
    Result<(Option<String>, BTreeMap<String, SiteAnswer>), Unanswered>,
)>
where
    S: Fn(SiteQuestionsRequest) -> Fut,
    Fut: Future<Output = Result<String, AgentCallError>>,
{
    stream::iter(requests)
        .map(|request| async move {
            let sites = request.sites.clone();
            let result = match tokio::time::timeout(ASK_TIMEOUT, send(request)).await {
                Ok(Ok(text)) => Ok(parse_response(&text)),
                // The transport has already retried what it could.
                Ok(Err(error)) => Err(Unanswered {
                    code: error.code,
                    retriable: false,
                }),
                Err(_) => Err(Unanswered {
                    code: TIMEOUT_CODE.to_string(),
                    retriable: false,
                }),
            };
            (sites, result)
        })
        .buffer_unordered(IN_FLIGHT)
        .collect()
        .await
}

/// Ask about `sites` through `send`, which puts one request on the wire and
/// returns its 2xx body.
///
/// Every site ends answered or unanswered. A site the cloud answers with a
/// retriable failure is asked once more, in a second round carrying only
/// those sites; still failing, it is unanswered. Two sites with one id: the
/// first is asked, the second is dropped. This reads no switch; the caller
/// checks [`enabled`] ([`ask_service`] does).
pub async fn ask<S, Fut>(
    repo: &str,
    service: Option<&str>,
    sites: Vec<SiteQuestion>,
    send: S,
) -> SiteAnswers
where
    S: Fn(SiteQuestionsRequest) -> Fut,
    Fut: Future<Output = Result<String, AgentCallError>>,
{
    ask_within(repo, service, sites, send, || {
        !crate::retry_budget::remaining().is_zero()
    })
    .await
}

/// [`ask`], with the second round asked only while `may_retry` says so
/// (carrick#2439): the round of retriable sites draws on the run's retry
/// budget ([`crate::retry_budget`]), the one a scan's patient calls share,
/// so a run that has spent it asks each site once.
pub async fn ask_within<S, Fut, R>(
    repo: &str,
    service: Option<&str>,
    sites: Vec<SiteQuestion>,
    send: S,
    may_retry: R,
) -> SiteAnswers
where
    S: Fn(SiteQuestionsRequest) -> Fut,
    Fut: Future<Output = Result<String, AgentCallError>>,
    R: Fn() -> bool,
{
    let mut out = SiteAnswers::default();
    let mut seen = BTreeSet::new();
    let mut pending: Vec<SiteQuestion> = sites
        .into_iter()
        .filter(|site| seen.insert(site.site_id.clone()))
        .collect();
    for attempt in 0..2 {
        if pending.is_empty() || (attempt > 0 && !may_retry()) {
            break;
        }
        let (requests, too_large) = batches(repo, service, out.question_set.as_deref(), pending);
        for site in too_large {
            out.unanswered.insert(
                site.site_id,
                Unanswered {
                    code: TOO_LARGE_CODE.to_string(),
                    retriable: false,
                },
            );
        }
        out.requests += requests.len();
        let mut again = Vec::new();
        for (sites, result) in round(requests, &send).await {
            let (question_set, mut answers) = match result {
                Ok(read) => read,
                Err(unanswered) => {
                    for site in sites {
                        out.unanswered.insert(site.site_id, unanswered.clone());
                    }
                    continue;
                }
            };
            if question_set.is_some() {
                out.question_set = question_set;
            }
            for site in sites {
                match answers.remove(&site.site_id) {
                    Some(SiteAnswer::Answered(answered)) => {
                        out.unanswered.remove(&site.site_id);
                        out.answered.insert(site.site_id, *answered);
                    }
                    Some(SiteAnswer::Failed(failed)) if failed.retriable && attempt == 0 => {
                        out.unanswered.insert(site.site_id.clone(), failed);
                        again.push(site);
                    }
                    Some(SiteAnswer::Failed(failed)) => {
                        out.unanswered.insert(site.site_id, failed);
                    }
                    None => {
                        out.unanswered.insert(
                            site.site_id,
                            Unanswered {
                                code: NO_ANSWER_CODE.to_string(),
                                retriable: false,
                            },
                        );
                    }
                }
            }
        }
        pending = again;
    }
    debug!(
        answered = out.answered.len(),
        unanswered = out.unanswered.len(),
        requests = out.requests,
        "site questions: answered"
    );
    out
}

/// [`ask`] through the agent service, when the switch is on. Off, nothing is
/// sent and nothing is answered. `agent` should retry under
/// [`crate::agent_service::RetryPolicy::RUN_BUDGETED`] ([`agent`]), so the
/// requests' own retries draw on the run's budget too.
pub async fn ask_service(
    agent: &AgentService,
    repo: &str,
    service: Option<&str>,
    sites: Vec<SiteQuestion>,
) -> SiteAnswers {
    if !enabled() || sites.is_empty() {
        return SiteAnswers::default();
    }
    ask(repo, service, sites, |request| async move {
        agent.post_to_lambda(ROUTE, &request, MOCK_SEED).await
    })
    .await
}
