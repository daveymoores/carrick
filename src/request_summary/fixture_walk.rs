//! One service of a fixture tree under `tests/fixtures/`, read by discovery
//! with the rows the real sidecar walks or with none, as a run without
//! `CARRICK_POSITION_ROWS` (carrick#2413). Shared by the tests of what the
//! summaries read through the rows.

use super::*;
use crate::config::Config;
use crate::position_rows::PositionIndex;

fn copy_tree(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for entry in std::fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        let target = to.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_tree(&entry.path(), &target);
        } else {
            std::fs::copy(entry.path(), &target).unwrap();
        }
    }
}

/// The sidecar script, when node and the built sidecar are here. CI builds
/// it before the Rust tests, so a skip there would mean nothing.
pub(crate) fn sidecar() -> Option<PathBuf> {
    let script = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/sidecar/dist/src/index.js");
    let node = std::process::Command::new("node")
        .arg("--version")
        .output()
        .is_ok();
    if script.exists() && node {
        return Some(script);
    }
    assert!(
        std::env::var("CI").is_err(),
        "node or the built sidecar is missing in CI"
    );
    crate::errln!("Skipping: node or the built sidecar is missing");
    None
}

/// A fixture service, read.
pub(crate) struct Walked {
    _dir: tempfile::TempDir,
    /// The copied repo's root, canonical.
    pub(crate) repo: PathBuf,
    pub(crate) inputs: RequestSummaryInputs,
}

impl Walked {
    /// The path discovery read `file` (relative to the repo root) under.
    pub(crate) fn path(&self, file: &str) -> PathBuf {
        let wanted = self.repo.join(file);
        self.inputs
            .files
            .keys()
            .find(|path| **path == wanted)
            .cloned()
            .unwrap_or_else(|| panic!("{file} is not read"))
    }
}

/// `tests/fixtures/<fixture>/repo`'s first service, rooted at `service_dir`
/// and named `service`, with `edit` applied (paths under `service_dir`,
/// directories made as needed; empty text removes the file).
pub(crate) fn walked(
    fixture: &str,
    service_dir: &str,
    service: &str,
    edit: &[(&str, &str)],
    with_rows: bool,
) -> Option<Walked> {
    let script = sidecar()?;
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
    let dir = tempfile::tempdir().unwrap();
    let repo = std::fs::canonicalize(dir.path()).unwrap().join("repo");
    copy_tree(
        &manifest.join("tests/fixtures").join(fixture).join("repo"),
        &repo,
    );
    let service_root = repo.join(service_dir);
    for (name, text) in edit {
        let path = service_root.join(name);
        if text.is_empty() {
            std::fs::remove_file(&path).unwrap();
        } else {
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(&path, text).unwrap();
        }
    }
    let config = Config::load_services(vec![repo.join("carrick.json")])
        .unwrap()
        .remove(0);
    let positions = with_rows.then(|| {
        let mut files =
            crate::file_finder::walk_service(&repo.to_string_lossy(), &config, &[]).files;
        files.sort();
        // A directory beside the tree: the walk refuses to write into it.
        let positions = dir.path().join("positions");
        std::fs::create_dir_all(&positions).unwrap();
        let out = positions.join(format!("repo__{service}.jsonl.gz"));
        let sidecar = crate::services::TypeSidecar::spawn(&script).expect("spawn the sidecar");
        sidecar.set_scan_root(&repo);
        let recipe = sidecar.recipe();
        drop(sidecar);
        crate::position_rows::record_service_rows(
            &recipe,
            &crate::position_rows::ServiceWalk {
                service_root: &service_root,
                tsconfig: Some("tsconfig.json"),
                scan_root: &repo,
                files: &files,
                out: &out,
                service,
            },
        )
        .expect("the walk writes its rows");
        PositionIndex::of(
            &repo,
            crate::position_rows::read_rows_file(&out).expect("the rows read"),
        )
    });
    let inputs = crate::engine::discover_service_inputs(&repo, &config, positions);
    Some(Walked {
        _dir: dir,
        repo,
        inputs,
    })
}
