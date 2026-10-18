//! `carrick derive --workspace <root>` and a scan of `<root>` name the same
//! services (carrick#1858).
//!
//! The Action's installer asks the command which services to prepare
//! (`scripts/install-scanned-deps.sh`), and the scan derives its own from the
//! same root with `service_derivation::resolve`. The command first decides
//! which REPOSITORIES the folder holds, and at the root of one git repository
//! whose packages sit in directories below it, it answered "a folder of
//! repositories": each package directory was a repository with one service,
//! and the scan's services were never asked for.
//!
//! Each case asks both and compares the service roots, resolved the way the
//! installer resolves them: a service's `directory` against its repository's
//! path.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::process::Command;

fn write(root: &Path, file: &str, body: &str) {
    let target = root.join(file);
    std::fs::create_dir_all(target.parent().unwrap()).unwrap();
    std::fs::write(target, body).unwrap();
}

fn package(root: &Path, directory: &str, name: &str) {
    write(
        root,
        &format!("{directory}/package.json"),
        &format!(r#"{{"name":"{name}"}}"#),
    );
    write(
        root,
        &format!("{directory}/src/index.ts"),
        "export const port = 3000;\n",
    );
}

/// The service roots the installer is told about.
fn asked_of_the_command(root: &Path) -> BTreeSet<PathBuf> {
    let output = Command::new(env!("CARGO_BIN_EXE_carrick"))
        .args(["derive", "--workspace"])
        .arg(root)
        .arg("--json")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let derived: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    derived["repos"]
        .as_array()
        .unwrap()
        .iter()
        .flat_map(|repo| {
            let path = PathBuf::from(repo["path"].as_str().unwrap());
            repo["services"]
                .as_array()
                .unwrap()
                .iter()
                .map(move |service| match service["directory"].as_str() {
                    Some(directory) => path.join(directory),
                    None => path.clone(),
                })
                .collect::<Vec<_>>()
        })
        .collect()
}

/// The service roots a scan of `root` reads.
fn asked_of_the_scan(root: &Path) -> BTreeSet<PathBuf> {
    carrick::service_derivation::resolve(root)
        .unwrap()
        .services
        .iter()
        .map(|service| match service.directory.as_deref() {
            Some(directory) => root.join(directory),
            None => root.to_path_buf(),
        })
        .collect()
}

/// One git repository, a root manifest that declares no workspace, and
/// package directories one and two levels down.
#[test]
fn a_git_repository_with_package_directories_is_one_repository_to_both() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap().join("shop");
    std::fs::create_dir_all(root.join(".git")).unwrap();
    write(&root, "package.json", r#"{"private":true}"#);
    package(&root, "web", "web");
    package(&root, "api", "api");
    package(&root, "tools/sync", "sync");

    assert_eq!(asked_of_the_command(&root), asked_of_the_scan(&root));
}

/// The shapes that already agreed still do: a declared workspace, an explicit
/// `carrick.json`, and a single package.
#[test]
fn a_declared_workspace_an_explicit_config_and_a_single_package_agree() {
    let dir = tempfile::tempdir().unwrap();
    let base = dir.path().canonicalize().unwrap();

    let workspace = base.join("workspace");
    std::fs::create_dir_all(workspace.join(".git")).unwrap();
    write(
        &workspace,
        "package.json",
        r#"{"private":true,"workspaces":["apps/*"]}"#,
    );
    package(&workspace, "apps/web", "web");
    package(&workspace, "apps/api", "api");
    assert_eq!(
        asked_of_the_command(&workspace),
        asked_of_the_scan(&workspace)
    );

    let declared = base.join("declared");
    std::fs::create_dir_all(declared.join(".git")).unwrap();
    package(&declared, "web", "web");
    package(&declared, "api", "api");
    write(
        &declared,
        "carrick.json",
        r#"{"services":[{"name":"web","directory":"web"},{"name":"api","directory":"api"}]}"#,
    );
    assert_eq!(
        asked_of_the_command(&declared),
        asked_of_the_scan(&declared)
    );

    let single = base.join("single");
    std::fs::create_dir_all(single.join(".git")).unwrap();
    package(&single, ".", "single");
    assert_eq!(asked_of_the_command(&single), asked_of_the_scan(&single));
}

/// `carrick derive` refuses a `carrick.json` whose service directory leaves
/// the repository (carrick#2245), so the installer is never told to prepare
/// it; the scan refuses it through the same `resolve`
/// (`service_derivation_test`).
#[test]
fn derive_refuses_a_service_directory_outside_the_repository() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("repo");
    package(&root, "api", "api");
    package(dir.path(), "outside", "outside");
    write(
        &root,
        "carrick.json",
        r#"{"services":[{"name":"api","directory":"../outside"}]}"#,
    );
    let output = Command::new(env!("CARGO_BIN_EXE_carrick"))
        .args(["derive", "--workspace"])
        .arg(&root)
        .arg("--json")
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(!output.status.success(), "derive refused: {stderr}");
    assert!(
        stderr.contains("which is outside the repository"),
        "derive says why: {stderr}"
    );
}

