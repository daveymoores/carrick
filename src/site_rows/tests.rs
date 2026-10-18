//! Site rows and the fold (carrick#2439), over hand-built records and the
//! cassette answers in `tests/fixtures/site_rows/answers.json`.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use serde_json::json;

use super::*;
use crate::possible_calls::UnreadReason;
use crate::request_summary::decided_sites::{
    Decided, DecidedSite, HandlerAt, LabelledEnd, PayloadBinding, SiteLabel,
};
use crate::request_summary::site_record::{
    AnchorAt, Bucket, HandlerAnchor, SiteRecord, TypeAnchors,
};
use crate::site_questions::{SiteAnswer, SiteAnswers, SiteState, Slot, parse_response};

const LINE: u32 = 7;

/// The cassette, read as [`crate::site_questions::ask`] reads a response.
fn cassette() -> SiteAnswers {
    let text = std::fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/site_rows/answers.json"),
    )
    .unwrap();
    let (question_set, answers) = parse_response(&text);
    let mut out = SiteAnswers {
        question_set,
        ..SiteAnswers::default()
    };
    for (id, answer) in answers {
        match answer {
            SiteAnswer::Answered(answered) => {
                out.answered.insert(id, *answered);
            }
            SiteAnswer::Failed(failed) => {
                out.unanswered.insert(id, failed);
            }
        }
    }
    out
}

/// A decided site in `file`, at line [`LINE`], span 100..130, reaching
/// `export` of a package labelled `role` with op `op`.
fn site(file: &str, role: &str, op: &str, export: &str) -> DecidedSite {
    DecidedSite {
        file: file.to_string(),
        line: LINE,
        span_start: 100,
        span_end: 130,
        end: LabelledEnd {
            package: "@fixture/wire".to_string(),
            export: export.to_string(),
            member: Some(op.to_string()),
            role: role.to_string(),
            op: Some(op.to_string()),
            file: file.to_string(),
            line: LINE,
            span_start: 100,
            span_end: 130,
        },
        name: Vec::new(),
        key: None,
        name_args: BTreeSet::new(),
        via: Vec::new(),
        payload: Some(PayloadBinding::Arg { arg: 1, key: None }),
        handler: Some(HandlerAt {
            file: file.to_string(),
            line: LINE,
            span_start: 120,
            span_end: 129,
        }),
        method: None,
    }
}

fn at(file: &str, line: u32, span_start: u32, span_end: u32) -> AnchorAt {
    AnchorAt {
        file: file.to_string(),
        line,
        span_start,
        span_end,
    }
}

/// `site`'s record: `forced` slots stated, `asked` slots asked.
fn record(site: &DecidedSite, forced: &[(Slot, &str)], asked: &[Slot]) -> SiteRecord {
    let forced: BTreeMap<Slot, String> = forced
        .iter()
        .map(|(slot, text)| (*slot, text.to_string()))
        .collect();
    let state: SiteState = serde_json::from_value(json!({
        "point": { "at": format!("{}:{}", site.file, site.line), "code": "" },
        "compiler": { "names": [], "callees": [], "enclosing": null, "reached": [] },
        "syntax": [],
        "libraries": [],
        "call": null,
        "stated": { "names": [], "methods": [] },
        "asked": asked,
        "forced": forced,
    }))
    .unwrap();
    SiteRecord {
        bucket: if asked.is_empty() {
            Bucket::Forced
        } else {
            Bucket::Asked
        },
        state,
        anchors: TypeAnchors {
            call: at(&site.file, site.line, site.span_start, site.span_end),
            payload: site.payload.clone(),
            payload_at: Some(at(&site.file, site.line, 112, 118)),
            handler: Some(HandlerAnchor {
                at: at(&site.file, site.line, 120, 129),
                param: 0,
                param_name: Some("message".to_string()),
            }),
        },
        lead: None,
    }
}

const ASKED: &[Slot] = &[Slot::Kind, Slot::Role, Slot::Name];

/// The labels: the server's socket and the client's.
fn labels() -> Vec<SiteLabel> {
    let label = |export: &str, side: Option<&str>| SiteLabel {
        package: "@fixture/wire".to_string(),
        export: export.to_string(),
        member: Some("emit".to_string()),
        role: "socket".to_string(),
        op: Some("emit".to_string()),
        side: side.map(str::to_string),
        name: None,
        payload: None,
        handler: None,
        method: None,
    };
    vec![
        label("Server", Some("server")),
        label("connect", Some("client")),
    ]
}

