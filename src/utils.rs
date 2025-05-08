pub fn join_path_segments(segments: Vec<String>) -> String {
    // Join all segments, ensuring correct handling of slashes
    let mut result = String::new();
    for segment in segments {
        if segment.is_empty() {
            continue;
        }

        // Remove leading/trailing slashes from segment
        let segment = segment.trim_matches('/');
        if segment.is_empty() {
            continue;
        }

        // Add a slash between segments if needed
        if !result.is_empty() && !result.ends_with('/') {
            result.push('/');
        }

        result.push_str(segment);
    }

    // Ensure path starts with a slash
    if !result.starts_with('/') {
        result = format!("/{}", result);
    }

    result
}
