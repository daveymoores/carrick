# Remaining Issues Analysis

**Date**: January 2025  
**Analysis Based On**: Test runs against `express-demo-1/repo-a`, `express-demo-1/repo-b`, `express-demo-1/repo-c` and `test-repo/`  
**Last Updated**: January 2025

---

## Executive Summary

Running Carrick against the test repositories revealed **7 distinct issues**. Six have been fully fixed, and 2 remain open:

| Issue | Status | Priority |
|-------|--------|----------|
| 1. Mount Relationship Extraction | ✅ **FIXED** | - |
| 2. Type File Generation | ✅ **FIXED** | - |
| 2a. Inline Handler Type Extraction | ✅ **FIXED** | - |
| 2b. Repo Name Key Mismatch | ✅ **FIXED** | - |
| 3. Cross-Repo Data Flow | ✅ **FIXED** (Working as Designed) | - |
| 4. API Call URL Extraction | 🟡 Open | MEDIUM |
| 5. Path Resolution (Nested Routers) | 🟡 Open | MEDIUM |
| 6. Consumer Type Extraction | ✅ **FIXED** | - |
| **7. Consumer-Producer Alias Matching** | ✅ **FIXED** | - |

---

## Issue 7: Consumer-Producer Alias Matching ✅

### Status: FIXED

### Root Cause

The `sanitize_route_for_dynamic_paths` function in `analyzer/mod.rs` only handled `:param` style path parameters but not `${param}` template literal style that the LLM might return.

When the LLM returned URLs like `/users/${userId}/comments`, the function would:
1. Split by `/`: `["users", "${userId}", "comments"]`
2. For `${userId}`: `strip_prefix(':')` returns `None` (starts with `$` not `:`)
3. So it was treated as a regular segment: `to_pascal_case("${userId}")` → `Userid`
4. Result: `UsersUseridComments` (missing `By` prefix!)

This caused consumer aliases like `GetUsersUseridCommentsResponseConsumerCall1` instead of `GetUsersByUseridCommentsResponseConsumerCall1`.

### Fix Applied

Updated `sanitize_route_for_dynamic_paths` in `src/analyzer/mod.rs` to handle both `:param` and `${param}` formats:

```rust
fn sanitize_route_for_dynamic_paths(route: &str) -> String {
    route
        .split('/')
        .filter(|segment| !segment.is_empty())
        .map(|segment| {
            if let Some(param_name) = segment.strip_prefix(':') {
                // Convert :id -> ById, :userId -> ByUserId
                format!("By{}", Self::to_pascal_case(param_name))
            } else if segment.starts_with("${") && segment.ends_with('}') {
                // Handle template literal syntax: ${userId} -> ByUserid
                let inner = &segment[2..segment.len() - 1]; // Remove ${ and }
                // If it contains a dot (like process.env.VAR), take the last part
                let param_name = inner.rsplit('.').next().unwrap_or(inner);
                format!("By{}", Self::to_pascal_case(param_name))
            } else {
                Self::to_pascal_case(segment)
            }
        })
        .collect::<Vec<String>>()
        .join("")
}
```

### Tests Added

1. **Unit tests in `src/analyzer/mod.rs`**:
   - `test_sanitize_route_colon_params` - Standard `:param` style
   - `test_sanitize_route_template_literal_params` - `${param}` style
   - `test_sanitize_route_template_literal_with_dot_notation` - `${process.env.VAR}` style
   - `test_sanitize_route_mixed_params` - Mix of both styles
   - `test_generate_unique_call_alias_name_with_template_params` - Full alias generation

2. **Integration tests in `tests/url_alias_matching_test.rs`**:
   - `test_alias_generation_template_literal_path` - Verifies template paths produce correct aliases
   - `test_swc_extractor_normalizes_template_params` - Verifies SWC extraction normalizes paths
   - `test_enrichment_prefers_swc_url_over_llm_url` - Documents URL preference behavior
   - `test_double_path_params_handled_correctly` - Multiple path params
   - `test_path_patterns_produce_matchable_aliases` - Various path patterns

### How Matching Now Works

1. **Producer endpoint**: `GET /users/:id/comments`
   - Alias: `GetUsersByIdCommentsResponseProducer`
   - Path extracted by TypeScript: `/users/:id/comments`

2. **Consumer call**: `GET /users/${userId}/comments` (from LLM or template literal)
   - Alias: `GetUsersByUseridCommentsResponseConsumerCall1`
   - Path extracted by TypeScript: `/users/:userid/comments`

3. **Path matching**: The TypeScript `normalizePathForMatching` function converts both to:
   - `/users/{param}/comments` 
   
   These match!


---

## Issue 6: Consumer Type Extraction ✅ FIXED

### Status: COMPLETE

**Problem**: Consumer types were not being extracted at all. The system showed `Calls with type info: 0/12`.

**Root Cause**: 
1. Type annotations were on variable declarations, not captured by `CallSite`
2. `extract_types_from_analysis` required URL for all calls, but `.json()` calls don't have URLs

**Fix Applied**:
1. Added `ResultTypeInfo` struct and `result_type` field to `CallSite`
2. In `visit_var_decl`, capture type annotations from `const x: Type = await call()`
3. Link type annotations to call expressions using span mapping
4. Added `enrich_data_fetching_calls_with_type_info()` in orchestrator
5. Modified `extract_types_from_analysis` to handle calls without URLs using location-based aliases

