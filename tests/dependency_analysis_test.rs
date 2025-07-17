use carrick::analyzer::{Analyzer, ConflictSeverity};
use carrick::config::Config;
use carrick::packages::{PackageJson, Packages};
use std::collections::HashMap;
use std::path::PathBuf;
use swc_common::{SourceMap, sync::Lrc};

#[tokio::test]
async fn test_dependency_conflict_detection() {
    // Create analyzer
    let config = Config::default();
    let cm: Lrc<SourceMap> = Default::default();
    let mut analyzer = Analyzer::new(config, cm);

    // Create mock packages for repo-a with express 4.18.0
    let mut deps_a = HashMap::new();
    deps_a.insert("express".to_string(), "4.18.0".to_string());
    deps_a.insert("lodash".to_string(), "4.17.21".to_string());

    let package_json_a = PackageJson {
        name: Some("repo-a".to_string()),
        version: Some("1.0.0".to_string()),
        dependencies: deps_a,
        dev_dependencies: HashMap::new(),
        peer_dependencies: HashMap::new(),
    };

    let mut packages_a = Packages::default();
    packages_a.package_jsons.push(package_json_a);
    packages_a
        .source_paths
        .push(PathBuf::from("repo-a/package.json"));
    packages_a.resolve_dependencies();

    // Create mock packages for repo-b with express 3.17.0 (major version conflict!)
    let mut deps_b = HashMap::new();
    deps_b.insert("express".to_string(), "3.17.0".to_string());
    deps_b.insert("axios".to_string(), "1.3.0".to_string());

    let package_json_b = PackageJson {
        name: Some("repo-b".to_string()),
        version: Some("1.0.0".to_string()),
        dependencies: deps_b,
        dev_dependencies: HashMap::new(),
        peer_dependencies: HashMap::new(),
    };

    let mut packages_b = Packages::default();
    packages_b.package_jsons.push(package_json_b);
    packages_b
        .source_paths
        .push(PathBuf::from("repo-b/package.json"));
    packages_b.resolve_dependencies();

    // Add packages to analyzer
    analyzer.add_repo_packages("repo-a".to_string(), packages_a);
    analyzer.add_repo_packages("repo-b".to_string(), packages_b);

    // Run dependency analysis
    let conflicts = analyzer.analyze_dependencies();

    // Verify we found the express version conflict
    assert_eq!(conflicts.len(), 1);

    let express_conflict = &conflicts[0];
    assert_eq!(express_conflict.package_name, "express");
    assert_eq!(express_conflict.repos.len(), 2);

    // Check that both repos are represented with correct versions
    let repo_versions: HashMap<String, String> = express_conflict
        .repos
        .iter()
        .map(|repo| (repo.repo_name.clone(), repo.version.clone()))
        .collect();

    assert_eq!(repo_versions.get("repo-a"), Some(&"4.18.0".to_string()));
    assert_eq!(repo_versions.get("repo-b"), Some(&"3.17.0".to_string()));

    // Check that it's correctly identified as a critical conflict (major version difference)
    matches!(express_conflict.severity, ConflictSeverity::Critical);
}

#[tokio::test]
async fn test_no_dependency_conflicts_when_versions_match() {
    // Create analyzer
    let config = Config::default();
    let cm: Lrc<SourceMap> = Default::default();
    let mut analyzer = Analyzer::new(config, cm);

    // Create mock packages for repo-a with express 4.18.0
    let mut deps_a = HashMap::new();
    deps_a.insert("express".to_string(), "4.18.0".to_string());

    let package_json_a = PackageJson {
        name: Some("repo-a".to_string()),
        version: Some("1.0.0".to_string()),
        dependencies: deps_a,
        dev_dependencies: HashMap::new(),
        peer_dependencies: HashMap::new(),
    };

    let mut packages_a = Packages::default();
    packages_a.package_jsons.push(package_json_a);
    packages_a
        .source_paths
        .push(PathBuf::from("repo-a/package.json"));
    packages_a.resolve_dependencies();

    // Create mock packages for repo-b with same express version (no conflict)
    let mut deps_b = HashMap::new();
    deps_b.insert("express".to_string(), "4.18.0".to_string());

    let package_json_b = PackageJson {
        name: Some("repo-b".to_string()),
        version: Some("1.0.0".to_string()),
        dependencies: deps_b,
        dev_dependencies: HashMap::new(),
        peer_dependencies: HashMap::new(),
    };

    let mut packages_b = Packages::default();
    packages_b.package_jsons.push(package_json_b);
    packages_b
        .source_paths
        .push(PathBuf::from("repo-b/package.json"));
    packages_b.resolve_dependencies();

    // Add packages to analyzer
    analyzer.add_repo_packages("repo-a".to_string(), packages_a);
    analyzer.add_repo_packages("repo-b".to_string(), packages_b);

    // Run dependency analysis
    let conflicts = analyzer.analyze_dependencies();

    // Verify no conflicts found when versions match
    assert_eq!(conflicts.len(), 0);
}

