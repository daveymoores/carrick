//! Routes registered inside an inline plugin take the prefix the registration
//! states, and nothing else (carrick#2092).
//!
//! `app.register(async (api) => { api.register(items) }, { prefix: … })`
//! hands the callback an instance of its own. Every inline plugin names that
//! instance the same way, so the scan has to tell them apart by where they
//! are registered, and read the prefix only where the source states it as a
//! literal.
//!
//! Each test drives the whole file analysis over the fixture with canned
//! model answers (`cached_model_results`), so no model is asked: what moves
//! is what the deterministic layer does with the rows the model states.

use carrick::agent_service::AgentService;
use carrick::agents::file_analyzer_agent::FileAnalysisResult;
use carrick::agents::file_orchestrator::{FileCentricAnalysisResult, FileOrchestrator};
use carrick::agents::framework_guidance_agent::{FrameworkGuidance, ProtocolGuidance};
use carrick::framework_detector::DetectionResult;
use carrick::operation::Protocol;
use carrick::swc_scanner::SwcScanner;
use serial_test::serial;
use std::collections::{BTreeSet, HashMap};
use std::path::{Path, PathBuf};

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/inline-registration-scope")
}

fn source(file: &str) -> String {
    std::fs::read_to_string(root().join(file)).expect("fixture file")
}

/// The 1-based line holding `needle`, which must appear once.
fn line_of(file: &str, needle: &str) -> usize {
    let text = source(file);
    let lines: Vec<usize> = text
        .lines()
        .enumerate()
        .filter(|(_, line)| line.contains(needle))
        .map(|(index, _)| index + 1)
        .collect();
    assert_eq!(lines.len(), 1, "{needle:?} in {file}: {lines:?}");
    lines[0]
}

/// The id the scanner gives the call that starts on `line`, as the model
/// would echo it.
fn candidate_at(file: &str, line: usize) -> String {
    let scanned = SwcScanner::new().scan_content(&root().join(file), &source(file), &[], &[]);
    scanned
        .candidates
        .iter()
        .find(|candidate| candidate.line_number == line)
        .unwrap_or_else(|| panic!("no candidate at {file}:{line}"))
        .candidate_id
        .clone()
}

/// A mount row the model states: `(needle on the call's line, parent,
/// child, mount_path, import_source)`.
type Mount<'a> = (&'a str, &'a str, &'a str, &'a str, Option<&'a str>);
/// An endpoint row the model states: `(needle, owner, method, path)`.
type Route<'a> = (&'a str, &'a str, &'a str, &'a str);

fn answer(file: &str, mounts: &[Mount], routes: &[Route]) -> (String, FileAnalysisResult) {
    let mounts: Vec<serde_json::Value> = mounts
        .iter()
        .map(|(needle, parent, child, path, import)| {
            serde_json::json!({
                "line_number": line_of(file, needle),
                "parent_node": parent,
                "child_node": child,
                "mount_path": path,
                "import_source": import,
                "pattern_matched": ".register(",
            })
        })
        .collect();
    let endpoints: Vec<serde_json::Value> = routes
        .iter()
        .map(|(needle, owner, method, path)| {
            let line = line_of(file, needle);
            serde_json::json!({
                "candidate_id": candidate_at(file, line),
                "line_number": line,
                "owner_node": owner,
                "method": method,
                "path": path,
                "handler_name": "anonymous",
                "pattern_matched": ".get(",
                "payload_expression_text": null,
                "payload_expression_line": null,
                "response_expression_text": null,
                "response_expression_line": null,
                "primary_type_symbol": null,
                "type_import_source": null
            })
        })
        .collect();
    let result: FileAnalysisResult = serde_json::from_value(serde_json::json!({
        "mounts": mounts,
        "endpoints": endpoints,
        "data_calls": [],
    }))
    .expect("a canned answer");
    (root().join(file).to_string_lossy().into_owned(), result)
}

fn items() -> (String, FileAnalysisResult) {
    answer(
        "src/items.ts",
        &[],
        &[("app.get('/items'", "app", "GET", "/items")],
    )
}

fn orders() -> (String, FileAnalysisResult) {
    answer(
        "src/orders.ts",
        &[],
        &[("app.get('/orders'", "app", "GET", "/orders")],
    )
}

/// The registering file's answer when the model states the outer
/// registration with `prefix_text` as its mount path.
fn stated(file: &str, prefix_text: &str) -> (String, FileAnalysisResult) {
    answer(
        file,
        &[
            ("app.register(", "app", "api", prefix_text, None),
            (
                "api.register(itemRoutes)",
                "api",
                "itemRoutes",
                "",
                Some("./items"),
            ),
        ],
        &[],
    )
}

async fn analyze(answers: Vec<(String, FileAnalysisResult)>) -> FileCentricAnalysisResult {
    analyze_with(answers, &[]).await
}

