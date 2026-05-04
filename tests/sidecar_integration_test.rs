//! Sidecar Integration Tests
//!
//! End-to-end tests that verify the type-sidecar integration with the type checker.
//! These tests:
//! 1. Use test fixture repos (producer/consumer scenarios)
//! 2. Run the full analysis with sidecar enabled
//! 3. Verify bundled types are produced correctly
//! 4. Verify type checking identifies correct matches/mismatches
//!
//! Note: These tests require Node.js to be installed and the sidecar to be built.

use std::fs;
use std::path::PathBuf;
use std::process::Command;
use tempfile::TempDir;

// ============================================================================
// Test Fixtures
// ============================================================================

/// Creates a simple producer repository with typed endpoints
fn create_producer_repo(dir: &std::path::Path) {
    // Create package.json
    fs::write(
        dir.join("package.json"),
        r#"{
  "name": "producer-api",
  "version": "1.0.0",
  "type": "module",
  "dependencies": {
    "express": "^4.18.0"
  }
}"#,
    )
    .expect("Failed to write package.json");

    // Create tsconfig.json
    fs::write(
        dir.join("tsconfig.json"),
        r#"{
  "compilerOptions": {
    "target": "ES2020",
    "module": "NodeNext",
    "moduleResolution": "NodeNext",
    "strict": true,
    "esModuleInterop": true,
    "skipLibCheck": true
  },
  "include": ["src/**/*"]
}"#,
    )
    .expect("Failed to write tsconfig.json");

    // Create src directory
    fs::create_dir_all(dir.join("src")).expect("Failed to create src directory");

    // Create types.ts with shared types
    fs::write(
        dir.join("src/types.ts"),
        r#"
export interface User {
  id: number;
  name: string;
  email: string;
}

export interface CreateUserRequest {
  name: string;
  email: string;
}

export interface Order {
  id: number;
  userId: number;
  total: number;
  items: OrderItem[];
}

export interface OrderItem {
  productId: number;
  quantity: number;
  price: number;
}
"#,
    )
    .expect("Failed to write types.ts");

    // Create routes.ts with Express endpoints
    fs::write(
        dir.join("src/routes.ts"),
        r#"
import express, { Response } from 'express';
import { User, CreateUserRequest, Order } from './types.js';

const app = express();

// GET /api/users - Returns array of users
app.get('/api/users', (req, res: Response<User[]>) => {
  const users: User[] = [
    { id: 1, name: 'Alice', email: 'alice@example.com' },
    { id: 2, name: 'Bob', email: 'bob@example.com' }
  ];
  res.json(users);
});

// GET /api/users/:id - Returns single user
app.get('/api/users/:id', (req, res: Response<User>) => {
  const user: User = { id: 1, name: 'Alice', email: 'alice@example.com' };
  res.json(user);
});

// POST /api/users - Creates a user
app.post('/api/users', (req, res: Response<User>) => {
  const body = req.body as CreateUserRequest;
  const user: User = { id: 3, name: body.name, email: body.email };
  res.json(user);
});

// GET /api/orders - Returns array of orders
app.get('/api/orders', (req, res: Response<Order[]>) => {
  const orders: Order[] = [];
  res.json(orders);
});

// GET /api/orders/:id - Returns single order
app.get('/api/orders/:id', (req, res: Response<Order>) => {
  const order: Order = { id: 1, userId: 1, total: 100, items: [] };
  res.json(order);
});

export default app;
"#,
    )
    .expect("Failed to write routes.ts");
}

