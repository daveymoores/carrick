use std::env;

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