/// [`analyze`] with `unanswered` files scanned beside the answered ones: a
/// file with no canned answer goes through the scan as it would in a run.
async fn analyze_with(
    answers: Vec<(String, FileAnalysisResult)>,
    unanswered: &[&str],
) -> FileCentricAnalysisResult {
    // SAFETY: serial test; env vars are process-global. Every file a model
    // would be asked about has a canned answer, so nothing is dispatched; the
    // mock is a backstop.
    unsafe {
        std::env::set_var("CARRICK_MOCK_ALL", "1");
    }
    let root = root();
    let files: Vec<PathBuf> = answers
        .iter()
        .map(|(path, _)| PathBuf::from(path))
        .chain(unanswered.iter().map(|file| root.join(file)))
        .collect();
    let cached: HashMap<String, FileAnalysisResult> = answers.into_iter().collect();
    FileOrchestrator::new(AgentService::new())
        .analyze_files(
            &files,
            &cached,
            // Guidance shapes only the prompt, and nothing is prompted here.
            &ProtocolGuidance::from([(
                Protocol::Http,
                FrameworkGuidance {
                    mount_patterns: vec![],
                    endpoint_patterns: vec![],
                    middleware_patterns: vec![],
                    data_fetching_patterns: vec![],
                    triage_hints: String::new(),
                    parsing_notes: String::new(),
                    guidance_key: None,
                },
            )]),
            &DetectionResult {
                frameworks: vec![],
                data_fetchers: vec![],
                messaging_clients: vec![],
                socket_clients: vec![],
                notes: String::new(),
                client_semantics: None,
            },
            &root,
            &root,
            &[],
            &Default::default(),
            &Default::default(),
            &carrick::url_normalizer::UrlNormalizer::default_permissive(),
            &carrick::workspace_resolver::WorkspaceIndex::build_with_aliases(&root, None),
            None,
        )
        .await
        .expect("the analysis ran")
}

fn served(result: &FileCentricAnalysisResult) -> BTreeSet<String> {
    result
        .mount_graph
        .get_resolved_endpoints()
        .iter()
        .map(|endpoint| format!("{} {}", endpoint.method, endpoint.full_path))
        .collect()
}

fn set(routes: &[&str]) -> BTreeSet<String> {
    routes.iter().map(|route| route.to_string()).collect()
}

#[tokio::test]
#[serial]
async fn a_nullish_default_prefix_reaches_the_routes() {
    let result = analyze(vec![
        stated("src/nullish.ts", "opts.prefix ?? '/api/v1'"),
        items(),
    ])
    .await;
    assert!(
        served(&result).contains("GET /api/v1/items"),
        "{:?}",
        served(&result)
    );
    assert!(result.unread_mount_prefixes.is_empty());
}

#[tokio::test]
#[serial]
async fn a_logical_or_default_prefix_reaches_the_routes() {
    let result = analyze(vec![
        stated("src/either.ts", "opts.prefix || '/api/v2'"),
        items(),
    ])
    .await;
    assert_eq!(served(&result), set(&["GET /api/v2/items"]));
}

#[tokio::test]
#[serial]
async fn a_ternary_of_literals_serves_both_prefixes() {
    let result = analyze(vec![
        stated("src/ternary.ts", "opts.legacy ? '/legacy' : '/api/v3'"),
        items(),
    ])
    .await;
    assert_eq!(
        served(&result),
        set(&["GET /api/v3/items", "GET /legacy/items"])
    );
}

#[tokio::test]
#[serial]
async fn a_parameter_default_prefix_reaches_the_routes() {
    let result = analyze(vec![stated("src/param-default.ts", "prefix"), items()]).await;
    assert_eq!(served(&result), set(&["GET /api/v4/items"]));
}

#[tokio::test]
#[serial]
async fn a_const_bound_prefix_reaches_the_routes() {
    let result = analyze(vec![stated("src/const-prefix.ts", "API_PREFIX"), items()]).await;
    assert_eq!(served(&result), set(&["GET /api/v5/items"]));
}

#[tokio::test]
#[serial]
async fn an_unreadable_prefix_is_recorded_not_guessed() {
    let result = analyze(vec![stated("src/unread.ts", "opts.prefix"), items()]).await;
    assert_eq!(served(&result), set(&["GET /items"]));
    let line = line_of("src/unread.ts", "app.register(");
    let sites: Vec<&str> = result
        .unread_mount_prefixes
        .iter()
        .map(|record| record.site.as_str())
        .collect();
    assert_eq!(sites, vec![format!("src/unread.ts:{line}").as_str()]);
    assert_eq!(result.unread_mount_prefixes[0].expression, "opts.prefix");
    assert_eq!(result.stats.mount_prefixes_unread, 1);
}