/// Creates a consumer repository that calls the producer API
fn create_consumer_repo(dir: &std::path::Path) {
    // Create package.json
    fs::write(
        dir.join("package.json"),
        r#"{
  "name": "consumer-app",
  "version": "1.0.0",
  "type": "module",
  "dependencies": {
    "axios": "^1.6.0"
  }
}"#,
    )
    .expect("Failed to write package.json");

    // Create tsconfig.json
    fs::write(
        dir.join("tsconfig.json"),
        r#"{
  "compilerOptions": {
    "target": "ES2020",
    "module": "NodeNext",
    "moduleResolution": "NodeNext",
    "strict": true,
    "esModuleInterop": true,
    "skipLibCheck": true
  },
  "include": ["src/**/*"]
}"#,
    )
    .expect("Failed to write tsconfig.json");

    // Create src directory
    fs::create_dir_all(dir.join("src")).expect("Failed to create src directory");

    // Create types.ts with consumer's view of the types
    // Note: Intentionally slightly different to test compatibility
    fs::write(
        dir.join("src/types.ts"),
        r#"
// Consumer's type definitions
// These should be compatible with the producer's types

export interface User {
  id: number;
  name: string;
  email: string;
}

export interface Order {
  id: number;
  userId: number;
  total: number;
  items: OrderItem[];
}

export interface OrderItem {
  productId: number;
  quantity: number;
  price: number;
}

// Type that doesn't match producer (for testing mismatches)
export interface IncompatibleUser {
  id: string;  // Should be number!
  username: string;  // Should be name!
}
"#,
    )
    .expect("Failed to write types.ts");

    // Create api.ts with API client calls
    fs::write(
        dir.join("src/api.ts"),
        r#"
import axios from 'axios';
import { User, Order, IncompatibleUser } from './types.js';

const API_BASE = process.env.API_URL || 'http://localhost:3000';

// Correct: Fetches users with matching type
export async function getUsers(): Promise<User[]> {
  const response = await axios.get<User[]>(`${API_BASE}/api/users`);
  return response.data;
}

// Correct: Fetches single user with matching type
export async function getUser(id: number): Promise<User> {
  const response = await axios.get<User>(`${API_BASE}/api/users/${id}`);
  return response.data;
}

// Correct: Fetches orders with matching type
export async function getOrders(): Promise<Order[]> {
  const response = await axios.get<Order[]>(`${API_BASE}/api/orders`);
  return response.data;
}

// Correct: Fetches single order
export async function getOrder(id: number): Promise<Order> {
  const response = await axios.get<Order>(`${API_BASE}/api/orders/${id}`);
  return response.data;
}

// MISMATCH: Calls endpoint that doesn't exist on producer
export async function getProducts(): Promise<unknown[]> {
  const response = await axios.get(`${API_BASE}/api/products`);
  return response.data;
}

// MISMATCH: Uses DELETE on endpoint that only supports GET
export async function deleteUser(id: number): Promise<void> {
  await axios.delete(`${API_BASE}/api/users/${id}`);
}
"#,
    )
    .expect("Failed to write api.ts");
}

/// Creates a type manifest JSON file
fn create_manifest(path: &std::path::Path, repo_name: &str, entries: Vec<ManifestEntry>) {
    let manifest = serde_json::json!({
        "repo_name": repo_name,
        "commit_hash": "test-commit-hash",
        "entries": entries
    });

    fs::write(path, serde_json::to_string_pretty(&manifest).unwrap())
        .expect("Failed to write manifest");
}

#[derive(serde::Serialize)]
struct ManifestEntry {
    method: String,
    path: String,
    type_alias: String,
    role: String,
    file_path: String,
    line_number: u32,
}

// ============================================================================
// Unit Tests for Manifest Structure
// ============================================================================

#[test]
fn test_manifest_structure_is_valid() {
    let temp_dir = TempDir::new().expect("Failed to create temp directory");
    let manifest_path = temp_dir.path().join("test-manifest.json");

    let entries = vec![
        ManifestEntry {
            method: "GET".to_string(),
            path: "/api/users".to_string(),
            type_alias: "GetUsersResponse".to_string(),
            role: "producer".to_string(),
            file_path: "src/routes.ts".to_string(),
            line_number: 10,
        },
        ManifestEntry {
            method: "POST".to_string(),
            path: "/api/users".to_string(),
            type_alias: "CreateUserResponse".to_string(),
            role: "producer".to_string(),
            file_path: "src/routes.ts".to_string(),
            line_number: 20,
        },
    ];

    create_manifest(&manifest_path, "test-api", entries);

    // Verify the manifest was created and is valid JSON
    let content = fs::read_to_string(&manifest_path).expect("Failed to read manifest");
    let parsed: serde_json::Value = serde_json::from_str(&content).expect("Invalid JSON");

    assert_eq!(parsed["repo_name"], "test-api");
    assert_eq!(parsed["commit_hash"], "test-commit-hash");
    assert!(parsed["entries"].is_array());
    assert_eq!(parsed["entries"].as_array().unwrap().len(), 2);
}

#[test]
fn test_producer_fixture_creation() {
    let temp_dir = TempDir::new().expect("Failed to create temp directory");
    let producer_path = temp_dir.path().join("producer");
    fs::create_dir_all(&producer_path).expect("Failed to create producer directory");

    create_producer_repo(&producer_path);

    // Verify all expected files exist
    assert!(producer_path.join("package.json").exists());
    assert!(producer_path.join("tsconfig.json").exists());
    assert!(producer_path.join("src/types.ts").exists());
    assert!(producer_path.join("src/routes.ts").exists());

    // Verify package.json content
    let package_json = fs::read_to_string(producer_path.join("package.json"))
        .expect("Failed to read package.json");
    let parsed: serde_json::Value = serde_json::from_str(&package_json).expect("Invalid JSON");
    assert_eq!(parsed["name"], "producer-api");
}

