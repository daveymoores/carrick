# Phase 2 Remaining Work: Complete Guide for Future Implementation

**Target Audience**: AI Agent or Engineer completing Phase 2 Priority 2 & 3  
**Prerequisites**: Phase 2 Priority 1 complete (adapter layer removed)  
**Estimated Time**: 5-7 hours total  
**Last Updated**: January 2025

---

## Table of Contents

1. [Context & Background](#context--background)
2. [Priority 2: Remove Legacy Analysis Methods](#priority-2-remove-legacy-analysis-methods)
3. [Priority 3: DependencyVisitor Simplification](#priority-3-dependencyvisitor-simplification)
4. [Testing Strategy](#testing-strategy)
5. [Success Criteria](#success-criteria)
6. [Risk Mitigation](#risk-mitigation)

---

## Context & Background

### What Has Been Done (Phase 2 Priority 1)

✅ **Adapter layer removed**: `CloudRepoData` is now built directly from `MultiAgentAnalysisResult`  
✅ **All tests passing**: 46/46 tests pass  
✅ **Multi-framework validated**: Express, Fastify, Koa all working  

### Current Architecture State

```
MultiAgentOrchestrator
    ↓
MultiAgentAnalysisResult (contains MountGraph)
    ↓
CloudRepoData::from_multi_agent_results() ← NEW in Priority 1
    ↓
CloudRepoData (serialized to cloud storage)
    ↓
Cross-Repo Analysis (reconstructs Analyzer from multiple CloudRepoData)
    ↓
Analyzer::get_results() ← STILL USES LEGACY METHODS
    ↓
Print results
```

### The Problem

The `Analyzer::get_results()` method still calls legacy analysis methods:
- `analyze_matches()` - finds orphaned endpoints and missing API calls
- `compare_calls_to_endpoints()` - compares request/response body types
- `find_matching_endpoint()` - helper for analyze_matches()

These methods use Express-specific pattern matching and don't leverage the mount graph's framework-agnostic matching capabilities.

### Why This Matters

1. **Framework Agnosticism**: Legacy methods have Express-specific logic
2. **Code Duplication**: Mount graph already does endpoint matching better
3. **Maintenance Burden**: Two systems doing the same thing
4. **Technical Debt**: Blocking full migration to multi-agent architecture

---

## Documents to Read First

### Required Reading (in order)

1. **`.thoughts/multi_agent_framework_agnostic_analysis.md`**
   - Lines 1693-1733 (Phase 3: Legacy Code Removal section)
   - Understand the vision for removing legacy code

2. **`research/MIGRATION_STATUS.md`**
   - Lines 135-245 (Phase 2 status and what still exists)
   - Current state of the codebase

3. **`research/PHASE_2_COMPLETE.md`**
   - What was done in Priority 1
   - Architecture changes made

4. **`src/mount_graph.rs`**
   - Lines 612-650 (`find_matching_endpoints()` method)
   - This is what should replace `find_matching_endpoint()`

5. **`src/analyzer/mod.rs`**
   - Lines 769-916 (`analyze_matches()` method)
   - Lines 994-1073 (`compare_calls_to_endpoints()` method)
   - Lines 917-990 (`find_matching_endpoint()` method)
   - Lines 1386-1405 (`get_results()` method that calls them)

### Optional Context

6. **`tests/endpoint_matching_test.rs`**
   - Tests that verify legacy analysis methods
   - Will need to be updated or removed

7. **`src/visitor.rs`**
   - Lines 1-500 (DependencyVisitor implementation)
   - For Priority 3 work

---

## Priority 2: Remove Legacy Analysis Methods

**Goal**: Refactor `Analyzer::get_results()` to use mount graph instead of legacy methods  
**Estimated Time**: 2-3 hours  
**Risk Level**: Medium  

### Step 1: Understand Current get_results() Implementation

**Location**: `src/analyzer/mod.rs:1386-1405`

```rust
pub fn get_results(&self) -> ApiAnalysisResult {
    let (call_issues, endpoint_issues, env_var_calls) = self.analyze_matches();
    let mismatches = self.compare_calls_to_endpoints();
    let type_mismatches = self.get_type_mismatches();
    let dependency_conflicts = self.analyze_dependencies();

    ApiAnalysisResult {
        endpoints: self.endpoints.clone(),
        calls: self.calls.clone(),
        issues: ApiIssues {
            call_issues,
            endpoint_issues,
            env_var_calls,
            mismatches,
            type_mismatches,
            dependency_conflicts,
        },
    }
}
```

**What needs to change**:
- Replace `self.analyze_matches()` with mount graph logic
- Replace `self.compare_calls_to_endpoints()` with mount graph logic
- Keep `self.get_type_mismatches()` and `self.analyze_dependencies()` (still needed)

### Step 2: Access Mount Graph in Cross-Repo Analysis

**Challenge**: `Analyzer` doesn't currently have access to mount graph in cross-repo mode.

**Solution Options**:

**Option A: Store mount graph in CloudRepoData** (RECOMMENDED)
```rust
// In src/cloud_storage/mod.rs
pub struct CloudRepoData {
    // ... existing fields ...
    pub mount_graph: Option<MountGraph>, // NEW field
}

// Update from_multi_agent_results() to include mount graph
impl CloudRepoData {
    pub fn from_multi_agent_results(...) -> Self {
        // ...
        mount_graph: Some(analysis_result.mount_graph.clone()),
    }
}
```

**Option B: Reconstruct mount graph from Analyzer data**
```rust
// In src/analyzer/mod.rs
impl Analyzer {
    pub fn build_mount_graph(&self) -> MountGraph {
        // Reconstruct from self.endpoints, self.calls, self.mounts
        // Less ideal, but possible
    }
}
```

**Recommendation**: Use Option A. Mount graph serialization is already working (it's in `MultiAgentAnalysisResult`), and it's the source of truth for all endpoint/call relationships.

### Step 3: Add Mount Graph to Analyzer

```rust
// In src/analyzer/mod.rs
pub struct Analyzer {
    // ... existing fields ...
    mount_graph: Option<MountGraph>, // NEW field
}

impl Analyzer {
    pub fn new(config: Config, source_map: Lrc<SourceMap>) -> Self {
        Self {
            // ... existing initialization ...
            mount_graph: None, // NEW
        }
    }

    pub fn set_mount_graph(&mut self, mount_graph: MountGraph) {
        self.mount_graph = Some(mount_graph);
    }
}
```

### Step 4: Update analyze_current_repo to Pass Mount Graph

**Location**: `src/engine/mod.rs:268-325`

```rust
async fn analyze_current_repo(repo_path: &str) -> Result<CloudRepoData, Box<dyn std::error::Error>> {
    // ... existing code ...
    
    // 4. Run the complete multi-agent analysis
    let analysis_result = orchestrator
        .run_complete_analysis(files, &packages, &all_imported_symbols)
        .await?;

    // ... existing code ...

    // 6. Build CloudRepoData directly (UPDATED to include mount_graph)
    let cloud_data = CloudRepoData::from_multi_agent_results(
        repo_name.clone(),
        &analysis_result,
        serde_json::to_string(&config).ok(),
        serde_json::to_string(&packages).ok(),
        Some(packages.clone()),
    );
    // Note: from_multi_agent_results() will now set mount_graph field

    // ... rest of function ...
}
```

### Step 5: Update build_cross_repo_analyzer to Restore Mount Graphs

**Location**: `src/engine/mod.rs:328-362` (build_cross_repo_analyzer function)

```rust
async fn build_cross_repo_analyzer<T: CloudStorage>(
    mut all_repo_data: Vec<CloudRepoData>,
    current_repo_data: CloudRepoData,
    repo_s3_urls: HashMap<String, String>,
    storage: &T,
) -> Result<Analyzer, Box<dyn std::error::Error>> {
    all_repo_data.push(current_repo_data);
    
    // ... existing merge logic ...
    
    let mut analyzer = builder.build_from_repo_data(all_repo_data.clone()).await?;

    // NEW: Merge mount graphs from all repos
    let merged_mount_graph = MountGraph::merge_from_repos(&all_repo_data);
    analyzer.set_mount_graph(merged_mount_graph);

    // ... rest of function ...
}
```

**Note**: You'll need to implement `MountGraph::merge_from_repos()`. This should:
1. Combine nodes, edges, endpoints, and calls from all repos
2. Handle potential conflicts (same endpoint in multiple repos)
3. Preserve path resolution and mount relationships

### Step 6: Refactor get_results() to Use Mount Graph

**Location**: `src/analyzer/mod.rs:1386-1405`

```rust
pub fn get_results(&self) -> ApiAnalysisResult {
    // NEW: Use mount graph if available, fallback to legacy methods
    let (call_issues, endpoint_issues, env_var_calls) = if let Some(ref mount_graph) = self.mount_graph {
        self.analyze_matches_with_mount_graph(mount_graph)
    } else {
        // Fallback for old code paths (will be removed eventually)
        self.analyze_matches()
    };

    let mismatches = if let Some(ref mount_graph) = self.mount_graph {
        self.compare_calls_with_mount_graph(mount_graph)
    } else {
        self.compare_calls_to_endpoints()
    };

    let type_mismatches = self.get_type_mismatches();
    let dependency_conflicts = self.analyze_dependencies();

    ApiAnalysisResult {
        endpoints: self.endpoints.clone(),
        calls: self.calls.clone(),
        issues: ApiIssues {
            call_issues,
            endpoint_issues,
            env_var_calls,
            mismatches,
            type_mismatches,
            dependency_conflicts,
        },
    }
}
```

### Step 7: Implement New Mount Graph-Based Methods

**Add these new methods to `src/analyzer/mod.rs`**:

```rust
impl Analyzer {
    /// Find orphaned endpoints and missing API calls using mount graph
    fn analyze_matches_with_mount_graph(
        &self,
        mount_graph: &MountGraph,
    ) -> (Vec<String>, Vec<String>, Vec<String>) {
        let mut call_issues = Vec::new();
        let mut endpoint_issues = Vec::new();
        let mut env_var_calls = Vec::new();

        // Track which endpoints have been matched
        let mut matched_endpoints: HashSet<String> = HashSet::new();

        // For each call, try to find a matching endpoint using mount graph
        for call in &self.calls {
            // Check for environment variable URLs
            if call.route.contains("process.env") || call.route.contains("${") {
                env_var_calls.push(format!(
                    "API call with environment variable URL: {} {} in {}",
                    call.method,
                    call.route,
                    call.file_path.display()
                ));
                continue;
            }

            // Use mount graph to find matching endpoints
            let matching_endpoints = mount_graph.find_matching_endpoints(&call.route, &call.method);

            if matching_endpoints.is_empty() {
                call_issues.push(format!(
                    "Missing endpoint for {} {} (called from {})",
                    call.method,
                    call.route,
                    call.file_path.display()
                ));
            } else {
                // Mark endpoints as matched
                for endpoint in matching_endpoints {
                    let key = format!("{} {}", endpoint.method, endpoint.full_path);
                    matched_endpoints.insert(key);
                }
            }
        }

        // Find orphaned endpoints (not matched by any call)
        for endpoint in mount_graph.get_resolved_endpoints() {
            let key = format!("{} {}", endpoint.method, endpoint.full_path);
            if !matched_endpoints.contains(&key) {
                endpoint_issues.push(format!(
                    "Orphaned endpoint: {} {} in {}",
                    endpoint.method,
                    endpoint.full_path,
                    endpoint.file_location
                ));
            }
        }

        (call_issues, endpoint_issues, env_var_calls)
    }

    /// Compare request/response types using mount graph
    fn compare_calls_with_mount_graph(
        &self,
        mount_graph: &MountGraph,
    ) -> Vec<String> {
        let mut issues = Vec::new();

        for call in &self.calls {
            let matching_endpoints = mount_graph.find_matching_endpoints(&call.route, &call.method);

            for endpoint in matching_endpoints {
                // Compare request body types if both exist
                if let (Some(call_req), Some(endpoint_req)) = (&call.request_body, &self.endpoints.iter()
                    .find(|e| e.route == endpoint.full_path && e.method == endpoint.method)
                    .and_then(|e| e.request_body.as_ref()))
                {
                    if call_req != endpoint_req {
                        issues.push(format!(
                            "Request body type mismatch for {} {}: call expects '{}' but endpoint has '{}'",
                            call.method,
                            call.route,
                            call_req,
                            endpoint_req
                        ));
                    }
                }

                // Compare response body types if both exist
                if let (Some(call_resp), Some(endpoint_resp)) = (&call.response_body, &self.endpoints.iter()
                    .find(|e| e.route == endpoint.full_path && e.method == endpoint.method)
                    .and_then(|e| e.response_body.as_ref()))
                {
                    if call_resp != endpoint_resp {
                        issues.push(format!(
                            "Response body type mismatch for {} {}: call expects '{}' but endpoint returns '{}'",
                            call.method,
                            call.route,
                            call_resp,
                            endpoint_resp
                        ));
                    }
                }
            }
        }

        issues
    }
}
```

### Step 8: Add MountGraph Merge Method

**Location**: `src/mount_graph.rs` (add at end of impl block)

```rust
impl MountGraph {
    /// Merge mount graphs from multiple repos
    pub fn merge_from_repos(all_repo_data: &[CloudRepoData]) -> Self {
        let mut merged = MountGraph::new();

        for repo_data in all_repo_data {
            if let Some(ref mount_graph) = repo_data.mount_graph {
                // Merge nodes
                for (node_name, node_type) in &mount_graph.nodes {
                    merged.nodes.insert(node_name.clone(), node_type.clone());
                }

                // Merge edges
                merged.edges.extend(mount_graph.edges.clone());

                // Merge endpoints
                merged.endpoints.extend(mount_graph.endpoints.clone());

                // Merge data calls
                merged.data_calls.extend(mount_graph.data_calls.clone());
            }
        }

        merged
    }
}
```

### Step 9: Update CloudRepoData Serialization

**Ensure mount_graph is properly serialized/deserialized**:

```rust
// In src/cloud_storage/mod.rs
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct CloudRepoData {
    // ... existing fields ...
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mount_graph: Option<MountGraph>, // Make sure MountGraph derives Serialize/Deserialize
}
```

**Check `src/mount_graph.rs`**:
```rust
#[derive(Debug, Clone, Serialize, Deserialize)] // Add Serialize, Deserialize if missing
pub struct MountGraph {
    // ...
}
```

### Step 10: Remove Legacy Methods (After Testing)

Once all tests pass with the new implementation:

1. **Delete legacy methods**:
   - `Analyzer::analyze_matches()` (lines 769-916)
   - `Analyzer::compare_calls_to_endpoints()` (lines 994-1073)
   - `Analyzer::find_matching_endpoint()` (lines 917-990)

2. **Remove fallback logic in get_results()**:
   ```rust
   pub fn get_results(&self) -> ApiAnalysisResult {
       let mount_graph = self.mount_graph.as_ref()
           .expect("Mount graph must be set before calling get_results()");
       
       let (call_issues, endpoint_issues, env_var_calls) = 
           self.analyze_matches_with_mount_graph(mount_graph);
       let mismatches = self.compare_calls_with_mount_graph(mount_graph);
       
       // ... rest of method ...
   }
   ```

3. **Update or remove tests in `tests/endpoint_matching_test.rs`**:
   - These tests directly call the legacy methods
   - Either update them to use `get_results()` or remove entirely
   - The integration tests already validate endpoint matching

### Step 11: Run All Tests

```bash
CARRICK_API_ENDPOINT=http://localhost:8000 cargo test
CARRICK_API_ENDPOINT=http://localhost:8000 cargo clippy --all-targets -- -D warnings
cargo fmt
```

**Expected results**:
- All tests pass
- No clippy warnings
- Code is formatted

---

## Priority 3: DependencyVisitor Simplification

**Goal**: Remove unused endpoint/call/mount extraction from DependencyVisitor  
**Estimated Time**: 3-4 hours  
**Risk Level**: Medium-High (more invasive change)  

### Current State

**Location**: `src/visitor.rs` (500+ lines)

`DependencyVisitor` currently extracts:
1. ✅ **ImportedSymbol** - NEEDED by multi-agent system
2. ✅ **FunctionDefinition** - NEEDED for type resolution
3. ❌ **Endpoints** - NOT NEEDED (agents extract these)
4. ❌ **API Calls** - NOT NEEDED (agents extract these)
5. ❌ **Mounts** - NOT NEEDED (agents extract these)

### Step 1: Audit DependencyVisitor Usage

```bash
# Find where DependencyVisitor is used
rg "DependencyVisitor::new" --type rust

# Find where visitor fields are accessed
rg "visitor\.endpoints" --type rust
rg "visitor\.calls" --type rust
rg "visitor\.mounts" --type rust
rg "visitor\.imported_symbols" --type rust
rg "visitor\.function_definitions" --type rust
```

**Expected findings**:
- `imported_symbols` used in: `engine/mod.rs` (line ~226)
- `function_definitions` used in: various places
- `endpoints`, `calls`, `mounts` should NOT be used (verify!)

### Step 2: Verify No Usage of Legacy Fields

Search for any remaining usage:
```bash
rg "\.endpoints" src/engine/mod.rs
rg "\.calls" src/engine/mod.rs
rg "\.mounts" src/engine/mod.rs
```

If you find usage, those code paths need to be updated to use multi-agent results instead.

### Step 3: Create Simplified SymbolExtractor (Option A)

**Location**: Create new file `src/symbol_extractor.rs`

```rust
use swc_common::{SourceMap, Span, sync::Lrc};
use swc_ecma_ast::*;
use swc_ecma_visit::{Visit, VisitWith};
use std::collections::HashMap;

/// Extracts imported symbols and function definitions from TypeScript/JavaScript files
/// This is needed by the multi-agent system for import resolution
pub struct SymbolExtractor {
    file_path: String,
    repo_name: String,
    source_map: Lrc<SourceMap>,
    pub imported_symbols: HashMap<String, ImportedSymbol>,
    pub function_definitions: HashMap<String, FunctionDefinition>,
}

impl SymbolExtractor {
    pub fn new(file_path: String, repo_name: &str, source_map: Lrc<SourceMap>) -> Self {
        Self {
            file_path,
            repo_name: repo_name.to_string(),
            source_map,
            imported_symbols: HashMap::new(),
            function_definitions: HashMap::new(),
        }
    }
}

impl Visit for SymbolExtractor {
    // Copy ONLY the import and function definition visitor methods from DependencyVisitor
    // Remove all endpoint/call/mount extraction logic
    
    fn visit_import_decl(&mut self, import: &ImportDecl) {
        // Copy from DependencyVisitor
    }
    
    fn visit_fn_decl(&mut self, fn_decl: &FnDecl) {
        // Copy from DependencyVisitor
    }
    
    // Do NOT include:
    // - visit_call_expr (used for endpoint/call extraction)
    // - visit_member_prop (used for endpoint extraction)
    // - Any Express/framework-specific logic
}
```

### Step 4: Update engine/mod.rs to Use SymbolExtractor

**Location**: `src/engine/mod.rs:discover_files_and_symbols()`

```rust
// Change from:
use crate::visitor::DependencyVisitor;

// To:
use crate::symbol_extractor::SymbolExtractor;

// In discover_files_and_symbols():
for file_path in &files {
    if let Some(module) = parse_file(file_path, &cm, &handler) {
        let mut extractor = SymbolExtractor::new(
            file_path.clone(),
            &repo_name,
            cm.clone()
        );
        module.visit_with(&mut extractor);
        all_imported_symbols.extend(extractor.imported_symbols);
    }
}
```

### Step 5: Update lib.rs Module Declarations

```rust
// In src/lib.rs
pub mod symbol_extractor; // NEW
// Keep visitor for now (backward compatibility)
```

### Step 6: Run Tests and Verify

```bash
CARRICK_API_ENDPOINT=http://localhost:8000 cargo test
```

If all tests pass, the migration is successful.

### Step 7: Remove Old DependencyVisitor (Optional)

Once you're confident the new code works:

```rust
// In src/lib.rs
// Remove: pub mod visitor;
```

Delete `src/visitor.rs` or strip out all the unused endpoint/call/mount logic.

**Note**: Consider keeping visitor.rs for the type definitions (ImportedSymbol, FunctionDefinition, Mount, etc.) that are used throughout the codebase.

---

## Testing Strategy

### Test Categories

1. **Unit Tests** (11 tests)
   - Should all continue to pass
   - No changes needed

2. **Integration Tests** (3 tests)
   - `test_basic_endpoint_detection` - verify still works with mount graph
   - `test_no_duplicate_processing_regression` - verify no regressions
   - `test_imported_router_endpoint_resolution` - verify import resolution works

3. **Endpoint Matching Tests** (10 tests in `tests/endpoint_matching_test.rs`)
   - These directly call legacy methods
   - **Option A**: Update to call `get_results()` instead
   - **Option B**: Remove entirely (integration tests cover this)
   - **Recommendation**: Option B (reduce test duplication)

4. **Cross-Repo Tests**
   - Verify cross-repo analysis still works with mount graph
   - Test with mock storage
   - Ensure serialization/deserialization works

### Testing Checklist

Before committing:
- [ ] All unit tests pass (11/11)
- [ ] All integration tests pass (3/3)
- [ ] Cross-repo analysis works
- [ ] Mount graph serialization works
- [ ] No clippy warnings
- [ ] Code is formatted (`cargo fmt`)
- [ ] Multi-framework tests pass (Express, Fastify, Koa)

### How to Test Cross-Repo Functionality

```bash
# Run with mock storage to test cross-repo analysis
CARRICK_API_ENDPOINT=http://localhost:8000 cargo test test_cross_repo_analyzer_builder_no_sourcemap_issues

# Run storage tests
CARRICK_API_ENDPOINT=http://localhost:8000 cargo test --test mock_storage_test
```

---

## Success Criteria

### Phase 2 Priority 2 Complete When:

- [ ] `get_results()` uses mount graph instead of legacy methods
- [ ] `analyze_matches()` deleted
- [ ] `compare_calls_to_endpoints()` deleted
- [ ] `find_matching_endpoint()` deleted
- [ ] All tests pass (46/46 or adjusted count)
- [ ] No clippy warnings
- [ ] Cross-repo analysis works with mount graph
- [ ] CloudRepoData includes mount_graph field

### Phase 2 Priority 3 Complete When:

- [ ] `SymbolExtractor` created (or DependencyVisitor simplified)
- [ ] Only extracts imports and function definitions
- [ ] No endpoint/call/mount extraction logic remains
- [ ] All tests pass
- [ ] No clippy warnings
- [ ] Code is cleaner and more maintainable

### Overall Phase 2 Complete When:

- [ ] Priority 1 ✅ (already done)
- [ ] Priority 2 ✅ (legacy methods removed)
- [ ] Priority 3 ✅ (visitor simplified)
- [ ] No adapter layer exists
- [ ] No legacy analysis methods exist
- [ ] All tests pass (estimated 40-45 tests after cleanup)
- [ ] Framework agnostic throughout

---

## Risk Mitigation

### Risk: Mount Graph Serialization Breaks

**Symptom**: CloudRepoData fails to serialize/deserialize mount_graph

**Solution**:
1. Ensure all MountGraph types derive Serialize/Deserialize
2. Check for circular references or complex types
3. Add serialization tests:
```rust
#[test]
fn test_mount_graph_serialization() {
    let mount_graph = MountGraph::new();
    // Add some data...
    let serialized = serde_json::to_string(&mount_graph).unwrap();
    let deserialized: MountGraph = serde_json::from_str(&serialized).unwrap();
    // Assert equality
}
```

### Risk: Cross-Repo Analysis Fails

**Symptom**: Tests fail when reconstructing analyzer from multiple repos

**Solution**:
1. Add debug logging in `build_cross_repo_analyzer()`
2. Verify mount graphs are being loaded correctly
3. Check that `merge_from_repos()` handles conflicts properly
4. Test with mock storage first, then real AWS S3

### Risk: Tests Break After Removing Legacy Methods

**Symptom**: endpoint_matching_test.rs fails

**Solution**:
1. Update tests to call `get_results()` instead of legacy methods
2. Or delete tests if they're redundant with integration tests
3. Ensure integration tests cover all the scenarios

### Risk: Performance Degradation

**Symptom**: Cross-repo analysis is slower with mount graph

**Solution**:
1. Profile the code to find bottlenecks
2. Consider caching mount graph queries
3. Optimize `find_matching_endpoints()` if needed
4. Mount graph should be faster (it's already built, no runtime matching needed)

### Risk: Breaking Change in CloudRepoData Format

**Symptom**: Can't deserialize old CloudRepoData without mount_graph field

**Solution**:
1. Use `#[serde(skip_serializing_if = "Option::is_none")]` on mount_graph field
2. Make mount_graph `Option<MountGraph>` (already done)
3. Handle None case gracefully (fallback to legacy methods temporarily)
4. Document the migration path for existing stored data

---

## Commit Strategy

### Commit 1: Add Mount Graph to CloudRepoData
```bash
git add src/cloud_storage/mod.rs src/mount_graph.rs
git commit -m "Add mount_graph field to CloudRepoData and merge method"
```

### Commit 2: Add Mount Graph Support to Analyzer
```bash
git add src/analyzer/mod.rs
git commit -m "Add mount_graph field and setter to Analyzer"
```

### Commit 3: Implement New Mount Graph-Based Methods
```bash
git add src/analyzer/mod.rs
git commit -m "Implement analyze_matches_with_mount_graph and compare_calls_with_mount_graph"
```

### Commit 4: Update get_results() to Use New Methods
```bash
git add src/analyzer/mod.rs src/engine/mod.rs
git commit -m "Refactor get_results() to use mount graph-based analysis"
git push  # Run tests first!
```

### Commit 5: Remove Legacy Methods
```bash
git add src/analyzer/mod.rs tests/endpoint_matching_test.rs
git commit -m "Remove legacy analysis methods (analyze_matches, compare_calls_to_endpoints)"
```

### Commit 6: Simplify DependencyVisitor
```bash
git add src/symbol_extractor.rs src/engine/mod.rs src/lib.rs
git commit -m "Create SymbolExtractor, remove endpoint/call extraction from visitor"
```

### Final Commit: Phase 2 Complete
```bash
git add research/MIGRATION_STATUS.md
git commit -m "Phase 2 Complete: All legacy analysis code removed"
```

---

## Troubleshooting Guide

### Issue: "Mount graph must be set" panic

**Cause**: Analyzer created without mount graph in some code path

**Fix**: Search for `Analyzer::new()` calls and ensure mount graph is set:
```bash
rg "Analyzer::new" --type rust
```

### Issue: Clippy warnings about unused fields

**Cause**: Removed usage but didn't update struct

**Fix**: Add `#[allow(dead_code)]` temporarily or remove the field

### Issue: Tests timeout or hang

**Cause**: Circular reference in mount graph merge

**Fix**: Add debug logging in `merge_from_repos()`, check for infinite loops

### Issue: Different results than legacy system

**Cause**: Mount graph matching logic differs from legacy pattern matching

**Fix**: 
1. Add debug logging to compare results
2. Check if the difference is a bug or an improvement
3. Update tests if mount graph is more accurate

---

## Final Checklist

Before marking Phase 2 as complete:

### Code Quality
- [ ] No `TODO` or `FIXME` comments related to Phase 2
- [ ] No dead code warnings
- [ ] No clippy warnings
- [ ] Code is formatted with `cargo fmt`
- [ ] Documentation updated (README, MIGRATION_STATUS, etc.)

### Functionality
- [ ] All tests pass (estimated 40-45 after cleanup)
- [ ] Cross-repo analysis works
- [ ] Multi-framework support validated
- [ ] Mount graph serialization works
- [ ] Type checking still works
- [ ] Import resolution still works

### Architecture
- [ ] No adapter layer exists
- [ ] No legacy analysis methods exist
- [ ] Single source of truth (mount graph)
- [ ] Framework agnostic throughout
- [ ] Clean separation of concerns

### Documentation
- [ ] MIGRATION_STATUS.md updated to show Phase 2 100% complete
- [ ] PHASE_2_COMPLETE.md updated with Priority 2 & 3 details
- [ ] This file (PHASE_2_REMAINING_WORK.md) can be archived
- [ ] README updated if necessary

---

## Questions? Stuck?

If you encounter issues:

1. **Read the tests**: Integration tests show how the system should work
2. **Check git history**: See how Phase 0 and 1 were completed
3. **Use debugging**: Add `println!` statements liberally
4. **Run clippy**: `cargo clippy` often gives helpful hints
5. **Test incrementally**: Don't make all changes at once

**Key insight**: The mount graph already has all the logic. You're just replacing legacy methods with mount graph queries. Keep it simple!

---

## Time Estimates

| Task | Estimated Time | Complexity |
|------|----------------|------------|
| Read documentation | 30 min | Low |
| Add mount_graph to CloudRepoData | 30 min | Low |
| Add mount_graph to Analyzer | 15 min | Low |
| Implement new mount graph methods | 1 hour | Medium |
| Refactor get_results() | 30 min | Low |
| Update cross-repo logic | 1 hour | Medium |
| Test and debug | 1 hour | Medium |
| Remove legacy methods | 30 min | Low |
| Simplify DependencyVisitor | 2 hours | Medium-High |
| Final testing and cleanup | 1 hour | Low |
| **Total** | **7-8 hours** | **Medium** |

---

**Good luck! The hard work (Phase 0 and 1) is already done. You're just cleaning up the last pieces of legacy code. 🚀**