#[tokio::test]
#[serial]
async fn two_inline_scopes_with_one_parameter_name_stay_apart() {
    let file = "src/two-scopes.ts";
    let text = source(file);
    let registrations: Vec<usize> = text
        .lines()
        .enumerate()
        .filter(|(_, line)| line.contains("app.register("))
        .map(|(index, _)| index + 1)
        .collect();
    let (path, mut answer) = answer(
        file,
        &[
            (
                "api.register(itemRoutes)",
                "api",
                "itemRoutes",
                "",
                Some("./items"),
            ),
            (
                "api.register(orderRoutes)",
                "api",
                "orderRoutes",
                "",
                Some("./orders"),
            ),
        ],
        &[],
    );
    for (line, prefix) in registrations.iter().zip(["/a", "/b"]) {
        answer.mounts.push(
            serde_json::from_value(serde_json::json!({
                "line_number": line,
                "parent_node": "app",
                "child_node": "api",
                "mount_path": prefix,
                "import_source": null,
                "pattern_matched": ".register(",
            }))
            .expect("a mount row"),
        );
    }
    let result = analyze(vec![(path, answer), items(), orders()]).await;
    assert_eq!(served(&result), set(&["GET /a/items", "GET /b/orders"]));
}

#[tokio::test]
#[serial]
async fn a_registration_the_model_left_unstated_is_recorded() {
    let file = "src/silent.ts";
    let silent = answer(
        file,
        &[(
            "api.register(itemRoutes)",
            "api",
            "itemRoutes",
            "",
            Some("./items"),
        )],
        &[],
    );
    let result = analyze(vec![silent, items()]).await;
    assert_eq!(served(&result), set(&["GET /items"]));
    let line = line_of(file, "app.register(");
    let scope = format!("api@{file}:{line}");
    assert!(
        result
            .mount_graph
            .get_mounts()
            .iter()
            .any(|edge| edge.parent == "app" && edge.child == scope && edge.path_prefix.is_empty()),
        "the registration is an edge of its own: {:?}",
        result.mount_graph.get_mounts()
    );
    assert_eq!(result.unread_mount_prefixes.len(), 1);
    assert_eq!(
        result.unread_mount_prefixes[0].expression,
        "{ prefix: opts.prefix ?? '/api/v1' }"
    );
}

#[tokio::test]
#[serial]
async fn a_registration_with_no_options_is_an_edge_and_no_record() {
    let file = "src/no-options.ts";
    let quiet = answer(
        file,
        &[(
            "scope.register(orderRoutes)",
            "scope",
            "orderRoutes",
            "",
            Some("./orders"),
        )],
        &[],
    );
    let result = analyze(vec![quiet, orders()]).await;
    assert_eq!(served(&result), set(&["GET /orders"]));
    assert!(result.unread_mount_prefixes.is_empty());
}

#[tokio::test]
#[serial]
async fn inline_callbacks_that_head_no_rows_emit_nothing() {
    let callbacks = answer(
        "src/callbacks.ts",
        &[],
        &[("app.get('/ids'", "app", "GET", "/ids")],
    );
    let result = analyze(vec![callbacks]).await;
    assert!(result.mount_graph.get_mounts().is_empty());
    assert_eq!(served(&result), set(&["GET /ids"]));
    assert!(result.unread_mount_prefixes.is_empty());
}

#[tokio::test]
#[serial]
async fn a_route_handler_callback_is_not_a_scope() {
    // The model may read `c.get('user')` inside the handler as a row hanging
    // from `c`. The call that passes the callback registers a route, so the
    // callback is handed a request and is no scope.
    let handler = answer(
        "src/handler.ts",
        &[],
        &[
            ("router.get('/me'", "router", "GET", "/me"),
            ("c.get('user')", "c", "GET", "user"),
        ],
    );
    let result = analyze(vec![handler]).await;
    assert!(
        result.mount_graph.get_mounts().is_empty(),
        "{:?}",
        result.mount_graph.get_mounts()
    );
    assert_eq!(served(&result), set(&["GET /me"]));
}

#[tokio::test]
#[serial]
async fn a_group_callback_with_a_first_argument_prefix() {
    let group = answer(
        "src/group.ts",
        &[
            ("app.group(", "app", "g", "/v2", None),
            ("app.group(", "g", "orderRoutes", "", Some("./orders")),
        ],
        &[],
    );
    let result = analyze(vec![group, orders()]).await;
    assert_eq!(served(&result), set(&["GET /v2/orders"]));
}

#[tokio::test]
#[serial]
async fn a_route_registered_directly_in_the_scope_takes_its_prefix() {
    let file = "src/nullish.ts";
    let (path, mut nullish) = stated(file, "opts.prefix ?? '/api/v1'");
    let (_, health) = answer(file, &[], &[("api.get('/health'", "api", "GET", "/health")]);
    nullish.endpoints = health.endpoints;
    let result = analyze(vec![(path, nullish), items()]).await;
    assert_eq!(
        served(&result),
        set(&["GET /api/v1/health", "GET /api/v1/items"])
    );
}

