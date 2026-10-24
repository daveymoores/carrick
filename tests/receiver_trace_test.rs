//! The receiver trace's switch (carrick#2474 R1).
//!
//! Drives the real scanner binary on `.`, as the Action runs it, over a
//! committed copy of `tests/fixtures/receiver-trace/repo` (one root service),
//! offline, with the fixture's store answer replayed as the library store's.
//! With the fact layer on (`CARRICK_POSITION_ROWS=1`,
//! `CARRICK_SITE_QUESTIONS=1`) and `CARRICK_RECEIVER_TRACE` unset, the routes
//! only the trace finds leave nothing in the blob; with it set they reach
//! the blob as the site pass writes route sites. With the fact layer off the
//! switch changes no byte.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;

fn fixture_dir(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name)
}

fn run_git(dir: &Path, args: &[&str]) {
    let status = Command::new("git")
        .args(args)
        .current_dir(dir)
        .env("GIT_AUTHOR_NAME", "carrick-test")
        .env("GIT_AUTHOR_EMAIL", "test@example.com")
        .env("GIT_COMMITTER_NAME", "carrick-test")
        .env("GIT_COMMITTER_EMAIL", "test@example.com")
        // One commit hash for every copy, so two scans' blobs compare.
        .env("GIT_AUTHOR_DATE", "2026-01-01T00:00:00Z")
        .env("GIT_COMMITTER_DATE", "2026-01-01T00:00:00Z")
        .status()
        .expect("git runs");
    assert!(status.success(), "git {args:?} failed");
}

fn copy_dir(src: &Path, dst: &Path) {
    std::fs::create_dir_all(dst).unwrap();
    for entry in std::fs::read_dir(src).unwrap() {
        let entry = entry.unwrap();
        let to = dst.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_dir(&entry.path(), &to);
        } else {
            std::fs::copy(entry.path(), &to).unwrap();
        }
    }
}

/// Every blob the scan stored, by file name, with the run's own stamps
/// removed.
type Blobs = BTreeMap<String, String>;

/// One offline scan of a committed copy of the fixture: the fact layer on or
/// off, and the trace switch set to `trace` or unset.
fn scan(fact_layer: bool, trace: Option<&str>) -> Blobs {
    scan_fixture("receiver-trace", fact_layer, trace)
}

/// [`scan`] of the fixture `name`.
fn scan_fixture(name: &str, fact_layer: bool, trace: Option<&str>) -> Blobs {
    scan_runs(name, &[], fact_layer, trace, 1).remove(0)
}

