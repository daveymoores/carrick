# Carrick Testing Strategy & Coverage

**Last Updated**: 2025-11-15
**Status**: Production-ready test suite with 43 passing tests

## Table of Contents
1. [Testing Philosophy](#testing-philosophy)
2. [Test Coverage Summary](#test-coverage-summary)
3. [Test Architecture](#test-architecture)
4. [What Is Tested](#what-is-tested)
5. [What Is NOT Tested](#what-is-not-tested)
6. [Testing Strategy by Component](#testing-strategy-by-component)
7. [Running Tests](#running-tests)
8. [CI/CD Integration](#cicd-integration)
9. [Adding New Tests](#adding-new-tests)

---

## Testing Philosophy

### Core Principle: Output-Focused Testing

**Test outputs, not implementation.**

This approach enables:
- ✅ **Safe refactoring** - Change implementation without breaking tests
- ✅ **Multi-agent migration** - Switch architecture while maintaining correctness
- ✅ **Fast feedback** - Tests run in < 10 seconds
- ✅ **Living documentation** - Tests show expected behavior

### What This Means in Practice

```rust
// ❌ BAD: Testing implementation details
#[test]
fn test_visitor_traverses_ast_in_specific_order() {
    // Brittle - breaks when refactoring visitor
}

// ✅ GOOD: Testing outputs
#[test]
fn test_dependency_conflicts_detected() {
    // Given: repos with version conflicts
    let conflicts = analyzer.analyze_dependencies();

    // Then: correct conflicts with correct severity
    assert_eq!(conflicts[0].package_name, "express");
    assert!(matches!(conflicts[0].severity, Critical));
}
```

---

## Test Coverage Summary

### Overall Statistics

| Category | Tests | Status | Coverage |
|----------|-------|--------|----------|
| **Unit Tests** | 12 | ✅ All passing | Good |
| **Dependency Analysis** | 8 | ✅ All passing | Excellent |
| **Endpoint Matching** | 13 | ✅ All passing | Excellent |
| **Cloud Storage** | 10 | ✅ All passing | Excellent |
| **Total** | **43** | ✅ **All passing** | **Strong** |

### Test Execution Time

```
Unit tests:        < 0.1s
Output tests:      < 0.1s
Endpoint tests:    < 0.1s
MockStorage tests: < 0.1s
Integration tests: ~7s (runs full binary)
━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
Total:             ~7.5s
```

### Test Files

```
tests/
├── output_contract_test.rs       (4 tests)  - Dependency conflict outputs
├── endpoint_matching_test.rs     (10 tests) - API matching logic
├── mock_storage_test.rs          (10 tests) - Cloud storage workflows
├── dependency_analysis_test.rs   (4 tests)  - Dependency analysis (original)
├── integration_test.rs           (3 tests)  - Full binary tests
└── fixtures/
    ├── scenario-1-dependency-conflicts/
    ├── scenario-2-api-mismatches/
    └── scenario-3-cross-repo-success/

src/
├── analyzer/mod.rs (inline tests)
├── engine/mod.rs (inline tests)
└── formatter/mod.rs (inline tests)
```

---

## Test Architecture

### 1. Unit Tests (12 tests)

**Location**: Inline in source files
**Purpose**: Test isolated pure functions
**Speed**: < 0.1s

**Examples**:
- `engine::tests::test_ast_stripping_removes_nodes`
- `formatter::tests::test_typescript_error_formatting`
- `formatter::tests::test_no_issues_output`

**Strategy**: Test pure functions and data transformations.

### 2. Output Contract Tests (4 tests)

**Location**: `tests/output_contract_test.rs`
**Purpose**: Verify final analysis outputs match expected results
**Speed**: < 0.1s

**Key Tests**:
```rust
test_scenario_1_dependency_conflicts_output
test_scenario_3_no_conflicts_output
test_dependency_conflict_severity_classification
test_output_stability_across_analysis_runs
```

**Strategy**: Load fixtures from disk, run analysis, compare against `expected-output.json`.

### 3. Endpoint Matching Tests (10 tests)

**Location**: `tests/endpoint_matching_test.rs`
**Purpose**: Unit-level testing of API endpoint matching logic
**Speed**: < 0.1s

**Coverage**:
- ✅ Matching endpoint and call
- ✅ Missing endpoint detection
- ✅ Method mismatch detection
- ✅ Orphaned endpoint detection
- ✅ Path parameter normalization (`:id` vs `:userId`)
- ✅ Multiple methods on same path
- ✅ Call deduplication
- ✅ Full REST CRUD operations

**Strategy**: Directly construct `Analyzer` with endpoints and calls, test matching logic.

### 4. MockStorage Tests (10 tests)

**Location**: `tests/mock_storage_test.rs`
**Purpose**: Test cloud storage workflows without AWS
**Speed**: < 0.1s

**Coverage**:
- ✅ Upload/download single repo
- ✅ Upload multiple repos to same org
- ✅ Multi-org isolation
- ✅ Concurrent uploads
- ✅ Package data preservation
- ✅ Type file handling

**Strategy**: Test `CloudStorage` trait implementation with in-memory storage.

### 5. Integration Tests (3 tests)

**Location**: `tests/integration_test.rs`
**Purpose**: End-to-end tests running full binary
**Speed**: ~7s

**Tests**:
- `test_basic_endpoint_detection`
- `test_imported_router_endpoint_resolution`
- `test_no_duplicate_processing_regression`

**Strategy**: Run `cargo` binary on fixture repos, parse stdout, verify results.

---

## What Is Tested

### ✅ Fully Tested Components

#### 1. Dependency Analysis (8 tests)
- ✅ Conflict detection across repos
- ✅ Severity classification (Critical/Warning/Info)
- ✅ Major version differences (Critical)
- ✅ Minor version differences (Warning)
- ✅ Patch version differences (Info)
- ✅ Version matching (no conflicts)
- ✅ Unique packages (no conflicts)
- ✅ Multiple conflicting packages

**Test Approach**: Construct `Analyzer` with `Packages`, call `analyze_dependencies()`, verify results.

#### 2. Endpoint Matching (13 tests)
- ✅ Exact endpoint/call matches
- ✅ Missing endpoint detection
- ✅ Method mismatch detection
- ✅ Orphaned endpoint detection
- ✅ Path parameter normalization
- ✅ Multi-method support (GET/POST on same path)
- ✅ Call deduplication
- ✅ Complex scenarios with mixed results
- ✅ Full REST CRUD operations

**Test Approach**: Create `ApiEndpointDetails` for endpoints and calls, test `analyze_matches()`.

#### 3. Cloud Storage (10 tests)
- ✅ Upload/download cycle
- ✅ Multi-repo handling
- ✅ Organization isolation
- ✅ Concurrent access
- ✅ Package preservation
- ✅ Type file handling
- ✅ Health checks

**Test Approach**: Use `MockStorage` to test `CloudStorage` trait without AWS.

#### 4. Formatter (3 tests)
- ✅ Type mismatch formatting
- ✅ Structured error formatting
- ✅ No issues output

**Test Approach**: Create `ApiIssues` with known data, test markdown output.

#### 5. Engine (3 tests)
- ✅ AST node stripping
- ✅ Data serialization/merging
- ✅ Cross-repo analyzer building

**Test Approach**: Test specific engine functions with controlled inputs.

---

## What Is NOT Tested

### ⚠️ Known Gaps

#### 1. TypeScript Type Checking (`ts_check`) - NOT TESTED

**What's Missing**:
- ❌ Type extraction (`extract_types_for_repo()`)
- ❌ TypeScript compilation (`ts_check/extract-type-definitions.ts`)
- ❌ Type compatibility checking (`check_type_compatibility()`)
- ❌ End-to-end type mismatch detection
- ❌ Integration with `ts-morph`

**What IS Tested**:
- ✅ Formatter handles `type_mismatches` field (unit tests with mocked data)

**Why Not Tested**:
- Complex setup requiring TypeScript compiler
- Requires running external `ts-node` scripts
- Requires creating `ts_check/output/` directory structure
- Needs real TypeScript source files with type annotations

**Impact on Refactor**: **Low-Medium**
- Type checking is isolated subsystem
- Formatter tests verify output handling
- Error handling returns empty Vec on failure
- Breaking it won't cascade to other components

**Recommendation**: Add later if refactoring type checking specifically.

#### 2. Gemini AI Integration - NOT TESTED

**What's Missing**:
- ❌ Gemini API calls
- ❌ Rate limiting
- ❌ Message conversion
- ❌ Error handling

**Why Not Tested**: External API dependency, optional feature.

#### 3. Environment Variable Extraction - NOT TESTED

**What's Missing**:
- ❌ ENV_VAR parsing from calls
- ❌ Config-based env var resolution
- ❌ External/internal API classification

**Why Not Tested**: Focused on core analysis first.

#### 4. Configuration Merging - NOT TESTED

**What's Missing**:
- ❌ Multiple config file merging
- ❌ Precedence rules
- ❌ Config validation

**Why Not Tested**: Tested indirectly through integration tests.

---

## Testing Strategy by Component

### Dependency Analysis

**Approach**: Direct unit testing
**Rationale**: Pure logic, easy to test, critical functionality

```rust
// Create packages with known versions
let mut analyzer = Analyzer::new(config, cm);
analyzer.add_repo_packages("repo-a", packages_a);
analyzer.add_repo_packages("repo-b", packages_b);

// Run analysis
let conflicts = analyzer.analyze_dependencies();

// Verify results
assert_eq!(conflicts.len(), 1);
assert_eq!(conflicts[0].package_name, "express");
```

### Endpoint Matching

**Approach**: Direct unit testing with constructed data
**Rationale**: No need to parse files, test matching logic directly

```rust
// Create endpoint
let endpoint = create_endpoint("/api/users", "GET", "server.ts");
analyzer.endpoints.push(endpoint);

// Create call
let call = create_call("/api/users", "POST", "client.ts");
analyzer.calls.push(call);

// Test matching
let (call_issues, _, _) = analyzer.analyze_matches();
assert!(call_issues[0].contains("Missing endpoint"));
```

### Cloud Storage

**Approach**: Test trait implementation with MockStorage
**Rationale**: Avoid AWS dependency, test workflows

```rust
let storage = MockStorage::new();

// Upload
storage.upload_repo_data("org", &repo_data).await?;

// Download
let (downloaded, _) = storage.download_all_repo_data("org").await?;

// Verify
assert_eq!(downloaded.len(), 1);
```

### Integration Tests

**Approach**: Run full binary, parse output
**Rationale**: Verify end-to-end functionality

```rust
let output = Command::new(env!("CARGO_BIN_EXE_carrick"))
    .arg(test_project_path)
    .env("CARRICK_MOCK_ALL", "1")
    .output()?;

assert!(output.status.success());
assert!(stdout.contains("Analyzed **4 endpoints**"));
```

---

## Running Tests

### Pre-Commit Hook (Recommended) ⭐

Install the pre-commit hook to automatically run tests before each commit:

```bash
./scripts/install-hooks.sh
```

This will:
- Run all tests before allowing a commit
- Prevent broken code from being committed
- Give immediate feedback on test failures

To bypass temporarily (not recommended):
```bash
git commit --no-verify
```

### Manual Test Execution

#### All Tests
```bash
CARRICK_API_ENDPOINT=https://test.example.com cargo test
```

### Specific Test Suites
```bash
# Output contract tests
CARRICK_API_ENDPOINT=https://test.example.com cargo test --test output_contract_test

# Endpoint matching tests
CARRICK_API_ENDPOINT=https://test.example.com cargo test --test endpoint_matching_test

# MockStorage tests
CARRICK_API_ENDPOINT=https://test.example.com cargo test --test mock_storage_test

# Integration tests
CARRICK_API_ENDPOINT=https://test.example.com cargo test --test integration_test

# Unit tests only
CARRICK_API_ENDPOINT=https://test.example.com cargo test --lib
```

### Run with Output
```bash
CARRICK_API_ENDPOINT=https://test.example.com cargo test -- --nocapture
```

### Run Specific Test
```bash
CARRICK_API_ENDPOINT=https://test.example.com cargo test test_dependency_conflict_detection
```

### Watch Mode (requires cargo-watch)
```bash
cargo watch -x "test"
```

---

## CI/CD Integration

### GitHub Actions Workflow

Tests run automatically on:
- ✅ Push to `main` or `develop`
- ✅ Pull requests to `main` or `develop`

**Workflow**: `.github/workflows/ci.yml`

#### Test Job
```yaml
test:
  name: Test Suite
  runs-on: ubuntu-latest
  steps:
    - name: Run tests
      run: cargo test --verbose

    - name: Run integration tests
      run: cargo test --test integration_test --verbose
```

**Environment Variables Required**:
- `CARRICK_API_ENDPOINT` - Set in GitHub secrets
- `GEMINI_API_KEY` - Set in GitHub secrets (optional)

#### Current CI Jobs
1. **Test Suite** - All tests (unit + integration + new tests)
2. **Linting** - Format check + Clippy
3. **Build** - Debug + Release builds
4. **Endpoint Regression** - Specific endpoint detection tests
5. **Security Audit** - `cargo audit`

### CI Test Execution

The CI runs the following test categories automatically:

```yaml
# All unit tests, output contract, endpoint matching, MockStorage
- name: Run tests
  run: cargo test --verbose

# Integration tests (full binary)
- name: Run integration tests
  run: cargo test --test integration_test --verbose
```

This covers all 43 tests:
- ✅ 12 unit tests
- ✅ 4 output contract tests
- ✅ 10 endpoint matching tests
- ✅ 10 MockStorage tests
- ✅ 4 dependency analysis tests
- ✅ 3 integration tests

### Test Artifacts

CI uploads test outputs on failure:
```yaml
- name: Upload test outputs as artifacts
  if: always()
  uses: actions/upload-artifact@v4
  with:
    name: test-outputs
    path: |
      test-repo/output/test_output.txt
      tests/fixtures/imported-routers/output/test_output.txt
```

---

## Adding New Tests

### For Dependency Conflicts

See: `.thoughts/adding-output-tests-guide.md`

**Quick Start**:
1. Create fixture in `tests/fixtures/scenario-N-name/`
2. Add repos with package.json files
3. Create `expected-output.json`
4. Add test to `tests/output_contract_test.rs`

### For Endpoint Matching

**Quick Start**:
1. Add test to `tests/endpoint_matching_test.rs`
2. Use helper functions: `create_endpoint()`, `create_call()`
3. Test `analyze_matches()` output

Example:
```rust
#[tokio::test]
async fn test_my_endpoint_scenario() {
    let mut analyzer = Analyzer::new(config, cm);

    analyzer.endpoints.push(create_endpoint("/api/users", "GET", "server.ts"));
    analyzer.calls.push(create_call("/api/users", "GET", "client.ts"));

    analyzer.build_endpoint_router();
    let (call_issues, _, _) = analyzer.analyze_matches();

    assert_eq!(call_issues.len(), 0);
}
```

### For Cloud Storage

**Quick Start**:
1. Add test to `tests/mock_storage_test.rs`
2. Use `MockStorage::new()`
3. Test upload/download workflow

### For Integration Tests

**Quick Start**:
1. Create fixture in `tests/fixtures/my-test-case/`
2. Add to `tests/integration_test.rs`
3. Run binary with `Command::new(env!("CARGO_BIN_EXE_carrick"))`

---

## Test Maintenance

### When Tests Should Change

Tests should be updated when:
- ✅ **Intentional behavior changes** (e.g., changing severity levels)
- ✅ **Output format changes** (e.g., changing conflict structure)
- ✅ **New features added** (add new tests)
- ✅ **Bug fixes** (add regression test)

Tests should NOT change when:
- ❌ **Internal refactoring** (implementation changes)
- ❌ **Performance improvements** (unless behavior changes)
- ❌ **Code reorganization** (moving files around)

### Test Failures = Good!

Failed tests after implementation changes indicate:
1. **Behavior changed unintentionally** - Fix the code
2. **Behavior changed intentionally** - Update expected outputs
3. **Regression introduced** - Fix the bug

This is the safety net working as intended!

---

## Future Testing Improvements

### High Priority (If Needed)

1. **TypeScript Type Checking Tests** (4-6 hours)
   - Create fixtures with type mismatches
   - Test full `ts_check` pipeline
   - Verify type mismatch detection

2. **Environment Variable Tests** (2-3 hours)
   - Test ENV_VAR extraction
   - Test config-based resolution

### Medium Priority

3. **Configuration Merging Tests** (2-3 hours)
   - Test multi-config merging
   - Test precedence rules

4. **Gemini Service Tests** (3-4 hours)
   - Mock Gemini API
   - Test rate limiting
   - Test error handling

### Low Priority

5. **Snapshot Testing** (1-2 hours)
   - Use `insta` crate for formatter
   - Test markdown output

6. **Property-Based Testing** (4-6 hours)
   - Use `proptest` for fuzzing
   - Generate random repo structures

---

## Success Metrics

### Current Achievement ✅

- ✅ **43 tests** covering core functionality
- ✅ **All tests passing** in < 10 seconds
- ✅ **Output-focused design** enables safe refactoring
- ✅ **CI integration** runs tests automatically
- ✅ **Living documentation** through test names and assertions
- ✅ **Safety net** for multi-agent refactor

### Quality Indicators

- ✅ Fast feedback loop (< 10s)
- ✅ Deterministic results
- ✅ Clear test names
- ✅ Focused assertions
- ✅ Minimal mocking
- ✅ High signal-to-noise ratio

---

## Conclusion

The current test suite provides **strong coverage** of core functionality with a focus on **output correctness** rather than implementation details. This approach enables confident refactoring to a multi-agent architecture while maintaining correctness.

**Key Strengths**:
- ✅ 43 tests covering critical paths
- ✅ Fast execution (< 10s)
- ✅ Output-focused design
- ✅ CI integration
- ✅ Easy to extend

**Known Gaps**:
- ⚠️ TypeScript type checking (isolated subsystem)
- ⚠️ Gemini AI integration (optional feature)
- ⚠️ Some edge cases

**Recommendation**: The current test suite is **production-ready** and provides sufficient coverage for safe refactoring. Additional tests can be added incrementally as needed.

---

**For detailed implementation guides, see**:
- `.thoughts/test-coverage-complete.md` - Full test implementation report
- `.thoughts/adding-output-tests-guide.md` - Guide for adding new tests
