//! carrick#2452: a class client whose base is a constructor parameter is read
//! with the value its one construction passes.
//!
//! Run as the Action does: `carrick .` from the repo root, one root service.
//! Deterministic end to end: the LLM is replayed from `__llm__/`, and each
//! cassette holds no rows, so every row here is stated by the source.
//!
//! See `tests/fixtures/class-client-construction/README.md` for the shape.

use std::process::Command;

fn calls() -> Vec<serde_json::Value> {
    let repo = env!("CARGO_MANIFEST_DIR");
    let fixture = format!("{repo}/tests/fixtures/class-client-construction");
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
    let stdout = String::from_utf8(output.stdout).expect("scanner stdout was not UTF-8");
    let projection: serde_json::Value =
        serde_json::from_str(&stdout).expect("scanner output was not valid JSON");
    projection["calls"]
        .as_array()
        .expect("projection carries a calls array")
        .clone()
}

fn row_at(calls: &[serde_json::Value], file: &str, line: i64) -> serde_json::Value {
    calls
        .iter()
        .find(|call| {
            call["line"].as_i64() == Some(line)
                && call["file"]
                    .as_str()
                    .is_some_and(|f| f.ends_with(&format!("src/{file}")))
        })
        .unwrap_or_else(|| panic!("no row at {file}:{line}: {calls:#?}"))
        .clone()
}

/// A parameter property, one construction: the route is the construction's
/// path joined to the request's.
#[test]
fn a_parameter_property_reads_as_the_construction_passes_it() {
    let calls = calls();
    let row = row_at(&calls, "use.ts", 4);
    assert_eq!(row["method"], "GET", "{row:#}");
    assert_eq!(row["path"], "/api/v1/users", "{row:#}");
}

/// An assigned field built from an environment read: the base is that
/// variable, undeclared, which is an expected find.
#[test]
fn an_assigned_field_reads_the_environment_the_construction_passes() {
    let calls = calls();
    let row = row_at(&calls, "use.ts", 5);
    assert_eq!(row["method"], "GET", "{row:#}");
    assert_eq!(row["base"]["env_var"], "ITEMS_URL", "{row:#}");
    assert_eq!(row["path"], "${process.env.ITEMS_URL}/items", "{row:#}");
}

/// Two constructions with different literals: the base is no value the
/// source states, so the row keeps the field's own name.
#[test]
fn constructions_that_disagree_keep_the_field_opaque() {
    let calls = calls();
    let row = row_at(&calls, "use.ts", 6);
    assert_ne!(row["path"], "/east/ping", "{row:#}");
    assert_ne!(row["path"], "/west/ping", "{row:#}");
}
