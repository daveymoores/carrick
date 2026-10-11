//! carrick#2230: with `CARRICK_UNTRUSTED_TREE` set, a scan reads nothing
//! whose real path is outside the checkout.
//!
//! The tree is generated here: a checkout beside a directory outside it. The
//! checkout holds a link to a file outside (never read with the switch) and a
//! link to a folder inside it (read either way, as a package store's links
//! are). The scan runs with the model mocked, and `CARRICK_EVAL_DUMP_DIR`
//! records each prompt the analyzer would have sent.

use std::path::{Path, PathBuf};
use std::process::Command;

/// In the outside file only. Anywhere a scan writes it, the file was read.
const SENTINEL: &str = "untrusted-tree-sentinel-7f3a";
/// In the folder the inside link points to.
const INSIDE: &str = "inside-link-marker-41c2";

fn write(root: &Path, relative: &str, text: &str) {
    let path = root.join(relative);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
}

fn git(dir: &Path, args: &[&str]) {
    let status = Command::new("git")
        .args(args)
        .current_dir(dir)
        .env("GIT_AUTHOR_NAME", "t")
        .env("GIT_AUTHOR_EMAIL", "t@t")
        .env("GIT_COMMITTER_NAME", "t")
        .env("GIT_COMMITTER_EMAIL", "t@t")
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .env_remove("GIT_INDEX_FILE")
        .status()
        .expect("git failed to spawn");
    assert!(status.success(), "git {args:?} failed");
}

/// `<tmp>/outside/secret.ts` and a committed checkout at `<tmp>/repo`.
#[cfg(unix)]
fn tree() -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    write(
        dir.path(),
        "outside/secret.ts",
        &format!(
            "export async function leak() {{\n  return fetch(\"https://example.com/{SENTINEL}\");\n}}\n"
        ),
    );
    let root = dir.path().join("repo");
    write(
        &root,
        "package.json",
        r#"{"name":"api","version":"1.0.0","dependencies":{"express":"^4.0.0"}}"#,
    );
    write(
        &root,
        "src/index.ts",
        "import express from \"express\";\nconst app = express();\napp.get(\"/api/orders\", (_req, res) => {\n  res.json([]);\n});\napp.listen(3000);\n",
    );
    write(
        &root,
        "shared/client.ts",
        &format!(
            "export async function orders() {{\n  return fetch(\"https://example.com/{INSIDE}\");\n}}\n"
        ),
    );
    std::os::unix::fs::symlink(
        dir.path().join("outside/secret.ts"),
        root.join("src/link.ts"),
    )
    .unwrap();
    std::os::unix::fs::symlink(root.join("shared"), root.join("src/lib")).unwrap();
    git(&root, &["init", "-q"]);
    git(&root, &["add", "-A"]);
    git(&root, &["commit", "-q", "-m", "fixture"]);
    (dir, root)
}

struct Scan {
    /// Every prompt the analyzer built, with the file it was built for.
    prompts: Vec<String>,
    /// Every blob the scan wrote.
    blobs: Vec<String>,
    output: String,
}

fn scan(root: &Path, untrusted: bool) -> Scan {
    let dump = tempfile::tempdir().unwrap();
    let storage = tempfile::tempdir().unwrap();
    let cache = tempfile::tempdir().unwrap();
    let cassettes = tempfile::tempdir().unwrap();
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_carrick"));
    cmd.arg(root)
        .env("CARRICK_EVAL_DUMP_DIR", dump.path())
        .env("CARRICK_LOCAL_STORAGE_DIR", storage.path())
        .env("CARRICK_LOCAL_STORAGE_ISOLATE", "1")
        .env("CARRICK_CACHE_DIR", cache.path())
        .env("CARRICK_MOCK_ALL", "1")
        .env(
            "CARRICK_MOCK_FIXTURE_DIR",
            format!("{}/", cassettes.path().display()),
        )
        .env("CARRICK_SKIP_INTENTS", "1")
        .env("CARRICK_ALLOW_MISSING_TYPES", "1");
    if untrusted {
        cmd.env("CARRICK_UNTRUSTED_TREE", "1");
    } else {
        cmd.env_remove("CARRICK_UNTRUSTED_TREE");
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
    let result = cmd.output().expect("failed to spawn carrick");
    let output = format!(
        "{}{}",
        String::from_utf8_lossy(&result.stdout),
        String::from_utf8_lossy(&result.stderr)
    );
    assert!(result.status.success(), "scan exited non-zero:\n{output}");
    let read_all = |dir: &Path| -> Vec<String> {
        walkdir::WalkDir::new(dir)
            .into_iter()
            .flatten()
            .filter(|entry| entry.file_type().is_file())
            .map(|entry| String::from_utf8_lossy(&std::fs::read(entry.path()).unwrap()).into())
            .collect()
    };
    Scan {
        prompts: read_all(dump.path()),
        blobs: read_all(storage.path()),
        output,
    }
}

/// Without the switch the outside file is read through its link, as on a
/// developer's own machine: the known positive the untrusted assertions
/// below are measured against.
#[cfg(unix)]
#[test]
fn without_the_switch_a_link_out_of_the_checkout_is_read() {
    let (_dir, root) = tree();
    let scan = scan(&root, false);
    assert!(
        scan.prompts.iter().any(|prompt| prompt.contains(SENTINEL)),
        "the outside file reached a prompt:\n{}",
        scan.output
    );
    assert!(
        scan.prompts.iter().any(|prompt| prompt.contains(INSIDE)),
        "the inside link was read:\n{}",
        scan.output
    );
}

/// With the switch, the outside file reaches no prompt, no blob and no line
/// of output; the scan says it left one link out; and a link that lands
/// inside the checkout is still read.
#[cfg(unix)]
#[test]
fn with_the_switch_a_link_out_of_the_checkout_is_never_read() {
    let (_dir, root) = tree();
    let scan = scan(&root, true);
    assert!(
        !scan.prompts.iter().any(|prompt| prompt.contains(SENTINEL)),
        "the outside file reached a prompt"
    );
    assert!(
        !scan.blobs.iter().any(|blob| blob.contains(SENTINEL)),
        "the outside file reached a blob"
    );
    assert!(
        !scan.output.contains(SENTINEL),
        "the outside file reached the output:\n{}",
        scan.output
    );
    assert_eq!(
        scan.output
            .matches(
                "Left out 1 symbolic link(s) whose real path is outside the repository: \
                 src/link.ts."
            )
            .count(),
        1,
        "the scan says once what it left out:\n{}",
        scan.output
    );
    assert!(
        scan.prompts.iter().any(|prompt| prompt.contains(INSIDE)),
        "a link inside the checkout is still read:\n{}",
        scan.output
    );
}
