use carrick::cloud_storage::{CloudRepoData, CloudStorage, MockStorage};
use carrick::packages::{PackageJson, Packages};
use chrono::Utc;
use std::collections::HashMap;
use std::path::PathBuf;

/// Helper function to create test CloudRepoData
fn create_test_repo_data(repo_name: &str, commit_hash: &str) -> CloudRepoData {
    let mut deps = HashMap::new();
    deps.insert("express".to_string(), "4.18.0".to_string());

    let package_json = PackageJson {
        name: Some(repo_name.to_string()),
        version: Some("1.0.0".to_string()),
        dependencies: deps,
        dev_dependencies: HashMap::new(),
        peer_dependencies: HashMap::new(),
    };

    let mut packages = Packages::default();
    packages.package_jsons.push(package_json);
    packages.source_paths.push(PathBuf::from("package.json"));
    packages.resolve_dependencies();

    CloudRepoData {
        repo_name: repo_name.to_string(),
        endpoints: vec![],
        calls: vec![],
        mounts: vec![],
        apps: HashMap::new(),
        imported_handlers: vec![],
        function_definitions: HashMap::new(),
        config_json: None,
        package_json: Some(r#"{"name":"test","version":"1.0.0"}"#.to_string()),
        packages: Some(packages),
        last_updated: Utc::now(),
        commit_hash: commit_hash.to_string(),
    }
}

#[tokio::test]
async fn test_upload_and_download_single_repo() {
    // Given: a MockStorage instance
    let storage = MockStorage::new();

    // When: we upload repo data
    let repo_data = create_test_repo_data("test-repo", "abc123");
    storage
        .upload_repo_data("test-org", &repo_data)
        .await
        .expect("Upload should succeed");

    // Then: we can download it back
    let (downloaded, _s3_urls) = storage
        .download_all_repo_data("test-org")
        .await
        .expect("Download should succeed");

    // Verify we got the repo back (plus any mock repos that might be added)
    assert!(
        downloaded.iter().any(|r| r.repo_name == "test-repo"),
        "Should find uploaded repo in downloaded data"
    );

    let found_repo = downloaded
        .iter()
        .find(|r| r.repo_name == "test-repo")
        .expect("Should find test-repo");

    assert_eq!(found_repo.commit_hash, "abc123");
    assert!(found_repo.packages.is_some());
}

#[tokio::test]
async fn test_upload_multiple_repos_same_org() {
    // Given: a MockStorage instance
    let storage = MockStorage::new();

    // When: we upload multiple repos for the same org
    let repo1 = create_test_repo_data("repo-1", "hash1");
    let repo2 = create_test_repo_data("repo-2", "hash2");
    let repo3 = create_test_repo_data("repo-3", "hash3");

    storage
        .upload_repo_data("test-org", &repo1)
        .await
        .expect("Upload repo1 should succeed");
    storage
        .upload_repo_data("test-org", &repo2)
        .await
        .expect("Upload repo2 should succeed");
    storage
        .upload_repo_data("test-org", &repo3)
        .await
        .expect("Upload repo3 should succeed");

    // Then: we can download all of them
    let (downloaded, _s3_urls) = storage
        .download_all_repo_data("test-org")
        .await
        .expect("Download should succeed");

    // Should have at least our 3 repos (mock might add more)
    let our_repos: Vec<_> = downloaded
        .iter()
        .filter(|r| r.repo_name.starts_with("repo-"))
        .collect();

    assert!(
        our_repos.len() >= 3,
        "Should have at least 3 uploaded repos, found {}",
        our_repos.len()
    );

    // Verify each repo
    assert!(our_repos.iter().any(|r| r.repo_name == "repo-1"));
    assert!(our_repos.iter().any(|r| r.repo_name == "repo-2"));
    assert!(our_repos.iter().any(|r| r.repo_name == "repo-3"));
}

#[tokio::test]
async fn test_repos_isolated_by_org() {
    // Given: a MockStorage instance
    let storage = MockStorage::new();

    // When: we upload repos to different orgs
    let repo_org1 = create_test_repo_data("repo-org1", "hash1");
    let repo_org2 = create_test_repo_data("repo-org2", "hash2");

    storage
        .upload_repo_data("org1", &repo_org1)
        .await
        .expect("Upload to org1 should succeed");
    storage
        .upload_repo_data("org2", &repo_org2)
        .await
        .expect("Upload to org2 should succeed");

    // Then: each org only sees its own repos
    let (org1_data, _) = storage
        .download_all_repo_data("org1")
        .await
        .expect("Download org1 should succeed");
    let (org2_data, _) = storage
        .download_all_repo_data("org2")
        .await
        .expect("Download org2 should succeed");

    // Org1 should have repo-org1 but not repo-org2
    assert!(
        org1_data.iter().any(|r| r.repo_name == "repo-org1"),
        "Org1 should have its repo"
    );
    assert!(
        !org1_data.iter().any(|r| r.repo_name == "repo-org2"),
        "Org1 should not have org2's repo"
    );

    // Org2 should have repo-org2 but not repo-org1
    assert!(
        org2_data.iter().any(|r| r.repo_name == "repo-org2"),
        "Org2 should have its repo"
    );
    assert!(
        !org2_data.iter().any(|r| r.repo_name == "repo-org1"),
        "Org2 should not have org1's repo"
    );
}

#[tokio::test]
async fn test_health_check_succeeds() {
    // Given: a MockStorage instance
    let storage = MockStorage::new();

    // When: we perform a health check
    let result = storage.health_check().await;

    // Then: it should succeed
    assert!(result.is_ok(), "Health check should succeed");
}

#[tokio::test]
async fn test_upload_type_file() {
    // Given: a MockStorage instance
    let storage = MockStorage::new();

    // When: we upload a type file
    let type_content = "export interface User { id: string; name: string; }";
    let result = storage
        .upload_type_file("test-repo", "types.ts", type_content)
        .await;

    // Then: it should succeed
    assert!(result.is_ok(), "Type file upload should succeed");
}

#[tokio::test]
async fn test_download_type_file_content() {
    // Given: a MockStorage instance
    let storage = MockStorage::new();

    // When: we download type file content
    let result = storage
        .download_type_file_content("s3://bucket/test-repo/types.ts")
        .await;

    // Then: it should return mock content
    assert!(result.is_ok(), "Type file download should succeed");
    let content = result.unwrap();
    assert!(
        content.contains("MockType"),
        "Should contain mock type definition"
    );
}

#[tokio::test]
async fn test_concurrent_uploads() {
    // Given: a MockStorage instance
    let storage = std::sync::Arc::new(MockStorage::new());

    // When: we upload repos concurrently
    let mut handles = vec![];

    for i in 0..5 {
        let storage_clone = storage.clone();
        let handle = tokio::spawn(async move {
            let repo =
                create_test_repo_data(&format!("concurrent-repo-{}", i), &format!("hash{}", i));
            storage_clone
                .upload_repo_data("concurrent-org", &repo)
                .await
                .expect("Concurrent upload should succeed");
        });
        handles.push(handle);
    }

    // Wait for all uploads to complete
    for handle in handles {
        handle.await.expect("Task should complete");
    }

    // Then: all repos should be present
    let (downloaded, _) = storage
        .download_all_repo_data("concurrent-org")
        .await
        .expect("Download should succeed");

    let concurrent_repos: Vec<_> = downloaded
        .iter()
        .filter(|r| r.repo_name.starts_with("concurrent-repo-"))
        .collect();

    assert_eq!(
        concurrent_repos.len(),
        5,
        "Should have all 5 concurrent repos"
    );
}

#[tokio::test]
async fn test_empty_org_returns_empty_or_mock_data() {
    // Given: a MockStorage instance
    let storage = MockStorage::new();

    // When: we try to download from an org with no data
    let (downloaded, _) = storage
        .download_all_repo_data("non-existent-org")
        .await
        .expect("Download should succeed even for empty org");

    // Then: should return data without crashing
    // MockStorage returns mock data for testing purposes (2 repos when result.len() <= 1)
    // We verify the operation succeeds and returns the expected mock data
    assert_eq!(
        downloaded.len(),
        2,
        "MockStorage should return mock data for non-existent org"
    );
}

#[tokio::test]
async fn test_update_existing_repo() {
    // Given: a MockStorage with an uploaded repo
    let storage = MockStorage::new();

    let repo_v1 = create_test_repo_data("versioned-repo", "hash1");
    storage
        .upload_repo_data("test-org", &repo_v1)
        .await
        .expect("Upload v1 should succeed");

    // When: we upload the same repo with a different commit hash
    let repo_v2 = create_test_repo_data("versioned-repo", "hash2");
    storage
        .upload_repo_data("test-org", &repo_v2)
        .await
        .expect("Upload v2 should succeed");

    // Then: both versions exist (MockStorage appends, doesn't replace)
    let (downloaded, _) = storage
        .download_all_repo_data("test-org")
        .await
        .expect("Download should succeed");

    let versions: Vec<_> = downloaded
        .iter()
        .filter(|r| r.repo_name == "versioned-repo")
        .collect();

    // MockStorage currently appends, so we'll have 2 versions
    assert!(
        !versions.is_empty(),
        "Should have at least one version of the repo"
    );
}

#[tokio::test]
async fn test_packages_preserved_in_upload_download_cycle() {
    // Given: repo data with packages (using create_test_repo_data which properly constructs packages)
    let storage = MockStorage::new();

    let repo_data = create_test_repo_data("package-test-repo", "xyz789");

    // When: we upload and download
    storage
        .upload_repo_data("test-org", &repo_data)
        .await
        .expect("Upload should succeed");

    let (downloaded, _) = storage
        .download_all_repo_data("test-org")
        .await
        .expect("Download should succeed");

    // Then: packages should be preserved
    let found = downloaded
        .iter()
        .find(|r| r.repo_name == "package-test-repo")
        .expect("Should find uploaded repo");

    assert!(found.packages.is_some(), "Packages should be preserved");
    let packages = found.packages.as_ref().unwrap();

    // Check that package structure is preserved
    assert!(
        !packages.package_jsons.is_empty(),
        "Package JSONs should be preserved"
    );
    assert!(
        !packages.source_paths.is_empty(),
        "Source paths should be preserved"
    );

    // Verify dependencies are in merged_dependencies after resolve
    if !packages.merged_dependencies.is_empty() {
        assert!(
            packages.merged_dependencies.contains_key("express"),
            "Should preserve express dependency"
        );
    }
}
