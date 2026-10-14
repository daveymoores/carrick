//! carrick#2048 through the whole scanner, mocked ($0, no model): a file-based
//! route whose handler dispatches on a body field keeps one producer row per
//! case, and each call that sends a case links to it.
//!
//! See `tests/fixtures/dispatch-file-route/README.md` for the shape. The three
//! cases echo three different kinds of id, so one scan takes every path the
//! join has for a row the file layout already states.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::OnceLock;

fn fixture() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/dispatch-file-route")
}

/// The scanner over `root`, replaying the fixture's own model answers.
fn carrick(root: &Path) -> Command {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_carrick"));
    cmd.arg(root)
        .env("CARRICK_MOCK_ALL", "1")
        .env(
            "CARRICK_MOCK_FIXTURE_DIR",
            format!("{}/", root.join("__llm__").display()),
        )
        .env("CARRICK_SKIP_INTENTS", "1")
        .env("CARRICK_LOCAL_STORAGE_ISOLATE", "1");
    for var in [
        "GITHUB_REPOSITORY",
        "GITHUB_REF",
        "GITHUB_EVENT_NAME",
        "GITHUB_SHA",
        "GITHUB_RUN_ID",
        "GITHUB_ACTIONS",
        "GITHUB_WORKSPACE",
        "CI",
        "ACTIONS_ID_TOKEN_REQUEST_URL",
        "ACTIONS_ID_TOKEN_REQUEST_TOKEN",
    ] {
        cmd.env_remove(var);
    }
    cmd
}

/// The scan's JSON projection: endpoints, calls and edges.
fn scan() -> &'static serde_json::Value {
    static SCAN: OnceLock<serde_json::Value> = OnceLock::new();
    SCAN.get_or_init(|| {
        let cache = tempfile::tempdir().expect("temp cache dir");
        let output = carrick(&fixture())
            .env("CARRICK_LOCAL_STORAGE_DIR", cache.path())
            .env("CARRICK_OUTPUT_JSON", "1")
            .output()
            .expect("failed to spawn carrick");
        let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
        assert!(
            output.status.success(),
            "scan failed:\n{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let start = stdout
            .find("\n{")
            .map(|i| i + 1)
            .or_else(|| stdout.starts_with('{').then_some(0))
            .unwrap_or_else(|| panic!("no JSON in scanner stdout:\n{stdout}"));
        serde_json::from_str(&stdout[start..])
            .unwrap_or_else(|e| panic!("projection parse failed: {e}\n{stdout}"))
    })
}

/// `(method, path, dispatch value)` for every row of `section`.
fn rows(section: &str) -> BTreeSet<(String, String, Option<String>)> {
    scan()[section]
        .as_array()
        .unwrap_or_else(|| panic!("a {section} array"))
        .iter()
        .map(|row| {
            (
                row["method"].as_str().expect("method").to_string(),
                row["path"].as_str().expect("path").to_string(),
                row["dispatch"]["value"].as_str().map(str::to_string),
            )
        })
        .collect()
}

fn row(method: &str, path: &str, case: Option<&str>) -> (String, String, Option<String>) {
    (
        method.to_string(),
        path.to_string(),
        case.map(str::to_string),
    )
}

const ROUTE: &str = "/api/orders/:orderId";
const REPORTS: &str = "/api/reports";

/// Every endpoint row the scan holds for `path`.
fn endpoints_at(path: &str) -> Vec<&'static serde_json::Value> {
    scan()["endpoints"]
        .as_array()
        .expect("an endpoints array")
        .iter()
        .filter(|row| row["path"] == path)
        .collect()
}

/// The `POST` route is one row per case the handler answers, each stated by
/// the file layout, and no row without a case is left beside them to answer
/// every call whatever it sends.
#[test]
fn a_file_route_that_dispatches_on_its_body_keeps_every_case() {
    assert_eq!(
        rows("endpoints"),
        BTreeSet::from([
            row("GET", REPORTS, None),
            row("GET", ROUTE, None),
            row("POST", ROUTE, Some("cancel")),
            row("POST", ROUTE, Some("confirm")),
            row("POST", ROUTE, Some("refund")),
        ]),
    );
    for endpoint in endpoints_at(ROUTE) {
        assert_eq!(
            endpoint["resolution_source"], "file_based_route",
            "every case is a case of the route the layout states: {endpoint}"
        );
        assert_eq!(endpoint["file"], "app/api/orders/[orderId]/route.ts");
    }
}

/// A `GET` carries no request body, so the three values its handler reads off
/// a search parameter are not dispatch cases, whatever location the model
/// wrote for them (carrick#2110). The route is one row with no case, stated
/// by the file layout. Its three branches answer three responses, and the
/// route's response is all three, never the first branch's alone. The join
/// keeps no one branch's response anchor (the unit test
/// `a_body_case_on_a_method_with_no_body_is_dropped_and_the_rows_are_one`
/// pins that); the type here is the handler's own inferred return.
#[test]
fn a_get_file_route_that_switches_on_a_search_parameter_is_one_row() {
    let reports = endpoints_at(REPORTS);
    assert_eq!(reports.len(), 1, "one row for the route: {reports:#?}");
    let report = reports[0];
    assert_eq!(report["method"], "GET");
    assert!(report["dispatch"].is_null(), "no case: {report}");
    assert_eq!(report["resolution_source"], "file_based_route");
    assert_eq!(report["file"], "app/api/reports/route.ts");
    let response = report["expanded_definition"].as_str().unwrap_or_default();
    for branch in ["totals", "trend", "rows"] {
        assert!(
            response.contains(branch),
            "the route answers every branch's response, `{branch}` too: {report}"
        );
    }
}

/// Each call links to the route once: one edge per call. A call that states
/// its case reaches that case alone (`carrick_match::dispatch_verdict`), and
/// the route has no row without a case. The `replay` call sends a body the
/// scan cannot read the case from: it keeps one edge to the route with its
/// case unknown (`carrick_match::dispatch_outcome`), never one per case and
/// never none.
#[test]
fn each_call_links_to_the_case_it_sends() {
    let calls = rows("calls");
    assert_eq!(
        calls,
        BTreeSet::from([
            row("GET", ROUTE, None),
            row("POST", ROUTE, None),
            row("POST", ROUTE, Some("cancel")),
            row("POST", ROUTE, Some("confirm")),
            row("POST", ROUTE, Some("refund")),
        ]),
        "each call states the case it sends, and `replay` states none"
    );

    let edges = scan()["cross_repo_matches"]
        .as_array()
        .expect("a cross_repo_matches array");
    let producers: Vec<&str> = edges
        .iter()
        .map(|edge| edge["producer_key"].as_str().expect("producer_key"))
        .collect();
    assert_eq!(
        producers
            .iter()
            .filter(|key| **key == "http|POST|/api/orders/:orderId")
            .count(),
        4,
        "each POST call links to the route once: {edges:#?}"
    );
    assert_eq!(edges.len(), 5, "one edge per call: {edges:#?}");
}
