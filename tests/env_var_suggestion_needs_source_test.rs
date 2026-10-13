//! carrick#2318: a configuration suggestion never asks the user to declare an
//! environment variable that the file neither reads nor binds.
//!
//! The LLM is replayed from `__llm__/`. The cassette states a call through a
//! client with a base (`MADE_UP`) that appears nowhere in the file, beside a
//! call whose base the file really reads (`REAL_URL`). What is under test is
//! which of the two the scan summary asks the user to classify.
//!
//! See `tests/fixtures/env-var-suggestion-needs-source/` for the shape.

use std::process::Command;

fn scan_summary() -> String {
    let repo = env!("CARGO_MANIFEST_DIR");
    let fixture = format!("{repo}/tests/fixtures/env-var-suggestion-needs-source");
    let mock_dir = format!("{fixture}/__llm__/");

    let output = Command::new(env!("CARGO_BIN_EXE_carrick"))
        .arg(&fixture)
        .env("CARRICK_MOCK_ALL", "1")
        .env("CARRICK_MOCK_FIXTURE_DIR", &mock_dir)
        .env("CARRICK_SKIP_INTENTS", "1")
        .env_remove("CARRICK_OUTPUT_JSON")
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
    String::from_utf8(output.stdout).expect("scanner stdout was not UTF-8")
}

#[test]
fn a_base_the_file_never_names_is_not_suggested_and_one_it_reads_is() {
    let summary = scan_summary();

    assert!(
        summary.contains("using **[REAL_URL]**"),
        "an undeclared base the file reads is still an expected find:\n{summary}"
    );
    assert!(
        !summary.contains("MADE_UP"),
        "a base the file never names must not be offered for declaration:\n{summary}"
    );
    assert!(
        summary.contains("Configuration suggestions (1)"),
        "one suggestion remains:\n{summary}"
    );
}