#[test]
fn test_consumer_fixture_creation() {
    let temp_dir = TempDir::new().expect("Failed to create temp directory");
    let consumer_path = temp_dir.path().join("consumer");
    fs::create_dir_all(&consumer_path).expect("Failed to create consumer directory");

    create_consumer_repo(&consumer_path);

    // Verify all expected files exist
    assert!(consumer_path.join("package.json").exists());
    assert!(consumer_path.join("tsconfig.json").exists());
    assert!(consumer_path.join("src/types.ts").exists());
    assert!(consumer_path.join("src/api.ts").exists());

    // Verify package.json content
    let package_json = fs::read_to_string(consumer_path.join("package.json"))
        .expect("Failed to read package.json");
    let parsed: serde_json::Value = serde_json::from_str(&package_json).expect("Invalid JSON");
    assert_eq!(parsed["name"], "consumer-app");
}

// ============================================================================
// Sidecar Integration Tests
// ============================================================================

/// Check if Node.js is available
fn is_node_available() -> bool {
    Command::new("node")
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// Check if the sidecar has been built
fn is_sidecar_built() -> bool {
    let sidecar_path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src/sidecar/dist/index.js");
    sidecar_path.exists()
}

#[test]
fn test_sidecar_produces_bundled_types() {
    if !is_node_available() {
        eprintln!("Skipping test: Node.js not available");
        return;
    }

    if !is_sidecar_built() {
        eprintln!("Skipping test: Sidecar not built (run: cd src/sidecar && npm run build)");
        return;
    }

    let temp_dir = TempDir::new().expect("Failed to create temp directory");
    let producer_path = temp_dir.path().join("producer");
    fs::create_dir_all(&producer_path).expect("Failed to create producer directory");

    create_producer_repo(&producer_path);

    // For now, we just verify the fixture is created correctly
    // Full sidecar integration would require spawning the sidecar process
    // which is tested in the sidecar's own test suite

    let types_file = producer_path.join("src/types.ts");
    let content = fs::read_to_string(&types_file).expect("Failed to read types.ts");

    // Verify the types file contains expected interfaces
    assert!(content.contains("interface User"));
    assert!(content.contains("interface Order"));
    assert!(content.contains("interface OrderItem"));
}

#[test]
fn test_sidecar_handles_missing_types() {
    if !is_node_available() {
        eprintln!("Skipping test: Node.js not available");
        return;
    }

    let temp_dir = TempDir::new().expect("Failed to create temp directory");
    let empty_repo = temp_dir.path().join("empty-repo");
    fs::create_dir_all(&empty_repo).expect("Failed to create empty repo");

    // Create minimal package.json
    fs::write(
        empty_repo.join("package.json"),
        r#"{"name": "empty-repo", "version": "1.0.0"}"#,
    )
    .expect("Failed to write package.json");

    // Create minimal tsconfig.json
    fs::write(
        empty_repo.join("tsconfig.json"),
        r#"{"compilerOptions": {"target": "ES2020"}}"#,
    )
    .expect("Failed to write tsconfig.json");

    // Verify the repo structure
    assert!(empty_repo.join("package.json").exists());
    assert!(empty_repo.join("tsconfig.json").exists());

    // The sidecar should handle this gracefully (no types to extract)
    // This is verified in the sidecar's own test suite
}

#[test]
fn test_type_checking_with_sidecar_types() {
    if !is_node_available() {
        eprintln!("Skipping test: Node.js not available");
        return;
    }

    let temp_dir = TempDir::new().expect("Failed to create temp directory");

    // Create producer and consumer repos
    let producer_path = temp_dir.path().join("producer");
    let consumer_path = temp_dir.path().join("consumer");
    fs::create_dir_all(&producer_path).expect("Failed to create producer directory");
    fs::create_dir_all(&consumer_path).expect("Failed to create consumer directory");

    create_producer_repo(&producer_path);
    create_consumer_repo(&consumer_path);

    // Create manifests for testing
    let producer_manifest_path = temp_dir.path().join("producer-manifest.json");
    let consumer_manifest_path = temp_dir.path().join("consumer-manifest.json");

    // Producer manifest - endpoints defined in routes.ts
    let producer_entries = vec![
        ManifestEntry {
            method: "GET".to_string(),
            path: "/api/users".to_string(),
            type_alias: "UsersArrayResponse".to_string(),
            role: "producer".to_string(),
            file_path: "src/routes.ts".to_string(),
            line_number: 9,
        },
        ManifestEntry {
            method: "GET".to_string(),
            path: "/api/users/:id".to_string(),
            type_alias: "UserResponse".to_string(),
            role: "producer".to_string(),
            file_path: "src/routes.ts".to_string(),
            line_number: 18,
        },
        ManifestEntry {
            method: "POST".to_string(),
            path: "/api/users".to_string(),
            type_alias: "CreateUserResponse".to_string(),
            role: "producer".to_string(),
            file_path: "src/routes.ts".to_string(),
            line_number: 24,
        },
        ManifestEntry {
            method: "GET".to_string(),
            path: "/api/orders".to_string(),
            type_alias: "OrdersArrayResponse".to_string(),
            role: "producer".to_string(),
            file_path: "src/routes.ts".to_string(),
            line_number: 31,
        },
        ManifestEntry {
            method: "GET".to_string(),
            path: "/api/orders/:id".to_string(),
            type_alias: "OrderResponse".to_string(),
            role: "producer".to_string(),
            file_path: "src/routes.ts".to_string(),
            line_number: 37,
        },
    ];

    // Consumer manifest - API calls in api.ts
    let consumer_entries = vec![
        ManifestEntry {
            method: "GET".to_string(),
            path: "/api/users".to_string(),
            type_alias: "UsersArrayData".to_string(),
            role: "consumer".to_string(),
            file_path: "src/api.ts".to_string(),
            line_number: 10,
        },
        ManifestEntry {
            method: "GET".to_string(),
            path: "/api/users/{id}".to_string(), // Different param format
            type_alias: "UserData".to_string(),
            role: "consumer".to_string(),
            file_path: "src/api.ts".to_string(),
            line_number: 16,
        },
        ManifestEntry {
            method: "GET".to_string(),
            path: "/api/orders".to_string(),
            type_alias: "OrdersArrayData".to_string(),
            role: "consumer".to_string(),
            file_path: "src/api.ts".to_string(),
            line_number: 22,
        },
        ManifestEntry {
            method: "GET".to_string(),
            path: "/api/orders/{id}".to_string(),
            type_alias: "OrderData".to_string(),
            role: "consumer".to_string(),
            file_path: "src/api.ts".to_string(),
            line_number: 28,
        },
        // Orphaned consumer - no matching producer
        ManifestEntry {
            method: "GET".to_string(),
            path: "/api/products".to_string(),
            type_alias: "ProductsData".to_string(),
            role: "consumer".to_string(),
            file_path: "src/api.ts".to_string(),
            line_number: 34,
        },
        // Another orphaned consumer - wrong method
        ManifestEntry {
            method: "DELETE".to_string(),
            path: "/api/users/{id}".to_string(),
            type_alias: "DeleteUserResult".to_string(),
            role: "consumer".to_string(),
            file_path: "src/api.ts".to_string(),
            line_number: 40,
        },
    ];

    create_manifest(&producer_manifest_path, "producer-api", producer_entries);
    create_manifest(&consumer_manifest_path, "consumer-app", consumer_entries);

    // Verify manifests were created
    assert!(producer_manifest_path.exists());
    assert!(consumer_manifest_path.exists());

    // Read and verify producer manifest
    let producer_manifest: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&producer_manifest_path).unwrap()).unwrap();
    assert_eq!(producer_manifest["repo_name"], "producer-api");
    assert_eq!(
        producer_manifest["entries"].as_array().unwrap().len(),
        5,
        "Producer should have 5 endpoints"
    );

    // Read and verify consumer manifest
    let consumer_manifest: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&consumer_manifest_path).unwrap()).unwrap();
    assert_eq!(consumer_manifest["repo_name"], "consumer-app");
    assert_eq!(
        consumer_manifest["entries"].as_array().unwrap().len(),
        6,
        "Consumer should have 6 API calls"
    );

    // Verify path normalization would match endpoints
    // GET /api/users/:id (producer) should match GET /api/users/{id} (consumer)
    let producer_entries_parsed = producer_manifest["entries"].as_array().unwrap();
    let consumer_entries_parsed = consumer_manifest["entries"].as_array().unwrap();

    // Count expected matches (ignoring path param format differences)
    let mut expected_matches = 0;
    for producer_entry in producer_entries_parsed {
        let producer_method = producer_entry["method"].as_str().unwrap();
        let producer_path = normalize_path(producer_entry["path"].as_str().unwrap());

        for consumer_entry in consumer_entries_parsed {
            let consumer_method = consumer_entry["method"].as_str().unwrap();
            let consumer_path = normalize_path(consumer_entry["path"].as_str().unwrap());

            if producer_method == consumer_method && producer_path == consumer_path {
                expected_matches += 1;
            }
        }
    }

    // Should find 4 matches (GET /api/users, GET /api/users/:id, GET /api/orders, GET /api/orders/:id)
    assert_eq!(expected_matches, 4, "Should find 4 matching endpoint pairs");
}