#[tokio::test]
#[serial]
async fn a_scope_inside_a_mounted_plugin_composes_under_both() {
    // `index.ts` mounts the plugin `nullish.ts` exports. The file-first
    // identity hands every owner in a mounted file to that binding; a scope
    // id must keep its own node, or the inner prefix is lost.
    let file = "src/nullish.ts";
    let (path, mut nullish) = stated(file, "opts.prefix ?? '/api/v1'");
    let (_, health) = answer(file, &[], &[("api.get('/health'", "api", "GET", "/health")]);
    nullish.endpoints = health.endpoints;
    let index = answer(
        "src/index.ts",
        &[(
            "server.register(api",
            "server",
            "api",
            "/root",
            Some("./nullish"),
        )],
        &[],
    );
    let result = analyze(vec![index, (path, nullish), items()]).await;
    assert_eq!(
        served(&result),
        set(&["GET /root/api/v1/health", "GET /root/api/v1/items"])
    );
}

// --- a prefix the service's own descriptors corroborate (slice 1b) --------
//
// The model stated no row for the registration, so the prefix is unread. The
// registration's other arguments read to one route path, and the service
// documents its routes as data: where the documentation states a route under
// that path and never without it, the prefix is read.

fn item_detail() -> (String, FileAnalysisResult) {
    answer(
        "src/item-detail.ts",
        &[],
        &[("app.get('/items/:id'", "app", "GET", "/items/:id")],
    )
}

/// A registering file whose outer registration the model left unstated.
fn unstated(file: &str) -> (String, FileAnalysisResult) {
    answer(
        file,
        &[(
            "api.register(itemDetailRoutes)",
            "api",
            "itemDetailRoutes",
            "",
            Some("./item-detail"),
        )],
        &[],
    )
}

/// A documentation file: the deterministic layer reads it, the model is not
/// asked about it.
fn docs(file: &str) -> (String, FileAnalysisResult) {
    (
        root().join(file).to_string_lossy().into_owned(),
        FileAnalysisResult::default(),
    )
}

/// What the handler file serves, documentation rows left out.
fn handler_serves(result: &FileCentricAnalysisResult) -> BTreeSet<String> {
    result
        .mount_graph
        .get_resolved_endpoints()
        .iter()
        .filter(|endpoint| endpoint.file_location.contains("src/item-detail.ts"))
        .map(|endpoint| format!("{} {}", endpoint.method, endpoint.full_path))
        .collect()
}

#[tokio::test]
#[serial]
async fn a_scope_prefix_the_service_documents_is_read() {
    let result = analyze(vec![
        unstated("src/documented.ts"),
        item_detail(),
        docs("src/docs.ts"),
    ])
    .await;
    assert_eq!(handler_serves(&result), set(&["GET /api/v1/items/:id"]));
    assert_eq!(result.stats.mount_prefixes_corroborated, 1);
    assert_eq!(result.stats.mount_prefixes_unread, 0);
}

#[tokio::test]
#[serial]
async fn an_undocumented_scope_keeps_its_prefix_unread() {
    let result = analyze(vec![unstated("src/documented.ts"), item_detail()]).await;
    assert_eq!(handler_serves(&result), set(&["GET /items/:id"]));
    assert_eq!(result.stats.mount_prefixes_corroborated, 0);
    assert_eq!(result.stats.mount_prefixes_unread, 1);
}

#[tokio::test]
#[serial]
async fn a_descriptor_under_another_prefix_refuses_the_candidate() {
    let result = analyze(vec![
        unstated("src/documented.ts"),
        item_detail(),
        docs("src/docs-v2.ts"),
    ])
    .await;
    assert_eq!(handler_serves(&result), set(&["GET /items/:id"]));
    assert_eq!(result.stats.mount_prefixes_corroborated, 0);
}

#[tokio::test]
#[serial]
async fn two_route_shaped_option_values_abstain() {
    let result = analyze(vec![
        unstated("src/two-options.ts"),
        item_detail(),
        docs("src/docs.ts"),
    ])
    .await;
    assert_eq!(handler_serves(&result), set(&["GET /items/:id"]));
    assert_eq!(result.stats.mount_prefixes_corroborated, 0);
}

#[tokio::test]
#[serial]
async fn a_descriptor_documenting_the_unprefixed_path_refuses_the_candidate() {
    let result = analyze(vec![
        unstated("src/documented.ts"),
        item_detail(),
        docs("src/docs-unprefixed.ts"),
    ])
    .await;
    assert_eq!(handler_serves(&result), set(&["GET /items/:id"]));
    assert_eq!(result.stats.mount_prefixes_corroborated, 0);
}

// --- one row per operation (carrick#2094) ---------------------------------
//
// Once the scope's prefix is read, the handler and the documentation state
// one operation. It is one row: the handler's site, which its types are read
// at, under the documentation's fact label.

/// The rows serving `GET /api/v1/items/<param>`, as (file, label).
fn item_rows(result: &FileCentricAnalysisResult) -> Vec<(String, String)> {
    let mut rows: Vec<(String, String)> = result
        .mount_graph
        .get_resolved_endpoints()
        .iter()
        .filter(|endpoint| {
            endpoint.method == "GET"
                && endpoint
                    .full_path
                    .strip_prefix("/api/v1/items/")
                    .is_some_and(|rest| !rest.contains('/'))
        })
        .map(|endpoint| {
            let file = endpoint
                .file_location
                .rsplit_once("/src/")
                .map(|(_, file)| file.split(':').next().unwrap_or(file).to_string())
                .unwrap_or_default();
            (file, format!("{:?}", endpoint.resolution_source))
        })
        .collect();
    rows.sort();
    rows
}

