//! carrick#2383: a route registration that takes a path, an options bag and
//! a handler is not a request.
//!
//! `src/setup.ts` registers two routes through `define(path, { method, body },
//! handler)`, a call the request summaries read as the request it is shaped
//! like. The source states an endpoint at each call (the model's cassette
//! does), and a call the file states as a route is not a request, so no row
//! reaches the call of `setup()` in `src/app.ts`. `src/check.ts` takes the
//! same three arguments and states no route: it keeps its row.
//!
//! The binary runs as the Action runs it: the scan path is the relative `.`
//! from inside the repository, the service at the repository root.
//!
//! See `tests/fixtures/registration-veto/README.md` for the answer key.

use std::process::Command;
use std::sync::OnceLock;

struct Scan {
    calls: Vec<serde_json::Value>,
    endpoints: Vec<serde_json::Value>,
}

fn scan() -> &'static Scan {
    static SCAN: OnceLock<Scan> = OnceLock::new();
    SCAN.get_or_init(|| {
        let repo = env!("CARGO_MANIFEST_DIR");
        let fixture = format!("{repo}/tests/fixtures/registration-veto");
        let mock_dir = format!("{fixture}/__llm__/");

        let output = Command::new(env!("CARGO_BIN_EXE_carrick"))
            .arg(".")
            .current_dir(&fixture)
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
        let projection: serde_json::Value =
            serde_json::from_slice(&output.stdout).expect("scanner output was not valid JSON");
        let rows = |key: &str| {
            projection[key]
                .as_array()
                .unwrap_or_else(|| panic!("projection carries a {key} array"))
                .clone()
        };
        Scan {
            calls: rows("calls"),
            endpoints: rows("endpoints"),
        }
    })
}

/// `(method, target)` of every request row at `file:line`, sorted.
fn calls_at(file: &str, line: i64) -> Vec<(String, String)> {
    let mut rows: Vec<(String, String)> = scan()
        .calls
        .iter()
        .filter(|call| call["file"].as_str() == Some(file) && call["line"].as_i64() == Some(line))
        .map(|call| {
            let text = |key: &str| call[key].as_str().unwrap_or_default().to_string();
            (text("method"), text("target_url"))
        })
        .collect();
    rows.sort();
    rows
}

#[test]
fn the_registrations_are_the_endpoints_the_source_states() {
    let mut endpoints: Vec<(String, String)> = scan()
        .endpoints
        .iter()
        .filter(|endpoint| endpoint["file"].as_str() == Some("src/setup.ts"))
        .map(|endpoint| {
            let text = |key: &str| endpoint[key].as_str().unwrap_or_default().to_string();
            (text("method"), text("path"))
        })
        .collect();
    endpoints.sort();
    assert_eq!(
        endpoints,
        vec![
            ("DELETE".to_string(), "/purge".to_string()),
            ("POST".to_string(), "/reset".to_string()),
        ]
    );
}

#[test]
fn a_registration_is_no_request_where_it_is_called() {
    // `setup()` is called at line 5 of `src/app.ts`; its own calls are at
    // lines 6 and 7 of `src/setup.ts`.
    assert_eq!(calls_at("src/app.ts", 5), Vec::new());
    assert_eq!(calls_at("src/setup.ts", 6), Vec::new());
    assert_eq!(calls_at("src/setup.ts", 7), Vec::new());
}

#[test]
fn a_request_that_takes_the_same_three_arguments_keeps_its_row() {
    assert_eq!(
        calls_at("src/app.ts", 6),
        vec![("GET".to_string(), "/status".to_string())]
    );
}
