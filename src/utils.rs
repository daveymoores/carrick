use std::env;

pub fn join_prefix_and_path(prefix: &str, path: &str) -> String {
    let prefix = prefix.trim_end_matches('/');
    let path = path.trim_start_matches('/');

    if prefix.is_empty() || prefix == "/" {
        format!("/{}", path)
    } else if path.is_empty() {
        format!("{}", prefix)
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
    repo_path
        .split("/")
        .filter(|s| !s.is_empty())
        .last()
        .expect("repo_suffix not found")
        .to_string()
}
