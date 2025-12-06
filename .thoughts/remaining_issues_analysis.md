# Remaining Issues Analysis

**Date**: January 2025  
**Analysis Based On**: Test runs against `express-demo-1/repo-a`, `express-demo-1/repo-b`, `express-demo-1/repo-c` and `test-repo/`  
**Last Updated**: January 2025

---

## Executive Summary

Running Carrick against the test repositories revealed **7 distinct issues**. Five have been fully fixed, and 2 remain open:

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
| **7. Consumer-Producer Alias Matching** | 🟡 **PARTIALLY FIXED** | **CRITICAL** |

---

## Issue 7: Consumer-Producer Alias Matching 🟡

### Status: PARTIALLY FIXED - NEEDS INVESTIGATION

### What Was Done

Two commits implemented fetch-to-json call correlation:

1. **`b6c8947`** - `feat: implement fetch-to-json call correlation for consumer type matching`
   - Added `FetchCallInfo` struct to track fetch() call URL/method/location
   - Added `fetch_result_vars: HashMap<String, FetchCallInfo>` to `CallSiteExtractor`
   - When processing `const resp = await fetch(url)`, stores fetch info by variable name
   - When processing `resp.json()`, looks up correlated fetch info and attaches to CallSite
   - Added `correlated_fetch` field to `CallSite` struct
   - Updated `enrich_data_fetching_calls_with_type_info` to copy URL/method from correlated_fetch
   - All tests pass

2. **`8a8f439`** - `fix: normalize template literal path params to :param style`
   - Added `normalize_template_params()` to convert `${varName}` to `:varName` format
   - Updated `extract_path_from_url()` to call the normalizer
   - Tests verify: `/users/${userId}/profile` → `/users/:userId/profile`

### Current Test Output (STILL BROKEN)

```
Type checking summary:
  Compatible pairs: 0
  Incompatible pairs: 0
  Orphaned producers: 6
  Orphaned consumers: 6
  Orphaned producers: GET /dynamic (GetDynamicResponseProducer), GET /users/:id/comments (GetUsersByIdCommentsResponseProducer), ...
  Orphaned consumers: GET /api/comments/userid/userid (GetApiCommentsUseridUseridResponseConsumerCall1), GET /orders/userid/userid (GetOrdersUseridUseridResponseConsumerCall1), ...
```

### Problem Analysis

The consumer aliases still show `userid` (lowercase, no colon):
- `GetApiCommentsUseridUseridResponseConsumerCall1`
- `GetOrdersUseridUseridResponseConsumerCall1`
- `GetUsersUseridCommentsResponseConsumerCall1`

This indicates that **the fix in `extract_path_from_url` is NOT being applied** to these paths. The `${userId}` template expressions are being processed by `sanitize_route_for_dynamic_paths` in `analyzer/mod.rs` instead, which doesn't recognize `${...}` patterns.

### Investigation Needed

The fix added `normalize_template_params()` in `call_site_extractor.rs`, but the consumer aliases are being generated elsewhere. Need to trace:

1. **Where are consumer aliases generated?**
   - `multi_agent_orchestrator.rs` → `extract_types_from_analysis()` calls `Analyzer::generate_unique_call_alias_name()`
   - This uses `sanitize_route_for_dynamic_paths()` which doesn't handle `${...}`

2. **Why isn't the fix being applied?**
   - The fix is in `extract_path_from_url()` which is called by `extract_fetch_url()` 
   - This populates `FetchCallInfo.url` when tracking fetch() calls
   - But the URL might be coming from a different source (LLM extraction?)

3. **Possible causes:**
   - The LLM-extracted URL (from ConsumerAgent) overrides the SWC-extracted URL
   - The enrichment isn't happening before alias generation
   - The path is being extracted correctly but then re-processed incorrectly

### Files to Investigate

| File | What to Check |
|------|---------------|
| `src/call_site_extractor.rs` | Is `normalize_template_params()` being called? |
| `src/agents/orchestrator.rs` | Is `correlated_fetch` being used correctly? |
| `src/multi_agent_orchestrator.rs` | What URL is passed to `generate_unique_call_alias_name()`? |
| `src/analyzer/mod.rs` | Does `sanitize_route_for_dynamic_paths()` need to handle `${...}`? |

### Recommended Next Steps

1. **Add debug logging** to trace where the URL `/api/comments/userid/userid` comes from
2. **Check if LLM extraction overrides SWC extraction** - the ConsumerAgent might be returning the raw template literal
3. **Consider fixing `sanitize_route_for_dynamic_paths()`** to also handle `${...}` patterns as a fallback

### Tests Added (All Passing)

In `tests/consumer_type_extraction_test.rs`:
- `test_fetch_to_json_correlation` - Basic correlation works
- `test_fetch_to_json_correlation_template_literal` - Template URL extraction
- `test_fetch_to_json_correlation_post_method` - POST method extraction
- `test_multiple_fetch_to_json_correlations` - Multiple correlations in same function
- `test_non_json_call_no_correlation` - Only json() calls get correlation
- `test_template_literal_with_dynamic_path_param` - Verifies `:userId` normalization
- `test_template_literal_with_multiple_dynamic_params` - Multiple params
- `test_base_url_stripped_path_params_preserved` - Base URL stripping

The unit tests pass, meaning the SWC-level extraction works correctly. The issue is in how the extracted data flows through the system to alias generation.

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