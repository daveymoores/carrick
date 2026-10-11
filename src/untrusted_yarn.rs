//! Whether a Yarn root can be installed without running a file from the tree,
//! on a machine that scans repositories it does not own (carrick#2246).
//!
//! `scripts/install-scanned-deps.sh` asks this in `CARRICK_INSTALL_UNTRUSTED`
//! mode, through the internal command [`COMMAND`], before it starts Yarn. Two
//! Yarn Berry settings hand the install to the repository's own code, and no
//! flag or environment variable turns them off:
//!
//! - `plugins`: every listed file is loaded when Yarn starts;
//! - `yarnPath` with no `packageManager` pin: `YARN_IGNORE_PATH=1` stops the
//!   launcher handing over to the committed release, but then the version that
//!   runs is unknown, because only that file says what it is.
//!
//! Yarn reads `.yarnrc.yml` in the directory it runs in AND in every parent,
//! so each one from the install root up to the clone root is read, and read as
//! YAML: a quoted key, a flow mapping or a merge key is the same setting to
//! Yarn as `plugins:` at the start of a line. A file this cannot read is an
//! error, never "nothing found": the caller refuses the root.

use std::collections::BTreeSet;
use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};

use serde_yaml_ng::Value;

/// The internal command the install script runs: `carrick <COMMAND> <root>`.
/// Not listed in `carrick --help`; nothing but the script calls it.
pub const COMMAND: &str = "__untrusted-yarn-refusal";

const RC_FILE: &str = ".yarnrc.yml";

/// Run [`COMMAND`]. Prints the reason the root is refused on stdout, or
/// nothing when it can be installed; exits 0 either way. Exit 1, with the
/// problem on stderr, means the answer could not be read, which the caller
/// treats as a refusal.
pub fn run(args: &[String]) -> i32 {
    let [root] = args else {
        crate::errln!("usage: carrick {COMMAND} <install-root>");
        return 2;
    };
    match refusal(Path::new(root)) {
        Ok(Some(reason)) => {
            crate::outln!("{reason}");
            0
        }
        Ok(None) => 0,
        Err(problem) => {
            crate::errln!("carrick {COMMAND}: {problem}");
            1
        }
    }
}

/// Why Yarn cannot install `install_root` without running the tree's code, or
/// `None` when it can.
pub fn refusal(install_root: &Path) -> Result<Option<String>, String> {
    let install_root = install_root
        .canonicalize()
        .map_err(|error| format!("{}: {error}", install_root.display()))?;
    let chain = rc_chain(&install_root);
    let clone_root = chain
        .last()
        .cloned()
        .unwrap_or_else(|| install_root.clone());
    let mut yarn_path: Option<PathBuf> = None;
    for dir in &chain {
        let rc = dir.join(RC_FILE);
        let text = match fs::read_to_string(&rc) {
            Ok(text) => text,
            Err(error) if error.kind() == ErrorKind::NotFound => continue,
            Err(error) => return Err(format!("{}: {error}", rc.display())),
        };
        let keys = top_level_keys(&text).map_err(|error| format!("{}: {error}", rc.display()))?;
        let shown = shown(&rc, &clone_root);
        if keys.contains("plugins") {
            return Ok(Some(format!(
                "{shown} lists plugins, which Yarn loads at startup"
            )));
        }
        if keys.contains("yarnPath") && yarn_path.is_none() {
            yarn_path = Some(rc);
        }
    }
    if let Some(rc) = yarn_path
        && !pinned(&chain)
    {
        return Ok(Some(format!(
            "{} names a yarnPath and no package.json pins a packageManager, so the Yarn version \
             cannot be known without running the repository's own file",
            shown(&rc, &clone_root)
        )));
    }
    Ok(None)
}

/// The directories whose `.yarnrc.yml` Yarn reads for this root that the
/// repository controls: the root, then each parent up to and including the
/// clone root, the first with a `.git` DIRECTORY. A submodule or a worktree has
/// a `.git` file, and its parents are still the clone's, so the walk passes
/// it. With no `.git` directory above, every parent is read: more files read
/// can only refuse more, never less.
fn rc_chain(install_root: &Path) -> Vec<PathBuf> {
    let mut chain = Vec::new();
    for dir in install_root.ancestors() {
        chain.push(dir.to_path_buf());
        if dir.join(".git").is_dir() {
            break;
        }
    }
    chain
}

/// The keys of one YAML document's top-level mapping, merge keys applied.
fn top_level_keys(text: &str) -> Result<BTreeSet<String>, String> {
    let mut value: Value = serde_yaml_ng::from_str(text).map_err(|error| error.to_string())?;
    value.apply_merge().map_err(|error| error.to_string())?;
    match untagged(value) {
        // An empty file sets nothing.
        Value::Null => Ok(BTreeSet::new()),
        Value::Mapping(mapping) => Ok(mapping
            .into_iter()
            .filter_map(|(key, _)| match untagged(key) {
                Value::String(key) => Some(key),
                _ => None,
            })
            .collect()),
        _ => Err("the top level is not a mapping".to_string()),
    }
}

fn untagged(value: Value) -> Value {
    match value {
        Value::Tagged(tagged) => untagged(tagged.value),
        other => other,
    }
}