#[tokio::test]
#[serial]
async fn a_documented_handler_is_one_operation() {
    let result = analyze(vec![
        unstated("src/documented.ts"),
        item_detail(),
        docs("src/docs.ts"),
    ])
    .await;
    assert_eq!(
        item_rows(&result),
        vec![(
            "item-detail.ts".to_string(),
            "Some(DescriptorRoute)".to_string()
        )]
    );
}

#[tokio::test]
#[serial]
async fn a_documented_route_with_no_handler_stays_untyped() {
    let result = analyze(vec![docs("src/docs.ts")]).await;
    assert_eq!(
        item_rows(&result),
        vec![("docs.ts".to_string(), "Some(DescriptorRoute)".to_string())]
    );
}

#[tokio::test]
#[serial]
async fn two_handlers_for_one_documented_path_are_not_merged() {
    let twin = answer(
        "src/twin.ts",
        &[],
        &[(
            "app.get('/api/v1/items/:id'",
            "app",
            "GET",
            "/api/v1/items/:id",
        )],
    );
    let result = analyze(vec![
        unstated("src/documented.ts"),
        item_detail(),
        twin,
        docs("src/docs.ts"),
    ])
    .await;
    assert_eq!(
        item_rows(&result),
        vec![
            ("docs.ts".to_string(), "Some(DescriptorRoute)".to_string()),
            ("item-detail.ts".to_string(), "Some(Model)".to_string()),
            ("twin.ts".to_string(), "Some(Model)".to_string()),
        ]
    );
}

// --- a case on a method that carries no body (carrick#2110) ---------------
//
// `reports.ts` holds four shapes behind one plugin. The model states each
// value a handler switches on as a dispatch case, and a case has only two
// locations, so a field read off the query string of a GET arrives as `body`.

/// A dispatch case the model states: `(location, field, value)`.
type Case<'a> = (&'a str, &'a str, &'a str);

/// The model's answer for `src/reports.ts`: one row per `(needle on the
/// registration's line, which registration on that line, method, path,
/// case)`.
fn reports(rows: &[(&str, usize, &str, &str, Option<Case>)]) -> (String, FileAnalysisResult) {
    let file = "src/reports.ts";
    let scanned = SwcScanner::new().scan_content(&root().join(file), &source(file), &[], &[]);
    let endpoints: Vec<serde_json::Value> = rows
        .iter()
        .map(|(needle, nth, method, path, case)| {
            let line = line_of(file, needle);
            let mut on_line: Vec<_> = scanned
                .candidates
                .iter()
                .filter(|candidate| candidate.line_number == line)
                .collect();
            on_line.sort_by_key(|candidate| candidate.span_start);
            let mut row = serde_json::json!({
                "candidate_id": on_line[*nth].candidate_id,
                "line_number": line,
                "owner_node": "app",
                "method": method,
                "path": path,
                "handler_name": "anonymous",
                "pattern_matched": "app.",
                "payload_expression_text": null,
                "payload_expression_line": null,
                "response_expression_text": null,
                "response_expression_line": null,
                "primary_type_symbol": null,
                "type_import_source": null
            });
            if let Some((location, field, value)) = case {
                row["dispatch"] =
                    serde_json::json!({ "location": location, "field": field, "value": value });
            }
            row
        })
        .collect();
    let result: FileAnalysisResult = serde_json::from_value(serde_json::json!({
        "mounts": [],
        "endpoints": endpoints,
        "data_calls": [],
    }))
    .expect("a canned answer");
    (root().join(file).to_string_lossy().into_owned(), result)
}

/// The registering file for `reports.ts`, its outer registration unstated.
fn reports_scope() -> (String, FileAnalysisResult) {
    answer(
        "src/documented-modes.ts",
        &[(
            "api.register(reportRoutes)",
            "api",
            "reportRoutes",
            "",
            Some("./reports"),
        )],
        &[],
    )
}

/// Every row serving `method full_path`, as (file, label, case value).
fn rows_serving(
    result: &FileCentricAnalysisResult,
    method: &str,
    full_path: &str,
) -> Vec<(String, String, Option<String>)> {
    let mut rows: Vec<_> = result
        .mount_graph
        .get_resolved_endpoints()
        .iter()
        .filter(|endpoint| {
            endpoint.method == method
                && carrick_match::paths_match(&endpoint.full_path, full_path)
                && endpoint.full_path.split('/').count() == full_path.split('/').count()
        })
        .map(|endpoint| {
            let file = endpoint
                .file_location
                .rsplit_once("/src/")
                .map(|(_, file)| file.split(':').next().unwrap_or(file).to_string())
                .unwrap_or_default();
            (
                file,
                format!("{:?}", endpoint.resolution_source),
                endpoint.dispatch.as_ref().map(|case| case.value.clone()),
            )
        })
        .collect();
    rows.sort();
    rows
}

