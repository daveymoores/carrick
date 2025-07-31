use std::path::PathBuf;
use walkdir::WalkDir;

/// Find all JavaScript and TypeScript files in a directory
/// Also looks for carrick.json configuration file
/// Returns (js_ts_files, config_file_option)
pub fn find_files(
    dir: &str,
    ignore_patterns: &[&str],
) -> (Vec<PathBuf>, Option<PathBuf>, Option<PathBuf>) {
    let mut js_ts_files = Vec::new();
    let mut config_file = None;
    let mut package_json = None;

    for entry in WalkDir::new(dir)
        .follow_links(true)
        .into_iter()
        .filter_map(|e| e.ok())
    {
        let path = entry.path();

        // Check if it's a file
        if path.is_file() {
            // Check if path matches any ignore pattern for all file types
            let should_ignore = ignore_patterns
                .iter()
                .any(|pattern| path.to_string_lossy().contains(pattern));

            if should_ignore {
                continue;
            }

            // Check if it's carrick.json
            if path.file_name().is_some_and(|name| name == "carrick.json") {
                config_file = Some(path.to_path_buf());
                continue;
            }
            // Get the package.json file
            if path.file_name().is_some_and(|name| name == "package.json") {
                package_json = Some(path.to_path_buf());
                continue;
            }

            // Check if it's a JS/TS file
            if let Some(extension) = path.extension() {
                let ext_str = extension.to_string_lossy().to_lowercase();
                if ext_str == "js" || ext_str == "ts" || ext_str == "jsx" || ext_str == "tsx" {
                    js_ts_files.push(path.to_path_buf());
                }
            }
        }
    }

    (js_ts_files, config_file, package_json)
}
