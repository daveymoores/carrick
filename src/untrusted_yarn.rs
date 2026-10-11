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
//!
//! A git dependency hands the install to code no file in the tree shows
//! (carrick#2251): Yarn prepares it by starting a package manager inside the
//! clone (`prepareExternalProject`), Yarn, Yarn Classic or npm as the clone's
//! own files choose, and that manager reads the clone's config, `plugins`
//! included. Nothing the parent install sets reaches it, so a Berry root whose
//! `yarn.lock` resolves any package from a git repository is refused too.
//! So is a classic root whose lockfile does (classic Yarn runs a git
//! dependency's `prepare` in the clone under `--ignore-scripts`,
//! carrick#2258), and a Berry root resolving a package with the `exec:`
//! protocol, which runs a file from the tree to build it (carrick#2257).

use std::collections::BTreeSet;
use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};

use serde_yaml_ng::Value;

/// The internal command the install script runs: `carrick <COMMAND> <root>`.
/// Not listed in `carrick --help`; nothing but the script calls it.
pub const COMMAND: &str = "__untrusted-yarn-refusal";

const RC_FILE: &str = ".yarnrc.yml";
const LOCKFILE: &str = "yarn.lock";

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
    let lockfile = install_root.join(LOCKFILE);
    if let Some((package, fetch)) = code_running_resolution(&lockfile)? {
        let shown = shown(&lockfile, &clone_root);
        return Ok(Some(match fetch {
            Fetch::Git => format!(
                "{shown} resolves {package} from a git repository, which Yarn prepares by \
                 running a package manager with that repository's own settings"
            ),
            Fetch::Exec => format!(
                "{shown} resolves {package} with the exec: protocol, which runs a file from the \
                 repository to build the package"
            ),
        }));
    }
    Ok(None)
}

/// How Yarn fetches a locked package when fetching it runs code no install
/// flag turns off.
#[derive(Debug, PartialEq)]
enum Fetch {
    /// Cloned and prepared by a package manager started inside the clone.
    Git,
    /// Built by running a file from the tree (Berry's `exec:` protocol).
    Exec,
}

/// The first package the root's `yarn.lock` resolves through a [`Fetch`]
/// that runs code, or `None`. A missing lockfile resolves nothing; a Berry
/// one that is not a YAML mapping is an error.
fn code_running_resolution(lockfile: &Path) -> Result<Option<(String, Fetch)>, String> {
    let text = match fs::read_to_string(lockfile) {
        Ok(text) => text,
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(format!("{}: {error}", lockfile.display())),
    };
    if text.lines().any(|line| line.trim() == "# yarn lockfile v1") {
        return Ok(classic_git_package(&text).map(|package| (package, Fetch::Git)));
    }
    let entries = match untagged(
        serde_yaml_ng::from_str::<Value>(&text)
            .map_err(|error| format!("{}: {error}", lockfile.display()))?,
    ) {
        Value::Null => return Ok(None),
        Value::Mapping(entries) => entries,
        _ => {
            return Err(format!(
                "{}: the top level is not a mapping",
                lockfile.display()
            ));
        }
    };
    for (_, entry) in entries {
        let Some(resolution) = entry.get("resolution").and_then(Value::as_str) else {
            continue;
        };
        if let Some((package, reference)) = split_locator(resolution)
            && let Some(fetch) = berry_fetch(reference)
        {
            return Ok(Some((package.to_string(), fetch)));
        }
    }
    Ok(None)
}

/// A locator `name@reference` split at the `@` that ends the name, which for
/// a scoped package is the second one.
fn split_locator(locator: &str) -> Option<(&str, &str)> {
    let at = locator.get(1..)?.find('@')? + 1;
    Some((&locator[..at], &locator[at + 1..]))
}

/// The code-running [`Fetch`] a Berry resolution reference names, if any.
///
/// - `exec:<file>`: the exec fetcher runs the file (carrick#2257).
/// - Git: Yarn's git resolver writes every git resolution, whatever URL form
///   the manifest used, as `<repo>#<query>` with a `commit` key in the query
///   (`GitUtils.resolveUrl`), and the GitHub fetcher reads the same form.
/// - `patch:<source>#<patch>`: the patch fetcher fetches the percent-encoded
///   source locator first, so the source's own fetch is what runs.
fn berry_fetch(reference: &str) -> Option<Fetch> {
    if let Some(patched) = reference.strip_prefix("patch:") {
        let source = patched.split('#').next().unwrap_or_default();
        let source = percent_encoding::percent_decode_str(source).decode_utf8_lossy();
        return split_locator(&source).and_then(|(_, reference)| berry_fetch(reference));
    }
    if reference.starts_with("exec:") {
        return Some(Fetch::Exec);
    }
    let names_a_commit = reference.split('#').skip(1).any(|fragment| {
        fragment
            .split('&')
            .any(|pair| pair.split('=').next() == Some("commit"))
    });
    names_a_commit.then_some(Fetch::Git)
}