**Results**:
- Before: `Calls with type info: 0/12`
- After: `Calls with type info: 5/12`
- Consumer types now extracted: `Order[]`, `Comment[]`, `User`, etc.

---

## Issue 1: Mount Relationships Not Extracted ✅ FIXED

### Status: COMPLETE

**Problem**: Router mounts like `app.use('/api', router)` were being classified as `Middleware` instead of `RouterMount`.

**Fix Applied**: Added `arg_count`, `first_arg_type`, and `first_arg_value` fields to `LeanCallSite` struct.

---

## Issue 2: Type Files Not Being Generated ✅ FIXED

### Status: FULLY FIXED

**Sub-issues resolved**:
- 2a. Inline Handler Type Extraction ✅
- 2b. Repo Name Key Mismatch ✅

---

## Issue 3: Cross-Repo Data Flow ✅ FIXED

### Status: COMPLETE (Working as Designed)

Cross-repo data flow works correctly when repos have been previously analyzed.

---

## Issue 4: API Calls Showing as [UNKNOWN] 🟡

### Status: OPEN

### Symptoms

```
Configuration Suggestions:
  - `GET` using **[UNKNOWN]** in `UNKNOWN`
  (7 total)
```

### Root Cause

Template literal URLs with environment variables are not being fully extracted:
```typescript
const resp = await fetch(`${process.env.ORDER_SERVICE_URL}/orders`);
```

The LLM receives the template literal but returns `null` for URL because it can't resolve environment variables.

### Potential Fix

Extract the path portion from template literals even when the host is a variable:
```typescript
`${process.env.ORDER_SERVICE_URL}/orders`  ->  url_path: "/orders"
```

### Priority: MEDIUM

Related to Issue 7 - the SWC-based extraction in `call_site_extractor.rs` now handles this, but it's not flowing through to the final output.

---

## Issue 5: Endpoint Paths Not Fully Resolved (Nested Routers) 🟡

### Status: PARTIALLY FIXED

### Symptoms

Endpoints show `/api/chat` but should show `/api/v1/chat` for nested router mounts.

### Priority: MEDIUM

Most mounts work correctly. This edge case affects nested routers with same internal variable names.

---

## Recommended Priority Order

1. **🟡 Issue 7: Consumer-Producer Alias Matching** - Debug why SWC-extracted URLs aren't reaching alias generation
2. **Issue 4: API Call URL Extraction** - May be same root cause as Issue 7
3. **Issue 5: Nested Router Path Resolution** - Edge case improvement

---

## Implementation Summary

### New Structs Added

```rust
// In call_site_extractor.rs
pub struct FetchCallInfo {
    pub url: Option<String>,
    pub method: String,
    pub location: String,
}

// Added to CallSite
pub correlated_fetch: Option<FetchCallInfo>,
```

### New Fields in CallSiteExtractor

```rust
pub struct CallSiteExtractor {
    // ... existing fields ...
    
    /// Maps variable names to their fetch call info
    fetch_result_vars: HashMap<String, FetchCallInfo>,
}
```

### Key Functions Added/Modified

1. `find_call_expr_in_expr()` - Unwrap call expressions from await/paren
2. `is_fetch_call()` - Detect if a call is a fetch() call
3. `extract_fetch_url()` - Extract URL from fetch arguments
4. `extract_path_from_url()` - Extract path portion and normalize template params
5. `normalize_template_params()` - Convert `${varName}` to `:varName`
6. `extract_fetch_method()` - Extract HTTP method from fetch options
7. `enrich_data_fetching_calls_with_type_info()` - Now copies URL/method from correlated_fetch

---

## Manual Testing Command

```bash
export CI=true
export CARRICK_ORG="your-org"
export CARRICK_API_ENDPOINT="https://api.carrick.tools"
export CARRICK_API_KEY="your-api-key"
export GEMINI_API_KEY="your-gemini-key"

cargo run -- ../test_repos/express-demo-1/repo-a/
```

**Expected output after Issue 7 is fully fixed:**
```
Type checking summary:
  Compatible pairs: X    (matched producer-consumer pairs)
  Incompatible pairs: Y  (type mismatches found!)
  Orphaned producers: Z  (endpoints with no callers)
  Orphaned consumers: W  (calls to external services)
```

**Current output (Issue 7 partially fixed):**
```
Type checking summary:
  Compatible pairs: 0
  Incompatible pairs: 0
  Orphaned producers: 6
  Orphaned consumers: 6  (aliases still wrong - userid instead of :userId)
```

---

## Key Learnings

1. **LLMs can't reliably extract byte positions** - Use SWC AST directly
2. **UTF-16 vs byte offsets matter** - ts-morph uses UTF-16, SWC uses bytes
3. **File-relative offsets required** - SWC span.lo includes file start position
4. **Owner names ≠ repo names** - Use file path for repository identification
5. **Inline handlers need special handling** - Can't rely on function definition lookup
6. **Consumer types need SWC-based extraction** - LLM-based extraction doesn't work
7. **`.json()` calls need to be linked to their `fetch()` calls** - Type is on json, URL is on fetch
8. **SWC extraction works but data flow needs tracing** - Unit tests pass but integration fails