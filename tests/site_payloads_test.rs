//! The type capture reads a payload where the fact layer places it (phase 3
//! step 1, part of carrick#2474).
//!
//! Drives the real scanner binary offline over `tests/fixtures/site-payloads/`
//! with the type sidecar LIVE (no `CARRICK_ALLOW_MISSING_TYPES`, so a sidecar
//! that cannot start fails the test) and the compiler's position rows on
//! (`CARRICK_POSITION_ROWS=1`), as the Action runs a scan with them.
//!
//! `src/orders.ts:14` calls `http.post<Receipt>('/orders', order)` through a
//! package the library store labels an HTTP client whose `post` sends its
//! second argument. The reader's cassette states the call, `POST /orders`,
//! with no locator at all: no call text, no span, no payload text, no type
//! symbol (carrick-cloud#2522's shape).
//!
//! Answer key:
//!
//! | switch | request alias | response alias |
//! |---|---|---|
//! | off | `unknown` (a manifest placeholder) | names `Receipt` (read at the call's line) |
//! | `CARRICK_SITE_PAYLOADS=1` | `Order`'s shape, `{ sku: string; quantity: number }` (the body argument) | names `Receipt` (the call's result) |

use std::path::{Path, PathBuf};
use std::process::Command;

fn fixture_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/site-payloads")
}

/// The blob one offline scan of `repo` writes, with `env` set.
fn scan(repo: &Path, env: &[(&str, &str)]) -> serde_json::Value {
    let storage = tempfile::tempdir().expect("temp storage dir");
    let cache = tempfile::tempdir().expect("temp cache dir");
    let mut cmd = Command::new(PathBuf::from(env!("CARGO_BIN_EXE_carrick")));
    cmd.arg(repo)
        .env("CARRICK_LOCAL_STORAGE_DIR", storage.path())
        .env("CARRICK_LOCAL_STORAGE_ISOLATE", "1")
        .env("CARRICK_CACHE_DIR", cache.path())
        .env("CARRICK_MOCK_ALL", "1")
        .env(
            "CARRICK_MOCK_FIXTURE_DIR",
            format!("{}/", repo.join("__llm__").display()),
        )
        .env("CARRICK_SKIP_INTENTS", "1")
        .env("CARRICK_POSITION_ROWS", "1")
        .env_remove("CARRICK_ALLOW_MISSING_TYPES")
        .env_remove("CARRICK_SITE_PAYLOADS")
        .env_remove("CARRICK_SITE_QUESTIONS");
    for (name, value) in env {
        cmd.env(name, value);
    }
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
        "scan exited non-zero:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let mut blobs: Vec<serde_json::Value> = std::fs::read_dir(storage.path())
        .expect("storage dir")
        .flatten()
        .filter(|entry| entry.path().extension().is_some_and(|ext| ext == "json"))
        .map(|entry| {
            serde_json::from_slice(&std::fs::read(entry.path()).expect("read blob"))
                .expect("parse blob")
        })
        .collect();
    assert_eq!(blobs.len(), 1, "one service, one blob");
    blobs.remove(0)
}

/// The alias of the consumer `kind` entry at `src/orders.ts:14`.
fn alias(blob: &serde_json::Value, kind: &str) -> String {
    let entries: Vec<&serde_json::Value> = blob["type_manifest"]
        .as_array()
        .expect("type_manifest")
        .iter()
        .filter(|entry| {
            entry["role"] == "consumer"
                && entry["type_kind"] == kind
                && entry["file_path"] == "src/orders.ts"
                && entry["line_number"] == 14
        })
        .collect();
    assert_eq!(entries.len(), 1, "one consumer {kind} entry: {entries:#?}");
    entries[0]["type_alias"].as_str().unwrap().to_string()
}

/// What the capture stub says `alias` is: the right-hand side of its
/// `export type` line.
fn captured(blob: &serde_json::Value, alias: &str) -> String {
    let files = blob["capture_stub"]["files"]
        .as_object()
        .expect("a capture stub: the sidecar ran");
    let head = format!("export type {alias} = ");
    files
        .values()
        .filter_map(|text| text.as_str())
        .flat_map(str::lines)
        .find_map(|line| line.trim().strip_prefix(&head).map(str::to_string))
        .unwrap_or_else(|| panic!("no `{head}` line in the stub: {files:#?}"))
}

/// The whole declaration of `alias` in the stub's surface, across lines.
fn captured_block(blob: &serde_json::Value, alias: &str) -> String {
    let surface = blob["capture_stub"]["files"]["types/surface.d.ts"]
        .as_str()
        .expect("a capture surface: the sidecar ran");
    let head = format!("export type {alias} = ");
    let start = surface
        .find(&head)
        .unwrap_or_else(|| panic!("no `{head}` in the surface: {surface}"));
    let rest = &surface[start + head.len()..];
    rest[..rest.find("\nexport ").unwrap_or(rest.len())].to_string()
}

#[test]
fn a_call_the_reader_left_unlocated_is_typed_at_its_payload_slots() {
    let off = scan(&fixture_dir(), &[]);
    let on = scan(&fixture_dir(), &[("CARRICK_SITE_PAYLOADS", "1")]);

    // The known negative: the reader states no locator, so off, the request
    // is a placeholder. The response is read at the call's line already
    // (a line-only request finds the call there), and must stay so.
    let (request, response) = (alias(&off, "request"), alias(&off, "response"));
    assert_eq!(captured(&off, &request).trim_end_matches(';'), "unknown");
    assert!(
        captured(&off, &response).contains("Receipt"),
        "response off: {}",
        captured(&off, &response)
    );

    // On, the request is read at the body argument and the response at the
    // call's result.
    assert_eq!(alias(&on, "request"), request, "the aliases do not move");
    // The capture prints a body inferred at an expression structurally.
    let request_type = captured_block(&on, &request);
    let response_type = captured(&on, &response);
    assert!(
        request_type.contains("sku: string") && request_type.contains("quantity: number"),
        "request: {request_type}"
    );
    assert!(
        response_type.contains("Receipt"),
        "response: {response_type}"
    );
}

#[test]
fn with_the_switch_at_anything_but_one_the_blob_is_unchanged() {
    let off = scan(&fixture_dir(), &[]);
    let not_one = scan(&fixture_dir(), &[("CARRICK_SITE_PAYLOADS", "true")]);
    let (request, response) = (alias(&not_one, "request"), alias(&not_one, "response"));
    assert_eq!(captured(&not_one, &request), captured(&off, &request));
    assert_eq!(captured(&not_one, &response), captured(&off, &response));
    assert_eq!(off["type_manifest"], not_one["type_manifest"]);
    assert_eq!(off["capture_stub"], not_one["capture_stub"]);
}
