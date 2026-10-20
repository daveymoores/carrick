//! `carrick derive --json --candidates` states, per service, the files its
//! declared entry points and its narrowed program reach, and lists the rest as
//! units, each with the `exclude` pattern that would leave exactly it out
//! (carrick#2397, the scanner half of carrick-cloud#2348).
//!
//! Nothing here leaves a file out: a derive only states what reach keeps and
//! what it cannot place. Each case runs the binary as the cloud runner does.

use std::collections::BTreeSet;
use std::path::Path;
use std::process::Command;

use serde_json::Value;

fn write(root: &Path, file: &str, body: &str) {
    let target = root.join(file);
    std::fs::create_dir_all(target.parent().unwrap()).unwrap();
    std::fs::write(target, body).unwrap();
}

fn tree(files: &[(&str, &str)]) -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("repo");
    std::fs::create_dir_all(&root).unwrap();
    let git = Command::new("git")
        .args(["init", "-q"])
        .current_dir(&root)
        .status()
        .unwrap();
    assert!(git.success());
    for (file, body) in files {
        write(&root, file, body);
    }
    dir
}

fn derive_bytes(dir: &tempfile::TempDir) -> Vec<u8> {
    let output = Command::new(env!("CARGO_BIN_EXE_carrick"))
        .args(["derive", "--workspace"])
        .arg(dir.path().join("repo"))
        .args(["--json", "--candidates"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    output.stdout
}

/// Every service's `candidates`, keyed by its directory (`.` for the root).
fn derive(dir: &tempfile::TempDir) -> Vec<(String, Value)> {
    let derived: Value = serde_json::from_slice(&derive_bytes(dir)).unwrap();
    derived["repos"]
        .as_array()
        .unwrap()
        .iter()
        .flat_map(|repo| repo["services"].as_array().unwrap().clone())
        .map(|service| {
            (
                service["directory"].as_str().unwrap_or(".").to_string(),
                service["candidates"].clone(),
            )
        })
        .collect()
}

/// The one service's candidates.
fn only(dir: &tempfile::TempDir) -> Value {
    let mut services = derive(dir);
    assert_eq!(services.len(), 1, "{services:?}");
    services.remove(0).1
}

fn units(candidates: &Value) -> Vec<String> {
    candidates["directories"]
        .as_array()
        .unwrap_or_else(|| panic!("no directories: {candidates}"))
        .iter()
        .map(|unit| unit["unit"].as_str().unwrap().to_string())
        .collect()
}

fn unit<'a>(candidates: &'a Value, name: &str) -> &'a Value {
    candidates["directories"]
        .as_array()
        .unwrap()
        .iter()
        .find(|unit| unit["unit"] == name)
        .unwrap_or_else(|| panic!("no unit {name}: {candidates}"))
}

