use std::path::PathBuf;
use walkdir::WalkDir;

/// Find all JavaScript and TypeScript files in a directory
pub fn find_files(dir: &str, ignore_patterns: &[&str]) -> Vec<PathBuf> {
    let mut files = Vec::new();

    for entry in WalkDir::new(dir)
        .follow_links(true)
        .into_iter()
        .filter_map(|e| e.ok())
    {
        let path = entry.path();

        // Check if it's a file with JS/TS extension
        if path.is_file() {
            if let Some(extension) = path.extension() {
                let ext_str = extension.to_string_lossy().to_lowercase();
                if ext_str == "js" || ext_str == "ts" || ext_str == "jsx" || ext_str == "tsx" {
                    // Check if path matches any ignore pattern
                    if !ignore_patterns
                        .iter()
                        .any(|pattern| path.to_string_lossy().contains(pattern))
                    {
                        files.push(path.to_path_buf());
                    }
                }
            }
        }
    }

    files
}