/// Every case the cassette answers, with a forced HTTP send and a forced
/// topic subscriber beside them.
fn cases() -> (Decided, Vec<SiteRecord>) {
    let sites = vec![
        site("src/http.ts", "http_client", "request", "get"),
        site("src/topic.ts", "broker", "send", "Queue"),
        site("src/message.ts", "broker", "receive", "Queue"),
        site("src/event.ts", "in_process_bus", "send", "Emitter"),
        site("src/server-socket.ts", "socket", "emit", "Server"),
        site("src/client-socket.ts", "socket", "emit", "connect"),
        site("src/low.ts", "broker", "send", "Queue"),
        site("src/disagree.ts", "broker", "send", "Queue"),
        site("src/no-name.ts", "broker", "send", "Queue"),
        site("src/neither.ts", "broker", "send", "Queue"),
        site("src/low-name.ts", "http_client", "request", "get"),
        site("src/mixed.ts", "broker", "send", "Queue"),
        site("src/role-split.ts", "broker", "send", "Queue"),
        site("src/failed.ts", "broker", "send", "Queue"),
        site("src/forced-http.ts", "http_client", "request", "get"),
        site("src/forced-topic.ts", "broker", "receive", "Queue"),
    ];
    let http_forced: &[(Slot, &str)] = &[
        (Slot::Kind, "http"),
        (Slot::Role, "sends"),
        (Slot::Method, "GET"),
    ];
    let records = sites
        .iter()
        .map(|site| match site.file.as_str() {
            "src/http.ts" | "src/low-name.ts" => record(site, http_forced, &[Slot::Name]),
            "src/forced-http.ts" => record(
                site,
                &[
                    (Slot::Kind, "http"),
                    (Slot::Role, "sends"),
                    (Slot::Method, "POST"),
                    (Slot::Name, "/v1/items"),
                ],
                &[],
            ),
            "src/forced-topic.ts" => record(
                site,
                &[
                    (Slot::Kind, "topic"),
                    (Slot::Role, "handles"),
                    (Slot::Name, "item.added"),
                ],
                &[],
            ),
            _ => record(site, &[], ASKED),
        })
        .collect();
    (
        Decided {
            sites,
            ..Decided::default()
        },
        records,
    )
}

fn rows() -> SiteRows {
    let (decided, records) = cases();
    of(&decided, &records, &labels(), &cassette())
}

fn row<'r>(rows: &'r SiteRows, file: &str) -> Option<&'r SiteRow> {
    rows.rows.iter().find(|row| row.file == file)
}

fn lead_reason(rows: &[PossibleCall], file: &str) -> Option<UnreadReason> {
    rows.iter()
        .find(|lead| lead.file_path == file)
        .map(|lead| lead.unread.reason)
}

/// Each kind names its key: HTTP, a topic, a message (global scope) and an
/// event (service scope), and a socket event whose direction is read from
/// the label's side.
#[test]
fn each_kind_names_its_key_and_scope() {
    let rows = rows();
    let key = |file: &str| row(&rows, file).map(|row| (row.key.clone(), row.side));
    assert_eq!(
        key("src/http.ts"),
        Some((OperationKey::http("GET", "/v1/orders"), RowSide::Call))
    );
    assert_eq!(
        key("src/topic.ts"),
        Some((OperationKey::pubsub("order.settled"), RowSide::Call))
    );
    assert_eq!(row(&rows, "src/topic.ts").unwrap().name_scope, None);
    assert_eq!(
        key("src/message.ts"),
        Some((OperationKey::pubsub("invoice.sent"), RowSide::Endpoint))
    );
    assert_eq!(
        row(&rows, "src/message.ts").unwrap().name_scope,
        Some(carrick_match::NameScope {
            scope: carrick_match::GLOBAL_SCOPE.to_string(),
            namespace: Some("billing".to_string()),
        })
    );
    assert_eq!(
        row(&rows, "src/event.ts")
            .unwrap()
            .name_scope
            .as_ref()
            .map(|scope| scope.scope.as_str()),
        Some(carrick_match::SERVICE_SCOPE)
    );
}

