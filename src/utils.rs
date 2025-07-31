use std::env;
use std::path::{Path, PathBuf};

pub fn join_prefix_and_path(prefix: &str, path: &str) -> String {
    let prefix = prefix.trim_end_matches('/');
    let path = path.trim_start_matches('/');

    if prefix.is_empty() || prefix == "/" {
        format!("/{}", path)
    } else if path.is_empty() {
        prefix.to_string()
    } else {
        format!("{}/{}", prefix, path)
    }
}

/// Get repository name, checking GITHUB_REPOSITORY environment variable first
pub fn get_repository_name(repo_path: &str) -> String {
    // Check for GitHub Actions environment variable (format: "owner/repo")
    if let Ok(github_repo) = env::var("GITHUB_REPOSITORY") {
        if let Some(repo_name) = github_repo.split('/').last() {
            return repo_name.to_string();
        }
    }

    // Fall back to extracting from path
    let path_name = repo_path
        .split("/")
        .filter(|s| !s.is_empty())
        .last()
        .unwrap_or(".");

    // If we got "." (current directory), use the actual directory name
    if path_name == "." {
        if let Ok(current_dir) = env::current_dir() {
            if let Some(dir_name) = current_dir.file_name() {
                return dir_name.to_string_lossy().to_string();
            }
        }
    }

    path_name.to_string()
}

/// Resolves a relative import path to an absolute file path.
pub fn resolve_import_path(base_file: &Path, import_path: &str) -> Option<PathBuf> {
    if import_path.starts_with('.') {
        // It's a relative import
        let base_dir = base_file.parent()?;

        // Remove leading "./" or "../" but keep the path structure
        let normalized_path = if import_path.starts_with("./") {
            &import_path[2..]
        } else if import_path.starts_with("../") {
            let mut dir_path = base_dir.to_path_buf();
            dir_path.pop(); // Go up one directory for ../
            return resolve_import_path(&dir_path.join("dummy.js"), &import_path[3..]);
        } else {
            import_path
        };
        // Try different extensions and index files
        let extensions = ["", ".js", ".ts", ".jsx", ".tsx"];
        let index_extensions = ["/index.js", "/index.ts", "/index.jsx", "/index.tsx"];

        for ext in &extensions {
            let full_path = base_dir.join(format!("{}{}", normalized_path, ext));
            if full_path.exists() {
                return Some(full_path);
            }
        }

        // Try as directory with index file
        for index_ext in &index_extensions {
            let full_path = base_dir.join(format!("{}{}", normalized_path, index_ext));
            if full_path.exists() {
                return Some(full_path);
            }
        }

        // If we couldn't find the file with extensions, return the base path anyway
        // The file might be in a different format or require more complex resolution
        Some(base_dir.join(normalized_path))
    } else {
        // Non-relative imports (e.g., 'express', 'cors') - not local files
        None
    }
}
