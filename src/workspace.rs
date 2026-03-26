use serde::Deserialize;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone)]
pub struct WorkspacePackage {
    /// Package name from package.json "name" field
    pub name: String,
    /// Absolute path to the package directory
    pub path: PathBuf,
}

#[derive(Debug)]
pub struct WorkspaceInfo {
    pub is_monorepo: bool,
    pub packages: Vec<WorkspacePackage>,
}

#[derive(Deserialize)]
struct RootPackageJson {
    #[serde(default)]
    workspaces: WorkspacesField,
}

/// package.json "workspaces" can be an array of globs or an object with a "packages" array
#[derive(Deserialize, Default)]
#[serde(untagged)]
enum WorkspacesField {
    Array(Vec<String>),
    Object {
        #[serde(default)]
        packages: Vec<String>,
    },
    #[default]
    None,
}

impl WorkspacesField {
    fn patterns(&self) -> &[String] {
        match self {
            WorkspacesField::Array(v) => v,
            WorkspacesField::Object { packages } => packages,
            WorkspacesField::None => &[],
        }
    }
}

#[derive(Deserialize)]
struct PackageJsonName {
    name: Option<String>,
}

/// Detect whether the given repo path is a monorepo with workspace packages.
/// Reads the root package.json for a "workspaces" field and expands the glob patterns.
pub fn detect_workspace(repo_path: &str) -> WorkspaceInfo {
    let root = Path::new(repo_path);
    let pkg_path = root.join("package.json");

    let content = match std::fs::read_to_string(&pkg_path) {
        Ok(c) => c,
        Err(_) => {
            return WorkspaceInfo {
                is_monorepo: false,
                packages: vec![],
            };
        }
    };

    let root_pkg: RootPackageJson = match serde_json::from_str(&content) {
        Ok(p) => p,
        Err(_) => {
            return WorkspaceInfo {
                is_monorepo: false,
                packages: vec![],
            };
        }
    };

    let patterns = root_pkg.workspaces.patterns();
    if patterns.is_empty() {
        return WorkspaceInfo {
            is_monorepo: false,
            packages: vec![],
        };
    }

    let mut packages = Vec::new();

    for pattern in patterns {
        // Handle simple glob patterns like "apps/*" or "packages/*"
        // Strip trailing /* or /** to get the base directory
        let base_dir = pattern
            .trim_end_matches("/**")
            .trim_end_matches("/*")
            .trim_end_matches('/');

        let search_dir = root.join(base_dir);
        if !search_dir.is_dir() {
            continue;
        }

        let entries = match std::fs::read_dir(&search_dir) {
            Ok(e) => e,
            Err(_) => continue,
        };

        for entry in entries.flatten() {
            let entry_path = entry.path();
            if !entry_path.is_dir() {
                continue;
            }

            let pkg_json_path = entry_path.join("package.json");
            if !pkg_json_path.exists() {
                continue;
            }

            // Read the package name
            let pkg_content = match std::fs::read_to_string(&pkg_json_path) {
                Ok(c) => c,
                Err(_) => continue,
            };

            let pkg: PackageJsonName = match serde_json::from_str(&pkg_content) {
                Ok(p) => p,
                Err(_) => continue,
            };

            let name = pkg.name.unwrap_or_else(|| {
                entry_path
                    .file_name()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .to_string()
            });

            packages.push(WorkspacePackage {
                name,
                path: entry_path,
            });
        }
    }

    // Sort by name for deterministic ordering
    packages.sort_by(|a, b| a.name.cmp(&b.name));

    let is_monorepo = !packages.is_empty();
    if is_monorepo {
        println!(
            "[workspace] Detected monorepo with {} packages:",
            packages.len()
        );
        for pkg in &packages {
            println!("  - {} ({})", pkg.name, pkg.path.display());
        }
    }

    WorkspaceInfo {
        is_monorepo,
        packages,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    #[test]
    fn detect_workspace_with_array_workspaces() {
        let tmp = tempdir().expect("temp dir");
        let root = tmp.path();

        // Create root package.json with workspaces
        fs::write(
            root.join("package.json"),
            r#"{"name": "monorepo", "workspaces": ["apps/*"]}"#,
        )
        .unwrap();

        // Create two app packages
        let app_a = root.join("apps").join("app-a");
        fs::create_dir_all(&app_a).unwrap();
        fs::write(app_a.join("package.json"), r#"{"name": "app-a"}"#).unwrap();

        let app_b = root.join("apps").join("app-b");
        fs::create_dir_all(&app_b).unwrap();
        fs::write(app_b.join("package.json"), r#"{"name": "app-b"}"#).unwrap();

        let info = detect_workspace(root.to_str().unwrap());

        assert!(info.is_monorepo);
        assert_eq!(info.packages.len(), 2);
        assert_eq!(info.packages[0].name, "app-a");
        assert_eq!(info.packages[1].name, "app-b");
    }

    #[test]
    fn detect_workspace_not_monorepo() {
        let tmp = tempdir().expect("temp dir");
        let root = tmp.path();

        fs::write(
            root.join("package.json"),
            r#"{"name": "single-repo", "version": "1.0.0"}"#,
        )
        .unwrap();

        let info = detect_workspace(root.to_str().unwrap());

        assert!(!info.is_monorepo);
        assert!(info.packages.is_empty());
    }

    #[test]
    fn detect_workspace_no_package_json() {
        let tmp = tempdir().expect("temp dir");
        let info = detect_workspace(tmp.path().to_str().unwrap());

        assert!(!info.is_monorepo);
        assert!(info.packages.is_empty());
    }

    #[test]
    fn detect_workspace_with_multiple_patterns() {
        let tmp = tempdir().expect("temp dir");
        let root = tmp.path();

        fs::write(
            root.join("package.json"),
            r#"{"name": "monorepo", "workspaces": ["apps/*", "libs/*"]}"#,
        )
        .unwrap();

        let app = root.join("apps").join("my-app");
        fs::create_dir_all(&app).unwrap();
        fs::write(app.join("package.json"), r#"{"name": "my-app"}"#).unwrap();

        let lib = root.join("libs").join("my-lib");
        fs::create_dir_all(&lib).unwrap();
        fs::write(lib.join("package.json"), r#"{"name": "@scope/my-lib"}"#).unwrap();

        let info = detect_workspace(root.to_str().unwrap());

        assert!(info.is_monorepo);
        assert_eq!(info.packages.len(), 2);
        // Sorted by name: "@scope/my-lib" comes before "my-app"
        assert_eq!(info.packages[0].name, "@scope/my-lib");
        assert_eq!(info.packages[1].name, "my-app");
    }

    #[test]
    fn detect_workspace_skips_dirs_without_package_json() {
        let tmp = tempdir().expect("temp dir");
        let root = tmp.path();

        fs::write(
            root.join("package.json"),
            r#"{"name": "monorepo", "workspaces": ["apps/*"]}"#,
        )
        .unwrap();

        // One app with package.json
        let app = root.join("apps").join("real-app");
        fs::create_dir_all(&app).unwrap();
        fs::write(app.join("package.json"), r#"{"name": "real-app"}"#).unwrap();

        // One dir without package.json
        let no_pkg = root.join("apps").join("not-a-package");
        fs::create_dir_all(&no_pkg).unwrap();

        let info = detect_workspace(root.to_str().unwrap());

        assert!(info.is_monorepo);
        assert_eq!(info.packages.len(), 1);
        assert_eq!(info.packages[0].name, "real-app");
    }
}