/// Whether any `package.json` on the chain names a `packageManager`, which is
/// how corepack picks the Yarn that runs.
fn pinned(chain: &[PathBuf]) -> bool {
    chain.iter().any(|dir| {
        fs::read_to_string(dir.join("package.json"))
            .ok()
            .and_then(|text| serde_json::from_str::<serde_json::Value>(&text).ok())
            .is_some_and(|manifest| {
                manifest
                    .get("packageManager")
                    .is_some_and(|v| v.is_string())
            })
    })
}

/// A path as the warning names it: relative to the clone root.
fn shown(path: &Path, clone_root: &Path) -> String {
    path.strip_prefix(clone_root)
        .unwrap_or(path)
        .display()
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn clone_with(files: &[(&str, &str)]) -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir(dir.path().join(".git")).unwrap();
        for (path, body) in files {
            let path = dir.path().join(path);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, body).unwrap();
        }
        dir
    }

    fn answer(clone: &tempfile::TempDir, root: &str) -> Result<Option<String>, String> {
        refusal(&clone.path().join(root))
    }

    #[test]
    fn plugins_refuse_however_the_yaml_spells_them() {
        for rc in [
            "plugins:\n  - path: p.cjs\n",
            "\"plugins\": [{path: p.cjs}]\n",
            "'plugins':\n  - p.cjs\n",
            "{plugins: [p.cjs], nodeLinker: node-modules}\n",
            "? plugins\n: [p.cjs]\n",
            "base: &base\n  plugins: [p.cjs]\n<<: *base\n",
            "!!str plugins: []\n",
        ] {
            let clone = clone_with(&[("svc/.yarnrc.yml", rc), ("svc/package.json", "{}")]);
            let reason = answer(&clone, "svc").unwrap();
            assert!(
                reason
                    .as_deref()
                    .is_some_and(|r| r.contains("lists plugins")),
                "{rc:?} -> {reason:?}"
            );
        }
    }

    #[test]
    fn a_parent_rc_file_up_to_the_clone_root_is_read() {
        let clone = clone_with(&[
            (".yarnrc.yml", "plugins:\n  - path: p.cjs\n"),
            ("apps/svc/package.json", "{}"),
        ]);
        let reason = answer(&clone, "apps/svc").unwrap().unwrap();
        assert_eq!(
            reason,
            ".yarnrc.yml lists plugins, which Yarn loads at startup"
        );
    }

    #[test]
    fn the_walk_passes_a_git_file_and_stops_at_the_clone() {
        let outer = tempfile::tempdir().unwrap();
        fs::write(outer.path().join(".yarnrc.yml"), "plugins: [p.cjs]\n").unwrap();
        let clone = outer.path().join("clone");
        fs::create_dir_all(clone.join(".git")).unwrap();
        fs::create_dir_all(clone.join("sub")).unwrap();
        fs::write(clone.join("sub/.git"), "gitdir: ../.git/modules/sub\n").unwrap();
        fs::write(clone.join(".yarnrc.yml"), "yarnPath: r.cjs\n").unwrap();
        // The clone's own file is read through the submodule's `.git` file;
        // the machine's file above the clone is not.
        let reason = refusal(&clone.join("sub")).unwrap().unwrap();
        assert!(reason.contains("names a yarnPath"), "{reason}");
    }

    #[test]
    fn a_yarn_path_refuses_only_without_a_pin_on_the_chain() {
        let unpinned = clone_with(&[
            ("svc/.yarnrc.yml", "\"yarnPath\": .yarn/r.cjs\n"),
            (
                "svc/package.json",
                r#"{"engines":{"packageManager":"yarn@4.9.2"}}"#,
            ),
        ]);
        assert!(
            answer(&unpinned, "svc")
                .unwrap()
                .is_some_and(|r| r.contains("pins a packageManager"))
        );

        let pinned_above = clone_with(&[
            ("svc/.yarnrc.yml", "yarnPath: .yarn/r.cjs\n"),
            ("svc/package.json", "{}"),
            ("package.json", r#"{"packageManager":"yarn@4.9.2"}"#),
        ]);
        assert_eq!(answer(&pinned_above, "svc").unwrap(), None);
    }

    #[test]
    fn nothing_to_refuse_is_none() {
        let clone = clone_with(&[
            ("svc/.yarnrc.yml", "nodeLinker: node-modules\n"),
            (".yarnrc.yml", ""),
            ("svc/package.json", "{}"),
        ]);
        assert_eq!(answer(&clone, "svc").unwrap(), None);
    }

    #[test]
    fn a_file_yarn_could_not_read_either_is_an_error() {
        for rc in ["plugins: [\n", "- plugins\n", "a: 1\n---\nplugins: []\n"] {
            let clone = clone_with(&[("svc/.yarnrc.yml", rc)]);
            assert!(answer(&clone, "svc").is_err(), "{rc:?}");
        }
        let clone = clone_with(&[]);
        fs::create_dir_all(clone.path().join("svc/.yarnrc.yml")).unwrap();
        assert!(
            answer(&clone, "svc").is_err(),
            "a directory named .yarnrc.yml"
        );
    }
}
