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
