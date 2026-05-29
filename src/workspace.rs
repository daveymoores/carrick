//! Monorepo workspace support.
//!
//! Two concerns live here, both file-only (no `nx`/`turbo`/package-manager
//! invocation):
//!
//! 1. **Config-driven app discovery** — expand the `projects` globs from the
//!    root `carrick.json` into concrete app directories. Each becomes its own
//!    analysis unit, keyed `<repo>::<app>` for the cross-repo engine.
//! 2. **Marker detection** — a shallow check for `nx.json` / `turbo.json` /
//!    `pnpm-workspace.yaml` / `package.json#workspaces` so we can *nudge* the
//!    user toward configuring `projects` when they haven't (Phase 2).
//!
//! Guiding principle (see docs/research/monorepo-support.md): incomplete is
//! fine, silently incorrect is not. A `projects` glob that matches nothing, or
//! a matched directory with no `package.json`, is surfaced as a warning rather
//! than dropped silently — see `ProjectExpansion::warnings`.

use crate::packages::PackageJson;
use std::path::{Path, PathBuf};

/// One deployable app within a monorepo, discovered from `projects` config.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkspacePackage {
    /// Bare app name (from the app's `package.json#name`, falling back to the
    /// directory name). Used for the composite repo key and for display.
    pub name: String,
    /// Absolute (or repo-relative) path to the app directory.
    pub path: PathBuf,
}

/// Result of expanding the configured `projects` globs against the filesystem.
///
/// `warnings` carries the honesty signal: globs that matched nothing and
/// matched directories that turned out not to be packages. Callers surface
/// these so a misconfiguration never looks like "analyzed, all clean".
#[derive(Debug, Default, Clone)]
pub struct ProjectExpansion {
    pub packages: Vec<WorkspacePackage>,
    pub warnings: Vec<String>,
}

/// Which monorepo tool a repo looks like it uses. Detection only; drives the
/// suggestion text, not behavior.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MonorepoMarker {
    Nx,
    Turborepo,
    PnpmWorkspace,
    NpmWorkspaces,
}

impl MonorepoMarker {
    /// Human-readable label for the suggestion message.
    pub fn label(self) -> &'static str {
        match self {
            MonorepoMarker::Nx => "Nx",
            MonorepoMarker::Turborepo => "Turborepo",
            MonorepoMarker::PnpmWorkspace => "pnpm workspace",
            MonorepoMarker::NpmWorkspaces => "npm/Yarn workspaces",
        }
    }
}

/// Shallow check for monorepo markers at the repo root. Returns every marker
/// found (a repo can be e.g. both Turborepo and pnpm). Reads at most a handful
/// of files; does not walk the tree.
pub fn detect_markers(repo_path: &str) -> Vec<MonorepoMarker> {
    let root = Path::new(repo_path);
    let mut markers = Vec::new();

    if root.join("nx.json").is_file() {
        markers.push(MonorepoMarker::Nx);
    }
    if root.join("turbo.json").is_file() {
        markers.push(MonorepoMarker::Turborepo);
    }
    if root.join("pnpm-workspace.yaml").is_file() || root.join("pnpm-workspace.yml").is_file() {
        markers.push(MonorepoMarker::PnpmWorkspace);
    }
    if root_package_json_has_workspaces(root) {
        markers.push(MonorepoMarker::NpmWorkspaces);
    }

    markers
}

/// True if the root `package.json` declares a non-empty `workspaces` field
/// (array form or `{ "packages": [...] }` object form).
fn root_package_json_has_workspaces(root: &Path) -> bool {
    let Ok(content) = std::fs::read_to_string(root.join("package.json")) else {
        return false;
    };
    let Ok(value) = serde_json::from_str::<serde_json::Value>(&content) else {
        return false;
    };
    match value.get("workspaces") {
        Some(serde_json::Value::Array(a)) => !a.is_empty(),
        Some(serde_json::Value::Object(o)) => o
            .get("packages")
            .and_then(|p| p.as_array())
            .map(|a| !a.is_empty())
            .unwrap_or(false),
        _ => false,
    }
}

