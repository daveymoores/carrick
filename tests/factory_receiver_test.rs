//! Call edges through a receiver bound from a factory call (carrick#1146).
//!
//! Drives the real scanner binary, offline with a cassette-mocked model, over
//! `tests/fixtures/factory-receiver/`. The one scanned service binds a
//! receiver from `createClient()`, where the class is stated by the
//! declaration of the function called and by nothing at the binding itself.
//! Asserts on the uploaded blob, as `workspace_member_callers_test` does,
//! because the edges live in `function_definitions[].calls`.
//!
//! The sibling package publishes two classes that declare the same member
//! names, so the package-surface join (carrick#781) answers nothing for these
//! receivers. Every positive assertion therefore fails without the factory
//! read. The other callers are the answer key for what states no class.

use std::collections::HashMap;
use std::path::PathBuf;
use std::process::Command;

/// One resolved edge, as the blob records it.
#[derive(Debug, PartialEq, Eq, PartialOrd, Ord)]
struct Edge {
    callee: String,
    callee_file: String,
    call_site_line: u64,
}

fn fixture_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/factory-receiver")
}

/// Scan the fixture offline and return every function's resolved call edges,
/// keyed by definition key and sorted.
fn scan_edges() -> HashMap<String, Vec<Edge>> {
    let storage = tempfile::tempdir().expect("temp storage dir");
    let cache = tempfile::tempdir().expect("temp cache dir");
    let cassettes = fixture_dir().join("__llm__");
    assert!(cassettes.exists(), "fixture cassette dir missing");

    let mut cmd = Command::new(PathBuf::from(env!("CARGO_BIN_EXE_carrick")));
    cmd.arg(fixture_dir())
        .env("CARRICK_LOCAL_STORAGE_DIR", storage.path())
        .env("CARRICK_LOCAL_STORAGE_ISOLATE", "1")
        .env("CARRICK_CACHE_DIR", cache.path())
        .env("CARRICK_MOCK_ALL", "1")
        .env(
            "CARRICK_MOCK_FIXTURE_DIR",
            format!("{}/", cassettes.display()),
        )
        .env("CARRICK_SKIP_INTENTS", "1");
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
    let output = cmd.output().expect("failed to spawn carrick");
    assert!(
        output.status.success(),
        "fixture scan exited non-zero:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let mut blobs = std::fs::read_dir(storage.path())
        .expect("storage dir")
        .filter_map(|entry| entry.ok().map(|e| e.path()))
        .filter(|path| path.extension().is_some_and(|ext| ext == "json"))
        .collect::<Vec<_>>();
    blobs.sort();
    assert_eq!(blobs.len(), 1, "expected one uploaded blob, got {blobs:?}");

    let blob: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&blobs[0]).expect("read blob")).expect("parse blob");
    blob["function_definitions"]
        .as_object()
        .expect("blob has no function_definitions")
        .iter()
        .map(|(key, def)| {
            let mut edges: Vec<Edge> = def["calls"]
                .as_array()
                .cloned()
                .unwrap_or_default()
                .iter()
                .map(|call| Edge {
                    callee: call["name"].as_str().unwrap_or_default().to_string(),
                    callee_file: call["file_path"].as_str().unwrap_or_default().to_string(),
                    call_site_line: call["call_site_line"].as_u64().unwrap_or_default(),
                })
                .collect();
            edges.sort();
            (key.clone(), edges)
        })
        .collect()
}

const SDK: &str = "packages/sdk/src/index.ts";
const SESSION: &str = "packages/app/src/session.ts";

fn edge(callee: &str, file: &str, line: u64) -> Edge {
    Edge {
        callee: callee.to_string(),
        callee_file: file.to_string(),
        call_site_line: line,
    }
}

#[test]
fn a_receiver_bound_from_an_annotated_factory_records_the_member_edge() {
    let edges = scan_edges();
    assert_eq!(
        edges.get("showRun").map(Vec::as_slice),
        Some(
            [
                edge("Client.getRun", SDK, 13),
                edge("createClient", SDK, 12)
            ]
            .as_slice()
        ),
        "all edges were {edges:?}"
    );
}

#[test]
fn an_awaited_factory_that_promises_the_class_records_the_member_edge() {
    let edges = scan_edges();
    assert_eq!(
        edges.get("showRemoteRun").map(Vec::as_slice),
        Some([edge("Client.getRun", SDK, 19), edge("openClient", SDK, 18)].as_slice()),
        "all edges were {edges:?}"
    );
}

#[test]
fn a_factory_that_returns_another_factorys_call_is_followed_three_functions_deep() {
    let edges = scan_edges();
    assert_eq!(
        edges.get("listRuns").map(Vec::as_slice),
        Some(
            [
                edge("Client.listRuns", SDK, 25),
                edge("getClient", SESSION, 24)
            ]
            .as_slice()
        ),
        "all edges were {edges:?}"
    );
    assert_eq!(
        edges.get("listShared").map(Vec::as_slice),
        Some(
            [
                edge("Client.listRuns", SDK, 31),
                edge("sharedClient", SESSION, 30)
            ]
            .as_slice()
        ),
        "all edges were {edges:?}"
    );
}

/// Each of these binds a receiver from a call, and none of them states one
/// class: the only edge is the call to the factory itself.
#[test]
fn a_factory_that_states_no_class_records_only_its_own_call() {
    let edges = scan_edges();
    let mut wrong = Vec::new();
    for (caller, factory, file, line) in [
        // A promise of the class, never awaited.
        ("showPending", "openClient", SDK, 36),
        // An interface.
        ("showThroughReader", "createReader", SDK, 42),
        // A union of two classes.
        ("showEither", "createEither", SDK, 48),
        // No annotation, two `return` statements.
        ("showByMode", "createByMode", SDK, 54),
        // A `let`.
        ("showRebound", "createClient", SDK, 60),
        // A destructured binding.
        ("showPart", "createClient", SDK, 66),
        // Four functions deep.
        ("listDistant", "distantClient", SESSION, 81),
    ] {
        if edges.get(caller).map(Vec::as_slice) != Some([edge(factory, file, line)].as_slice()) {
            wrong.push((caller, edges.get(caller)));
        }
    }

    // The factory is a parameter that shadows the import of the same name.
    let injected = edges.get("showInjected").expect("showInjected is indexed");
    if injected.iter().any(|edge| edge.callee == "Client.getRun") {
        wrong.push(("showInjected", Some(injected)));
    }
    assert!(
        wrong.is_empty(),
        "each of these must record only its factory call: {wrong:?}"
    );
}