// A workspace member that holds no source (carrick#2298).
//
// A package manager's member is a directory with a manifest. Derive made each
// one a service, the scan of a service that walks no file is an error, and so
// one configuration package failed the scan of every other member.

fn git(dir: &Path, args: &[&str]) {
    let status = Command::new("git")
        .args(args)
        .current_dir(dir)
        .env("GIT_AUTHOR_NAME", "t")
        .env("GIT_AUTHOR_EMAIL", "t@t")
        .env("GIT_COMMITTER_NAME", "t")
        .env("GIT_COMMITTER_EMAIL", "t@t")
        .status()
        .expect("git failed to spawn");
    assert!(status.success(), "git {args:?} failed");
}

/// Three workspace members under `packages/`, committed: `api` holds source,
/// `settings` holds a manifest and a JSON file, and `checks` holds a manifest
/// and one test file, which carries a source extension and which no scan
/// reads. `root_source` adds a source file at the root and makes the root a
/// member too, so one service's directory is `.`.
fn workspace(base: &Path, name: &str, root_source: bool) -> PathBuf {
    let root = base.join(name);
    let members = if root_source {
        r#"[".", "packages/*"]"#
    } else {
        r#"["packages/*"]"#
    };
    write(
        &root,
        "package.json",
        &format!(r#"{{"name":"{name}","private":true,"workspaces":{members}}}"#),
    );
    if root_source {
        write(&root, "src/main.ts", "export const started = true;\n");
    }
    package(&root, "packages/api", "api");
    write(
        &root,
        "packages/settings/package.json",
        r#"{"name":"settings"}"#,
    );
    write(&root, "packages/settings/base.json", r#"{"strict":true}"#);
    write(
        &root,
        "packages/checks/package.json",
        r#"{"name":"checks"}"#,
    );
    write(
        &root,
        "packages/checks/src/api.test.ts",
        "export const checked = 1;\n",
    );
    git(&root, &["init", "-q"]);
    git(&root, &["add", "-A"]);
    git(&root, &["commit", "-q", "-m", "init"]);
    root
}

struct Scan {
    succeeded: bool,
    /// stdout and stderr, in that order.
    output: String,
    /// The `service_name` of every blob the run stored; `None` is the service
    /// at the repository root.
    stored: BTreeSet<Option<String>>,
    /// Every line of the run's own debug log (`~/.carrick/logs`).
    log: String,
}

/// Every line under `home/.carrick/logs`.
fn read_logs(home: &Path) -> String {
    let mut log = String::new();
    let mut dirs = vec![home.join(".carrick/logs")];
    while let Some(dir) = dirs.pop() {
        for entry in std::fs::read_dir(&dir).into_iter().flatten().flatten() {
            let path = entry.path();
            if path.is_dir() {
                dirs.push(path);
            } else {
                log.push_str(&std::fs::read_to_string(&path).unwrap_or_default());
            }
        }
    }
    log
}

/// Scan `repo` with the built binary as the GitHub Action does: from inside
/// the checkout, with the path it is given, offline.
fn scan(repo: &Path, path: &str, store: &Path) -> Scan {
    let _ = std::fs::remove_dir_all(store);
    std::fs::create_dir_all(store).unwrap();
    let home = store.with_file_name("home");
    let _ = std::fs::remove_dir_all(&home);
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_carrick"));
    cmd.arg(path)
        .current_dir(repo)
        .env("HOME", &home)
        .env("CARRICK_MOCK_ALL", "1")
        .env("CARRICK_SKIP_INTENTS", "1")
        .env("CARRICK_ALLOW_UNPREPARED", "1")
        .env("CARRICK_LOCAL_STORAGE_DIR", store);
    // A runner's own values would name this repository.
    for var in [
        "GITHUB_REPOSITORY",
        "GITHUB_EVENT_NAME",
        "GITHUB_EVENT_PATH",
        "GITHUB_REF",
        "GITHUB_RUN_ID",
        "GITHUB_ACTIONS",
        "CI",
        "CARRICK_OUTPUT_JSON",
        "CARRICK_LOCAL_STORAGE_ISOLATE",
        "ACTIONS_ID_TOKEN_REQUEST_URL",
        "ACTIONS_ID_TOKEN_REQUEST_TOKEN",
    ] {
        cmd.env_remove(var);
    }
    let run = cmd.output().expect("failed to spawn carrick");
    let stored = std::fs::read_dir(store)
        .unwrap()
        .flatten()
        .filter(|entry| entry.path().extension().is_some_and(|ext| ext == "json"))
        .map(|entry| {
            let blob: serde_json::Value =
                serde_json::from_slice(&std::fs::read(entry.path()).unwrap()).unwrap();
            blob["service_name"].as_str().map(str::to_owned)
        })
        .collect();
    Scan {
        succeeded: run.status.success(),
        output: format!(
            "{}{}",
            String::from_utf8_lossy(&run.stdout),
            String::from_utf8_lossy(&run.stderr)
        ),
        stored,
        log: read_logs(&home),
    }
}

fn names(services: &[carrick::config::Config]) -> Vec<Option<&str>> {
    services
        .iter()
        .map(|service| service.service_name.as_deref())
        .collect()
}

/// A member with no source file is no service, to the command and to the scan
/// alike, and the derive document says which directory it left out. A member
/// whose only source file is a test is kept: derive asks for the extension,
/// as the published input rule does, and the walk decides the rest.
#[test]
fn a_workspace_member_without_source_is_no_service() {
    let dir = tempfile::tempdir().unwrap();
    let root = workspace(&dir.path().canonicalize().unwrap(), "shop", false);

    let derived = carrick::service_derivation::resolve(&root).unwrap();
    assert_eq!(names(&derived.services), [Some("api"), Some("checks")]);
    assert_eq!(derived.without_source, ["packages/settings"]);
    assert!(
        derived
            .notes
            .contains(&"skipped_no_source: packages/settings".to_string()),
        "{:?}",
        derived.notes
    );
    assert_eq!(asked_of_the_command(&root), asked_of_the_scan(&root));
}

/// When no member holds source nothing says which directory was meant, so
/// every member stays and the scan reports the tree as it always did.
#[test]
fn members_that_all_lack_source_all_stay() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap().join("shop");
    write(
        &root,
        "package.json",
        r#"{"private":true,"workspaces":["packages/*"]}"#,
    );
    write(&root, "packages/a/package.json", r#"{"name":"a"}"#);
    write(&root, "packages/b/package.json", r#"{"name":"b"}"#);

    let derived = carrick::service_derivation::resolve(&root).unwrap();
    assert_eq!(names(&derived.services), [Some("a"), Some("b")]);
    assert!(derived.without_source.is_empty());
    assert!(derived.notes.iter().all(|note| !note.contains("no_source")));
}

/// Source inside another member's directory is that member's: a member that
/// only groups others holds none of its own.
#[test]
fn a_member_whose_only_source_is_another_members_is_no_service() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap().join("shop");
    write(
        &root,
        "package.json",
        r#"{"private":true,"workspaces":["groups/*","groups/*/*"]}"#,
    );
    write(&root, "groups/core/package.json", r#"{"name":"core"}"#);
    package(&root, "groups/core/api", "api");

    let derived = carrick::service_derivation::resolve(&root).unwrap();
    assert_eq!(names(&derived.services), [Some("api")]);
    assert_eq!(derived.without_source, ["groups/core"]);
}

/// The scan of that workspace, run as the Action runs it and by its absolute
/// path: it succeeds, the member with source is indexed, and the member derive
/// left out and the service whose walk read nothing are in the debug log and
/// not in what a person reads. Once with every service in a subdirectory and once with a service
/// at the repository root.
#[test]
fn a_scan_leaves_out_what_holds_no_source_and_indexes_the_rest() {
    let dir = tempfile::tempdir().unwrap();
    let base = dir.path().canonicalize().unwrap();
    let store = base.join("store");
    for (name, root_source) in [("nested", false), ("rooted", true)] {
        let root = workspace(&base, name, root_source);
        let absolute = root.to_string_lossy().into_owned();
        for path in [".", absolute.as_str()] {
            let run = scan(&root, path, &store);
            let said = &run.output;
            assert!(run.succeeded, "{name} at {path}: {said}");
            let mut indexed = BTreeSet::from([Some("api".to_string())]);
            if root_source {
                indexed.insert(None);
            }
            assert_eq!(run.stored, indexed, "{name} at {path}: {said}");
            for line in [
                "1 workspace member(s) left out, no JS/TS source: packages/settings",
                "checks: left out, the scan read no JS/TS source file in it",
            ] {
                assert!(
                    !said.contains(line),
                    "{name} at {path}: a person is told what the index lacks: {said}"
                );
                assert!(
                    run.log.contains(line),
                    "{name} at {path}: the debug log keeps it: {}",
                    run.log
                );
            }
        }
    }
}

/// A service somebody named in a committed `carrick.json` and that holds no
/// source is a wrong path, and fails the scan as before. The services that
/// finished land first, and the failure is reported at the stage the analysis
/// was in, not at the upload that landed them.
#[test]
fn a_declared_service_without_source_fails_the_scan_at_its_analysis_stage() {
    let dir = tempfile::tempdir().unwrap();
    let base = dir.path().canonicalize().unwrap();
    let root = workspace(&base, "declared", false);
    write(
        &root,
        "carrick.json",
        r#"{"services":[
            {"name":"api","directory":"packages/api"},
            {"name":"settings","directory":"packages/settings"}
        ]}"#,
    );

    let run = scan(&root, ".", &base.join("store"));
    let said = &run.output;
    assert!(!run.succeeded, "{said}");
    assert!(
        said.contains("No JS/TS source files found under service directory 'packages/settings'"),
        "{said}"
    );
    assert_eq!(
        run.stored,
        BTreeSet::from([Some("api".to_string())]),
        "the service that finished landed: {said}"
    );
    let stage = said
        .split("Analysis failed during ")
        .nth(1)
        .and_then(|rest| rest.split(':').next())
        .unwrap_or_else(|| panic!("the run names no stage: {said}"));
    // The stage is the process's, so with services analysed side by side it
    // is the last one any of them entered: always one of the analysis.
    assert!(
        [
            "discovery",
            "framework_detect",
            "file_analysis",
            "intents",
            "signatures",
            "type_capture",
            "definitions",
            "blob_build",
        ]
        .contains(&stage),
        "{said}"
    );
}

/// No service of the run holds source: the scan target is wrong, and the run
/// fails as it always did, for one root service and for a workspace alike.
#[test]
fn a_scan_with_no_source_at_all_still_fails() {
    let dir = tempfile::tempdir().unwrap();
    let base = dir.path().canonicalize().unwrap();

    let single = base.join("single");
    write(&single, "package.json", r#"{"name":"single"}"#);
    let run = scan(&single, ".", &base.join("store"));
    assert!(!run.succeeded, "{}", run.output);
    assert!(
        run.output
            .contains("No JS/TS source files found under repository root"),
        "{}",
        run.output
    );

    let members = base.join("members");
    write(
        &members,
        "package.json",
        r#"{"private":true,"workspaces":["packages/*"]}"#,
    );
    write(&members, "packages/a/package.json", r#"{"name":"a"}"#);
    write(&members, "packages/b/package.json", r#"{"name":"b"}"#);
    let run = scan(&members, ".", &base.join("store"));
    assert!(!run.succeeded, "{}", run.output);
    assert!(
        run.output
            .contains("No JS/TS source files found under service directory 'packages/a'"),
        "{}",
        run.output
    );
    assert!(run.stored.is_empty(), "{}", run.output);
}
