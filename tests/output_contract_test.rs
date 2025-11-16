use carrick::analyzer::{Analyzer, ConflictSeverity, DependencyConflict};
use carrick::config::Config;
use carrick::packages::{PackageJson, Packages};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use swc_common::{SourceMap, sync::Lrc};

// Expected output structures matching our fixture JSON files
#[derive(Debug, Deserialize, Serialize)]
struct ExpectedDependencyConflicts {
    dependency_conflicts: Vec<ExpectedConflict>,
}

#[derive(Debug, Deserialize, Serialize)]
struct ExpectedConflict {
    package_name: String,
    versions: Vec<ExpectedVersion>,
    severity: String,
}

#[derive(Debug, Deserialize, Serialize)]
struct ExpectedVersion {
    repo: String,
    version: String,
}

// Helper function to load package.json from a fixture directory
fn load_packages_from_fixture(fixture_path: &Path, repo_name: &str) -> Packages {
    let package_json_path = fixture_path.join(repo_name).join("package.json");

    let content = fs::read_to_string(&package_json_path)
        .unwrap_or_else(|e| panic!("Failed to read {}: {}", package_json_path.display(), e));

    let package_json: PackageJson = serde_json::from_str(&content)
        .unwrap_or_else(|e| panic!("Failed to parse {}: {}", package_json_path.display(), e));

    let mut packages = Packages::default();
    packages.package_jsons.push(package_json);
    packages.source_paths.push(package_json_path);
    packages.resolve_dependencies();

    packages
}

// Helper function to load expected output from fixture
fn load_expected_output(fixture_path: &Path) -> ExpectedDependencyConflicts {
    let expected_path = fixture_path.join("expected-output.json");

    let content = fs::read_to_string(&expected_path)
        .unwrap_or_else(|e| panic!("Failed to read {}: {}", expected_path.display(), e));

    serde_json::from_str(&content)
        .unwrap_or_else(|e| panic!("Failed to parse {}: {}", expected_path.display(), e))
}

// Helper function to convert ConflictSeverity to string for comparison
fn severity_to_string(severity: &ConflictSeverity) -> String {
    match severity {
        ConflictSeverity::Critical => "Critical".to_string(),
        ConflictSeverity::Warning => "Warning".to_string(),
        ConflictSeverity::Info => "Info".to_string(),
    }
}

// Helper function to assert dependency conflicts match expected output
fn assert_conflicts_match(actual: &[DependencyConflict], expected: &ExpectedDependencyConflicts) {
    // Check we have the right number of conflicts
    assert_eq!(
        actual.len(),
        expected.dependency_conflicts.len(),
        "Expected {} conflicts but found {}",
        expected.dependency_conflicts.len(),
        actual.len()
    );

    // Build a map of actual conflicts by package name for easier comparison
    let actual_map: HashMap<String, &DependencyConflict> =
        actual.iter().map(|c| (c.package_name.clone(), c)).collect();

    // Verify each expected conflict
    for expected_conflict in &expected.dependency_conflicts {
        let actual_conflict = actual_map
            .get(&expected_conflict.package_name)
            .unwrap_or_else(|| {
                panic!(
                    "Expected conflict for package '{}' not found in actual results",
                    expected_conflict.package_name
                )
            });

        // Check severity matches
        assert_eq!(
            severity_to_string(&actual_conflict.severity),
            expected_conflict.severity,
            "Severity mismatch for package '{}'",
            expected_conflict.package_name
        );

        // Check versions match (order independent)
        assert_eq!(
            actual_conflict.repos.len(),
            expected_conflict.versions.len(),
            "Version count mismatch for package '{}'",
            expected_conflict.package_name
        );

        let actual_versions: HashMap<String, String> = actual_conflict
            .repos
            .iter()
            .map(|repo| (repo.repo_name.clone(), repo.version.clone()))
            .collect();

        for expected_version in &expected_conflict.versions {
            let actual_version = actual_versions
                .get(&expected_version.repo)
                .unwrap_or_else(|| {
                    panic!(
                        "Expected repo '{}' for package '{}' not found",
                        expected_version.repo, expected_conflict.package_name
                    )
                });

            assert_eq!(
                actual_version, &expected_version.version,
                "Version mismatch for package '{}' in repo '{}'",
                expected_conflict.package_name, expected_version.repo
            );
        }
    }
}

#[tokio::test]
async fn test_scenario_1_dependency_conflicts_output() {
    // Given: fixtures for scenario-1 with known dependency conflicts
    let fixture_path = PathBuf::from("tests/fixtures/scenario-1-dependency-conflicts");

    let config = Config::default();
    let cm: Lrc<SourceMap> = Default::default();
    let mut analyzer = Analyzer::new(config, cm);

    // Load packages from both repos
    let packages_a = load_packages_from_fixture(&fixture_path, "repo-a");
    let packages_b = load_packages_from_fixture(&fixture_path, "repo-b");

    analyzer.add_repo_packages("repo-a".to_string(), packages_a);
    analyzer.add_repo_packages("repo-b".to_string(), packages_b);

    // When: analyze dependencies
    let actual_conflicts = analyzer.analyze_dependencies();

    // Then: output matches expected conflicts exactly
    let expected = load_expected_output(&fixture_path);
    assert_conflicts_match(&actual_conflicts, &expected);
}

