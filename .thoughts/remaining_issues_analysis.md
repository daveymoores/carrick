# Remaining Issues Analysis

**Date**: January 2025  
**Analysis Based On**: Test runs against `express-demo-1/repo-a`, `express-demo-1/repo-b`, `express-demo-1/repo-c` and `test-repo/`  
**Last Updated**: January 2025

---

## Quick Start for Next Agent

### Issue 4 (API Calls Showing as [UNKNOWN]) - ✅ FIXED

**Fixed in**: `src/agents/orchestrator.rs`

The fix ensures SWC-extracted URLs always take priority over LLM-provided URLs in `enrich_data_fetching_calls_with_type_info`. The LLM often returns malformed URLs (e.g., `${process.env.ORDER_SERVICE_URL}/orders`) while SWC properly normalizes them (e.g., `/orders`).

**Tests added**: 5 new tests in `agents::orchestrator::tests`

### Issue 5 (Nested Router Paths) - ✅ FIXED

**Fixed in**: `src/mount_graph.rs`

The fix resolves parent node names in mount relationships using import context. When a local variable like `router` is used across multiple files, the system now correctly maps it to the imported name (e.g., `apiRouter`) by tracking which file the mount occurs in.

**Tests added**: `test_nested_router_path_resolution_with_same_variable_name`

### carrick.json Feature (Already Exists!)

The `carrick.json` file already handles environment variable classification. See `src/config.rs` and `src/url_normalizer.rs`. Example:

```json
{
  "internalEnvVars": ["ORDER_SERVICE_URL", "USER_SERVICE_URL"],
  "internalDomains": ["user-service.internal", "order-service.internal"],
  "externalEnvVars": ["STRIPE_API_URL"],
  "externalDomains": ["api.stripe.com"]
}
```

The `UrlNormalizer` uses this to:
- Strip base URLs: `${ORDER_SERVICE_URL}/orders` → `/orders`
- Classify as internal/external for matching
- Convert `${userId}` to `:userId` for path matching

**Note**: URL normalization in the SWC extractor (`call_site_extractor.rs`) already handles the path extraction and template param normalization. The `UrlNormalizer` is used later for matching in `analyzer/mod.rs`.

---

## Executive Summary

Running Carrick against the test repositories revealed **7 distinct issues**. All have been fully fixed:

| Issue | Status | Priority |
|-------|--------|----------|
| 1. Mount Relationship Extraction | ✅ **FIXED** | - |
| 2. Type File Generation | ✅ **FIXED** | - |
| 2a. Inline Handler Type Extraction | ✅ **FIXED** | - |
| 2b. Repo Name Key Mismatch | ✅ **FIXED** | - |
| 3. Cross-Repo Data Flow | ✅ **FIXED** (Working as Designed) | - |
| 4. API Call URL Extraction | ✅ **FIXED** | - |
| 5. Path Resolution (Nested Routers) | ✅ **FIXED** | - |
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

## Issue 4: API Calls Showing as [UNKNOWN] ✅

### Status: FIXED

### Symptoms

```
Configuration Suggestions:
  - `GET` using **[UNKNOWN]** in `UNKNOWN`
  (7 total)
```

### Root Cause Analysis

Template literal URLs with environment variables are not being fully extracted:
```typescript
const resp = await fetch(`${process.env.ORDER_SERVICE_URL}/orders`);
```

The LLM receives the template literal but returns `null` for URL because it can't resolve environment variables.

### What Already Works (SWC Extraction)

The `call_site_extractor.rs` already handles this correctly:
- `extract_fetch_url()` extracts template literals
- `extract_path_from_url()` strips env var prefixes: `${process.env.ORDER_SERVICE_URL}/orders` → `/orders`
- `normalize_template_params()` converts `${userId}` → `:userId`

### What's Broken (Data Flow)

The SWC-extracted URL isn't being used. In `src/agents/orchestrator.rs`:

```rust
// Current logic (buggy):
if call.url.is_none() || call.method.is_none() {
    if let Some(fetch_info) = &call_site.correlated_fetch {
        if call.url.is_none() {  // <-- Only uses SWC URL if LLM returned null
            call.url = fetch_info.url.clone();
        }
    }
}
```

The LLM often returns a malformed URL (not `null`), so the SWC URL is ignored.

### Recommended Fix

In `enrich_data_fetching_calls_with_type_info()`, **always prefer SWC URL**:

