//! carrick#830: the function members of a module's exported object literal are
//! indexed, instead of the file reading as one with no functions in it.
//!
//! Deterministic and AST-only. The LLM is replayed from `__llm__/` and states
//! nothing, so every row asserted here is derived from the source.
//!
//! The scan is read from the stored blob rather than the projection: function
//! definitions are what this is about, and the projection carries operations.
//!
//! See `tests/fixtures/object-export-surface/README.md` for the shape, and
//! `tests/fixtures/default-export-members/README.md` for the carrick#2396
//! forms: the same surface written through a type check or a named `const`,
//! and the member call that reaches it through the import.

use std::path::{Path, PathBuf};
use std::process::Command;

fn fixture_dir(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name)
}

/// The function definitions of one isolated offline scan of this file's main
/// fixture, keyed as the scan keys them.
fn function_definitions() -> serde_json::Map<String, serde_json::Value> {
    function_definitions_of("object-export-surface")
}

/// The function definitions of one isolated offline scan of `fixture`.
fn function_definitions_of(fixture: &str) -> serde_json::Map<String, serde_json::Value> {
    let cache = tempfile::tempdir().expect("temp cache dir");
    let cassettes = fixture_dir(fixture).join("__llm__");

    let output = Command::new(env!("CARGO_BIN_EXE_carrick"))
        .arg(fixture_dir(fixture))
        .env("CARRICK_LOCAL_STORAGE_DIR", cache.path())
        .env("CARRICK_LOCAL_STORAGE_ISOLATE", "1")
        .env("CARRICK_MOCK_ALL", "1")
        .env(
            "CARRICK_MOCK_FIXTURE_DIR",
            format!("{}/", cassettes.display()),
        )
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

    let blob = find_blob(cache.path()).expect("the scan stored a blob");
    let text = std::fs::read_to_string(&blob).expect("read the stored blob");
    let stored: serde_json::Value = serde_json::from_str(&text).expect("the blob is JSON");
    stored["function_definitions"]
        .as_object()
        .expect("the blob carries function definitions")
        .clone()
}

/// The stored blob, wherever the isolated cache put it.
fn find_blob(dir: &Path) -> Option<PathBuf> {
    for entry in std::fs::read_dir(dir).ok()? {
        let path = entry.ok()?.path();
        if path.is_dir() {
            if let Some(found) = find_blob(&path) {
                return Some(found);
            }
        } else if path.extension().is_some_and(|ext| ext == "json") {
            let text = std::fs::read_to_string(&path).ok()?;
            if text.contains("\"function_definitions\"") {
                return Some(path);
            }
        }
    }
    None
}

/// The keys, in a form a panic message can print.
fn names(definitions: &serde_json::Map<String, serde_json::Value>) -> Vec<&String> {
    definitions.keys().collect()
}

fn definition<'a>(
    definitions: &'a serde_json::Map<String, serde_json::Value>,
    name: &str,
) -> &'a serde_json::Value {
    definitions
        .get(name)
        .unwrap_or_else(|| panic!("no definition named {name}: {:?}", names(definitions)))
}