/// A send on a server's socket goes server to client, a send on a
/// client's client to server; a handler takes the other direction, and a
/// side the label does not state is unknown.
#[test]
fn a_socket_event_takes_its_direction_from_the_side() {
    let rows = rows();
    assert_eq!(
        row(&rows, "src/server-socket.ts").map(|row| row.key.clone()),
        Some(OperationKey::socket(
            "chat",
            SocketDirection::ServerToClient
        ))
    );
    assert_eq!(
        row(&rows, "src/client-socket.ts").map(|row| row.key.clone()),
        Some(OperationKey::socket(
            "chat",
            SocketDirection::ClientToServer
        ))
    );
    assert_eq!(
        direction(Some("server"), RowSide::Endpoint),
        SocketDirection::ClientToServer
    );
    assert_eq!(
        direction(Some("client"), RowSide::Endpoint),
        SocketDirection::ServerToClient
    );
    for side in [None, Some("both")] {
        assert_eq!(direction(side, RowSide::Call), SocketDirection::Unknown);
    }
}

/// A forced site is a fact under the existing sources and carries no
/// inference; an asked one is the model's, with the answer's question set,
/// confidence and agreement.
#[test]
fn forced_rows_are_facts_and_asked_rows_are_candidates() {
    let rows = rows();
    let http = row(&rows, "src/forced-http.ts").unwrap();
    assert_eq!(http.source, ResolutionSource::RequestSummary);
    assert_eq!(http.inference, None);
    assert_eq!(http.key, OperationKey::http("POST", "/v1/items"));
    let topic = row(&rows, "src/forced-topic.ts").unwrap();
    assert_eq!(topic.source, ResolutionSource::LibraryClaim);
    assert_eq!(topic.inference, None);
    let asked = row(&rows, "src/topic.ts").unwrap();
    assert_eq!(asked.source, ResolutionSource::Model);
    assert_eq!(
        asked.inference,
        Some(SiteInference {
            by: InferenceBy::SiteQuestion,
            question_set: Some("qs-fixture".to_string()),
            confidence: 0.9,
            orders_agree: true,
        })
    );
}

/// The band reads every asked slot the row depends on: a row is in band
/// only when each is at 0.8 or above with both orders agreeing.
#[test]
fn the_band_reads_every_asked_slot_the_row_depends_on() {
    let rows = rows();
    let in_band = |file: &str| row(&rows, file).map(|row| row.in_band);
    assert_eq!(in_band("src/topic.ts"), Some(true));
    assert_eq!(in_band("src/low.ts"), Some(false));
    assert_eq!(in_band("src/disagree.ts"), Some(false));
    assert_eq!(in_band("src/http.ts"), Some(true));
    assert_eq!(in_band("src/low-name.ts"), Some(false));
    // Every asked slot counts: a high kind does not carry a low name or a
    // role whose orders split, and the row carries the lowest.
    assert_eq!(in_band("src/mixed.ts"), Some(false));
    assert_eq!(in_band("src/role-split.ts"), Some(false));
    assert_eq!(in_band("src/message.ts"), Some(true));
    let inference = |file: &str| row(&rows, file).and_then(|row| row.inference.clone());
    assert_eq!(inference("src/mixed.ts").map(|i| i.confidence), Some(0.5));
    assert_eq!(
        inference("src/role-split.ts").map(|i| i.orders_agree),
        Some(false)
    );
    assert_eq!(
        row(&rows, "src/low-name.ts")
            .and_then(|row| row.inference.as_ref())
            .map(|inference| inference.confidence),
        Some(0.58)
    );
}

/// A site that states no row: no option for the name is a lead below the
/// band, an unanswered site a lead unanswered, and a site that is no
/// network call nothing at all.
#[test]
fn sites_that_state_no_row() {
    let rows = rows();
    assert!(row(&rows, "src/no-name.ts").is_none());
    assert_eq!(
        lead_reason(&rows.leads, "src/no-name.ts"),
        Some(UnreadReason::BelowBand)
    );
    assert!(row(&rows, "src/failed.ts").is_none());
    assert_eq!(
        lead_reason(&rows.leads, "src/failed.ts"),
        Some(UnreadReason::Unanswered)
    );
    assert!(row(&rows, "src/neither.ts").is_none());
    assert_eq!(lead_reason(&rows.leads, "src/neither.ts"), None);
}