/// `runs` scans of one committed copy of the fixture `name`, with `extra`
/// files written into it before the commit, sharing one store and cache:
/// every scan after the first takes the incremental path. The blobs after
/// each scan.
fn scan_runs(
    name: &str,
    extra: &[(&str, &str)],
    fact_layer: bool,
    trace: Option<&str>,
    runs: usize,
) -> Vec<Blobs> {
    let tmp = tempfile::tempdir().expect("temp dir");
    let repo = tmp.path().join("workspace");
    copy_dir(&fixture_dir(name).join("repo"), &repo);
    for (relative, contents) in extra {
        std::fs::write(repo.join(relative), contents).unwrap();
    }
    run_git(&repo, &["init", "-q"]);
    run_git(&repo, &["add", "-A"]);
    run_git(&repo, &["commit", "-q", "-m", "init"]);
    let cassettes = tmp.path().join("cassettes");
    std::fs::create_dir_all(cassettes.join("library-claims")).unwrap();
    std::fs::copy(
        fixture_dir(name).join("library-claims.json"),
        cassettes.join("library-claims/default.json"),
    )
    .unwrap();
    let storage = tmp.path().join("storage");
    std::fs::create_dir_all(&storage).unwrap();
    let home = tmp.path().join("home");
    std::fs::create_dir_all(&home).unwrap();
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_carrick"));
    cmd.arg(".")
        .current_dir(&repo)
        .env("CARRICK_LOCAL_STORAGE_DIR", &storage)
        .env("CARRICK_LOCAL_STORAGE_ISOLATE", "1")
        .env("CARRICK_CACHE_DIR", tmp.path().join("cache"))
        .env("HOME", &home)
        .env("CARRICK_MOCK_ALL", "1")
        .env(
            "CARRICK_MOCK_FIXTURE_DIR",
            format!("{}/", cassettes.display()),
        )
        .env("CARRICK_SKIP_INTENTS", "1");
    for var in [
        "CARRICK_POSITION_ROWS",
        "CARRICK_SITE_QUESTIONS",
        "CARRICK_RECEIVER_TRACE",
        "GITHUB_REPOSITORY",
        "GITHUB_REF",
        "GITHUB_EVENT_NAME",
        "GITHUB_EVENT_PATH",
        "GITHUB_SHA",
        "GITHUB_RUN_ID",
        "GITHUB_ACTIONS",
        "GITHUB_WORKSPACE",
        "CI",
        "CARRICK_OUTPUT_JSON",
        "ACTIONS_ID_TOKEN_REQUEST_URL",
        "ACTIONS_ID_TOKEN_REQUEST_TOKEN",
    ] {
        cmd.env_remove(var);
    }
    if fact_layer {
        cmd.env("CARRICK_POSITION_ROWS", "1")
            .env("CARRICK_SITE_QUESTIONS", "1");
    }
    if let Some(trace) = trace {
        cmd.env("CARRICK_RECEIVER_TRACE", trace);
    }
    let mut scans = vec![run_scan(&mut cmd, &storage)];
    // A later scan reads the store back, so it finds the first scan's blob.
    cmd.env_remove("CARRICK_LOCAL_STORAGE_ISOLATE");
    scans.extend((1..runs).map(|_| run_scan(&mut cmd, &storage)));
    // Each later scan took the incremental path: its run log says so.
    let incremental = std::fs::read_dir(home.join(".carrick/logs/runs"))
        .map(|logs| {
            logs.filter(|log| {
                std::fs::read_to_string(log.as_ref().unwrap().path())
                    .unwrap_or_default()
                    .contains("Incremental analysis complete")
            })
            .count()
        })
        .unwrap_or_default();
    assert_eq!(incremental, runs - 1, "a re-scan took the cold path");
    scans
}