#[test]
fn a_module_that_exports_one_object_states_its_functions() {
    let definitions = function_definitions();

    let collect = definition(&definitions, "collect");
    assert_eq!(
        collect["signature"],
        "(ordinal: (number) | undefined, label: string, note?: string, ...values: number[]) => void"
    );
    assert_eq!(collect["arguments"][0]["has_default"], true);
    assert_eq!(collect["arguments"][0]["default_value"], "0");
    assert!(collect["arguments"][0].get("is_optional").is_none());
    assert!(collect["arguments"][1].get("has_default").is_none());
    assert_eq!(collect["arguments"][2]["is_optional"], true);
    assert_eq!(collect["arguments"][3]["is_rest"], true);

    // The ticket's shape: the whole module is `export default { async fetch }`.
    let fetch = definition(&definitions, "default.fetch");
    assert_eq!(fetch["file_path"], "src/edge.js");
    assert_eq!(fetch["line_number"], 8, "at the method, not the object");
    assert_eq!(
        fetch["is_exported"], true,
        "a member of the default export is exported: {fetch:#?}"
    );
    assert_eq!(fetch["signature"], "(request) => unknown");

    // The named form, in each of its three shapes.
    let list = definition(&definitions, "handlers.list");
    assert_eq!(list["file_path"], "src/handlers.ts");
    assert_eq!(list["signature"], "() => Promise<string[]>");
    assert_eq!(list["is_exported"], true);

    let create = definition(&definitions, "handlers.create");
    assert_eq!(create["signature"], "(name: string) => Promise<string>");

    let restore = definition(&definitions, "handlers.archive.restore");
    assert_eq!(restore["signature"], "(id: string) => string");

    // The control: an ordinary exported function reads exactly as it did.
    let normalize = definition(&definitions, "normalize");
    assert_eq!(normalize["signature"], "(input: string) => string");

    // The bound: a local object is a value the module uses, not a surface it
    // offers, and collecting every one of them would flood the index.
    assert!(
        !definitions.contains_key("internals.tidy"),
        "a module-local object is not an export surface: {:?}",
        names(&definitions)
    );
}

/// carrick#863: the same surface, stated the CommonJS way. Every `.js` lambda
/// writes its entry point as an assignment, and an assignment is none of the
/// shapes the extractor knew.
#[test]
fn a_module_that_exports_by_assignment_states_its_functions() {
    let definitions = function_definitions();

    // `exports.handler = async (event) => { … }`: the lambda entry point, and
    // the whole surface of most files written this way.
    let handler = definition(&definitions, "handler");
    assert_eq!(handler["file_path"], "src/lambda.js");
    assert_eq!(handler["line_number"], 6, "at the arrow, not the statement");
    assert_eq!(handler["is_exported"], true);
    assert_eq!(handler["signature"], "(event) => unknown");

    // `module.exports.health = function health(deep) { … }`.
    let health = definition(&definitions, "health");
    assert_eq!(health["file_path"], "src/lambda.js");
    assert_eq!(health["line_number"], 13);
    assert_eq!(health["is_exported"], true);
    assert_eq!(health["signature"], "(deep) => unknown");

    // An object assigned to a named export is a member bag, read exactly as
    // the ESM one is.
    let drop = definition(&definitions, "table.drop");
    assert_eq!(drop["signature"], "(id) => unknown");
    assert_eq!(drop["is_exported"], true);

    // `module.exports = { … }` is the default export, so its members key the
    // same way `export default { … }` does.
    let drain = definition(&definitions, "default.drain");
    assert_eq!(drain["file_path"], "src/queue.js");
    assert_eq!(drain["is_exported"], true);
    assert_eq!(drain["signature"], "(queue) => unknown");

    // A function offered by name has ONE definition, at its own key, and the
    // assignment says the module offers it. A second row for the same body
    // would double the count and re-bill the intent.
    for (offered, shorthand) in [("reset", false), ("consume", true)] {
        let def = definition(&definitions, offered);
        assert_eq!(
            def["is_exported"], true,
            "{offered} is offered by an export assignment: {def:#?}"
        );
        let alias = if shorthand {
            "default.consume"
        } else {
            "table.reset"
        };
        assert!(
            !definitions.contains_key(alias),
            "{offered} already has a definition; {alias} would be a second row for one body: {:?}",
            names(&definitions)
        );
    }

    // The bound: a module-local function no assignment reaches stays local.
    let sweep = definition(&definitions, "sweep");
    assert_eq!(
        sweep["is_exported"], false,
        "nothing exports sweep: {sweep:#?}"
    );
}

/// The carrick#2396 fixture's definitions, unkeyed. Two of its files each
/// define `default.fetch`, and the merge re-keys the second (carrick#582), so
/// these tests match a definition by `(file_path, name)`, never by key.
fn default_export_members() -> Vec<serde_json::Value> {
    function_definitions_of("default-export-members")
        .into_iter()
        .map(|(_, definition)| definition)
        .collect()
}