/// Only the asked records are asked, under their file and span.
#[test]
fn only_asked_records_are_asked() {
    let (_, records) = cases();
    let ids: Vec<String> = questions(&records)
        .into_iter()
        .map(|question| question.site_id)
        .collect();
    assert!(ids.contains(&"src/topic.ts:100-130".to_string()));
    assert!(!ids.iter().any(|id| id.starts_with("src/forced-")));
    assert_eq!(ids.len(), records.len() - 2);
}

// ---------------------------------------------------------------------------
// The fold.

fn blob_row(
    key: OperationKey,
    file: &str,
    line: u32,
    source: Option<ResolutionSource>,
) -> ApiEndpointDetails {
    let mut row: ApiEndpointDetails = serde_json::from_value(json!({
        "owner": null,
        "key": key,
        "params": [],
        "request_body": null,
        "response_body": null,
        "handler_name": null,
        "request_type": null,
        "response_type": null,
        "file_path": format!("{file}:{line}"),
    }))
    .unwrap();
    row.resolution_source = source;
    row
}

fn make(key: OperationKey, file: &Path, line: u32, _: RowSide) -> ApiEndpointDetails {
    blob_row(key, &file.to_string_lossy(), line, None)
}

/// The rows at `file` on `side`, as (key, source, inference).
fn at_site(
    rows: &[ApiEndpointDetails],
    file: &str,
) -> Vec<(OperationKey, Option<ResolutionSource>, Option<Inference>)> {
    rows.iter()
        .filter(|row| row.file_path.to_string_lossy() == format!("{file}:{LINE}"))
        .map(|row| {
            (
                row.key.clone(),
                row.resolution_source,
                row.inference.clone(),
            )
        })
        .collect()
}

/// One site's rows folded against `calls` and `endpoints`.
fn fold_one(
    file: &str,
    calls: &mut Vec<ApiEndpointDetails>,
    endpoints: &mut Vec<ApiEndpointDetails>,
) -> SiteFold {
    let all = rows();
    let one = SiteRows {
        enabled: true,
        rows: all
            .rows
            .into_iter()
            .filter(|row| row.file == file)
            .collect(),
        leads: Vec::new(),
        routes: Vec::new(),
    };
    fold_rows(calls, endpoints, &one, Path::new("."), make)
}

/// A fact site row folds the reader's row at its site, as library rows do.
#[test]
fn a_fact_folds_the_readers_row() {
    let file = "src/forced-http.ts";
    let mut calls = vec![blob_row(
        OperationKey::http("GET", "/v1/item"),
        file,
        LINE,
        Some(ResolutionSource::Model),
    )];
    let fold = fold_one(file, &mut calls, &mut Vec::new());
    assert_eq!(
        at_site(&calls, file),
        vec![(
            OperationKey::http("POST", "/v1/items"),
            Some(ResolutionSource::RequestSummary),
            None
        )]
    );
    assert_eq!(fold.withdrawn.len(), 1);
    assert_eq!(fold.stated.len(), 1);
}

/// A fact the scan already states at the site keeps it: the site row is
/// not stated a second time.
#[test]
fn a_stated_fact_keeps_its_site() {
    let file = "src/forced-http.ts";
    let summary = blob_row(
        OperationKey::http("POST", "/v1/items"),
        file,
        LINE,
        Some(ResolutionSource::RequestSummary),
    );
    let mut calls = vec![summary];
    let fold = fold_one(file, &mut calls, &mut Vec::new());
    assert_eq!(calls.len(), 1);
    assert!(fold.stated.is_empty() && fold.withdrawn.is_empty());
}

/// An answer in the band replaces the reader's row and carries its answer.
#[test]
fn an_answer_in_the_band_replaces_the_readers_row() {
    let file = "src/topic.ts";
    let mut calls = vec![blob_row(
        OperationKey::pubsub("orders"),
        file,
        LINE,
        Some(ResolutionSource::Model),
    )];
    let fold = fold_one(file, &mut calls, &mut Vec::new());
    let site = at_site(&calls, file);
    assert_eq!(site.len(), 1);
    assert_eq!(site[0].0, OperationKey::pubsub("order.settled"));
    assert_eq!(site[0].1, Some(ResolutionSource::Model));
    assert_eq!(
        site[0]
            .2
            .as_ref()
            .and_then(|inference| inference.answer.as_ref())
            .map(|answer| answer.confidence),
        Some(0.9)
    );
    assert_eq!(
        fold.withdrawn,
        vec![(
            file.to_string(),
            LINE,
            OperationKey::pubsub("orders"),
            RowSide::Call
        )]
    );
}

