//! Layer-1 cassette hard gate.
//!
//! Replays *frozen* LLM responses (the `__llm__/` cassettes) through the full
//! scanner binary and asserts the machine-readable projection is an exact match
//! against a committed golden. Because the LLM output is frozen, any change in
//! the final output can only come from a change in scanner code — so a failure
//! here is, by construction, a scanner regression. This is the one layer that is
//! safe to *hard-gate* a merge on (the scored eval, `eval_tier_a`, is stochastic
//! and report-only).
//!
//! It runs under plain `cargo test`: fully mocked, no network, no OIDC. No
//! cassette is ever re-recorded when a prompt or model changes — the cassette is
//! a pure scanner-machinery regression net; prompt/model effects are measured
//! live in the scored eval, never here.
//!
//! If this fails after an *intentional* output change, re-record the golden:
//! ```text
//! CARRICK_MOCK_ALL=1 CARRICK_OUTPUT_JSON=1 \
//!   CARRICK_MOCK_FIXTURE_DIR=tests/fixtures/llm-mocked-api/__llm__/ \
//!   cargo run -- tests/fixtures/llm-mocked-api \
//!   | sed "s#$(pwd)/##" \
//!   | sed -E 's/_Call[0-9a-f]{16}/_Call0000000000000000/g' \
//!   > tests/fixtures/llm-mocked-api/__golden__.json
//! ```
//!
//! The second sed masks consumer call-site ids: `build_call_site_id` hashes the
//! ABSOLUTE call-site file path (its uniqueness contract is per-run, so that is
//! fine at runtime), which makes any `Endpoint_<hash>_<Kind>_Call<id>` alias
//! machine-specific. The test applies the same mask to both sides, so the
//! comparison is invariant to where the golden was recorded while still gating
//! on everything semantic (keys, paths, resolved definitions, type states).

use std::process::Command;

/// Mask the 16-hex call-site id in `_Call<id>` alias suffixes (see module doc).
fn mask_call_site_ids(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    const TAG: &str = "_Call";
    while let Some(idx) = rest.find(TAG) {
        let after = &rest[idx + TAG.len()..];
        let is_id = after.len() >= 16 && after.as_bytes()[..16].iter().all(u8::is_ascii_hexdigit);
        if is_id {
            out.push_str(&rest[..idx + TAG.len()]);
            out.push_str("0000000000000000");
            rest = &after[16..];
        } else {
            out.push_str(&rest[..idx + TAG.len()]);
            rest = after;
        }
    }
    out.push_str(rest);
    out
}

#[test]
fn cassette_hard_gate_llm_mocked_api() {
    let repo = env!("CARGO_MANIFEST_DIR");
    let fixture = format!("{repo}/tests/fixtures/llm-mocked-api");
    let mock_dir = format!("{fixture}/__llm__/");

    let output = Command::new(env!("CARGO_BIN_EXE_carrick"))
        .arg(&fixture)
        .env("CARRICK_MOCK_ALL", "1")
        .env("CARRICK_MOCK_FIXTURE_DIR", &mock_dir)
        .env("CARRICK_OUTPUT_JSON", "1")
        .output()
        .expect("failed to spawn carrick binary");

    assert!(
        output.status.success(),
        "scanner exited non-zero:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8(output.stdout).expect("scanner stdout was not UTF-8");
    // Relativise the absolute repo root so the golden is portable across
    // machines / CI runners. The scanner is invoked with `{repo}/...`, so file
    // paths in the projection are anchored at the same prefix.
    // Mask machine-specific call-site ids on BOTH sides (see module doc): the
    // id hashes the absolute file path, so it can never byte-match across
    // machines/CI runners no matter where the golden was recorded.
    let normalized = mask_call_site_ids(&stdout.replace(&format!("{repo}/"), ""));

    let actual: serde_json::Value =
        serde_json::from_str(&normalized).expect("scanner output was not valid JSON");
    let golden: serde_json::Value = serde_json::from_str(&mask_call_site_ids(include_str!(
        "fixtures/llm-mocked-api/__golden__.json"
    )))
    .expect("golden fixture was not valid JSON");

    assert_eq!(
        actual, golden,
        "Full-pipeline output drifted from the golden while the LLM input is \
         frozen — this is a scanner-code regression. If the change is \
         intentional, re-record the golden (see the module doc-comment)."
    );
}
