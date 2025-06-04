# Testing & CI/CD Setup

This document describes the comprehensive testing setup and CI/CD pipeline for Carrick, including the critical bug fix for imported router endpoint resolution.

## 🐛 Bug Fix: Imported Router Endpoint Resolution

### Problem
The API analyzer was failing to extract endpoints from imported Express.js routers, resulting in `Unique endpoint paths: 0` instead of the expected endpoint count. This was a critical issue that prevented proper API analysis in real-world applications.

### Root Cause
The issue was in the file processing logic:

1. **Initial Discovery**: Files were discovered and processed with `imported_router_name: None`
2. **Import Detection**: During processing, imported routers were detected and queued again with `imported_router_name: Some("routerName")`
3. **Duplicate Prevention Bug**: The duplicate file detection logic prevented reprocessing, causing endpoint owner names to remain generic
4. **Mount Mismatch**: This resulted in endpoints having owner `OwnerType::Router("repo:router")` but mounts expecting `OwnerType::Router("repo:routerName")`, breaking path resolution

### Solution
Modified the duplicate detection logic to include the imported router name in the processing key:

```rust
let processing_key = match &imported_router_name {
    Some(name) => format!("{}#{}", path_str, name),
    None => path_str.clone(),
};
```

This allows the same file to be processed twice:
1. Once as a generic file (context: `None`)
2. Once as an imported router (context: `Some("routerName")`)

### Results
- **Before**: `Unique endpoint paths: 0`
- **After**: Proper endpoint detection (e.g., 6 endpoints in optaxe project, 10 in test fixture)

## 🧪 Test Suite

### Integration Tests (`tests/integration_test.rs`)

#### 1. `test_imported_router_endpoint_resolution`
- **Purpose**: Verifies imported router endpoints are correctly detected and resolved
- **Fixture**: `tests/fixtures/imported-routers/` - realistic Express.js app with multiple imported routers
- **Validation**:
  - At least 10 endpoints detected
  - No `Unique endpoint paths: 0` regression
  - Specific endpoint paths present (e.g., `/users/:id`, `/api/v1/posts`, `/health/status`)

#### 2. `test_basic_endpoint_detection`
- **Purpose**: Ensures existing functionality still works
- **Target**: `test-repo/` fixture
- **Validation**: Exactly 4 endpoints detected

#### 3. `test_no_duplicate_processing_regression`
- **Purpose**: Specifically catches the duplicate processing bug
- **Validation**:
  - Each router file parsed at most twice
  - No `Unique endpoint paths: 0` regression

### Test Fixtures

#### `tests/fixtures/imported-routers/`
Comprehensive test case that reproduces the original bug:

- **`app.ts`**: Main application that imports and mounts routers
- **`routes/users.ts`**: User management endpoints (3 routes)
- **`routes/api.ts`**: API endpoints with nested paths (4 routes)
- **`routes/health.ts`**: Health check endpoints (3 routes)
- **Total**: 10 endpoints across multiple routers with complex mounting patterns

## 🚀 CI/CD Pipeline (`.github/workflows/ci.yml`)

### Jobs

#### 1. Test Suite
- Runs unit and integration tests
- Installs Node.js dependencies for test fixtures
- Uses latest GitHub Actions (checkout@v4, cache@v4, setup-node@v4)

#### 2. Linting
- Code formatting check (`cargo fmt`)
- Clippy linting (`cargo clippy`)

#### 3. Build
- Debug and release builds
- Dependency caching for faster builds

#### 4. **Endpoint Detection Regression Test** ⭐
Critical job that specifically tests the bug fix:

```bash
# Test basic functionality
./target/release/carrick ./test-repo/ > test_output.txt 2>&1
if ! grep -q "Found 4 endpoints across all files" test_output.txt; then
  echo "ERROR: Expected to find 4 endpoints in test-repo"
  exit 1
fi

# Test imported router resolution (the main bug fix)
./target/release/carrick ./tests/fixtures/imported-routers/ > imported_test_output.txt 2>&1
if grep -q "Unique endpoint paths: 0" imported_test_output.txt; then
  echo "ERROR: Imported router resolution bug has regressed!"
  exit 1
fi
```

#### 5. Security Audit
- Runs `cargo audit` for dependency vulnerability scanning

### Triggers
- Every push to `main` or `develop`
- Every pull request to `main` or `develop`

### Artifacts
- Test outputs uploaded as artifacts for debugging
- Available for 90 days after workflow run

## 🛠️ Local Testing

### Quick Test
```bash
# Run integration tests
cargo test --test integration_test

# Test the specific bug fix
cargo run -- ./tests/fixtures/imported-routers/
# Should show: "Found 10 endpoints across all files"
# Should NOT show: "Unique endpoint paths: 0"
```

### Comprehensive Local Test
```bash
# Run the local workflow script
./test_workflow_local.sh
```

This script mimics the GitHub Actions workflow locally:
- ✅ Code formatting and linting
- ✅ Build (debug + release)
- ✅ Dependencies installation
- ✅ Unit and integration tests
- ✅ Endpoint detection regression tests
- ✅ Security audit (if cargo-audit installed)

## 📊 Test Coverage

### What's Tested
1. **Basic endpoint detection**: Simple Express.js apps
2. **Imported router resolution**: Complex routing with imported modules
3. **Regression prevention**: Specific tests for the duplicate processing bug
4. **Real-world scenarios**: Tests against actual project structures
5. **Error conditions**: Validates error messages and exit codes

### What's Protected
- ✅ Imported router endpoint resolution
- ✅ Basic endpoint detection
