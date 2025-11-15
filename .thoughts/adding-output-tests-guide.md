# Guide: Adding New Output Contract Tests

This guide explains how to add new output contract tests to ensure your refactoring doesn't break existing functionality.

## Quick Start

### 1. Create a Test Fixture

```bash
# Create directory structure
mkdir -p tests/fixtures/scenario-N-description/repo-name
```

Add files to the fixture:
- `package.json` (required for dependency tests)
- `index.ts` or other source files (for endpoint tests)
- `expected-output.json` (defines what the correct output should be)

### 2. Define Expected Output

Create `expected-output.json` with the structure matching what you want to test:

**For dependency conflicts:**
```json
{
  "dependency_conflicts": [
    {
      "package_name": "express",
      "versions": [
        {"repo": "repo-a", "version": "5.0.0"},
        {"repo": "repo-b", "version": "4.18.0"}
      ],
      "severity": "Critical"
    }
  ]
}
```

**For API endpoint tests:**
```json
{
  "endpoint_mismatches": [
    {
      "endpoint": "/api/users/:id",
      "consumer_method": "DELETE",
      "producer_methods": ["GET"],
      "issue": "method_not_found"
    }
  ],
  "matching_endpoints": [
    {
      "endpoint": "/api/users",
      "method": "GET"
    }
  ]
}
```

### 3. Add Test Function

Add to `tests/output_contract_test.rs`:

```rust
#[tokio::test]
async fn test_scenario_N_description() {
    // Given: fixtures with known scenario
    let fixture_path = PathBuf::from("tests/fixtures/scenario-N-description");

    let config = Config::default();
    let cm: Lrc<SourceMap> = Default::default();
    let mut analyzer = Analyzer::new(config, cm);

    // Load packages from fixtures
    let packages_a = load_packages_from_fixture(&fixture_path, "repo-a");
    analyzer.add_repo_packages("repo-a".to_string(), packages_a);

    // When: run analysis
    let actual = analyzer.analyze_dependencies();

    // Then: verify output matches expectations
    let expected = load_expected_output(&fixture_path);
    assert_conflicts_match(&actual, &expected);
}
```

### 4. Run the Test

```bash
CARRICK_API_ENDPOINT=https://test.example.com cargo test test_scenario_N_description
```

## Best Practices

### ✅ DO

1. **Test outputs, not implementation**
   - Assert on final results (conflicts, mismatches)
   - Don't assert on internal data structures

2. **Use realistic fixtures**
   - Base fixtures on real scenarios you've encountered
   - Keep them minimal but representative

3. **Make expected output explicit**
   - `expected-output.json` should be comprehensive
   - Include all fields that matter

4. **Sort results before comparing**
   - HashMap iteration order is non-deterministic
   - Always sort by a stable key (package name, endpoint, etc.)

5. **Test edge cases**
   - Empty repos
   - No conflicts
   - Many conflicts
   - Unusual version formats

### ❌ DON'T

1. **Don't test implementation details**
   - Avoid asserting on AST structure
   - Don't check intermediate processing steps
   - Don't verify internal visitor state

2. **Don't hardcode paths**
   - Use `PathBuf::from("tests/fixtures/...")`
   - Keep fixtures in the `tests/fixtures/` directory

3. **Don't make tests fragile**
   - Don't rely on exact error message wording
   - Don't depend on iteration order without sorting
   - Don't assert on internal IDs or pointers

4. **Don't skip expected-output.json**
   - Always create it - it's documentation + test oracle
   - Update it when behavior legitimately changes

## Example: Adding a New Dependency Test

Let's add a test for peer dependency conflicts:

### Step 1: Create fixture

```bash
mkdir -p tests/fixtures/scenario-4-peer-deps/repo-a
mkdir -p tests/fixtures/scenario-4-peer-deps/repo-b
```

### Step 2: Create `repo-a/package.json`

```json
{
  "name": "repo-a",
  "version": "1.0.0",
  "peerDependencies": {
    "react": "^18.0.0"
  }
}
```

### Step 3: Create `repo-b/package.json`

```json
{
  "name": "repo-b",
  "version": "1.0.0",
  "peerDependencies": {
    "react": "^17.0.0"
  }
}
```

### Step 4: Create `expected-output.json`

```json
{
  "dependency_conflicts": [
    {
      "package_name": "react",
      "versions": [
        {"repo": "repo-a", "version": "18.0.0"},
        {"repo": "repo-b", "version": "17.0.0"}
      ],
      "severity": "Critical"
    }
  ]
}
```

### Step 5: Add test