#[tokio::test]
async fn test_scenario_3_no_conflicts_output() {
    // Given: fixtures for scenario-3 with no conflicts (all versions match)
    let fixture_path = PathBuf::from("tests/fixtures/scenario-3-cross-repo-success");

    let config = Config::default();
    let cm: Lrc<SourceMap> = Default::default();
    let mut analyzer = Analyzer::new(config, cm);

    // Load packages from all three repos
    let packages_a = load_packages_from_fixture(&fixture_path, "repo-a");
    let packages_b = load_packages_from_fixture(&fixture_path, "repo-b");
    let packages_c = load_packages_from_fixture(&fixture_path, "repo-c");

    analyzer.add_repo_packages("repo-a".to_string(), packages_a);
    analyzer.add_repo_packages("repo-b".to_string(), packages_b);
    analyzer.add_repo_packages("repo-c".to_string(), packages_c);

    // When: analyze dependencies
    let actual_conflicts = analyzer.analyze_dependencies();

    // Then: no conflicts should be found
    assert_eq!(
        actual_conflicts.len(),
        0,
        "Expected no conflicts but found {}",
        actual_conflicts.len()
    );
}

#[tokio::test]
async fn test_dependency_conflict_severity_classification() {
    // Given: fixtures with major, minor, and patch version differences
    let fixture_path = PathBuf::from("tests/fixtures/scenario-1-dependency-conflicts");

    let config = Config::default();
    let cm: Lrc<SourceMap> = Default::default();
    let mut analyzer = Analyzer::new(config, cm);

    let packages_a = load_packages_from_fixture(&fixture_path, "repo-a");
    let packages_b = load_packages_from_fixture(&fixture_path, "repo-b");

    analyzer.add_repo_packages("repo-a".to_string(), packages_a);
    analyzer.add_repo_packages("repo-b".to_string(), packages_b);

    // When: analyze dependencies
    let conflicts = analyzer.analyze_dependencies();

    // Then: verify each conflict has the correct severity based on version diff
    let conflicts_by_package: HashMap<String, &DependencyConflict> = conflicts
        .iter()
        .map(|c| (c.package_name.clone(), c))
        .collect();

    // express: 5.0.0 vs 4.18.0 = Major version difference = Critical
    if let Some(express) = conflicts_by_package.get("express") {
        assert!(
            matches!(express.severity, ConflictSeverity::Critical),
            "express should be Critical (major version diff)"
        );
    }

    // react: 18.3.0 vs 18.2.0 = Minor version difference = Warning
    if let Some(react) = conflicts_by_package.get("react") {
        assert!(
            matches!(react.severity, ConflictSeverity::Warning),
            "react should be Warning (minor version diff)"
        );
    }

    // lodash: 4.17.22 vs 4.17.21 = Patch version difference = Info
    if let Some(lodash) = conflicts_by_package.get("lodash") {
        assert!(
            matches!(lodash.severity, ConflictSeverity::Info),
            "lodash should be Info (patch version diff)"
        );
    }
}

#[tokio::test]
async fn test_output_stability_across_analysis_runs() {
    // Given: same fixture analyzed multiple times
    let fixture_path = PathBuf::from("tests/fixtures/scenario-1-dependency-conflicts");

    let mut results = Vec::new();

    // When: run analysis 3 times
    for _ in 0..3 {
        let config = Config::default();
        let cm: Lrc<SourceMap> = Default::default();
        let mut analyzer = Analyzer::new(config, cm);

        let packages_a = load_packages_from_fixture(&fixture_path, "repo-a");
        let packages_b = load_packages_from_fixture(&fixture_path, "repo-b");

        analyzer.add_repo_packages("repo-a".to_string(), packages_a);
        analyzer.add_repo_packages("repo-b".to_string(), packages_b);

        let mut conflicts = analyzer.analyze_dependencies();
        // Sort by package name for deterministic comparison
        conflicts.sort_by(|a, b| a.package_name.cmp(&b.package_name));
        results.push(conflicts);
    }

    // Then: all runs should produce identical results (when sorted)
    let first_result = &results[0];
    for result in &results[1..] {
        assert_eq!(
            result.len(),
            first_result.len(),
            "Different runs produced different numbers of conflicts"
        );

        for (i, conflict) in result.iter().enumerate() {
            assert_eq!(
                conflict.package_name, first_result[i].package_name,
                "Package names differ between runs"
            );
            assert_eq!(
                conflict.repos.len(),
                first_result[i].repos.len(),
                "Repo counts differ for package '{}'",
                conflict.package_name
            );
        }
    }
}
