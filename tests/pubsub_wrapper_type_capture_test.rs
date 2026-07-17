//! Pub/sub wrapper-pattern type capture (expression-locator path).
//!
//! Drives the real scanner binary — offline, cassette-mocked LLM, real
//! sidecar — over `tests/fixtures/pubsub-wrapper-monorepo/`, a monorepo whose
//! messaging layer is built from three generic wrapper shapes:
//!
//! 1. a typed event bus parameterised by a topic → payload-tuple map,
//! 2. a queue worker generic over a schema catalog (payload types derived via
//!    mapped/conditional types),
//! 3. a channel-handle factory whose payload type is a declaration-site type
//!    argument.
//!
//! In all three, the payload type is never a bare named symbol at any
//! publish/subscribe site, so the `primary_type_symbol` anchor channel cannot
//! capture it (the schema correctly instructs null for inline payloads). The
//! cassettes instead carry the `payload_expression_text`/`payload_expression_line`
//! LOCATOR fields, and the scanner routes them through the sidecar's
//! location-based inference (`expression` for publishers, `function_param`
//! for subscribers) — the same LLM-locator + deterministic-tsc division the
//! HTTP family uses.
//!
//! Pre-fix baseline: every one of the six ops (3 shapes × 2 roles) reported
//! `type_state: "Unknown"` with no resolved definition. This test asserts the
//! post-fix state (`Implicit` + a structurally-correct expanded definition),
//! so it FAILS on the pre-fix scanner by construction.

use std::path::{Path, PathBuf};
use std::process::Command;

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn fixture_dir() -> PathBuf {
    repo_root().join("tests/fixtures/pubsub-wrapper-monorepo")
}

/// Same ambient-CI stripping as `xrepo_harness_test.rs`: keeps repo identity
/// tied to the scanned fixture dir and `should_upload_data()` deterministic.
fn strip_ci_env(cmd: &mut Command) -> &mut Command {
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

fn carrick_bin() -> PathBuf {
    let bin = PathBuf::from(env!("CARGO_BIN_EXE_carrick"));
    assert!(bin.exists(), "carrick binary not built: {}", bin.display());
    bin
}

/// One isolated offline scan of the fixture; returns the parsed projection.
fn scan_fixture() -> serde_json::Value {
    let cache = tempfile::tempdir().expect("temp cache dir");
    let cassettes = fixture_dir().join("__llm__");
    assert!(cassettes.exists(), "fixture cassette dir missing");

    let mut cmd = Command::new(carrick_bin());
    cmd.arg(fixture_dir())
        .env("CARRICK_LOCAL_STORAGE_DIR", cache.path())
        .env("CARRICK_LOCAL_STORAGE_ISOLATE", "1")
        .env("CARRICK_MOCK_ALL", "1")
        .env(
            "CARRICK_MOCK_FIXTURE_DIR",
            format!("{}/", cassettes.display()),
        )
        .env("CARRICK_OUTPUT_JSON", "1");
    strip_ci_env(&mut cmd);
    let output = cmd.output().expect("failed to spawn carrick");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "fixture scan exited non-zero:\n{stderr}"
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    let json_start = stdout
        .find('{')
        .unwrap_or_else(|| panic!("no JSON in scanner stdout:\n{stdout}"));
    serde_json::from_str(&stdout[json_start..])
        .unwrap_or_else(|e| panic!("projection parse failed: {e}\n{stdout}"))
}

/// Find the single op for `key` whose `file` ends with `file_suffix`, in the
/// given projection array (`endpoints` for subscribers, `calls` for
/// publishers).
fn find_op<'a>(
    projection: &'a serde_json::Value,
    side: &str,
    key: &str,
    file_suffix: &str,
) -> &'a serde_json::Value {
    let ops = projection[side]
        .as_array()
        .unwrap_or_else(|| panic!("projection has no `{side}` array"));
    let matched: Vec<&serde_json::Value> = ops
        .iter()
        .filter(|op| {
            op["key"].as_str() == Some(key)
                && op["file"].as_str().is_some_and(|f| {
                    Path::new(f).ends_with(file_suffix) || f.ends_with(file_suffix)
                })
        })
        .collect();
    assert_eq!(
        matched.len(),
        1,
        "expected exactly one `{side}` op with key `{key}` in `{file_suffix}` \
         (multi-site keys would make the assertions nondeterministic); \
         found {}; keys present: {:?}",
        matched.len(),
        ops.iter().map(|o| &o["key"]).collect::<Vec<_>>()
    );
    matched[0]
}

/// Assert an op's payload type resolved through the locator path: state
/// `Implicit` (location-inferred, not annotated) and an expanded definition
/// carrying the structural members of the payload.
fn assert_resolved(op: &serde_json::Value, label: &str, expect_members: &[&str]) {
    let state = op["type_state"].as_str();
    assert_eq!(
        state,
        Some("Implicit"),
        "{label}: expected type_state Implicit, got {state:?} (op: {op})"
    );
    let expanded = op["expanded_definition"]
        .as_str()
        .or_else(|| op["resolved_definition"].as_str())
        .unwrap_or_else(|| panic!("{label}: no resolved/expanded definition (op: {op})"));
    for member in expect_members {
        assert!(
            expanded.contains(member),
            "{label}: expanded definition missing `{member}`:\n{expanded}"
        );
    }
}

#[test]
fn pubsub_wrapper_payloads_resolve_on_both_sides() {
    let projection = scan_fixture();

    // Shape 1: typed event bus over a topic map.
    assert_resolved(
        find_op(&projection, "calls", "pubsub|itemArchived", "dispatch.ts"),
        "bus.emit publisher",
        &["time", "item", "status", "error"],
    );
    assert_resolved(
        find_op(&projection, "endpoints", "pubsub|itemArchived", "relay.ts"),
        "bus.on subscriber",
        &["time", "item", "status", "error"],
    );

    // Shape 2: schema-catalog queue worker (payload via mapped/conditional types).
    assert_resolved(
        find_op(
            &projection,
            "calls",
            "pubsub|records.reindex",
            "dispatch.ts",
        ),
        "worker.enqueue publisher",
        &["resourceId", "mode"],
    );
    assert_resolved(
        find_op(
            &projection,
            "endpoints",
            "pubsub|records.reindex",
            "relay.ts",
        ),
        "jobs-map subscriber (envelope binding element)",
        &["resourceId", "mode"],
    );

    // Shape 3: generic channel handle (declaration-site type argument).
    assert_resolved(
        find_op(&projection, "calls", "pubsub|approval", "dispatch.ts"),
        "channel send publisher",
        &["approved", "reviewer"],
    );
    assert_resolved(
        find_op(&projection, "endpoints", "pubsub|approval", "relay.ts"),
        "channel on subscriber",
        &["approved", "reviewer"],
    );
}