/// Expand the configured `projects` patterns into app directories.
///
/// Each pattern is a path or a simple one-level glob (`apps/*`, `services/*`),
/// relative to `repo_path`. Patterns are matched against immediate child
/// directories of the glob's base. Nested (`**`) and negation (`!`) patterns
/// are not supported and are reported as warnings rather than silently doing
/// something surprising.
pub fn expand_projects(repo_path: &str, patterns: &[String]) -> ProjectExpansion {
    let root = Path::new(repo_path);
    let mut expansion = ProjectExpansion::default();
    let mut seen: std::collections::HashSet<PathBuf> = std::collections::HashSet::new();

    for pattern in patterns {
        let pattern = pattern.trim();
        if pattern.is_empty() {
            continue;
        }
        if pattern.starts_with('!') {
            expansion.warnings.push(format!(
                "Ignoring unsupported negation pattern in `projects`: '{}'",
                pattern
            ));
            continue;
        }
        if pattern.contains("**") {
            expansion.warnings.push(format!(
                "Ignoring unsupported nested glob in `projects`: '{}' (only simple one-level globs like 'apps/*' are supported)",
                pattern
            ));
            continue;
        }

        let matched_dirs = if let Some(base) = pattern.strip_suffix("/*") {
            // Glob: every immediate child directory of `base`.
            let search_dir = root.join(base);
            if !search_dir.is_dir() {
                expansion.warnings.push(format!(
                    "`projects` pattern '{}' matched nothing: directory '{}' does not exist",
                    pattern, base
                ));
                continue;
            }
            let mut dirs: Vec<PathBuf> = match std::fs::read_dir(&search_dir) {
                Ok(entries) => entries
                    .flatten()
                    .map(|e| e.path())
                    .filter(|p| p.is_dir())
                    .collect(),
                Err(e) => {
                    expansion.warnings.push(format!(
                        "`projects` pattern '{}' could not read directory '{}': {}",
                        pattern, base, e
                    ));
                    continue;
                }
            };
            dirs.sort();
            if dirs.is_empty() {
                expansion.warnings.push(format!(
                    "`projects` pattern '{}' matched no subdirectories",
                    pattern
                ));
            }
            dirs
        } else {
            // Literal path.
            let dir = root.join(pattern);
            if !dir.is_dir() {
                expansion.warnings.push(format!(
                    "`projects` entry '{}' matched nothing: '{}' is not a directory",
                    pattern, pattern
                ));
                continue;
            }
            vec![dir]
        };

        for dir in matched_dirs {
            if !seen.insert(dir.clone()) {
                continue;
            }
            match read_package_name(&dir) {
                Some(name) => expansion
                    .packages
                    .push(WorkspacePackage { name, path: dir }),
                None => {
                    // A matched directory with no package.json is not an app.
                    // Surface it loudly — never drop it silently.
                    let display = dir
                        .strip_prefix(root)
                        .unwrap_or(&dir)
                        .to_string_lossy()
                        .to_string();
                    expansion.warnings.push(format!(
                        "Skipping '{}': matched by `projects` but has no package.json",
                        display
                    ));
                }
            }
        }
    }

    expansion.packages.sort_by(|a, b| a.name.cmp(&b.name));
    expansion
}