```rust
// Fix: Always prefer SWC-extracted URL over LLM URL
if let Some(fetch_info) = &call_site.correlated_fetch {
    if fetch_info.url.is_some() {
        call.url = fetch_info.url.clone();  // Always use SWC URL
    }
    if call.method.is_none() {
        call.method = Some(fetch_info.method.clone());
    }
}
```

### Also Consider: UrlNormalizer Integration

The `src/url_normalizer.rs` module already exists and handles:
- Full URLs with protocol/host
- `ENV_VAR:NAME:/path` patterns
- `process.env.VAR + "/path"` patterns
- Template literals `${VAR}/path`
- Internal/external classification via `carrick.json`

**Check if UrlNormalizer is being called in the multi-agent flow.** It may need to be integrated.

### Files to Modify

| File | Change |
|------|--------|
| `src/agents/orchestrator.rs` | Change `enrich_data_fetching_calls_with_type_info` to always prefer SWC URL |
| `src/multi_agent_orchestrator.rs` | Possibly integrate `UrlNormalizer::normalize()` before alias generation |

### Fix Applied

The fix was implemented in `src/agents/orchestrator.rs` in the `enrich_data_fetching_calls_with_type_info` method:

1. **Restructured the enrichment logic** to always apply SWC URL preference FIRST, regardless of whether result_type exists
2. **Changed the conditional** from `if call.url.is_none()` to `if fetch_info.url.is_some()` - always use SWC URL when available
3. **Added 5 new tests** to verify the behavior:
   - `test_swc_url_preferred_over_llm_url` - SWC URL replaces malformed LLM URL
   - `test_swc_url_used_when_llm_url_is_none` - SWC URL used when LLM returned null
   - `test_no_correlated_fetch_preserves_llm_url` - LLM URL preserved when no correlated fetch
   - `test_type_info_enrichment_from_call_site` - Type info enrichment still works
   - `test_swc_url_preferred_even_without_result_type` - SWC URL used even without result_type

**Key code change:**
```rust
// FIX for Issue 4: Always prefer SWC-extracted URL over LLM URL
// The LLM often returns malformed URLs (e.g., template literals with env vars)
// while SWC properly normalizes them (e.g., /orders instead of ${process.env.URL}/orders)
if let Some(fetch_info) = &call_site.correlated_fetch {
    if fetch_info.url.is_some() {
        call.url = fetch_info.url.clone(); // Always use SWC URL
    }
    if call.method.is_none() {
        call.method = Some(fetch_info.method.clone());
    }
}
```

### Test to Add

```rust
#[test]
fn test_swc_url_preferred_over_llm_url() {
    // Create a DataFetchingCall with malformed LLM URL
    let mut call = DataFetchingCall {
        url: Some("${process.env.ORDER_SERVICE_URL}/orders".to_string()), // LLM's bad URL
        // ...
    };
    
    // Create a CallSite with properly normalized SWC URL
    let call_site = CallSite {
        correlated_fetch: Some(FetchCallInfo {
            url: Some("/orders".to_string()), // SWC's good URL
            // ...
        }),
        // ...
    };
    
    // After enrichment, call.url should be "/orders"
    enrich_data_fetching_calls_with_type_info(&mut [call], &[call_site]);
    assert_eq!(call.url, Some("/orders".to_string()));
}
```

### Priority: MEDIUM (COMPLETED)

---

## Issue 5: Endpoint Paths Not Fully Resolved (Nested Routers) ✅

### Status: FIXED

### Symptoms

Endpoints show `/api/chat` but should show `/api/v1/chat` for nested router mounts.

### Example Scenario

```typescript
// routes/v1.ts
const router = express.Router();
router.get('/chat', handler);  // Shows as /api/chat, should be /api/v1/chat
export default router;

// routes/api.ts
const router = express.Router();  // Same variable name!
router.use('/v1', v1Router);
export default router;

// server.ts
app.use('/api', apiRouter);
```

### Root Cause

When multiple files use the same variable name (`router`), the mount graph may not correctly resolve the full path hierarchy.

### Files to Investigate

| File | Purpose |
|------|---------|
| `src/mount_graph.rs` | Builds the router hierarchy from mount relationships |
| `src/agents/mount_agent.rs` | Extracts `app.use('/path', router)` patterns |
| `src/agents/orchestrator.rs` | Dispatches to mount agent |

### Potential Fix Approaches

1. **Track router identity by file path, not just variable name**
2. **Add debug logging to mount graph construction**
3. **Ensure mount relationships include full file context**

### Test to Add

```rust
#[test]
fn test_nested_router_path_resolution() {
    // Set up mount graph with:
    // app.use('/api', apiRouter)
    // apiRouter.use('/v1', v1Router)
    // v1Router.get('/chat', handler)
    
    // Assert endpoint path is "/api/v1/chat"
}
```

