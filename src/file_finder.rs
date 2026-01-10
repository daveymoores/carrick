use std::path::{Path, PathBuf};
use walkdir::WalkDir;

const TEST_DIR_NAMES: &[&str] = &[
    "__tests__",
    "__mocks__",
    "__fixtures__",
    "tests",
    "test",
    "fixtures",
];

const TEST_FILE_SUFFIXES: &[&str] = &[
    ".test.ts",
    ".test.tsx",
    ".spec.ts",
    ".spec.tsx",
    ".test.js",
    ".test.jsx",
    ".spec.js",
    ".spec.jsx",
];

fn has_test_dir(path: &Path, root_dir: &Path) -> bool {
    // We only check for test directories relative to the root directory being scanned.
    // This allows scanning a directory that is itself inside a "tests" folder (e.g. fixtures).
    let relative_path = match path.strip_prefix(root_dir) {
        Ok(p) => p,
        Err(_) => return false,
    };

    relative_path.ancestors().any(|ancestor| {
        ancestor
            .file_name()
            .and_then(|name| name.to_str())
            .map(|name| {
                TEST_DIR_NAMES
                    .iter()
                    .any(|pattern| name.eq_ignore_ascii_case(pattern))
            })
            .unwrap_or(false)
    })
}

fn has_test_suffix(path: &Path) -> bool {
    path.file_name()
        .and_then(|name| name.to_str())
        .map(|name| {
            let lower = name.to_ascii_lowercase();
            TEST_FILE_SUFFIXES
                .iter()
                .any(|suffix| lower.ends_with(suffix))
        })
        .unwrap_or(false)
}

fn is_test_path(path: &Path, root_dir: &Path) -> bool {
    has_test_dir(path, root_dir) || has_test_suffix(path)
}

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
    let root_path = Path::new(dir);

    for entry in WalkDir::new(dir)
        .follow_links(true)
        .into_iter()
        .filter_map(|e| e.ok())
    {
        let path = entry.path();

        if !path.is_file() {
            continue;
        }

        if ignore_patterns
            .iter()
            .any(|pattern| path.to_string_lossy().contains(pattern))
        {
            continue;
        }

        if path.file_name().is_some_and(|name| name == "carrick.json") {
            config_file = Some(path.to_path_buf());
            continue;
        }

        if path.file_name().is_some_and(|name| name == "package.json") {
            package_json = Some(path.to_path_buf());
            continue;
        }

        if let Some(extension) = path.extension() {
            let ext_str = extension.to_string_lossy().to_lowercase();
            if matches!(ext_str.as_str(), "js" | "ts" | "jsx" | "tsx") {
                if is_test_path(path, root_path) {
                    continue;
                }
                js_ts_files.push(path.to_path_buf());
            }
        }
    }

    (js_ts_files, config_file, package_json)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs::{self, File};
    use tempfile::tempdir;

    #[test]
    fn find_files_skips_test_sources() {
        let tmp = tempdir().expect("temp dir");
        let root = tmp.path();

        fs::create_dir_all(root.join("src")).expect("src dir");
        fs::create_dir_all(root.join("__tests__")).expect("__tests__ dir");

        let source_file = root.join("src").join("app.ts");
        let test_file = root.join("__tests__").join("app.spec.ts");
        let config_path = root.join("carrick.json");
        let package_path = root.join("package.json");

        File::create(&source_file).expect("source file");
        File::create(&test_file).expect("test file");
        File::create(&config_path).expect("config file");
        File::create(&package_path).expect("package file");

        let (files, config, package) = find_files(root.to_str().unwrap(), &[]);

        assert_eq!(files, vec![source_file]);
        assert_eq!(config, Some(config_path));
        assert_eq!(package, Some(package_path));
    }

    #[test]
    fn find_files_skips_suffixes_and_fixture_dirs() {
        let tmp = tempdir().expect("temp dir");
        let root = tmp.path();

        fs::create_dir_all(root.join("services")).expect("services dir");
        fs::create_dir_all(root.join("fixtures").join("seeds")).expect("fixtures dir");

        let normal_file = root.join("services").join("handler.tsx");
        let suffix_file = root.join("services").join("handler.test.tsx");
        let fixture_file = root.join("fixtures").join("seeds").join("seed.ts");
        let config_path = root.join("carrick.json");
        let package_path = root.join("package.json");

        File::create(&normal_file).expect("normal file");
        File::create(&suffix_file).expect("suffix file");
        File::create(&fixture_file).expect("fixture file");
        File::create(&config_path).expect("config file");
        File::create(&package_path).expect("package file");

        let (files, config, package) = find_files(root.to_str().unwrap(), &[]);

        assert_eq!(files, vec![normal_file]);
        assert_eq!(config, Some(config_path));
        assert_eq!(package, Some(package_path));
    }

    #[test]
    fn find_files_allows_root_dir_matching_test_pattern() {
        let tmp = tempdir().expect("temp dir");
        // Create a root directory named "fixtures" which is normally excluded
        let root = tmp.path().join("fixtures");

        fs::create_dir_all(&root).expect("fixtures dir");
        fs::create_dir_all(root.join("src")).expect("src dir");

        let source_file = root.join("src").join("app.ts");
        let config_path = root.join("carrick.json");
        let package_path = root.join("package.json");

        File::create(&source_file).expect("source file");
        File::create(&config_path).expect("config file");
        File::create(&package_path).expect("package file");

        // Pass the "fixtures" directory as the root
        let (files, config, package) = find_files(root.to_str().unwrap(), &[]);

        assert_eq!(files, vec![source_file]);
        assert_eq!(config, Some(config_path));
        assert_eq!(package, Some(package_path));
    }
}