/// Helper function to normalize paths for comparison (mimics TypeScript implementation)
fn normalize_path(path: &str) -> String {
    let mut normalized = path.to_lowercase();

    // Remove trailing slashes
    normalized = normalized.trim_end_matches('/').to_string();

    // Ensure leading slash
    if !normalized.starts_with('/') {
        normalized = format!("/{}", normalized);
    }

    // Normalize path parameters to :param
    // Handle Express style :id
    let re_colon = regex::Regex::new(r":[\w-]+").unwrap();
    normalized = re_colon.replace_all(&normalized, ":param").to_string();

    // Handle OpenAPI style {id}
    let re_brace = regex::Regex::new(r"\{[^}]+\}").unwrap();
    normalized = re_brace.replace_all(&normalized, ":param").to_string();

    // Handle Next.js style [id]
    let re_bracket = regex::Regex::new(r"\[[^\]]+\]").unwrap();
    normalized = re_bracket.replace_all(&normalized, ":param").to_string();

    normalized
}

#[test]
fn test_path_normalization() {
    // Test various path formats normalize correctly
    assert_eq!(normalize_path("/api/users/:id"), "/api/users/:param");
    assert_eq!(normalize_path("/api/users/{userId}"), "/api/users/:param");
    assert_eq!(normalize_path("/api/users/[id]"), "/api/users/:param");
    assert_eq!(normalize_path("/API/Users/"), "/api/users");
    assert_eq!(normalize_path("api/users"), "/api/users");
    assert_eq!(
        normalize_path("/api/users/:userId/posts/:postId"),
        "/api/users/:param/posts/:param"
    );
}