fn serving(file: &str, label: &str, case: Option<&str>) -> (String, String, Option<String>) {
    (
        file.to_string(),
        label.to_string(),
        case.map(str::to_string),
    )
}

/// A GET carries no request body, so the two modes are one route stated
/// twice, and the documented operation has one handler row to fold onto.
#[tokio::test]
#[serial]
async fn a_documented_get_read_as_two_body_cases_is_one_operation() {
    let mode = |value| Some(("body", "mode", value));
    let result = analyze(vec![
        reports_scope(),
        reports(&[
            (
                "app.get('/reports/:id',",
                0,
                "GET",
                "/reports/:id",
                mode("advanced"),
            ),
            (
                "app.get('/reports/:id',",
                0,
                "GET",
                "/reports/:id",
                mode("simple"),
            ),
        ]),
        docs("src/docs-reports.ts"),
    ])
    .await;
    assert_eq!(
        rows_serving(&result, "GET", "/api/v1/reports/:id"),
        vec![serving("reports.ts", "Some(DescriptorRoute)", None)],
        "one handler row with no case, which the documented operation folds onto"
    );
}

/// Losing a case never merges two routes: the rows that become one agree on
/// owner, method and path, and a shared line is not part of that.
#[tokio::test]
#[serial]
async fn two_routes_on_one_line_stay_two_rows_when_their_cases_go() {
    let view = |value| Some(("body", "view", value));
    let needle = "app.get('/reports/:id/lines'";
    let result = analyze(vec![
        reports_scope(),
        reports(&[
            (needle, 0, "GET", "/reports/:id/lines", view("flat")),
            (needle, 0, "GET", "/reports/:id/lines", view("grouped")),
            (needle, 1, "GET", "/reports/:id/notes", view("flat")),
            (needle, 1, "GET", "/reports/:id/notes", view("grouped")),
        ]),
        docs("src/docs-reports.ts"),
    ])
    .await;
    for path in ["/api/v1/reports/:id/lines", "/api/v1/reports/:id/notes"] {
        assert_eq!(
            rows_serving(&result, "GET", path),
            vec![serving("reports.ts", "Some(DescriptorRoute)", None)],
            "{path}"
        );
    }
}

/// A GET does carry headers and a POST does carry a body, so both keep one
/// row per case (carrick#831). Nothing documents these two routes, so the
/// scope's prefix stays unread.
#[tokio::test]
#[serial]
async fn a_header_case_on_a_get_and_a_body_case_on_a_post_stay_cases() {
    let result = analyze(vec![
        reports_scope(),
        reports(&[
            (
                "app.get('/feed'",
                0,
                "GET",
                "/feed",
                Some(("header", "x-feed-kind", "full")),
            ),
            (
                "app.get('/feed'",
                0,
                "GET",
                "/feed",
                Some(("header", "x-feed-kind", "short")),
            ),
            (
                "app.post('/reports'",
                0,
                "POST",
                "/reports",
                Some(("body", "action", "archive")),
            ),
            (
                "app.post('/reports'",
                0,
                "POST",
                "/reports",
                Some(("body", "action", "create")),
            ),
        ]),
    ])
    .await;
    assert_eq!(
        rows_serving(&result, "GET", "/feed"),
        vec![
            serving("reports.ts", "Some(Model)", Some("full")),
            serving("reports.ts", "Some(Model)", Some("short")),
        ]
    );
    assert_eq!(
        rows_serving(&result, "POST", "/reports"),
        vec![
            serving("reports.ts", "Some(Model)", Some("archive")),
            serving("reports.ts", "Some(Model)", Some("create")),
        ]
    );
}

// --- a registering file the model states no row for (carrick#2303) --------
//
// The model's answer for the registering file is empty. The source still
// states the callback, the instance it is handed and what it registers on
// that instance, so the scope and its mounts are read from the source.

/// A registering file the model stated nothing for.
fn empty(file: &str) -> (String, FileAnalysisResult) {
    docs(file)
}

#[tokio::test]
#[serial]
async fn a_scope_the_model_left_empty_takes_the_prefix_its_service_documents() {
    let result = analyze(vec![
        empty("src/documented.ts"),
        item_detail(),
        docs("src/docs.ts"),
    ])
    .await;
    assert_eq!(handler_serves(&result), set(&["GET /api/v1/items/:id"]));
    assert_eq!(result.stats.mount_prefixes_corroborated, 1);
    assert_eq!(result.stats.mount_prefixes_unread, 0);
    assert_eq!(
        item_rows(&result),
        vec![(
            "item-detail.ts".to_string(),
            "Some(DescriptorRoute)".to_string()
        )]
    );
}

#[tokio::test]
#[serial]
async fn a_scope_the_model_left_empty_and_nothing_documents_records_its_prefix_unread() {
    let result = analyze(vec![empty("src/documented.ts"), item_detail()]).await;
    assert_eq!(handler_serves(&result), set(&["GET /items/:id"]));
    assert_eq!(result.stats.mount_prefixes_corroborated, 0);
    assert_eq!(result.stats.mount_prefixes_unread, 1);
    let line = line_of("src/documented.ts", "app.register(");
    assert_eq!(
        result
            .unread_mount_prefixes
            .iter()
            .map(|record| record.scope_id.as_str())
            .collect::<Vec<_>>(),
        vec![format!("api@src/documented.ts:{line}").as_str()]
    );
}