/// The first package a classic `yarn.lock` resolves from a git repository
/// (carrick#2258). Classic Yarn fetches a locked entry with its git fetcher,
/// which prepares a dependency declaring `prepare` by running an install in
/// the clone with that clone's settings, `--ignore-scripts` or not. It picks
/// that fetcher for a `resolved` URL its git resolver claims: one matching its
/// `GIT_PATTERN_MATCHERS` (`git:`, `git+<protocol>:`, `ssh:`, an `http(s)` URL
/// ending in `.git`) or an `http(s)` URL of exactly two path segments on a
/// git host it knows. A hosted shorthand (`github:o/r`, `o/r`) that resolved
/// over HTTP is locked as a codeload tarball, which the tarball fetcher only
/// unpacks, so it is not refused.
fn classic_git_package(text: &str) -> Option<String> {
    let mut entry: Option<&str> = None;
    for line in text.lines() {
        if !line.starts_with([' ', '#']) && line.ends_with(':') {
            entry = Some(line);
            continue;
        }
        let Some(value) = line.trim_start().strip_prefix("resolved ") else {
            continue;
        };
        let url = value.trim().trim_matches('"');
        if classic_git_url(url.split('#').next().unwrap_or_default()) {
            let first = entry?.split(", ").next()?.trim_end_matches(':');
            let pattern = first.trim_matches('"');
            return Some(
                split_locator(pattern)
                    .map_or(pattern, |(name, _)| name)
                    .to_string(),
            );
        }
    }
    None
}