/// Every definition as `(file_path, name)`, for a panic message.
fn located(definitions: &[serde_json::Value]) -> Vec<(String, String)> {
    let mut rows: Vec<(String, String)> = definitions
        .iter()
        .map(|d| {
            (
                d["file_path"].as_str().unwrap_or_default().to_string(),
                d["name"].as_str().unwrap_or_default().to_string(),
            )
        })
        .collect();
    rows.sort();
    rows
}

fn find_in<'a>(
    definitions: &'a [serde_json::Value],
    file: &str,
    name: &str,
) -> Option<&'a serde_json::Value> {
    definitions
        .iter()
        .find(|d| d["file_path"] == file && d["name"] == name)
}

fn definition_in<'a>(
    definitions: &'a [serde_json::Value],
    file: &str,
    name: &str,
) -> &'a serde_json::Value {
    find_in(definitions, file, name)
        .unwrap_or_else(|| panic!("no definition {name} in {file}: {:?}", located(definitions)))
}

/// carrick#2396: each exported object, in each form, states its member as an
/// exported function, keyed by the name the module declares the object under.
#[test]
fn every_exported_object_form_states_its_members() {
    let definitions = default_export_members();

    for (file, name, line) in [
        // `export default { … }`
        ("src/anonymous.ts", "default.fetch", 3),
        // `export default { … } satisfies Handler`
        ("src/satisfied.ts", "default.fetch", 5),
        // `const worker: Handler = { … }; export default worker`
        ("src/bound.ts", "worker.fetch", 5),
        // `export { jobs }` written before `const jobs = ({ … }) as Jobs`
        ("src/listed.ts", "jobs.run", 8),
        // `export { queue as tasks }`: keyed by the declared name
        ("src/renamed.ts", "queue.drain", 4),
        // `export const scheduled = { … } satisfies Jobs`
        ("src/declared.ts", "scheduled.run", 5),
        // `const store = { … }; module.exports = store`
        ("src/assigned.js", "store.flush", 4),
    ] {
        let def = definition_in(&definitions, file, name);
        assert_eq!(def["line_number"], line, "{file} {name}: {def:#?}");
        assert_eq!(def["is_exported"], true, "{file} {name}: {def:#?}");
    }

    // The bounds: a module-local object, a `let` that is exported but can be
    // replaced, and a function-local `const` that shares an exported name.
    for (file, name) in [
        ("src/bounds.ts", "internal.tidy"),
        ("src/bounds.ts", "state.reset"),
        ("src/bounds.ts", "state.hidden"),
    ] {
        assert!(
            find_in(&definitions, file, name).is_none(),
            "{name} is not on the module's export surface: {:?}",
            located(&definitions)
        );
    }
}

/// carrick#2396: a member call through the import the module gives its object
/// reaches that member's definition: the default import of an anonymous
/// object, of one checked with `satisfies`, of a named const, and a named
/// import of a const exported by name or under another name.
#[test]
fn a_member_call_through_the_import_reaches_the_definition() {
    let definitions = default_export_members();
    let dispatch = definition_in(&definitions, "src/caller.ts", "dispatch");

    let mut calls: Vec<(String, String, u64)> = dispatch["calls"]
        .as_array()
        .expect("calls")
        .iter()
        .map(|c| {
            (
                c["file_path"].as_str().unwrap_or_default().to_string(),
                c["name"].as_str().unwrap_or_default().to_string(),
                c["call_site_line"].as_u64().unwrap_or_default(),
            )
        })
        .collect();
    calls.sort();

    let expected: Vec<(String, String, u64)> = [
        ("src/anonymous.ts", "default.fetch", 11),
        ("src/bound.ts", "worker.fetch", 13),
        ("src/declared.ts", "scheduled.run", 15),
        ("src/listed.ts", "jobs.run", 16),
        ("src/renamed.ts", "queue.drain", 14),
        ("src/satisfied.ts", "default.fetch", 12),
    ]
    .into_iter()
    .map(|(f, n, l)| (f.to_string(), n.to_string(), l))
    .collect();
    assert_eq!(calls, expected);
}
