//! carrick#2411: a body that spreads the function's own parameter is read,
//! at a call, with the caller's argument in place of the spread.
//!
//! Deterministic end to end. The LLM is replayed from `__llm__/`, and the
//! cassette is wrong the way production's model is: every caller's row
//! carries an `op` invented from the member name, so a correct value can only
//! come from the source.
//!
//! See `tests/fixtures/spread-parameter-body/README.md` for the shape and
//! the answer key.

use std::process::Command;

fn calls() -> Vec<serde_json::Value> {
    let repo = env!("CARGO_MANIFEST_DIR");
    let fixture = format!("{repo}/tests/fixtures/spread-parameter-body");
    let mock_dir = format!("{fixture}/__llm__/");

    let output = Command::new(env!("CARGO_BIN_EXE_carrick"))
        .arg(&fixture)
        .env("CARRICK_MOCK_ALL", "1")
        .env("CARRICK_MOCK_FIXTURE_DIR", &mock_dir)
        .env("CARRICK_OUTPUT_JSON", "1")
        .env("CARRICK_SKIP_INTENTS", "1")
        .env_remove("GITHUB_REPOSITORY")
        .env_remove("GITHUB_ACTIONS")
        .env_remove("CI")
        .output()
        .expect("failed to spawn carrick binary");

    assert!(
        output.status.success(),
        "scanner exited non-zero:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).expect("scanner stdout was not UTF-8");
    let projection: serde_json::Value =
        serde_json::from_str(&stdout).expect("scanner output was not valid JSON");
    projection["calls"]
        .as_array()
        .expect("projection carries a calls array")
        .clone()
}

fn rows_at<'a>(
    calls: &'a [serde_json::Value],
    file: &str,
    line: i64,
) -> Vec<&'a serde_json::Value> {
    calls
        .iter()
        .filter(|call| {
            call["line"].as_i64() == Some(line)
                && call["file"]
                    .as_str()
                    .is_some_and(|f| f.ends_with(&format!("src/{file}")))
        })
        .collect()
}

/// The `op` values the rows at `file:line` send, sorted, each row stated by
/// the source as a `POST` to `path`. `None` for a row that states no value.
fn ops_at(calls: &[serde_json::Value], file: &str, line: i64, path: &str) -> Vec<Option<String>> {
    let rows = rows_at(calls, file, line);
    assert!(!rows.is_empty(), "no row at {file}:{line}");
    let mut ops: Vec<Option<String>> = rows
        .iter()
        .map(|row| {
            assert_eq!(
                row["resolution_source"].as_str(),
                Some("request_summary"),
                "{file}:{line} is stated by the source, not the model: {row:#}"
            );
            assert_eq!(
                row["method"].as_str(),
                Some("POST"),
                "{file}:{line}: {row:#}"
            );
            assert_eq!(row["path"].as_str(), Some(path), "{file}:{line}: {row:#}");
            let value = row["dispatch"]["value"].as_str().map(str::to_string);
            if value.is_some() {
                assert_eq!(row["dispatch"]["field"].as_str(), Some("op"), "{row:#}");
                assert_eq!(
                    row["dispatch"]["location"].as_str(),
                    Some("body"),
                    "{row:#}"
                );
            }
            value
        })
        .collect();
    ops.sort();
    ops
}

const OPS: &str = "${process.env.OPS_API_URL}/v1/ops";
const GATEWAY: &str = "${process.env.GATEWAY_API_URL}/v1/gateway";
const JOBS: &str = "${process.env.JOBS_API_URL}/v1/jobs";

fn one(op: &str) -> Vec<Option<String>> {
    vec![Some(op.to_string())]
}

#[test]
fn a_literal_in_place_of_the_spread_keeps_the_key_written_before_it() {
    let calls = calls();
    // `sendOp("purge", {})` and `sendOp("purge", { id })`.
    assert_eq!(ops_at(&calls, "purge.ts", 4, OPS), one("purge"));
    assert_eq!(ops_at(&calls, "purge.ts", 5, OPS), one("purge"));
    // `flag ? { id: "1" } : {}`: neither branch sets `op`.
    assert_eq!(ops_at(&calls, "purge.ts", 7, OPS), one("purge"));
}

#[test]
fn a_key_the_literal_sets_overwrites_the_one_before_the_spread() {
    // `sendOp("purge", { op: "evict" })` sends `evict`, as the runtime does.
    assert_eq!(ops_at(&calls(), "purge.ts", 6, OPS), one("evict"));
}

#[test]
fn each_caller_states_its_own_value_and_the_request_line_none() {
    let calls = calls();
    assert_eq!(ops_at(&calls, "alpha.ts", 4, OPS), one("alpha"));
    assert_eq!(ops_at(&calls, "beta.ts", 4, OPS), one("beta"));
    assert_eq!(ops_at(&calls, "send-op.ts", 6, OPS), vec![None]);
}

#[test]
fn an_argument_whose_keys_the_call_does_not_state_holds_no_value() {
    let calls = calls();
    // A local, a call's result, a spread argument, and no argument.
    for line in [9, 10, 11, 12] {
        assert_eq!(
            ops_at(&calls, "unread.ts", line, OPS),
            vec![None],
            "unread.ts:{line}"
        );
    }
}

#[test]
fn a_spread_passed_up_through_the_callers_parameter_is_filled_where_the_object_is_written() {
    let calls = calls();
    // `purgeAll(scope)` hands `scope` to `run`'s spread.
    assert_eq!(ops_at(&calls, "ops.ts", 4, GATEWAY), one("purge-all"));
    // `passOn(given)`: nothing fills `given`.
    assert_eq!(ops_at(&calls, "ops.ts", 12, GATEWAY), vec![None]);
}

/// Two calls of `run` with two literals are two requests: the site calling
/// the member that makes them states one row for each.
#[test]
fn a_member_calling_the_wrapper_twice_states_a_row_per_value() {
    let calls = calls();
    assert_eq!(
        ops_at(&calls, "ops.ts", 5, GATEWAY),
        vec![Some("refresh-a".to_string()), Some("refresh-b".to_string())]
    );
}

#[test]
fn a_helper_that_sends_its_parameter_keeps_the_spread_of_the_object_handed_to_it() {
    // `search(query)` calls `this.rpc({ op: "search", ...query })`.
    assert_eq!(ops_at(&calls(), "ops.ts", 6, GATEWAY), one("search"));
}

#[test]
fn a_key_written_after_the_spread_is_the_one_sent() {
    // `{ ...opts, op: "lock" }`, called with `{ op: "unlock" }`.
    assert_eq!(ops_at(&calls(), "ops.ts", 7, GATEWAY), one("lock"));
}

#[test]
fn a_spread_the_reading_cannot_prove_holds_no_value() {
    let calls = calls();
    // Two parameter spreads, a spread of a parameter's key, a parameter
    // assigned again, and one written through.
    for line in [4, 5, 6, 7] {
        assert_eq!(
            ops_at(&calls, "jobs.ts", line, JOBS),
            vec![None],
            "jobs.ts:{line}"
        );
    }
}