/// Read a directory's app name from `package.json#name`, falling back to the
/// directory name. Returns `None` only when there is no `package.json` at all.
fn read_package_name(dir: &Path) -> Option<String> {
    let pkg_path = dir.join("package.json");
    if !pkg_path.is_file() {
        return None;
    }
    let dir_name = || {
        dir.file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| "unknown".to_string())
    };
    match std::fs::read_to_string(&pkg_path) {
        Ok(content) => match serde_json::from_str::<PackageJson>(&content) {
            Ok(pkg) => Some(pkg.name.unwrap_or_else(dir_name)),
            // Malformed package.json: still a package directory; use dir name.
            Err(_) => Some(dir_name()),
        },
        Err(_) => Some(dir_name()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    fn write(path: &Path, contents: &str) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, contents).unwrap();
    }

    #[test]
    fn expand_glob_collects_apps_with_package_json() {
        let tmp = tempdir().unwrap();
        let root = tmp.path();
        write(
            &root.join("apps/orders/package.json"),
            r#"{"name": "orders-service"}"#,
        );
        write(
            &root.join("apps/billing/package.json"),
            r#"{"name": "billing-service"}"#,
        );

        let exp = expand_projects(root.to_str().unwrap(), &["apps/*".to_string()]);
        assert_eq!(exp.packages.len(), 2);
        // Sorted by name.
        assert_eq!(exp.packages[0].name, "billing-service");
        assert_eq!(exp.packages[1].name, "orders-service");
        assert!(exp.warnings.is_empty());
    }

    #[test]
    fn name_falls_back_to_dir_when_unnamed() {
        let tmp = tempdir().unwrap();
        let root = tmp.path();
        write(&root.join("apps/web/package.json"), r#"{"private": true}"#);

        let exp = expand_projects(root.to_str().unwrap(), &["apps/*".to_string()]);
        assert_eq!(exp.packages.len(), 1);
        assert_eq!(exp.packages[0].name, "web");
    }

    #[test]
    fn literal_path_is_matched() {
        let tmp = tempdir().unwrap();
        let root = tmp.path();
        write(
            &root.join("services/billing/package.json"),
            r#"{"name": "billing"}"#,
        );

        let exp = expand_projects(root.to_str().unwrap(), &["services/billing".to_string()]);
        assert_eq!(exp.packages.len(), 1);
        assert_eq!(exp.packages[0].name, "billing");
    }

    #[test]
    fn dir_without_package_json_warns_not_dropped() {
        let tmp = tempdir().unwrap();
        let root = tmp.path();
        write(&root.join("apps/real/package.json"), r#"{"name": "real"}"#);
        fs::create_dir_all(root.join("apps/not-a-package")).unwrap();

        let exp = expand_projects(root.to_str().unwrap(), &["apps/*".to_string()]);
        assert_eq!(exp.packages.len(), 1);
        assert_eq!(exp.packages[0].name, "real");
        assert_eq!(exp.warnings.len(), 1);
        assert!(exp.warnings[0].contains("not-a-package"));
        assert!(exp.warnings[0].contains("no package.json"));
    }

    #[test]
    fn glob_matching_nothing_warns() {
        let tmp = tempdir().unwrap();
        let root = tmp.path();
        // apps/ doesn't exist at all.
        let exp = expand_projects(root.to_str().unwrap(), &["apps/*".to_string()]);
        assert!(exp.packages.is_empty());
        assert_eq!(exp.warnings.len(), 1);
        assert!(exp.warnings[0].contains("does not exist"));
    }

    #[test]
    fn empty_glob_dir_warns() {
        let tmp = tempdir().unwrap();
        let root = tmp.path();
        fs::create_dir_all(root.join("apps")).unwrap();
        let exp = expand_projects(root.to_str().unwrap(), &["apps/*".to_string()]);
        assert!(exp.packages.is_empty());
        assert_eq!(exp.warnings.len(), 1);
        assert!(exp.warnings[0].contains("no subdirectories"));
    }

    #[test]
    fn unsupported_patterns_warn() {
        let tmp = tempdir().unwrap();
        let root = tmp.path();
        let exp = expand_projects(
            root.to_str().unwrap(),
            &["packages/**".to_string(), "!apps/legacy".to_string()],
        );
        assert!(exp.packages.is_empty());
        assert_eq!(exp.warnings.len(), 2);
        assert!(exp.warnings.iter().any(|w| w.contains("nested glob")));
        assert!(exp.warnings.iter().any(|w| w.contains("negation")));
    }

    #[test]
    fn duplicate_matches_deduped() {
        let tmp = tempdir().unwrap();
        let root = tmp.path();
        write(&root.join("apps/web/package.json"), r#"{"name": "web"}"#);

        let exp = expand_projects(
            root.to_str().unwrap(),
            &["apps/*".to_string(), "apps/web".to_string()],
        );
        assert_eq!(exp.packages.len(), 1);
    }

    #[test]
    fn detect_markers_finds_each_kind() {
        let tmp = tempdir().unwrap();
        let root = tmp.path();
        fs::write(root.join("nx.json"), "{}").unwrap();
        let markers = detect_markers(root.to_str().unwrap());
        assert_eq!(markers, vec![MonorepoMarker::Nx]);

        fs::write(root.join("turbo.json"), "{}").unwrap();
        let markers = detect_markers(root.to_str().unwrap());
        assert!(markers.contains(&MonorepoMarker::Turborepo));
        assert!(markers.contains(&MonorepoMarker::Nx));
    }

    #[test]
    fn detect_markers_reads_package_json_workspaces() {
        let tmp = tempdir().unwrap();
        let root = tmp.path();
        fs::write(
            root.join("package.json"),
            r#"{"name": "root", "workspaces": ["apps/*"]}"#,
        )
        .unwrap();
        let markers = detect_markers(root.to_str().unwrap());
        assert_eq!(markers, vec![MonorepoMarker::NpmWorkspaces]);
    }

    #[test]
    fn detect_markers_object_workspaces() {
        let tmp = tempdir().unwrap();
        let root = tmp.path();
        fs::write(
            root.join("package.json"),
            r#"{"workspaces": {"packages": ["apps/*"]}}"#,
        )
        .unwrap();
        let markers = detect_markers(root.to_str().unwrap());
        assert_eq!(markers, vec![MonorepoMarker::NpmWorkspaces]);
    }

    #[test]
    fn detect_markers_empty_when_none() {
        let tmp = tempdir().unwrap();
        let root = tmp.path();
        fs::write(root.join("package.json"), r#"{"name": "solo"}"#).unwrap();
        assert!(detect_markers(root.to_str().unwrap()).is_empty());
    }
}