/// Below the band, or with the option orders split, the reader's row stands
/// and keeps the answer as `inference.alternative`.
#[test]
fn below_the_band_the_readers_row_stands_with_the_alternative() {
    for (file, name) in [
        ("src/low.ts", "order.maybe"),
        ("src/disagree.ts", "order.split"),
        ("src/mixed.ts", "order.guessed"),
        ("src/role-split.ts", "order.rolesplit"),
    ] {
        let reader = OperationKey::pubsub("orders");
        let mut calls = vec![blob_row(
            reader.clone(),
            file,
            LINE,
            Some(ResolutionSource::Model),
        )];
        let fold = fold_one(file, &mut calls, &mut Vec::new());
        let site = at_site(&calls, file);
        assert_eq!(site.len(), 1, "{file}");
        assert_eq!(site[0].0, reader);
        let alternative = site[0]
            .2
            .as_ref()
            .and_then(|inference| inference.alternative.as_ref())
            .expect("the answer is kept");
        assert_eq!(alternative.key, OperationKey::pubsub(name));
        assert!(site[0].2.as_ref().unwrap().answer.is_none());
        assert!(fold.stated.is_empty() && fold.leads.is_empty());
    }
}

/// With no row at the site, an answer in the band stands; below it, the
/// site is a possible call.
#[test]
fn with_no_reader_row_the_band_decides_row_or_lead() {
    let mut calls = Vec::new();
    let fold = fold_one("src/topic.ts", &mut calls, &mut Vec::new());
    assert_eq!(at_site(&calls, "src/topic.ts").len(), 1);
    assert_eq!(fold.stated.len(), 1);

    for file in ["src/low.ts", "src/mixed.ts", "src/role-split.ts"] {
        let mut calls = Vec::new();
        let fold = fold_one(file, &mut calls, &mut Vec::new());
        assert!(calls.is_empty(), "{file}");
        assert_eq!(
            lead_reason(&fold.leads, file),
            Some(UnreadReason::BelowBand),
            "{file}"
        );
    }
}

/// A row on the other side of the index is another site: a subscriber
/// beside a publish call is not the publish's reader row.
#[test]
fn the_fold_keeps_to_its_side() {
    let file = "src/topic.ts";
    let mut endpoints = vec![blob_row(
        OperationKey::pubsub("orders"),
        file,
        LINE,
        Some(ResolutionSource::Model),
    )];
    let mut calls = Vec::new();
    fold_one(file, &mut calls, &mut endpoints);
    assert_eq!(endpoints.len(), 1);
    assert_eq!(endpoints[0].inference, None);
    assert_eq!(calls.len(), 1);
}

/// Off, nothing folds and nothing is written: the rows are byte for byte
/// what they were.
#[test]
fn off_the_fold_changes_nothing() {
    let file = "src/topic.ts";
    let mut calls = vec![blob_row(
        OperationKey::pubsub("orders"),
        file,
        LINE,
        Some(ResolutionSource::Model),
    )];
    let before = serde_json::to_vec(&calls).unwrap();
    let mut off = rows();
    off.enabled = false;
    let fold = fold_rows(&mut calls, &mut Vec::new(), &off, Path::new("."), make);
    assert_eq!(serde_json::to_vec(&calls).unwrap(), before);
    assert!(fold.stated.is_empty() && fold.leads.is_empty() && fold.withdrawn.is_empty());
}

// ---------------------------------------------------------------------------
// The wire.