#[tokio::test]
#[serial]
async fn a_stated_inner_mount_is_not_stated_twice() {
    // The model stated the inner registration and not the outer one. The
    // source read finds the same call, and the model's row is the one kept.
    let result = analyze(vec![unstated("src/documented.ts"), item_detail()]).await;
    let mounts = result.mount_graph.get_mounts();
    let inner: Vec<_> = mounts
        .iter()
        .filter(|edge| edge.child == "itemDetailRoutes")
        .collect();
    assert_eq!(inner.len(), 1, "{mounts:?}");
    assert_eq!(mounts.len(), 2, "the scope and its one mount: {mounts:?}");
}

#[tokio::test]
#[serial]
async fn a_function_that_registers_nothing_is_not_a_mount() {
    // `onError` is imported and passed first to a call on the scope's
    // parameter, exactly as a plugin is. Its module also declares a router
    // whose routes hang from a parameter of the same name, so only the span
    // of the function that was passed tells the two apart.
    let status = answer(
        "src/errors.ts",
        &[],
        &[("app.get('/status'", "app", "GET", "/status")],
    );
    let index = answer(
        "src/status-index.ts",
        &[(
            "server.register(statusRoutes",
            "server",
            "statusRoutes",
            "/internal",
            Some("./errors"),
        )],
        &[],
    );
    let result = analyze(vec![empty("src/not-a-plugin.ts"), status, index]).await;
    let mounts = result.mount_graph.get_mounts();
    assert!(
        mounts
            .iter()
            .all(|edge| edge.child == "statusRoutes" && edge.parent == "server"),
        "{mounts:?}"
    );
    assert_eq!(served(&result), set(&["GET /internal/status"]));
    assert!(result.unread_mount_prefixes.is_empty());
    assert_eq!(result.stats.mount_prefixes_unread, 0);
}

// carrick#2316: a callback that hands its instance to a helper as an
// argument. `registerItemRoutes(api, deps)` is a plain call, so no model row
// states a mount there. The source states which parameter of the helper
// receives the scope's instance, and the helper's own rows say whether it
// registers on it.

/// A registering file whose outer registration the model stated with the
/// literal prefix, and nothing for the calls inside the callback.
fn scope_only(file: &str) -> (String, FileAnalysisResult) {
    answer(
        file,
        &[("app.register(", "app", "api", "/api/v1", None)],
        &[],
    )
}

fn team_items() -> (String, FileAnalysisResult) {
    answer(
        "src/team-items.ts",
        &[],
        &[(
            "app.get('/teams/:id/items'",
            "app",
            "GET",
            "/teams/:id/items",
        )],
    )
}

fn team_notes() -> (String, FileAnalysisResult) {
    answer(
        "src/team-notes.ts",
        &[],
        &[(
            "app.get('/teams/:id/notes'",
            "app",
            "GET",
            "/teams/:id/notes",
        )],
    )
}

fn admin_routes() -> (String, FileAnalysisResult) {
    answer(
        "src/admin-routes.ts",
        &[],
        &[("app.get('/admin/users'", "app", "GET", "/admin/users")],
    )
}

fn chain() -> (String, FileAnalysisResult) {
    answer(
        "src/chain.ts",
        &[],
        &[("app.get('/leaf'", "app", "GET", "/leaf")],
    )
}

/// The children the graph holds a mount for.
fn mounted(result: &FileCentricAnalysisResult) -> BTreeSet<String> {
    result
        .mount_graph
        .get_mounts()
        .iter()
        .map(|edge| edge.child.clone())
        .collect()
}

#[tokio::test]
#[serial]
async fn a_helper_handed_the_scopes_instance_takes_its_prefix() {
    let result = analyze(vec![scope_only("src/helper-first.ts"), team_items()]).await;
    assert_eq!(served(&result), set(&["GET /api/v1/teams/:id/items"]));
    assert!(result.unread_mount_prefixes.is_empty());
}

#[tokio::test]
#[serial]
async fn a_helper_that_takes_the_instance_second_takes_its_prefix() {
    let result = analyze(vec![scope_only("src/helper-second.ts"), team_notes()]).await;
    assert_eq!(served(&result), set(&["GET /api/v1/teams/:id/notes"]));
}

#[tokio::test]
#[serial]
async fn a_helper_that_only_hands_the_instance_on_carries_the_prefix() {
    // `registerAllRoutes` registers nothing itself. Its file holds no row the
    // model reads, so it goes through the scan as any such file does, with no
    // canned answer.
    let result = analyze_with(
        vec![scope_only("src/helper-chain.ts"), team_items()],
        &["src/all-routes.ts"],
    )
    .await;
    assert_eq!(served(&result), set(&["GET /api/v1/teams/:id/items"]));
    let mounts = result.mount_graph.get_mounts();
    assert!(
        mounts
            .iter()
            .any(|edge| edge.parent == "registerAllRoutes" && edge.child == "registerItemRoutes"),
        "the hand-on is a mount of the file that makes it: {mounts:?}"
    );
}

