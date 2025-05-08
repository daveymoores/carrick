pub fn join_path_segments(segments: &[&str]) -> String {
    let mut result = String::new();
    for seg in segments {
        if seg.is_empty() {
            continue;
        }
        if !result.ends_with('/') && !result.is_empty() && !seg.starts_with('/') {
            result.push('/');
        }
        if result.ends_with('/') && seg.starts_with('/') {
            result.push_str(&seg[1..]);
        } else {
            result.push_str(seg);
        }
    }
    if !result.starts_with('/') {
        result.insert(0, '/');
    }
    result
}
