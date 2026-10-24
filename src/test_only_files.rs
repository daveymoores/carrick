//! The files only tests import (carrick#2775).
//!
//! The scan leaves test sources unread ([`crate::file_finder::find_files`])
//! because they never serve production. A helper that builds its own app for
//! the integration suite is the same thing under a name that escapes the
//! path conventions: no test directory, no `.test` or `.spec` suffix. What
//! marks it is who imports it. A source file is test-only when it has at
//! least one importer and every importer is a test source or another
//! test-only file.
//!
//! No entry rule: a file nothing imports is never test-only, and a
//! production entry that only a test imports reads as test-only. Program
//! membership from the tsconfig is the stronger signal and is not used, for
//! the reason `file_finder` gives (carrick#588).

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use crate::workspace_resolver::WorkspaceIndex;

/// The files of `walked` that only tests import, as walked.
///
/// Importers are read from every source file below `repo_root` outside the
/// ignored folders, test sources and other services' files included, so a
/// sibling package's production code importing a helper (by relative path,
/// alias or workspace package name) keeps it in play. A specifier is read
/// wherever the file loads a module: `import`, `export ... from`,
/// `import x = require()`, and `require` or `import()` with a literal.
pub fn of(
    repo_root: &Path,
    walked: &[PathBuf],
    workspace: &WorkspaceIndex,
    ignore_patterns: &[&str],
) -> HashSet<PathBuf> {
    let canonical = |path: &Path| path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    let mut reader = crate::parser::ModuleReader::quiet();
    let mut importers: HashMap<PathBuf, HashSet<PathBuf>> = HashMap::new();
    let mut tests: HashSet<PathBuf> = HashSet::new();
    for file in crate::file_finder::source_files_below(repo_root, ignore_patterns) {
        let importer = canonical(&file);
        if crate::file_finder::is_test_source(&file, repo_root) {
            tests.insert(importer.clone());
        }
        for specifier in reader.loaded_specifiers(&file) {
            let Some(target) = workspace.resolve_module_path(&file, &specifier) else {
                continue;
            };
            let target = canonical(&target);
            if target != importer {
                importers
                    .entry(target)
                    .or_default()
                    .insert(importer.clone());
            }
        }
    }
    let mut only: HashSet<PathBuf> = HashSet::new();
    loop {
        let before = only.len();
        for (file, by) in &importers {
            if !tests.contains(file)
                && !only.contains(file)
                && by
                    .iter()
                    .all(|who| tests.contains(who) || only.contains(who))
            {
                only.insert(file.clone());
            }
        }
        if only.len() == before {
            break;
        }
    }
    walked
        .iter()
        .filter(|file| only.contains(&canonical(file)))
        .cloned()
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(root: &Path, relative: &str, text: &str) {
        let path = root.join(relative);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }

    fn only_in(files: &[(&str, &str)]) -> Vec<String> {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        for (relative, text) in files {
            write(&root, relative, text);
        }
        let workspace = WorkspaceIndex::build_with_aliases(&root, None);
        let walked: Vec<PathBuf> = files
            .iter()
            .map(|(relative, _)| root.join(relative))
            .filter(|path| !crate::file_finder::is_test_source(path, &root))
            .collect();
        let mut out: Vec<String> = of(&root, &walked, &workspace, &["node_modules"])
            .iter()
            .map(|path| path.strip_prefix(&root).unwrap().display().to_string())
            .collect();
        out.sort();
        out
    }

    /// A helper every importer of which is a test is test-only, through a
    /// helper it alone imports; one a production file also imports, by
    /// import, re-export or `require`, is not; and a file nothing imports is
    /// not.
    #[test]
    fn a_file_is_test_only_when_every_importer_is_a_test() {
        let got = only_in(&[
            ("src/a.integration.test.ts", "import './harness';\n"),
            ("src/harness.ts", "import './deep';\nexport const h = 1;\n"),
            ("src/deep.ts", "export const d = 1;\n"),
            ("src/b.test.ts", "import './shared';\nimport './barrel';\n"),
            ("src/shared.ts", "export const s = 1;\n"),
            ("src/prod.ts", "import { s } from './shared';\n"),
            ("src/barrel.ts", "export * from './reexported';\n"),
            ("src/reexported.ts", "export const r = 1;\n"),
            ("src/index.ts", "export * from './barrel';\n"),
            ("src/loaded.ts", "export const l = 1;\n"),
            ("src/c.test.ts", "const l = require('./loaded');\n"),
            ("src/loader.ts", "const l = require('./loaded');\n"),
            ("src/lonely.ts", "export const o = 1;\n"),
        ]);
        assert_eq!(got, vec!["src/deep.ts", "src/harness.ts"]);
    }
}
