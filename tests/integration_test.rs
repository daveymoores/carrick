use std::fs;
use std::path::PathBuf;
use std::process::Command;
use tempfile::TempDir;

// Test utilities for parsing new formatter output
struct TestOutput {
    raw_output: String,
    endpoint_count: usize,
    has_success: bool,
}

impl TestOutput {
    fn parse(stdout: &str) -> Self {
        let raw_output = stdout.to_string();

        // Extract endpoint count from "Analyzed **X endpoints**" in new formatted output
        let endpoint_count = stdout
            .lines()
            .find(|line| line.contains("Analyzed **") && line.contains(" endpoints**"))
            .and_then(|line| {
                // Parse "Analyzed **4 endpoints** and **5 API calls**"
                let parts: Vec<&str> = line.split("**").collect();
                if parts.len() >= 2 {
                    parts[1]
                        .split_whitespace()
                        .next()
                        .and_then(|s| s.parse().ok())
                } else {
                    None
                }
            })
            .unwrap_or(0);

        // Check for successful analysis (presence of CARRICK header)
        let has_success = stdout.contains("🪢 CARRICK: API Analysis Results");

        Self {
            raw_output,
            endpoint_count,
            has_success,
        }
    }

    fn contains_endpoint(&self, endpoint: &str) -> bool {
        self.raw_output.contains(endpoint)
    }

    fn contains_no_duplicates_bug(&self) -> bool {
        self.raw_output.contains("Unique endpoint paths: 0")
    }
}

#[test]
fn test_imported_router_endpoint_resolution() {
    // Create a temporary directory for the test
    let temp_dir = TempDir::new().expect("Failed to create temp directory");
    let test_project_path = temp_dir.path().join("test_project");

    // Copy the test fixture to the temporary directory
    let fixture_path =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/imported-routers");

    copy_dir_recursive(&fixture_path, &test_project_path).expect("Failed to copy test fixture");

    // Run carrick on the test project
    let output = Command::new(env!("CARGO_BIN_EXE_carrick"))
        .arg(test_project_path.to_str().unwrap())
        .env("CARRICK_MOCK_ALL", "1")
        .env("CARRICK_ORG", "test-org")
        .env("CARRICK_API_KEY", "mock")
        .output()
        .expect("Failed to execute carrick");

    let stdout = String::from_utf8(output.stdout).expect("Invalid UTF-8 in stdout");
    let stderr = String::from_utf8(output.stderr).expect("Invalid UTF-8 in stderr");

    // Print output for debugging if test fails
    if !output.status.success() {
        eprintln!("Command failed with status: {}", output.status);
        eprintln!("STDOUT:\n{}", stdout);
        eprintln!("STDERR:\n{}", stderr);
    }

    // The command should succeed
    assert!(output.status.success(), "Carrick command failed");

    // Parse the output using our test utilities
    let test_output = TestOutput::parse(&stdout);

    // Check that analysis completed successfully
    assert!(
        test_output.has_success,
        "Should have successful CARRICK analysis output. Output: {}",
        stdout
    );

    // We should find at least 10 endpoints (the exact number depends on our fixture)
    assert!(
        test_output.endpoint_count >= 10,
        "Expected at least 10 endpoints, but found {}. This suggests imported router resolution failed. Output: {}",
        test_output.endpoint_count,
        stdout
    );

    // Check that "Unique endpoint paths: 0" does NOT appear (this was the bug)
    assert!(
        !test_output.contains_no_duplicates_bug(),
        "Found 'Unique endpoint paths: 0' which indicates the imported router bug. Output: {}",
        stdout
    );

    // Verify specific endpoint paths are detected
    let expected_endpoints = [
        "/users",
        "/users/:id",
        "/api/v1/posts",
        "/api/v1/stats",
        "/api/v1/posts/:id",
        "/health/status",
        "/health/ping",
        "/health/ready",
    ];

    for endpoint in &expected_endpoints {
        assert!(
            test_output.contains_endpoint(endpoint),
            "Expected to find endpoint '{}' in output. This suggests imported router resolution failed. Output: {}",
            endpoint,
            stdout
        );
    }
}