#[tokio::test]
#[serial]
async fn an_instance_is_followed_three_functions_on_and_no_further() {
    let result = analyze(vec![scope_only("src/helper-depth-3.ts"), chain()]).await;
    assert_eq!(served(&result), set(&["GET /api/v1/leaf"]));
    let result = analyze(vec![scope_only("src/helper-depth-4.ts"), chain()]).await;
    assert_eq!(served(&result), set(&["GET /leaf"]));
    assert_eq!(mounted(&result), set(&["api"]));
}

#[tokio::test]
#[serial]
async fn a_helper_called_with_another_instance_keeps_its_path() {
    // `registerAdminRoutes(app)` is called inside the callback with the
    // instance the plugin itself was given.
    let result = analyze(vec![
        scope_only("src/helper-other-instance.ts"),
        team_items(),
        admin_routes(),
    ])
    .await;
    assert_eq!(
        served(&result),
        set(&["GET /api/v1/teams/:id/items", "GET /admin/users"])
    );
    assert!(!mounted(&result).contains("registerAdminRoutes"));
}

#[tokio::test]
#[serial]
async fn a_member_a_wrapped_value_and_a_spread_are_not_the_instance() {
    let result = analyze(vec![
        scope_only("src/helper-not-the-instance.ts"),
        team_items(),
        team_notes(),
        admin_routes(),
    ])
    .await;
    assert_eq!(
        served(&result),
        set(&[
            "GET /teams/:id/items",
            "GET /teams/:id/notes",
            "GET /admin/users"
        ])
    );
    assert_eq!(mounted(&result), set(&["api"]));
}

#[tokio::test]
#[serial]
async fn another_binding_with_the_parameters_name_is_not_the_instance() {
    let result = analyze(vec![scope_only("src/helper-shadowed.ts"), team_items()]).await;
    assert_eq!(served(&result), set(&["GET /teams/:id/items"]));
    assert_eq!(mounted(&result), set(&["api"]));
}

#[tokio::test]
#[serial]
async fn a_helper_that_registers_nothing_on_the_instance_is_not_a_mount() {
    // `attachHooks` adds a hook. Its module also declares a router whose
    // routes hang from a parameter of the same name, so only the span of the
    // function that was called tells the two apart.
    let hooks = answer(
        "src/request-hooks.ts",
        &[],
        &[("app.get('/hook-status'", "app", "GET", "/hook-status")],
    );
    let result = analyze(vec![scope_only("src/helper-hook.ts"), hooks]).await;
    assert_eq!(served(&result), set(&["GET /hook-status"]));
    assert_eq!(mounted(&result), set(&["api"]));
}

#[tokio::test]
#[serial]
async fn a_helper_called_from_two_scopes_is_served_under_both() {
    let scopes = answer(
        "src/helper-two-scopes.ts",
        &[
            ("// first scope", "app", "api", "/a", None),
            ("// second scope", "app", "api", "/b", None),
        ],
        &[],
    );
    let result = analyze(vec![scopes, team_items()]).await;
    assert_eq!(
        served(&result),
        set(&["GET /a/teams/:id/items", "GET /b/teams/:id/items"])
    );
}

#[tokio::test]
#[serial]
async fn two_helpers_of_one_file_handed_one_scope_keep_their_paths() {
    // Both helpers are mounted from the scope, and both resolve to one file.
    // The graph does not decide which of the two a route belongs to, so
    // neither route takes the prefix: the refusal is pinned here.
    let pair = answer(
        "src/pair.ts",
        &[],
        &[
            ("app.get('/left'", "app", "GET", "/left"),
            ("app.get('/right'", "app", "GET", "/right"),
        ],
    );
    let result = analyze(vec![scope_only("src/helper-pair.ts"), pair]).await;
    assert_eq!(served(&result), set(&["GET /left", "GET /right"]));
    assert!(mounted(&result).contains("registerLeft"));
    assert!(mounted(&result).contains("registerRight"));
}

#[tokio::test]
#[serial]
async fn a_helper_declared_beside_the_scope_keeps_its_path() {
    // The mount is stated, but a function the registering file declares is
    // not a binding the graph attributes routes to, so the route keeps its
    // path. Pinned as it stands today.
    let local = answer(
        "src/helper-local.ts",
        &[("app.register(", "app", "api", "/api/v1", None)],
        &[("server.get('/local'", "server", "GET", "/local")],
    );
    let result = analyze(vec![local]).await;
    assert_eq!(served(&result), set(&["GET /local"]));
    assert!(mounted(&result).contains("registerLocalRoutes"));
}

#[test]
fn the_fixture_root_is_on_disk() {
    assert!(Path::new(&root()).join("src/nullish.ts").is_file());
}