/// Classic Yarn's own test for a git `resolved` URL (`GitResolver.isVersion`
/// applied to the URL without its `#` hash).
fn classic_git_url(url: &str) -> bool {
    const GIT_HOSTS: [&str; 4] = ["github.com", "gitlab.com", "bitbucket.com", "bitbucket.org"];
    if url.starts_with("git:") || url.starts_with("ssh:") {
        return true;
    }
    if let Some(rest) = url.strip_prefix("git+")
        && rest.contains(':')
    {
        return true;
    }
    let Some(rest) = url
        .strip_prefix("https://")
        .or_else(|| url.strip_prefix("http://"))
    else {
        return false;
    };
    if url.ends_with(".git") {
        return true;
    }
    let (host, path) = rest.split_once('/').unwrap_or((rest, ""));
    let host = host.rsplit('@').next().unwrap_or(host);
    let host = host.split(':').next().unwrap_or(host);
    GIT_HOSTS.contains(&host) && path.split('/').filter(|part| !part.is_empty()).count() == 2
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
    fn a_git_resolution_refuses_whatever_url_form_or_wrapper_it_has() {
        for resolution in [
            "dep@git://127.0.0.1:9418/dep.git#commit=c70ff5cf5b7a1431a45e8510783ceafe50537f45",
            "dep@https://github.com/o/dep.git#commit=c70ff5cf5b7a1431a45e8510783ceafe50537f45",
            "dep@git+ssh://git@host/o/dep.git#workspace=a&commit=c70ff5cf5b7a1431a45e8510783ceafe50537f45",
            "@scope/dep@ssh://host/dep.git#commit=c70ff5cf5b7a1431a45e8510783ceafe50537f45",
            "dep@patch:dep@git%3A//host/dep.git%23commit%3Dc70ff5cf#~/.yarn/patches/dep.patch::version=1.0.0&hash=abc&locator=r%40workspace%3A.",
        ] {
            let lock = format!(
                "__metadata:\n  version: 8\n\n\"dep@x\":\n  version: 1.0.0\n  resolution: \"{resolution}\"\n"
            );
            let clone = clone_with(&[("svc/yarn.lock", &lock), ("svc/package.json", "{}")]);
            let reason = answer(&clone, "svc").unwrap();
            assert!(
                reason
                    .as_deref()
                    .is_some_and(|r| r.starts_with("svc/yarn.lock resolves ")
                        && r.contains("from a git repository")),
                "{resolution} -> {reason:?}"
            );
        }
        let scoped = clone_with(&[(
            "svc/yarn.lock",
            "\"@scope/dep@x\":\n  resolution: \"@scope/dep@git://h/d.git#commit=abc\"\n",
        )]);
        assert!(
            answer(&scoped, "svc")
                .unwrap()
                .is_some_and(|r| r.contains("resolves @scope/dep from"))
        );
    }

    #[test]
    fn registry_workspace_and_local_resolutions_are_not_git() {
        let lock = "__metadata:\n  version: 8\n  cacheKey: 10c0\n\n\
            \"a@npm:^1.0.0\":\n  resolution: \"a@npm:1.0.0\"\n  checksum: 10c0/abc\n\n\
            \"b@patch:b@npm%3A1.0.0#~/.yarn/patches/b.patch\":\n  resolution: \"b@patch:b@npm%3A1.0.0#~/.yarn/patches/b.patch::version=1.0.0&hash=d0&locator=r%40workspace%3A.\"\n\n\
            \"c@https://example.test/c-1.0.0.tgz\":\n  resolution: \"c@https://example.test/c-1.0.0.tgz\"\n\n\
            \"r@workspace:.\":\n  resolution: \"r@workspace:.\"\n  languageName: unknown\n";
        let clone = clone_with(&[("svc/yarn.lock", lock), ("svc/package.json", "{}")]);
        assert_eq!(answer(&clone, "svc").unwrap(), None);
        // No lockfile and an empty one resolve nothing.
        for lock in [None, Some("")] {
            let mut files = vec![("svc/package.json", "{}")];
            if let Some(lock) = lock {
                files.push(("svc/yarn.lock", lock));
            }
            assert_eq!(
                answer(&clone_with(&files), "svc").unwrap(),
                None,
                "{lock:?}"
            );
        }
    }

    #[test]
    fn an_exec_resolution_refuses_bare_or_patched() {
        for resolution in [
            "x@exec:./gen.js#./gen.js::hash=51ae02&locator=r%40workspace%3A.",
            "x@patch:x@exec%3A./gen.js%23./gen.js%3A%3Ahash%3D51ae02#~/.yarn/patches/x.patch::version=1.0.0&hash=d0&locator=r%40workspace%3A.",
        ] {
            let lock =
                format!("__metadata:\n  version: 8\n\n\"x@x\":\n  resolution: \"{resolution}\"\n");
            let clone = clone_with(&[("svc/yarn.lock", &lock)]);
            assert_eq!(
                answer(&clone, "svc").unwrap().as_deref(),
                Some(
                    "svc/yarn.lock resolves x with the exec: protocol, which runs a file from \
                     the repository to build the package"
                ),
                "{resolution}"
            );
        }
    }

    #[test]
    fn a_classic_git_resolution_refuses_in_every_form_classic_writes() {
        for resolved in [
            "git://127.0.0.1:9418/dep.git#9f674cc9dd277d1b2fbd3fab0ea75ef7ae905837",
            "git+ssh://git@github.com/o/dep.git#9f674cc9",
            "git+https://example.test/o/dep.git#9f674cc9",
            "git+file:///srv/dep#9f674cc9",
            "ssh://git@example.test/o/dep.git#9f674cc9",
            "https://example.test/o/dep.git#9f674cc9",
            "https://github.com/o/dep#9f674cc9",
            "https://user@gitlab.com/o/dep#9f674cc9",
        ] {
            let lock = format!(
                "# THIS IS AN AUTOGENERATED FILE. DO NOT EDIT THIS FILE DIRECTLY.\n# yarn lockfile v1\n\n\n\
                 left-pad@^1.3.0:\n  version \"1.3.0\"\n  resolved \"https://registry.yarnpkg.com/left-pad/-/left-pad-1.3.0.tgz#abc\"\n\n\
                 \"dep@x\", \"dep@y\":\n  version \"1.0.0\"\n  resolved \"{resolved}\"\n"
            );
            let clone = clone_with(&[("svc/yarn.lock", &lock)]);
            assert_eq!(
                answer(&clone, "svc").unwrap().as_deref(),
                Some(
                    "svc/yarn.lock resolves dep from a git repository, which Yarn prepares by \
                     running a package manager with that repository's own settings"
                ),
                "{resolved}"
            );
        }
        let scoped = clone_with(&[(
            "svc/yarn.lock",
            "# yarn lockfile v1\n\n\"@s/dep@git://h/d.git\":\n  resolved \"git://h/d.git#abc\"\n",
        )]);
        assert!(
            answer(&scoped, "svc")
                .unwrap()
                .is_some_and(|r| r.contains("resolves @s/dep from"))
        );
    }

    #[test]
    fn classic_registry_and_codeload_tarballs_are_not_git() {
        let lock = "# yarn lockfile v1\n\n\n\
            left-pad@^1.3.0:\n  version \"1.3.0\"\n  resolved \"https://registry.yarnpkg.com/left-pad/-/left-pad-1.3.0.tgz#abc\"\n\n\
            \"dep@github:o/dep\":\n  version \"1.0.0\"\n  resolved \"https://codeload.github.com/o/dep/tar.gz/9f674cc9\"\n\n\
            \"arch@https://github.com/o/arch/archive/v1.0.0.tar.gz\":\n  version \"1.0.0\"\n  resolved \"https://github.com/o/arch/archive/v1.0.0.tar.gz\"\n\n\
            local@file:./local:\n  version \"1.0.0\"\n";
        let clone = clone_with(&[("svc/yarn.lock", lock)]);
        assert_eq!(answer(&clone, "svc").unwrap(), None);
    }

    #[test]
    fn a_file_yarn_could_not_read_either_is_an_error() {
        for rc in ["plugins: [\n", "- plugins\n", "a: 1\n---\nplugins: []\n"] {
            let clone = clone_with(&[("svc/.yarnrc.yml", rc)]);
            assert!(answer(&clone, "svc").is_err(), "{rc:?}");
        }
        for lock in ["\"a@npm:1\": [\n", "- a\n"] {
            let clone = clone_with(&[("svc/yarn.lock", lock)]);
            assert!(answer(&clone, "svc").is_err(), "{lock:?}");
        }
        let clone = clone_with(&[]);
        fs::create_dir_all(clone.path().join("svc/.yarnrc.yml")).unwrap();
        assert!(
            answer(&clone, "svc").is_err(),
            "a directory named .yarnrc.yml"
        );
    }
}