/// `inference` rides the wire on the rows that carry it, round-trips, and
/// is absent everywhere else: a row with none serialises without the key,
/// and a blob row written before the field reads as none.
#[test]
fn inference_on_the_wire() {
    let answer = SiteInference {
        by: InferenceBy::SiteQuestion,
        question_set: Some("qs-fixture".to_string()),
        confidence: 0.91,
        orders_agree: true,
    };
    let mut stated = blob_row(OperationKey::pubsub("a"), "src/a.ts", 1, None);
    stated.inference = Some(Inference {
        answer: Some(answer.clone()),
        alternative: None,
    });
    let value = serde_json::to_value(&stated).unwrap();
    assert_eq!(
        value["inference"],
        json!({ "by": "site_question", "question_set": "qs-fixture", "confidence": 0.91, "orders_agree": true })
    );
    let back: ApiEndpointDetails = serde_json::from_value(value).unwrap();
    assert_eq!(back.inference, stated.inference);

    let mut reader = blob_row(OperationKey::pubsub("a"), "src/a.ts", 1, None);
    reader.inference = Some(Inference {
        answer: None,
        alternative: Some(Alternative {
            answer,
            side: RowSide::Call,
            key: OperationKey::pubsub("b"),
        }),
    });
    let value = serde_json::to_value(&reader).unwrap();
    assert_eq!(
        value["inference"],
        json!({ "alternative": {
            "by": "site_question", "question_set": "qs-fixture", "confidence": 0.91,
            "orders_agree": true, "side": "call", "protocol": "pubsub", "topic": "b"
        } })
    );
    let back: ApiEndpointDetails = serde_json::from_value(value).unwrap();
    assert_eq!(back.inference, reader.inference);

    let plain = blob_row(OperationKey::pubsub("a"), "src/a.ts", 1, None);
    let value = serde_json::to_value(&plain).unwrap();
    assert!(value.get("inference").is_none());
    let old: ApiEndpointDetails = serde_json::from_value(value).unwrap();
    assert_eq!(old.inference, None);
}

// ---------------------------------------------------------------------------
// Routes a site cannot write (carrick#2517).

/// A made-up server framework's member at `file`, labelled `receive`.
fn route_site(file: &str, member: &str) -> DecidedSite {
    let mut site = site(file, "server_framework", "receive", "App");
    site.end.package = "@fixture/serve".to_string();
    site.end.member = Some(member.to_string());
    site
}

/// The record of a route site as the label states it: kind `http` and role
/// `handles` forced, the name forced, and the method `forced` or asked.
fn route_record(site: &DecidedSite, method: Option<&str>) -> SiteRecord {
    let mut forced = vec![
        (Slot::Kind, "http"),
        (Slot::Role, "handles"),
        (Slot::Name, "/items/:id"),
    ];
    let asked: &[Slot] = match method {
        Some(method) => {
            forced.push((Slot::Method, method));
            &[]
        }
        None => &[Slot::Method],
    };
    record(site, &forced, asked)
}

/// A reader's route at `file:LINE`, keyed with the prefix its router gives it.
fn reader_route(file: &str) -> ApiEndpointDetails {
    blob_row(
        OperationKey::http("GET", "/v1/items/:id"),
        file,
        LINE,
        Some(ResolutionSource::Model),
    )
}

/// One record's rows, folded against `endpoints`.
fn fold_route(
    site: &DecidedSite,
    record: SiteRecord,
    answers: &SiteAnswers,
    endpoints: &mut Vec<ApiEndpointDetails>,
) -> SiteFold {
    let decided = Decided {
        sites: vec![site.clone()],
        ..Decided::default()
    };
    let rows = of(&decided, &[record], &[], answers);
    assert!(rows.rows.is_empty(), "{:?}", rows.rows);
    fold_rows(&mut Vec::new(), endpoints, &rows, Path::new("."), make)
}

/// A route site the label states (kind `http`, role `handles`) is never
/// asked: no route can be written from its answer, because the site names no
/// router and so no prefix. Where the reader stated a route at its line the
/// reader's row stands untouched, the site is counted, and it is no lead.
#[test]
fn a_route_site_with_a_reader_row_is_not_asked_and_not_a_lead() {
    let file = "src/sub-app.ts";
    let site = route_site(file, "get");
    for method in [None, Some("GET")] {
        let record = route_record(&site, method);
        assert!(
            questions(std::slice::from_ref(&record)).is_empty(),
            "{method:?}"
        );
        let before = serde_json::to_string(&vec![reader_route(file)]).unwrap();
        let mut endpoints = vec![reader_route(file)];
        let fold = fold_route(&site, record, &SiteAnswers::default(), &mut endpoints);
        assert_eq!(serde_json::to_string(&endpoints).unwrap(), before);
        assert!(fold.leads.is_empty(), "{method:?}: {:?}", fold.leads);
        assert_eq!(fold.route_kept_reader, 1);
        assert!(fold.stated.is_empty() && fold.withdrawn.is_empty());
    }
}