/// One scan by `cmd`, and the blobs it left in `storage`.
fn run_scan(cmd: &mut Command, storage: &Path) -> Blobs {
    let output = cmd.output().expect("spawn carrick");
    assert!(
        output.status.success(),
        "scan exited non-zero:\n{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let mut blobs = Blobs::new();
    for entry in std::fs::read_dir(storage).expect("the store reads") {
        let path = entry.expect("dir entry").path();
        if path.extension().is_none_or(|ext| ext != "json") {
            continue;
        }
        let mut blob: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        if let Some(object) = blob.as_object_mut() {
            for stamp in ["scanner_version", "scanner_build", "last_updated"] {
                object.remove(stamp);
            }
        }
        blobs.insert(
            path.file_name().unwrap().to_string_lossy().into_owned(),
            serde_json::to_string(&blob).unwrap(),
        );
    }
    assert!(!blobs.is_empty(), "the scan stored a blob");
    blobs
}

/// Every possible call the blobs hold, as `file:line member`.
fn possible_calls(blobs: &Blobs) -> Vec<String> {
    let mut out: Vec<String> = blobs
        .values()
        .flat_map(|text| {
            let blob: serde_json::Value = serde_json::from_str(text).unwrap();
            blob["possible_calls"]
                .as_array()
                .cloned()
                .unwrap_or_default()
        })
        .map(|call| {
            format!(
                "{}:{} {}",
                call["file_path"].as_str().unwrap_or_default(),
                call["line_number"],
                call["labelled_end"]["member"].as_str().unwrap_or_default()
            )
        })
        .collect();
    out.sort();
    out
}

/// carrick#2474 R1: the routes only the trace finds (a parameter, a
/// callback's parameter, a field set from a constructor parameter) reach
/// the blob only with the switch set to `1`: as rows where the trace serves
/// them from the top (R3 S1), otherwise as route sites with no owner.
/// Unset, the blob holds the route sites found without it. With the fact
/// layer off the switch changes no byte.
#[test]
fn the_receiver_trace_switch_moves_only_what_it_finds() {
    let off = scan(true, None);
    assert_eq!(possible_calls(&off), vec!["src/app.ts:25 get".to_string()]);
    let on = scan(true, Some("1"));
    assert_eq!(
        possible_calls(&on),
        vec![
            "src/app.ts:19 post".to_string(),
            // The file's other routes have endpoint rows at their lines.
            "src/helpers.ts:47 get".to_string(),
            "src/helpers.ts:61 get".to_string(),
            "src/routes.ts:11 get".to_string(),
            // `src/app.ts:25` and `src/users.ts:8` are served from the top:
            // the trace writes their rows (carrick#2474 R3 S1).
        ]
    );
    // Only `1` turns it on.
    assert_eq!(scan(true, Some("true")), off);
    // With the fact layer off the switch changes no byte.
    assert_eq!(scan(false, Some("1")), scan(false, None));
}

/// Every HTTP endpoint row of the blobs' mount graphs, as `file:line METHOD
/// full_path source owner`, and the same of the payload's flat `endpoints`.
fn endpoint_rows(blobs: &Blobs) -> (Vec<String>, Vec<String>) {
    let source = |row: &serde_json::Value| {
        row["resolution_source"]
            .as_str()
            .unwrap_or_default()
            .to_string()
    };
    let mut graph = Vec::new();
    let mut flat = Vec::new();
    for text in blobs.values() {
        let blob: serde_json::Value = serde_json::from_str(text).unwrap();
        for row in blob["mount_graph"]["endpoints"]
            .as_array()
            .cloned()
            .unwrap_or_default()
        {
            graph.push(format!(
                "{} {} {} {} {}",
                row["file_location"].as_str().unwrap_or_default(),
                row["method"].as_str().unwrap_or_default(),
                row["full_path"].as_str().unwrap_or_default(),
                source(&row),
                row["owner"].as_str().unwrap_or_default(),
            ));
        }
        for row in blob["endpoints"].as_array().cloned().unwrap_or_default() {
            if row["key"]["protocol"] != "http" {
                continue;
            }
            flat.push(format!(
                "{} {} {} {}",
                row["file_path"].as_str().unwrap_or_default(),
                row["key"]["method"].as_str().unwrap_or_default(),
                row["key"]["path"].as_str().unwrap_or_default(),
                source(&row),
            ));
        }
    }
    graph.sort();
    flat.sort();
    (graph, flat)
}

/// carrick#2474 R3 S1: with the switch on, every route the trace serves at
/// a full path from the top (the answer key
/// `each_route_is_composed_under_its_scopes_and_mounts`) is a mount-graph
/// row at its line, `library_claim`, and so is the payload's flat row. The
/// mock answer's rows that agree are kept and relabelled; the two whose
/// path the trace disagrees with (`items.ts`, read without its plugin
/// prefixes) are kept as they are and nothing is written there
/// (orchestrator's ruling, 2026-10-23). No row is removed. A route the
/// trace cannot place (a callback option, a plugin nothing calls, a mount
/// it cannot verify) keeps today's reading, a possible call. With the
/// switch off, or with the fact layer off, nothing moves.
#[test]
fn traced_routes_take_their_composed_paths() {
    let off = scan_fixture("receiver-scopes", true, None);
    let (off_graph, _) = endpoint_rows(&off);
    let mock = vec![
        "src/index.ts:14 GET /bare model app",
        "src/index.ts:19 GET /health model app",
        "src/items.ts:12 GET /items model app",
        "src/items.ts:8 GET /leaf model app",
    ];
    assert_eq!(off_graph, mock);
    assert_eq!(
        possible_calls(&off),
        vec![
            "src/admin.ts:8 get",
            "src/extra.ts:9 post",
            "src/index.ts:46 get",
            "src/reports.ts:9 get",
            "src/shared.ts:8 get",
            "src/users.ts:8 get",
            "src/users.ts:9 post",
        ]
    );

    let on = scan_fixture("receiver-scopes", true, Some("1"));
    let (graph, flat) = endpoint_rows(&on);
    assert_eq!(
        graph,
        vec![
            "src/admin.ts:8 GET /admin/stats library_claim admin",
            "src/extra.ts:9 POST /a/pong library_claim shared",
            "src/extra.ts:9 POST /b/pong library_claim shared",
            "src/index.ts:14 GET /bare library_claim app",
            "src/index.ts:19 GET /health library_claim app",
            "src/index.ts:26 GET /in/inline library_claim scoped",
            "src/index.ts:37 GET /v2/x library_claim v2",
            "src/index.ts:38 GET /v3/y library_claim basePath",
            // The trace serves these at `/api/items` and `/api/deep/leaf`:
            // the rows disagree, so they stand and nothing is written.
            "src/items.ts:12 GET /items model app",
            "src/items.ts:8 GET /leaf model app",
            "src/reports.ts:9 GET /r/daily library_claim made",
            "src/shared.ts:8 GET /a/ping library_claim shared",
            "src/shared.ts:8 GET /b/ping library_claim shared",
            "src/users.ts:8 GET /v1/users library_claim users",
            "src/users.ts:9 POST /v1/users/:id library_claim users",
        ]
    );
    let flat_of_graph: Vec<String> = {
        let mut rows: Vec<String> = graph
            .iter()
            .map(|row| row.rsplit_once(' ').unwrap().0.to_string())
            .collect();
        rows.sort();
        rows
    };
    assert_eq!(flat, flat_of_graph);
    // Every row the switch-off scan states is still there, at its path.
    for row in &mock {
        let (site, _) = row.rsplit_once(" model ").unwrap();
        assert!(
            graph.iter().any(|kept| kept.starts_with(site)),
            "{site} was removed: {graph:#?}"
        );
    }
    assert_eq!(
        possible_calls(&on),
        vec![
            "src/index.ts:46 get",
            "src/options.ts:16 get",
            "src/versioned.ts:10 get",
            "src/versioned.ts:22 get",
            "src/versioned.ts:28 get",
        ]
    );
    assert_eq!(
        scan_fixture("receiver-scopes", false, Some("1")),
        scan_fixture("receiver-scopes", false, None)
    );
}

/// Why each possible call's path is unread, as `file:line` to reason. A call
/// whose path is read and whose owner is not is `owner_unknown`.
fn unread_reasons(blobs: &Blobs) -> BTreeMap<String, String> {
    blobs
        .values()
        .flat_map(|text| {
            let blob: serde_json::Value = serde_json::from_str(text).unwrap();
            blob["possible_calls"]
                .as_array()
                .cloned()
                .unwrap_or_default()
        })
        .map(|call| {
            (
                format!(
                    "{}:{}",
                    call["file_path"].as_str().unwrap_or_default(),
                    call["line_number"]
                ),
                call["unread"]["reason"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string(),
            )
        })
        .collect()
}

/// carrick#2662: a registration with a spread argument is a route site like
/// any other once the receiver trace is on. The arguments before the spread
/// read as written (the path is read, so only the owner is unknown, as for a
/// registration with none), and a slot at or after it reads as unread. The
/// registrations are on a local instance, on a parameter, and on a router
/// whose members no declaration verifies for, so only the instance the
/// maker builds names the op. Without the trace they stay no site.
///
/// Every spread here is at or after the handler slot, so the path slot is
/// read, and each instance is served from the top: with the trace on, the
/// trace writes each one's row at its path (carrick#2474 R3 S1) and no
/// possible call is left.
#[test]
fn a_spread_argument_leaves_the_slots_before_it_read() {
    let on_blobs = scan_fixture("receiver-spread", true, Some("1"));
    let on = unread_reasons(&on_blobs);
    let off = unread_reasons(&scan_fixture("receiver-spread", true, None));
    assert_eq!(on, BTreeMap::new());
    assert_eq!(
        endpoint_rows(&on_blobs).0,
        vec![
            "src/app.ts:20 GET /plain library_claim app",
            "src/app.ts:21 GET /spread library_claim app",
            "src/app.ts:22 POST /const library_claim app",
            "src/app.ts:29 GET /plain library_claim api",
            "src/app.ts:30 GET /spread library_claim api",
            "src/app.ts:31 POST /const library_claim api",
            "src/spread.ts:15 GET /by-id library_claim app",
            "src/spread.ts:16 POST /by-name library_claim app",
        ]
    );
    // The registrations with no spread, on the instances the maker builds.
    for line in ["src/app.ts:20", "src/app.ts:29"] {
        assert_eq!(off.get(line).map(String::as_str), Some("owner_unknown"));
    }
    for line in [
        // A spread of a call after a handler; a spread at the handler slot.
        "src/app.ts:21",
        "src/app.ts:22",
        // The same on the router whose declarations verify nothing.
        "src/app.ts:30",
        "src/app.ts:31",
        // A parameter.
        "src/spread.ts:15",
        "src/spread.ts:16",
    ] {
        assert!(!off.contains_key(line), "{line}: no site without the trace");
    }
    // With the fact layer off the switch changes no byte.
    assert_eq!(
        scan_fixture("receiver-spread", false, Some("1")),
        scan_fixture("receiver-spread", false, None)
    );
}

/// The `receiver-scopes` service with a `carrick.json` that declares two
/// operations on `GET /health`, a route the trace serves from the top.
const DECLARED_HEALTH: &str = r#"{
  "services": [{ "name": "web", "directory": ".", "tsconfig": "tsconfig.json" }],
  "operations": [{
    "service": "web",
    "route": "GET /health",
    "dispatch": { "location": "body", "field": "op" },
    "operations": [{ "value": "status" }, { "value": "ready" }]
  }]
}
"#;

/// PR #2687 review: a re-scan applies `carrick.json`'s declared operations
/// before the trace writes its routes, a cold scan after. The declared rows
/// keep `declared_operation` either way, so the re-scan's rows are the cold
/// scan's.
#[test]
fn a_rescan_keeps_the_declared_rows_at_a_traced_route() {
    let scans = scan_runs(
        "receiver-scopes",
        &[("carrick.json", DECLARED_HEALTH)],
        true,
        Some("1"),
        2,
    );
    let (cold, _) = endpoint_rows(&scans[0]);
    let (warm, _) = endpoint_rows(&scans[1]);
    let health: Vec<&String> = cold
        .iter()
        .filter(|row| row.starts_with("src/index.ts:19 "))
        .collect();
    assert_eq!(
        health,
        vec![
            "src/index.ts:19 GET /health declared_operation app",
            "src/index.ts:19 GET /health declared_operation app",
        ],
        "{cold:#?}"
    );
    assert_eq!(warm, cold);
}

/// A router whose routes a loop registers from a table: one call, one
/// span, a key per entry.
const LOOP_TABLE: &str = r#"import router, { Reply, Request } from "fixture-router";

function list(_request: Request, reply: Reply): void {
  reply.send("list");
}

function show(_request: Request, reply: Reply): void {
  reply.send("show");
}

const table = {
  "/t/list": list,
  "/t/show": show,
};

const api = router();
for (const [path, handler] of Object.entries(table)) {
  api.get(path, handler);
}
export default api;
"#;

/// PR #2687 review: a table's keys share their loop's call, so the trace
/// cannot say which key a site is; it writes none of them, and no key's
/// route is written at another key's line.
#[test]
fn routes_a_loop_registers_from_a_table_are_not_written() {
    let table_rows = |trace: Option<&str>| -> Vec<String> {
        let blobs = scan_runs(
            "receiver-scopes",
            &[("src/table.ts", LOOP_TABLE)],
            true,
            trace,
            1,
        )
        .remove(0);
        endpoint_rows(&blobs)
            .0
            .into_iter()
            .filter(|row| row.starts_with("src/table.ts:"))
            .collect()
    };
    assert_eq!(table_rows(Some("1")), table_rows(None));
}