#[test]
fn test_basic_endpoint_detection() {
    // Test with the existing test-repo to ensure we don't break existing functionality
    let test_repo_path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("test-repo");

    if !test_repo_path.exists() {
        panic!("test-repo directory not found at {:?}", test_repo_path);
    }

    let output = Command::new(env!("CARGO_BIN_EXE_carrick"))
        .arg(test_repo_path.to_str().unwrap())
        .env("CARRICK_MOCK_ALL", "1")
        .env("CARRICK_ORG", "test-org")
        .env("CARRICK_API_KEY", "mock")
        .output()
        .expect("Failed to execute carrick");

    let stdout = String::from_utf8(output.stdout).expect("Invalid UTF-8 in stdout");

    if !output.status.success() {
        let stderr = String::from_utf8(output.stderr).expect("Invalid UTF-8 in stderr");
        eprintln!("Command failed with status: {}", output.status);
        eprintln!("STDOUT:\n{}", stdout);
        eprintln!("STDERR:\n{}", stderr);
    }

    assert!(
        output.status.success(),
        "Carrick command failed on test-repo"
    );

    // Parse the output using our test utilities
    let test_output = TestOutput::parse(&stdout);

    // Check that analysis completed successfully
    assert!(
        test_output.has_success,
        "Should have successful CARRICK analysis output for test-repo. Output: {}",
        stdout
    );

    // test-repo should have 4 endpoints
    assert_eq!(
        test_output.endpoint_count, 4,
        "Expected 4 endpoints in test-repo, but found {}. Output: {}",
        test_output.endpoint_count, stdout
    );
}

// Helper function to recursively copy directories
fn copy_dir_recursive(src: &std::path::Path, dst: &std::path::Path) -> std::io::Result<()> {
    if !dst.exists() {
        fs::create_dir_all(dst)?;
    }

    for entry in fs::read_dir(src)? {
        let entry = entry?;
        let path = entry.path();
        let file_name = entry.file_name();
        let dest_path = dst.join(file_name);

        if path.is_dir() {
            copy_dir_recursive(&path, &dest_path)?;
        } else {
            fs::copy(&path, &dest_path)?;
        }
    }

    Ok(())
}

#[test]
fn test_no_duplicate_processing_regression() {
    // This test specifically checks that the same imported router file
    // doesn't get processed multiple times incorrectly
    let fixture_path =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/imported-routers");

    let output = Command::new(env!("CARGO_BIN_EXE_carrick"))
        .arg(fixture_path.to_str().unwrap())
        .env("CARRICK_MOCK_ALL", "1")
        .env("CARRICK_ORG", "test-org")
        .env("CARRICK_API_KEY", "mock")
        .output()
        .expect("Failed to execute carrick");

    let stdout = String::from_utf8(output.stdout).expect("Invalid UTF-8 in stdout");

    if !output.status.success() {
        let stderr = String::from_utf8(output.stderr).expect("Invalid UTF-8 in stderr");
        eprintln!("Command failed with status: {}", output.status);
        eprintln!("STDOUT:\n{}", stdout);
        eprintln!("STDERR:\n{}", stderr);
    }

    assert!(output.status.success(), "Carrick command failed");

    // Parse the output using our test utilities
    let test_output = TestOutput::parse(&stdout);

    // Check that analysis completed successfully
    assert!(
        test_output.has_success,
        "Should have successful CARRICK analysis output. Output: {}",
        stdout
    );

    // Count how many times each router file appears in the parsing logs
    let users_parse_count = stdout
        .matches("Parsing:")
        .filter(|line| line.contains("users.ts"))
        .count();
    let api_parse_count = stdout
        .matches("Parsing:")
        .filter(|line| line.contains("api.ts"))
        .count();
    let health_parse_count = stdout
        .matches("Parsing:")
        .filter(|line| line.contains("health.ts"))
        .count();

    // Each router file should be parsed exactly twice:
    // 1. Once when discovered as a regular file (with imported_router_name: None)
    // 2. Once when discovered as an imported router (with imported_router_name: Some(...))
    assert!(
        users_parse_count <= 2,
        "users.ts should be parsed at most 2 times, but was parsed {} times. This suggests duplicate processing.",
        users_parse_count
    );
    assert!(
        api_parse_count <= 2,
        "api.ts should be parsed at most 2 times, but was parsed {} times. This suggests duplicate processing.",
        api_parse_count
    );
    assert!(
        health_parse_count <= 2,
        "health.ts should be parsed at most 2 times, but was parsed {} times. This suggests duplicate processing.",
        health_parse_count
    );

    // Most importantly, we should NOT see "Unique endpoint paths: 0"
    assert!(
        !test_output.contains_no_duplicates_bug(),
        "Found 'Unique endpoint paths: 0' which indicates the imported router resolution bug has regressed. Output: {}",
        stdout
    );
}