#[tokio::test]
async fn test_no_conflicts_for_unique_packages() {
    // Create analyzer
    let config = Config::default();
    let cm: Lrc<SourceMap> = Default::default();
    let mut analyzer = Analyzer::new(config, cm);

    // Create mock packages for repo-a with lodash only
    let mut deps_a = HashMap::new();
    deps_a.insert("lodash".to_string(), "4.17.21".to_string());

    let package_json_a = PackageJson {
        name: Some("repo-a".to_string()),
        version: Some("1.0.0".to_string()),
        dependencies: deps_a,
        dev_dependencies: HashMap::new(),
        peer_dependencies: HashMap::new(),
    };

    let mut packages_a = Packages::default();
    packages_a.package_jsons.push(package_json_a);
    packages_a
        .source_paths
        .push(PathBuf::from("repo-a/package.json"));
    packages_a.resolve_dependencies();

    // Create mock packages for repo-b with axios only (no shared dependencies)
    let mut deps_b = HashMap::new();
    deps_b.insert("axios".to_string(), "1.3.0".to_string());

    let package_json_b = PackageJson {
        name: Some("repo-b".to_string()),
        version: Some("1.0.0".to_string()),
        dependencies: deps_b,
        dev_dependencies: HashMap::new(),
        peer_dependencies: HashMap::new(),
    };

    let mut packages_b = Packages::default();
    packages_b.package_jsons.push(package_json_b);
    packages_b
        .source_paths
        .push(PathBuf::from("repo-b/package.json"));
    packages_b.resolve_dependencies();

    // Add packages to analyzer
    analyzer.add_repo_packages("repo-a".to_string(), packages_a);
    analyzer.add_repo_packages("repo-b".to_string(), packages_b);

    // Run dependency analysis
    let conflicts = analyzer.analyze_dependencies();

    // Verify no conflicts found when packages are unique to each repo
    assert_eq!(conflicts.len(), 0);
}

#[tokio::test]
async fn test_severity_levels() {
    // Create analyzer
    let config = Config::default();
    let cm: Lrc<SourceMap> = Default::default();
    let mut analyzer = Analyzer::new(config, cm);

    // Create packages with different severity conflicts
    let mut deps_a = HashMap::new();
    deps_a.insert("critical_pkg".to_string(), "2.0.0".to_string()); // Major diff
    deps_a.insert("warning_pkg".to_string(), "1.2.0".to_string()); // Minor diff
    deps_a.insert("info_pkg".to_string(), "1.0.2".to_string()); // Patch diff

    let package_json_a = PackageJson {
        name: Some("repo-a".to_string()),
        version: Some("1.0.0".to_string()),
        dependencies: deps_a,
        dev_dependencies: HashMap::new(),
        peer_dependencies: HashMap::new(),
    };

    let mut packages_a = Packages::default();
    packages_a.package_jsons.push(package_json_a);
    packages_a
        .source_paths
        .push(PathBuf::from("repo-a/package.json"));
    packages_a.resolve_dependencies();

    let mut deps_b = HashMap::new();
    deps_b.insert("critical_pkg".to_string(), "1.0.0".to_string()); // Major diff
    deps_b.insert("warning_pkg".to_string(), "1.1.0".to_string()); // Minor diff
    deps_b.insert("info_pkg".to_string(), "1.0.1".to_string()); // Patch diff

    let package_json_b = PackageJson {
        name: Some("repo-b".to_string()),
        version: Some("1.0.0".to_string()),
        dependencies: deps_b,
        dev_dependencies: HashMap::new(),
        peer_dependencies: HashMap::new(),
    };

    let mut packages_b = Packages::default();
    packages_b.package_jsons.push(package_json_b);
    packages_b
        .source_paths
        .push(PathBuf::from("repo-b/package.json"));
    packages_b.resolve_dependencies();

    // Add packages to analyzer
    analyzer.add_repo_packages("repo-a".to_string(), packages_a);
    analyzer.add_repo_packages("repo-b".to_string(), packages_b);

    // Run dependency analysis
    let conflicts = analyzer.analyze_dependencies();

    // Should find 3 conflicts with different severities
    assert_eq!(conflicts.len(), 3);

    // Check severities
    for conflict in &conflicts {
        match conflict.package_name.as_str() {
            "critical_pkg" => assert!(matches!(conflict.severity, ConflictSeverity::Critical)),
            "warning_pkg" => assert!(matches!(conflict.severity, ConflictSeverity::Warning)),
            "info_pkg" => assert!(matches!(conflict.severity, ConflictSeverity::Info)),
            _ => panic!("Unexpected package: {}", conflict.package_name),
        }
    }
}