```rust
#[tokio::test]
async fn test_scenario_4_peer_dependency_conflicts() {
    let fixture_path = PathBuf::from("tests/fixtures/scenario-4-peer-deps");

    let config = Config::default();
    let cm: Lrc<SourceMap> = Default::default();
    let mut analyzer = Analyzer::new(config, cm);

    let packages_a = load_packages_from_fixture(&fixture_path, "repo-a");
    let packages_b = load_packages_from_fixture(&fixture_path, "repo-b");

    analyzer.add_repo_packages("repo-a".to_string(), packages_a);
    analyzer.add_repo_packages("repo-b".to_string(), packages_b);

    let actual = analyzer.analyze_dependencies();

    let expected = load_expected_output(&fixture_path);
    assert_conflicts_match(&actual, &expected);
}
```

### Step 6: Run

```bash
CARRICK_API_ENDPOINT=https://test.example.com cargo test test_scenario_4_peer_dependency_conflicts
```

## Common Patterns

### Pattern 1: Multiple Repos

```rust
let packages_a = load_packages_from_fixture(&fixture_path, "repo-a");
let packages_b = load_packages_from_fixture(&fixture_path, "repo-b");
let packages_c = load_packages_from_fixture(&fixture_path, "repo-c");

analyzer.add_repo_packages("repo-a".to_string(), packages_a);
analyzer.add_repo_packages("repo-b".to_string(), packages_b);
analyzer.add_repo_packages("repo-c".to_string(), packages_c);
```

### Pattern 2: Testing "No Conflicts"

```rust
let actual = analyzer.analyze_dependencies();

assert_eq!(
    actual.len(),
    0,
    "Expected no conflicts but found {}",
    actual.len()
);
```

### Pattern 3: Sorting for Determinism

```rust
let mut actual = analyzer.analyze_dependencies();
actual.sort_by(|a, b| a.package_name.cmp(&b.package_name));
```

### Pattern 4: Testing Specific Fields

```rust
let conflict = &actual[0];
assert_eq!(conflict.package_name, "express");
assert!(matches!(conflict.severity, ConflictSeverity::Critical));
assert_eq!(conflict.repos.len(), 2);
```

## Debugging Failed Tests

### Problem: Test fails with "Package not found"

**Cause**: Package name mismatch between actual and expected

**Fix**:
```bash
# Run test to see actual output
CARRICK_API_ENDPOINT=https://test.example.com cargo test test_name -- --nocapture

# Update expected-output.json to match actual package names
```

### Problem: Test fails with "Version mismatch"

**Cause**: Version parsing strips `^`, `~`, etc. from package.json

**Fix**: Use cleaned versions in expected-output.json
```json
{"version": "4.18.0"}  // ✅ Correct
{"version": "^4.18.0"} // ❌ Wrong
```

### Problem: Test fails intermittently

**Cause**: Non-deterministic ordering

**Fix**: Sort results before comparison
```rust
let mut actual = analyzer.analyze_dependencies();
actual.sort_by(|a, b| a.package_name.cmp(&b.package_name));
```

### Problem: Can't compile - "CARRICK_API_ENDPOINT must be set"

**Fix**: Always run with the environment variable
```bash
CARRICK_API_ENDPOINT=https://test.example.com cargo test
```

## Advanced: Custom Assertion Helpers

If you're testing a new output type, create a custom assertion helper:

```rust
fn assert_endpoint_mismatches_match(
    actual: &[EndpointMismatch],
    expected: &ExpectedMismatches,
) {
    assert_eq!(
        actual.len(),
        expected.mismatches.len(),
        "Mismatch count differs"
    );

    for (i, expected_mismatch) in expected.mismatches.iter().enumerate() {
        let actual_mismatch = &actual[i];
        assert_eq!(actual_mismatch.endpoint, expected_mismatch.endpoint);
        assert_eq!(actual_mismatch.method, expected_mismatch.method);
        // ... more assertions
    }
}
```

## Checklist for New Tests

- [ ] Fixture directory created
- [ ] All required files in fixture (package.json, source files)
- [ ] `expected-output.json` created and complete
- [ ] Test function added to `output_contract_test.rs`
- [ ] Test runs and passes
- [ ] Test is documented (clear name, comments if complex)
- [ ] Test is deterministic (sorts results if needed)
- [ ] Test focuses on outputs, not implementation

## Running Tests in CI

Add to your CI configuration:

```yaml
- name: Run output contract tests
  run: CARRICK_API_ENDPOINT=https://test.example.com cargo test --test output_contract_test
  env:
    CARRICK_API_ENDPOINT: https://test.example.com
```

## Summary

Output contract tests give you confidence to refactor by:
1. **Documenting** expected behavior in `expected-output.json`
2. **Verifying** outputs match expectations after changes
3. **Catching** regressions before they reach production
4. **Enabling** fearless refactoring of implementation

When in doubt: **Test outputs, not implementation!**