#[test]
fn a_narrowed_program_reaches_its_closure_and_lists_the_rest() {
    let dir = tree(&[
        ("package.json", r#"{"name":"svc","main":"src/index.ts"}"#),
        ("tsconfig.json", r#"{"include":["src"]}"#),
        (
            "src/index.ts",
            "import { db } from \"./db\";\nexport const x = db;\n",
        ),
        ("src/db.ts", "export const db = 1;\n"),
        (
            "scripts/once.ts",
            "// run once after the import\nimport { db } from \"../src/db\";\nconsole.log(db);\n",
        ),
        ("tool.config.ts", "export default {};\n"),
    ]);
    let found = only(&dir);
    assert_eq!(found["reach"]["program"], "narrows");
    assert_eq!(found["reach"]["reached"], 2);
    assert_eq!(found["reach"]["unclaimed"], 2);
    assert_eq!(units(&found), ["scripts", "tool.config.ts"]);
    let scripts = unit(&found, "scripts");
    assert_eq!(scripts["kind"], "dir");
    assert_eq!(scripts["pattern"], "/scripts/");
    assert_eq!(scripts["files"], 1);
    assert_eq!(scripts["paths"], serde_json::json!(["once.ts"]));
    assert_eq!(scripts["program"], "outside");
    assert_eq!(scripts["imports_from"], serde_json::json!(["src"]));
    assert_eq!(scripts["exporting_files"], 0);
    assert_eq!(
        scripts["samples"][0]["head"],
        serde_json::json!([
            "// run once after the import",
            "import { db } from \"../src/db\";",
            "console.log(db);"
        ])
    );
    let config = unit(&found, "tool.config.ts");
    assert_eq!(config["kind"], "file");
    assert_eq!(config["pattern"], "/tool.config.ts");
    assert_eq!(config["exporting_files"], 1);
}

#[test]
fn a_whole_tree_program_adds_no_roots_and_an_imported_unit_is_not_offered() {
    let dir = tree(&[
        ("tsconfig.json", "{}"),
        (
            "routes/a.ts",
            "import { x } from \"../lib/x\";\nexport const a = x;\n",
        ),
        ("lib/x.ts", "export const x = 1;\n"),
        ("scratch/try.ts", "console.log(1);\n"),
    ]);
    let found = only(&dir);
    assert_eq!(found["reach"]["program"], "whole_tree");
    assert_eq!(found["reach"]["reached"], 0);
    assert_eq!(units(&found), ["routes", "scratch"]);
    assert_eq!(
        unit(&found, "routes")["imports_from"],
        serde_json::json!(["lib"])
    );
}

#[test]
fn manifest_entries_and_scripts_are_roots() {
    let dir = tree(&[
        (
            "package.json",
            r#"{"name":"svc","bin":{"t":"bin/t.js"},"scripts":{"worker":"node workers/q.js && echo ok"},"exports":{".":{"import":"./src/index.ts"}}}"#,
        ),
        ("tsconfig.json", r#"{"include":["src"]}"#),
        ("src/index.ts", "export const i = 1;\n"),
        ("bin/t.js", "require(\"./helper\");\n"),
        ("bin/helper.js", "module.exports = 1;\n"),
        ("workers/q.js", "import \"../jobs/run.js\";\n"),
        ("jobs/run.js", "subscribe(\"topic\");\n"),
        ("scratch/x.js", "console.log(1);\n"),
    ]);
    let found = only(&dir);
    assert_eq!(found["reach"]["manifest_roots"], 2);
    assert_eq!(found["reach"]["script_roots"], 1);
    assert_eq!(found["reach"]["reached"], 5);
    assert_eq!(units(&found), ["scratch"]);
}

#[test]
fn a_solution_config_follows_references_and_extends() {
    let dir = tree(&[
        (
            "tsconfig.json",
            r#"{"files":[],"references":[{"path":"./tsconfig.app.json"}]}"#,
        ),
        ("tsconfig.app.json", r#"{"extends":"./tsconfig.base.json"}"#),
        ("tsconfig.base.json", r#"{"include":["src"]}"#),
        ("src/main.ts", "export const m = 1;\n"),
        (
            "edge/fn/index.ts",
            "serve(() => new Response(\"hello\"));\n",
        ),
    ]);
    let found = only(&dir);
    assert_eq!(found["reach"]["program"], "narrows");
    assert_eq!(found["reach"]["reached"], 1);
    assert_eq!(units(&found), ["edge"]);
    let edge = unit(&found, "edge");
    assert_eq!(edge["program"], "outside");
    assert_eq!(edge["exporting_files"], 0);
}

#[test]
fn a_nested_example_package_is_a_unit_with_its_manifest() {
    let dir = tree(&[
        (
            "examples/basic/package.json",
            r#"{"name":"basic-example","description":"An example server with two routes","main":"index.ts"}"#,
        ),
        ("examples/basic/tsconfig.json", r#"{"include":["*.ts"]}"#),
        (
            "examples/basic/index.ts",
            "const app = {};\nexport default app;\n",
        ),
    ]);
    let found = only(&dir);
    assert_eq!(found["reach"]["reached"], 0);
    assert_eq!(units(&found), ["examples"]);
    let examples = unit(&found, "examples");
    assert_eq!(examples["program"], "own");
    let manifest = &examples["manifests"][0];
    assert_eq!(manifest["path"], "basic/package.json");
    assert_eq!(manifest["name"], "basic-example");
    assert_eq!(manifest["description"], "An example server with two routes");
    assert_eq!(manifest["private"], false);
    assert_eq!(manifest["has_entries"], true);
    assert_eq!(manifest["depended_on_by"], serde_json::json!([]));
}

#[test]
fn a_program_above_a_member_narrows_it() {
    let dir = tree(&[
        ("package.json", r#"{"name":"root","private":true}"#),
        ("pnpm-workspace.yaml", "packages:\n  - packages/*\n"),
        ("tsconfig.json", r#"{"include":["packages/a/src"]}"#),
        ("packages/a/package.json", r#"{"name":"a"}"#),
        ("packages/a/src/i.ts", "export const i = 1;\n"),
        ("packages/a/scripts/x.ts", "console.log(1);\n"),
    ]);
    let services = derive(&dir);
    let (_, member) = services
        .iter()
        .find(|(directory, _)| directory == "packages/a")
        .unwrap_or_else(|| panic!("{services:?}"));
    assert_eq!(member["reach"]["program"], "narrows");
    assert_eq!(units(member), ["scripts"]);
}

#[test]
fn a_jsconfig_governs_where_no_tsconfig_sits() {
    let dir = tree(&[
        ("jsconfig.json", r#"{"include":["src"]}"#),
        ("src/a.js", "export const a = 1;\n"),
        ("scratch/b.js", "console.log(1);\n"),
    ]);
    let found = only(&dir);
    assert_eq!(found["reach"]["program"], "narrows");
    assert_eq!(units(&found), ["scratch"]);
}

#[test]
fn every_export_form_counts_and_a_script_does_not() {
    let dir = tree(&[
        ("tsconfig.json", r#"{"include":["src"]}"#),
        ("src/i.ts", "export const i = 1;\n"),
        ("misc/a.ts", "export const a = 1;\n"),
        ("misc/b.ts", "export default 1;\n"),
        ("misc/c.ts", "const c = 1;\nexport = c;\n"),
        ("misc/d.js", "module.exports = {};\n"),
        ("misc/e.js", "exports.a = 1;\n"),
        ("misc/f.js", "console.log(1);\n"),
    ]);
    let found = only(&dir);
    assert_eq!(unit(&found, "misc")["exporting_files"], 5);
}

#[test]
fn each_pattern_matches_exactly_its_unit_whatever_the_name() {
    let dir = tree(&[
        ("tsconfig.json", r#"{"include":["s"]}"#),
        ("s/x.ts", "export const x = 1;\n"),
        ("[slug]/a.ts", "console.log(1);\n"),
        ("#notes/a.ts", "console.log(1);\n"),
        ("!bang/a.ts", "console.log(1);\n"),
        ("a b /a.ts", "console.log(1);\n"),
        ("star*/a.ts", "console.log(1);\n"),
    ]);
    let found = only(&dir);
    let listed: BTreeSet<String> = units(&found).into_iter().collect();
    let wanted: BTreeSet<String> = ["!bang", "#notes", "[slug]", "a b ", "star*"]
        .into_iter()
        .map(str::to_string)
        .collect();
    assert_eq!(listed, wanted);
    for unit in found["directories"].as_array().unwrap() {
        let pattern = unit["pattern"].as_str().unwrap();
        assert!(
            pattern.starts_with('/') && pattern.ends_with('/'),
            "{pattern}"
        );
        // Each pattern alone, in gitignore syntax rooted at the service,
        // leaves out exactly this unit's file and nothing else.
        let mut builder = ignore::gitignore::GitignoreBuilder::new(dir.path().join("repo"));
        builder.add_line(None, pattern).unwrap();
        let matcher = builder.build().unwrap();
        let name = unit["unit"].as_str().unwrap();
        for other in [
            "s/x.ts",
            "[slug]/a.ts",
            "#notes/a.ts",
            "!bang/a.ts",
            "a b /a.ts",
            "star*/a.ts",
        ] {
            let hit = matcher
                .matched_path_or_any_parents(other, false)
                .is_ignore();
            assert_eq!(
                hit,
                other.starts_with(&format!("{name}/")),
                "{pattern} against {other}"
            );
        }
    }
}

#[test]
fn units_past_the_cap_are_counted_and_read() {
    let files: Vec<(String, String)> = (0..301)
        .map(|i| (format!("u{i:03}/a.ts"), "console.log(1);\n".to_string()))
        .collect();
    let borrowed: Vec<(&str, &str)> = files
        .iter()
        .map(|(file, body)| (file.as_str(), body.as_str()))
        .collect();
    let dir = tree(&borrowed);
    let found = only(&dir);
    assert_eq!(found["directories"].as_array().unwrap().len(), 300);
    assert_eq!(found["truncated"]["directories"], 1);
    assert_eq!(units(&found)[0], "u000");
}

#[test]
fn two_derives_of_one_tree_give_the_same_bytes() {
    let dir = tree(&[
        ("package.json", r#"{"name":"svc","main":"src/index.ts"}"#),
        ("tsconfig.json", r#"{"include":["src"]}"#),
        ("src/index.ts", "export const x = 1;\n"),
        ("b/one.ts", "console.log(1);\n"),
        ("a/two.ts", "console.log(2);\n"),
        ("c.ts", "console.log(3);\n"),
    ]);
    assert_eq!(derive_bytes(&dir), derive_bytes(&dir));
    assert_eq!(units(&only(&dir)), ["a", "b", "c.ts"]);
}

#[test]
fn nothing_unclaimed_lists_no_units() {
    let dir = tree(&[
        ("package.json", r#"{"name":"svc","main":"src/index.ts"}"#),
        ("tsconfig.json", r#"{"include":["src"]}"#),
        ("src/index.ts", "import \"./b\";\n"),
        ("src/b.ts", "export const b = 1;\n"),
    ]);
    let found = only(&dir);
    assert_eq!(found["directories"], serde_json::json!([]));
    assert_eq!(found["reach"]["unclaimed"], 0);
}

/// carrick-cloud#2348 amendment 1: a file a routing convention maps to a
/// route is reached, so the app that routes by location is never offered.
#[test]
fn files_a_routing_convention_maps_are_reached() {
    let dir = tree(&[
        (
            "package.json",
            r#"{"name":"web","dependencies":{"next":"15.0.0"}}"#,
        ),
        ("tsconfig.json", "{}"),
        (
            "app/users/route.ts",
            "import { q } from \"../../lib/q\";\nexport async function GET() { return q; }\n",
        ),
        ("lib/q.ts", "export const q = 1;\n"),
        ("scratch/try.ts", "console.log(1);\n"),
    ]);
    let found = only(&dir);
    assert_eq!(found["reach"]["route_roots"], 1);
    assert_eq!(units(&found), ["scratch"]);
}

/// The shape of a vendored, minified library and a documentation template
/// beside an app: neither is reached from the app's entry, so both are
/// offered as units with the evidence a judge reads, and nothing the app
/// reaches is in either.
#[test]
fn a_vendored_bundle_and_a_docs_template_beside_an_app_are_offered() {
    let minified = format!(
        "/*! editor v1.0 | (c) vendor | MIT */!function(e,t){{{}}}(this,function(){{return{{on:function(n,f){{window.addEventListener(n,f)}},emit:function(n,d){{window.postMessage({{n:n,d:d}},\"*\")}}}}}});\n",
        "var a=1;".repeat(400)
    );
    let dir = tree(&[
        (
            "package.json",
            r#"{"name":"field-app","main":"web/app.js"}"#,
        ),
        (
            "web/app.js",
            "import { start } from \"./lib/start.js\";\nstart();\n",
        ),
        ("web/lib/start.js", "export function start() {}\n"),
        ("web/vendor/editor.min.js", minified.as_str()),
        (
            "docs/template/package.json",
            r#"{"name":"docs-template","description":"Documentation site template","private":true}"#,
        ),
        (
            "docs/template/src/api.js",
            "export function user(login) {\n  return fetch(\"/users/\" + login);\n}\n",
        ),
        ("docs/template/assets/editor.min.js", minified.as_str()),
    ]);
    let found = only(&dir);
    assert_eq!(found["reach"]["reached"], 2);
    assert_eq!(units(&found), ["docs", "web/vendor"]);
    let vendor = unit(&found, "web/vendor");
    assert_eq!(vendor["pattern"], "/web/vendor/");
    assert_eq!(vendor["imports_from"], serde_json::json!([]));
    let head = vendor["samples"][0]["head"][0].as_str().unwrap();
    assert!(head.starts_with("/*! editor v1.0"), "{head}");
    assert_eq!(head.chars().count(), 160);
    let docs = unit(&found, "docs");
    assert_eq!(docs["files"], 2);
    assert_eq!(docs["manifests"][0]["path"], "template/package.json");
    assert_eq!(docs["manifests"][0]["private"], true);
    assert_eq!(docs["manifests"][0]["has_entries"], false);
    assert_eq!(docs["exporting_files"], 1);
}

/// The repo-wide bound: past 2,000 listed units in one repository, the rest
/// are counted on the service they belong to.
#[test]
fn units_past_the_repo_cap_are_counted() {
    let mut files: Vec<(String, String)> = vec![
        (
            "package.json".to_string(),
            r#"{"name":"root","private":true}"#.to_string(),
        ),
        (
            "pnpm-workspace.yaml".to_string(),
            "packages:\n  - packages/*\n".to_string(),
        ),
    ];
    for member in 0..7 {
        files.push((
            format!("packages/m{member}/package.json"),
            format!(r#"{{"name":"m{member}"}}"#),
        ));
        for i in 0..300 {
            files.push((
                format!("packages/m{member}/u{i:03}/a.ts"),
                "console.log(1);\n".to_string(),
            ));
        }
    }
    let borrowed: Vec<(&str, &str)> = files
        .iter()
        .map(|(file, body)| (file.as_str(), body.as_str()))
        .collect();
    let dir = tree(&borrowed);
    let services = derive(&dir);
    let listed: usize = services
        .iter()
        .map(|(_, found)| found["directories"].as_array().map_or(0, Vec::len))
        .sum();
    let cut: u64 = services
        .iter()
        .map(|(_, found)| found["truncated"]["directories"].as_u64().unwrap_or(0))
        .sum();
    assert_eq!(listed, 2000);
    assert_eq!(cut, 100);
}

/// A Deno config is the service's manifest too: its `exports` are entries and
/// its `tasks` are its scripts.
#[test]
fn a_deno_config_states_entries_and_tasks() {
    let dir = tree(&[
        (
            "deno.json",
            r#"{"exports":{".":"./mod.ts","./config":"./config.ts"},"tasks":{"dev":"deno run -A src/main.ts","check":{"command":"deno run tools/check.ts"}}}"#,
        ),
        ("mod.ts", "export * from \"./lib.ts\";\n"),
        ("lib.ts", "export const lib = 1;\n"),
        ("config.ts", "export const config = 1;\n"),
        ("src/main.ts", "console.log(1);\n"),
        ("tools/check.ts", "console.log(2);\n"),
        ("scratch/x.ts", "console.log(3);\n"),
    ]);
    let found = only(&dir);
    assert_eq!(found["reach"]["manifest_roots"], 2);
    assert_eq!(found["reach"]["script_roots"], 2);
    assert_eq!(found["reach"]["reached"], 5);
    assert_eq!(units(&found), ["scratch"]);
}

/// An entry that names build output is read back to the source the compiler
/// built it from: the path under a config's `outDir`, placed under its
/// `rootDir`, else under its `include` roots, with the output extension
/// swapped for a source one.
#[test]
fn an_entry_in_build_output_roots_the_source_it_was_built_from() {
    let dir = tree(&[
        (
            "package.json",
            r#"{"name":"worker","main":"./dist/index.cjs","types":"./dist/index.d.ts","exports":{".":{"types":"./dist/index.d.ts","import":"./dist/index.js"}}}"#,
        ),
        ("tsconfig.json", "{}"),
        (
            "tsconfig.build.json",
            r#"{"include":["src/**/*.ts"],"compilerOptions":{"outDir":"dist"}}"#,
        ),
        ("src/index.ts", "export * from \"./queue\";\n"),
        ("src/queue.ts", "export const queue = 1;\n"),
        ("scratch/x.ts", "console.log(1);\n"),
    ]);
    let found = only(&dir);
    assert_eq!(found["reach"]["manifest_roots"], 1);
    assert_eq!(found["reach"]["reached"], 2);
    assert_eq!(units(&found), ["scratch"]);
}

#[test]
fn a_script_running_build_output_roots_the_source_under_root_dir() {
    let dir = tree(&[
        (
            "package.json",
            r#"{"name":"server","main":"dist/server.js","scripts":{"start":"tsc && node dist/server.js"}}"#,
        ),
        (
            "tsconfig.json",
            r#"{"compilerOptions":{"rootDir":"lib","outDir":"dist"}}"#,
        ),
        (
            "lib/server.ts",
            "import { routes } from \"./routes/index.js\";\nexport default routes;\n",
        ),
        ("lib/routes/index.ts", "export const routes = [];\n"),
        ("one-off/fix.ts", "console.log(1);\n"),
    ]);
    let found = only(&dir);
    assert_eq!(found["reach"]["manifest_roots"], 1);
    assert_eq!(found["reach"]["script_roots"], 1);
    assert_eq!(found["reach"]["reached"], 2);
    assert_eq!(units(&found), ["one-off"]);
}