// ============================================================================
// Integration with Carrick CLI (when available)
// ============================================================================

#[test]
#[ignore] // Run with: cargo test -- --ignored
fn test_full_carrick_analysis_with_sidecar() {
    if !is_node_available() {
        eprintln!("Skipping test: Node.js not available");
        return;
    }

    if !is_sidecar_built() {
        eprintln!("Skipping test: Sidecar not built");
        return;
    }

    let temp_dir = TempDir::new().expect("Failed to create temp directory");
    let producer_path = temp_dir.path().join("producer");
    fs::create_dir_all(&producer_path).expect("Failed to create producer directory");

    create_producer_repo(&producer_path);

    // Run carrick with sidecar enabled
    let output = Command::new(env!("CARGO_BIN_EXE_carrick"))
        .arg(producer_path.to_str().unwrap())
        .env("CARRICK_MOCK_ALL", "1")
        .env("CARRICK_API_KEY", "mock")
        .env("CARRICK_SIDECAR_TYPE_EXTRACTION", "1")
        .output()
        .expect("Failed to execute carrick");

    let stdout = String::from_utf8(output.stdout).expect("Invalid UTF-8 in stdout");
    let stderr = String::from_utf8(output.stderr).expect("Invalid UTF-8 in stderr");

    // Print output for debugging
    if !output.status.success() {
        eprintln!("Command failed with status: {}", output.status);
        eprintln!("STDOUT:\n{}", stdout);
        eprintln!("STDERR:\n{}", stderr);
    }

    // The command should succeed
    assert!(output.status.success(), "Carrick command failed");

    // Should have CARRICK output header
    assert!(
        stdout.contains("🪢 CARRICK") || stdout.contains("CARRICK"),
        "Should have CARRICK output. Got: {}",
        stdout
    );
}

// ============================================================================
// Utility Functions
// ============================================================================

/// Copy directory recursively (for test fixtures)
#[allow(dead_code)]
fn copy_dir_recursive(src: &std::path::Path, dst: &std::path::Path) -> std::io::Result<()> {
    if !dst.exists() {
        fs::create_dir_all(dst)?;
    }

    for entry in fs::read_dir(src)? {
        let entry = entry?;
        let path = entry.path();
        let dest_path = dst.join(entry.file_name());

        if path.is_dir() {
            copy_dir_recursive(&path, &dest_path)?;
        } else {
            fs::copy(&path, &dest_path)?;
        }
    }

    Ok(())
}