/// The same route site with no reader row at its line is a call the index
/// lacks: one possible call, reason `owner_unknown`, and still no question.
#[test]
fn a_route_site_with_no_reader_row_is_one_owner_unknown_lead() {
    let file = "src/mount.ts";
    let site = route_site(file, "attach");
    for method in [None, Some("GET")] {
        let record = route_record(&site, method);
        assert!(
            questions(std::slice::from_ref(&record)).is_empty(),
            "{method:?}"
        );
        let mut endpoints = Vec::new();
        let fold = fold_route(&site, record, &SiteAnswers::default(), &mut endpoints);
        assert!(endpoints.is_empty());
        assert_eq!(fold.leads.len(), 1, "{method:?}: {:?}", fold.leads);
        let lead = &fold.leads[0];
        assert_eq!(lead.unread.reason, UnreadReason::OwnerUnknown);
        assert_eq!(lead.kind.as_deref(), Some("http"));
        assert_eq!(lead.role.as_deref(), Some("producer"));
        assert_eq!(fold.route_kept_reader, 0);
    }
}

/// A site whose role the label leaves open is asked. When the model answers
/// that it registers an HTTP route and names no method (or a word no route is
/// registered under), no endpoint is written: with no reader row at the
/// line it is a possible call `no_method`, and with one it is counted.
#[test]
fn an_asked_route_answer_with_no_method_writes_no_endpoint() {
    let file = "src/open.ts";
    let site = route_site(file, "use");
    let record = record(&site, &[(Slot::Kind, "http")], &[Slot::Role, Slot::Name]);
    assert_eq!(questions(std::slice::from_ref(&record)).len(), 1);
    for method in [None, Some("UNKNOWN"), Some("")] {
        let mut answers = SiteAnswers::default();
        let sure = |text: &str| SlotAnswer {
            value: Some(SlotValue::Text(text.to_string())),
            confidence: 1.0,
            orders_agree: true,
        };
        answers.answered.insert(
            site_id(&record.anchors),
            Answered {
                role: Some(sure("handles")),
                name: Some(sure("/items/:id")),
                method: method.map(sure),
                ..Answered::default()
            },
        );
        let mut endpoints = Vec::new();
        let fold = fold_route(&site, record.clone(), &answers, &mut endpoints);
        assert!(endpoints.is_empty(), "{method:?}");
        assert_eq!(
            lead_reason(&fold.leads, file),
            Some(UnreadReason::NoMethod),
            "{method:?}"
        );
        let mut endpoints = vec![reader_route(file)];
        let fold = fold_route(&site, record.clone(), &answers, &mut endpoints);
        assert!(fold.leads.is_empty(), "{method:?}: {:?}", fold.leads);
        assert_eq!(fold.route_kept_reader, 1);
    }
}

/// Receive-side rows with no prefix to lose are written as before: a
/// subscriber whose kind, role and name are forced.
#[test]
fn a_receive_row_with_no_prefix_is_still_written() {
    let rows = rows();
    let topic = row(&rows, "src/forced-topic.ts").expect("the subscriber is a row");
    assert_eq!(topic.side, RowSide::Endpoint);
    assert_eq!(topic.key, OperationKey::pubsub("item.added"));
}

/// Off, the fold leaves a route table byte for byte as it was, and states no
/// possible call and no count.
#[test]
fn off_a_route_site_leaves_the_route_table_byte_identical() {
    let file = "src/sub-app.ts";
    let site = route_site(file, "get");
    let decided = Decided {
        sites: vec![site.clone()],
        ..Decided::default()
    };
    for endpoints in [vec![reader_route(file)], Vec::new()] {
        let mut rows = of(
            &decided,
            &[route_record(&site, Some("GET"))],
            &[],
            &SiteAnswers::default(),
        );
        rows.enabled = false;
        let mut endpoints = endpoints;
        let before = serde_json::to_string(&endpoints).unwrap();
        let fold = fold_rows(&mut Vec::new(), &mut endpoints, &rows, Path::new("."), make);
        assert_eq!(serde_json::to_string(&endpoints).unwrap(), before);
        assert!(fold.leads.is_empty());
        assert_eq!(fold.route_kept_reader, 0);
    }
}