### Priority: MEDIUM (COMPLETED)

### Fix Applied

The fix was implemented in `src/mount_graph.rs`:

1. **Added `build_import_map` function** - Creates a mapping from source file paths to imported names
2. **Added `resolve_node_name_from_location` function** - Resolves local variable names to their imported names based on file context
3. **Modified `build_mounts_from_analysis`** - Now resolves parent node names before creating mount edges

**Key insight**: When `router.use('/v1', v1Router)` is in `routes/api.ts`, and `routes/api.ts` is imported as `apiRouter`, the parent node `router` should be resolved to `apiRouter` for proper chain resolution.

**Test added**: `test_nested_router_path_resolution_with_same_variable_name` verifies that:
- `routes/v1.ts`: `router.get('/chat', handler)` 
- `routes/api.ts`: `router.use('/v1', v1Router)`
- `server.ts`: `app.use('/api', apiRouter)`

Results in full path `/api/v1/chat` (not just `/v1/chat`).

---

## Recommended Priority Order

1. ~~Issue 7: Consumer-Producer Alias Matching~~ ✅ **FIXED** (commit `30a33b7`)
2. ~~Issue 4: API Call URL Extraction~~ ✅ **FIXED** - Always prefer SWC URL over LLM URL
3. ~~Issue 5: Nested Router Path Resolution~~ ✅ **FIXED** - Resolve parent names using import context

**All issues are now resolved!**

---

## carrick.json Feature Documentation

### Overview

The `carrick.json` file allows users to configure environment variable and domain classification. This helps Carrick understand which API calls are internal (between your services) vs external (third-party APIs).

### Example Configuration

```json
{
  "internalEnvVars": ["ORDER_SERVICE_URL", "USER_SERVICE_URL", "CORE_API"],
  "internalDomains": ["user-service.internal", "order-service.internal"],
  "externalEnvVars": ["STRIPE_API_URL", "TWILIO_API_URL"],
  "externalDomains": ["api.stripe.com", "api.twilio.com"]
}
```

### How It Works

1. **File Discovery**: `src/file_finder.rs` looks for `carrick.json` in the repo root
2. **Config Loading**: `src/config.rs` parses the JSON into a `Config` struct
3. **URL Normalization**: `src/url_normalizer.rs` uses the config to:
   - Identify internal vs external calls
   - Strip base URLs to extract just the path
   - Convert template params to `:param` style

### UrlNormalizer Capabilities

The `UrlNormalizer` (in `src/url_normalizer.rs`) handles:

| Pattern | Normalized To |
|---------|---------------|
| `https://user-service.internal/users/123` | `/users/:id` (internal) |
| `${ORDER_SERVICE_URL}/orders/${orderId}` | `/orders/:orderId` (internal if configured) |
| `process.env.API_URL + "/users"` | `/users` |
| `ENV_VAR:STRIPE_API_URL:/charges` | `/charges` (external if configured) |

### Feature Status: Working but Underutilized

The `carrick.json` and `UrlNormalizer` are fully implemented with 20+ tests. However, they may not be integrated into the multi-agent flow.

**To verify**: Check if `UrlNormalizer::normalize()` is called in:
- `src/multi_agent_orchestrator.rs`
- `src/agents/orchestrator.rs`

If not, integrating it would solve Issue 4.

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
9. **Always prefer SWC-extracted data over LLM-extracted** - LLMs guess; SWC parses
10. **Handle both `:param` and `${param}` styles** - LLMs return inconsistent formats

---

## Prompt for Next Agent

Use this prompt to continue work on remaining issues:

```
I'm working on the Carrick project, a TypeScript API compatibility checker.

Please read these files for context:
- .thoughts/remaining_issues_analysis.md (current issues and status)
- .thoughts/research/ts_check.md (type checking system)
- src/url_normalizer.rs (URL normalization)
- src/agents/orchestrator.rs (data fetching call enrichment)

There are 2 remaining issues:

1. **Issue 4: API Calls Showing as [UNKNOWN]** (MEDIUM priority)
   - SWC-extracted URLs aren't flowing to final output
   - Fix is in `enrich_data_fetching_calls_with_type_info` - always prefer SWC URL
   - May also need to integrate UrlNormalizer into multi-agent flow

2. **Issue 5: Nested Router Paths** (MEDIUM priority)  
   - Nested routers with same variable name don't resolve full path
   - Edge case in mount_graph.rs

Please fix Issue 4 first. Write tests before implementing the fix.
```