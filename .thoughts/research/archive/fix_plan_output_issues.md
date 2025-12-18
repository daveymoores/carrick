# Fix Plan: Output Issues in Carrick API Validation Tool

**Created**: January 2025  
**Status**: Phases 1-4 Complete  
**Priority**: High  
**Last Updated**: January 2025

---

## Executive Summary

This document outlines the issues identified in Carrick's output and provides a detailed fix plan for each. Analysis revealed more interconnected issues than initially identified:

### Completed Fixes (Phase 1-4)
1. ✅ **Query parameters creating wrong type aliases** - Query strings now stripped before alias generation
2. ✅ **Message format mismatch** - Analyzer now pushes messages in format formatter expects
3. ✅ **Configuration Suggestions show `[member_expr]`** - Fixed: `expr_to_string()` now recursively handles nested member expressions
4. ✅ **Path parameters incorrectly flagged as env var URLs** - Fixed: New `is_env_var_base_url()` helper distinguishes env vars from path params

### Remaining Issues (Phase 5)
5. 🟡 **Consumer types not extracted (0/5)** - Fetch correlation only works for specific patterns (deferred)

---

## Current Observed Behavior

After running against test repos, the output shows:

```
5 Configuration Suggestions
  - `GET` using **[member_expr]** in `/users/${author}`
  - `GET` using **[member_expr]** in `/api/orders/101`
  - `GET` using **[member_expr]** in `/users/1`
  - `GET` using **[member_expr]** in `/users/${order.userId}`
  - `GET` using **[member_expr]** in `/comments?orderId=${member_expr}`
```

And:
```
Calls with type info: 0/5
Correlated .json() call with fetch: url=None, method=GET
```

These indicate multiple root causes that have now been addressed in Phases 1-4.

---

## Root Cause Analysis: The `member_expr` Problem

### The Chain of Issues

The `[member_expr]` output reveals a chain of interconnected problems:

```
Source Code:           fetch(`${process.env.API_URL}/users`)
                                      ↓
expr_to_string():      Returns "member_expr" for nested MemberExpr
                                      ↓
Template Literal:      "${member_expr}/users"
                                      ↓
LLM Consumer Agent:    Returns URL with "${member_expr}" literally
                                      ↓
Analyzer Detection:    Flags as env var URL because contains "${"
                                      ↓
extract_env_var_name(): Extracts "member_expr" from "${member_expr}"
                                      ↓
Output:                "[member_expr]" instead of "[API_URL]"
```

### Root Cause 1: `expr_to_string()` Loses Nested Member Expression Info ✅ FIXED

**File**: `src/call_site_extractor.rs` lines 103-117

**Problem** (before fix):
```rust
fn expr_to_string(&self, expr: &Expr) -> String {
    match expr {
        Expr::Ident(ident) => ident.sym.to_string(),
        Expr::Member(member) => {
            if let (Expr::Ident(obj), MemberProp::Ident(prop)) = (&*member.obj, &member.prop) {
                format!("{}.{}", obj.sym, prop.sym)  // Only handles 1-level deep!
            } else {
                "member_expr".to_string()  // PROBLEM: Loses all info for nested expressions
            }
        }
        // ...
    }
}
```

**Solution** (Phase 3): Recursive member expression handling:
```rust
fn expr_to_string(&self, expr: &Expr) -> String {
    match expr {
        Expr::Ident(ident) => ident.sym.to_string(),
        Expr::Member(member) => {
            // Recursively build the full member expression
            let obj_str = self.expr_to_string(&member.obj);
            let prop_str = match &member.prop {
                MemberProp::Ident(ident) => ident.sym.to_string(),
                MemberProp::Computed(computed) => {
                    format!("[{}]", self.expr_to_string(&computed.expr))
                }
                MemberProp::PrivateName(name) => format!("#{}", name.name),
            };
            format!("{}.{}", obj_str, prop_str)
        }
        // ...
    }
}
```

Now `process.env.API_URL` correctly becomes `"process.env.API_URL"` instead of `"member_expr"`.

### Root Cause 2: Overly Broad Env Var Detection ✅ FIXED

**File**: `src/analyzer/mod.rs` in `analyze_matches_with_mount_graph()`

**Problem** (before fix):
```rust
if call.route.contains("ENV_VAR:")
    || call.route.contains("process.env")
    || call.route.contains("${")  // TOO BROAD!
{
```

This flagged ANY route containing `${` as an env var URL, including path parameters.

**Solution** (Phase 4): New `is_env_var_base_url()` helper with smart detection:
```rust
fn is_env_var_base_url(route: &str) -> bool {
    // Check for explicit ENV_VAR: prefix format
    if route.starts_with("ENV_VAR:") { return true; }
    
    // Check for process.env pattern
    if route.contains("process.env.") { return true; }
    
    // Check for ${UPPER_CASE_VAR} at the START only
    if route.starts_with("${") {
        if let Some(end) = route.find('}') {
            let var_name = &route[2..end];
            if var_name.contains('.') || var_name.chars().all(|c| c.is_uppercase() || c == '_' || c.is_ascii_digit()) {
                return true;
            }
        }
    }
    false
}
```

Now `/users/${userId}` is correctly identified as a path parameter, not an env var URL.

### Root Cause 3: LLM Returns Un-normalized URLs

The LLM consumer agent returns URLs like `/users/${author}` instead of the normalized `/users/:author` format. When SWC correlation works, it properly normalizes. But when falling back to LLM, we get raw template syntax. This is partially mitigated by the Phase 3 and 4 fixes.

---

## Issue 1: Configuration Suggestions Show `[UNKNOWN]` (Original Issue)

### Problem Description

When API calls use environment variables that aren't configured in `carrick.json`, the output shows:
```
- `GET` using **[UNKNOWN]** in `UNKNOWN`
```

Instead of showing the actual env var name so users know what to add to their config.

### Root Cause Analysis (Original)

There's a **format mismatch** between what the analyzer pushes and what the formatter expects:

#### What the Analyzer Pushes (`src/analyzer/mod.rs` L757-761):
```rust
env_var_calls.push(format!(
    "API call with environment variable URL: {} {} in {}",
    call.method,
    call.route,        // e.g., "${process.env.SERVICE_URL}/orders" OR "unknown"
    call.file_path.display()
));
```

#### What the Formatter Expects (`src/formatter/mod.rs` L553):
```rust
// Parse issues like "Environment variable endpoint: GET using env vars [API_URL] in ENV_VAR:API_URL:/users"
```

The formatter's `extract_env_var_info()` function looks for:
- `[` and `]` brackets to extract env var names

### Status: ✅ FIXED (Phase 2)

Message format updated to match formatter expectations.
- `ENV_VAR:` pattern to extract the path

But the actual message format doesn't include these patterns!

### Files Involved

| File | Function | Role |
|------|----------|------|
| `src/analyzer/mod.rs` | `analyze_matches_with_mount_graph()` L746-764 | Pushes env_var_calls messages |
| `src/formatter/mod.rs` | `extract_env_var_info()` L553-590 | Parses env_var_calls messages |
| `src/formatter/mod.rs` | `format_configuration_section()` L225-244 | Displays configuration suggestions |

### Proposed Fix

**Option A (Recommended): Update Analyzer to Match Expected Format**

Modify the analyzer to push messages in the format the formatter expects:

```rust
// In src/analyzer/mod.rs, analyze_matches_with_mount_graph()
// Replace lines 757-761 with:

// Extract env var name from the route
let env_var_name = Self::extract_env_var_name(&call.route);
let path = Self::extract_path_from_env_var_route(&call.route);

env_var_calls.push(format!(
    "Environment variable endpoint: {} using env vars [{}] in ENV_VAR:{}:{}",
    call.method,
    env_var_name,
    env_var_name,
    path
));
```

Add helper functions to extract env var name:
```rust
/// Extract environment variable name from a route
/// Examples:
/// - "ENV_VAR:API_URL:/users" -> "API_URL"
/// - "${process.env.SERVICE_URL}/orders" -> "SERVICE_URL"
/// - "unknown" -> "UNKNOWN_API"
fn extract_env_var_name(route: &str) -> String {
    if route.starts_with("ENV_VAR:") {
        let parts: Vec<&str> = route.splitn(3, ':').collect();
        if parts.len() >= 2 {
            return parts[1].to_string();
        }
    }
    
    // Handle ${process.env.VAR} or ${VAR} patterns
    if let Some(start) = route.find("${") {
        if let Some(end) = route[start..].find('}') {
            let inner = &route[start + 2..start + end];
            // Handle process.env.VAR -> VAR
            if let Some(last_dot) = inner.rfind('.') {
                return inner[last_dot + 1..].to_string();
            }
            return inner.to_string();
        }
    }
    
    // Handle process.env.VAR patterns
    if let Some(idx) = route.find("process.env.") {
        let after = &route[idx + 12..];
        let end = after.find(|c: char| !c.is_alphanumeric() && c != '_')
            .unwrap_or(after.len());
        return after[..end].to_string();
    }
    
    "UNKNOWN_API".to_string()
}

/// Extract path from environment variable route
fn extract_path_from_env_var_route(route: &str) -> String {
    if route.starts_with("ENV_VAR:") {
        let parts: Vec<&str> = route.splitn(3, ':').collect();
        if parts.len() >= 3 {
            return parts[2].to_string();
        }
    }
    
    // Handle ${VAR}/path patterns - extract after }
    if let Some(idx) = route.find("}/") {
        return route[idx + 1..].to_string();
    }
    
    // Handle process.env.VAR + "/path" patterns
    if let Some(idx) = route.find("+ \"") {
        let after = &route[idx + 3..];
        if let Some(end) = after.find('"') {
            return after[..end].to_string();
        }
    }
    
    "/".to_string()
}
```

### Test Cases to Add

```rust
#[test]
fn test_extract_env_var_name() {
    assert_eq!(extract_env_var_name("ENV_VAR:API_URL:/users"), "API_URL");
    assert_eq!(extract_env_var_name("${process.env.SERVICE_URL}/orders"), "SERVICE_URL");
    assert_eq!(extract_env_var_name("${API_BASE}/users"), "API_BASE");
    assert_eq!(extract_env_var_name("unknown"), "UNKNOWN_API");
    assert_eq!(extract_env_var_name("/users"), "UNKNOWN_API"); // No env var
}
```

---

## Issue 2: Env Var Names Lost During URL Extraction

### Problem Description

When SWC extracts URLs from fetch() calls, it strips the env var prefix:
- Input: `${process.env.ORDER_SERVICE_URL}/orders`
- Output: `/orders`

This causes:
1. Loss of env var information needed for configuration suggestions
2. Inability to distinguish between internal and external calls

### Root Cause Analysis

In `src/call_site_extractor.rs`, the `extract_fetch_url()` function calls `extract_path_from_url()` which intentionally strips env var prefixes:

```rust
// Line 304-307
fn extract_path_from_url(&self, url: &str) -> Option<String> {
    ...
    } else if let Some(idx) = url.find("}/") {
        // Template expression prefix like ${ENV}/ - extract path after it
        url[idx + 1..].to_string()
    }
    ...
}
```

### Files Involved

| File | Function | Role |
|------|----------|------|
| `src/call_site_extractor.rs` | `extract_fetch_url()` L274-298 | Extracts URL from fetch() calls |
| `src/call_site_extractor.rs` | `extract_path_from_url()` L301-326 | Strips env var prefix from URL |

### Proposed Fix

**Create a new method that preserves env var info in `ENV_VAR:NAME:/path` format**

```rust
/// Extract URL from fetch() call, preserving environment variable information
/// Returns URL in ENV_VAR:NAME:/path format when env vars are detected
fn extract_fetch_url_with_env_info(&self, call: &CallExpr) -> Option<String> {
    if call.args.is_empty() {
        return None;
    }

    let first_arg = &call.args[0].expr;
    match &**first_arg {
        // String literal: fetch("/orders") - no env var
        Expr::Lit(Lit::Str(s)) => Some(s.value.to_string()),
        
        // Template literal: fetch(`${BASE}/orders`)
        Expr::Tpl(tpl) => {
            let template_str = self.extract_template_literal(tpl);
            self.convert_to_env_var_format(&template_str)
        }
        
        // Variable: fetch(url) - try to resolve
        Expr::Ident(ident) => {
            let var_name = ident.sym.to_string();
            self.argument_values
                .get(&var_name)
                .cloned()
                .and_then(|v| self.convert_to_env_var_format(&v))
        }
        _ => None,
    }
}

/// Convert URL string to ENV_VAR:NAME:/path format if it contains env vars
/// Otherwise return the path with template params normalized to :param style
fn convert_to_env_var_format(&self, url: &str) -> Option<String> {
    // Check for ${process.env.VAR} or ${VAR} at the start
    if url.starts_with("${") {
        if let Some(end) = url.find('}') {
            let var_expr = &url[2..end];
            
            // Extract the actual variable name
            let var_name = if let Some(last_dot) = var_expr.rfind('.') {
                &var_expr[last_dot + 1..]
            } else {
                var_expr
            };
            
            // Extract the path after the variable
            let path = if url.len() > end + 1 {
                Self::normalize_template_params(&url[end + 1..])
            } else {
                "/".to_string()
            };
            
            return Some(format!("ENV_VAR:{}:{}", var_name, path));
        }
    }
    
    // Check for process.env.VAR + "/path" pattern
    if url.contains("process.env.") {
        if let Some(start) = url.find("process.env.") {
            let after_env = &url[start + 12..];
            let var_end = after_env.find(|c: char| !c.is_alphanumeric() && c != '_')
                .unwrap_or(after_env.len());
            let var_name = &after_env[..var_end];
            
            // Try to find the path
            let path = if let Some(plus_idx) = url.find('+') {
                let path_part = url[plus_idx + 1..].trim()
                    .trim_start_matches('"')
                    .trim_start_matches('\'')
                    .trim_end_matches('"')
                    .trim_end_matches('\'');
                path_part.to_string()
            } else if let Some(slash_idx) = url[start..].find('/') {
                url[start + slash_idx..].to_string()
            } else {
                "/".to_string()
            };
            
            return Some(format!("ENV_VAR:{}:{}", var_name, Self::normalize_template_params(&path)));
        }
    }
    
    // No env var detected - normalize path params and return
    Some(Self::normalize_template_params(url))
}
```

### Update Usage

In `visit_var_decl()` when storing fetch call info:
```rust
// Replace:
let url = self.extract_fetch_url(call_expr);
// With:
let url = self.extract_fetch_url_with_env_info(call_expr);
```

### Test Cases to Add

```rust
#[test]
fn test_convert_to_env_var_format() {
    let extractor = create_test_extractor();
    
    // Template literal with env var
    assert_eq!(
        extractor.convert_to_env_var_format("${process.env.API_URL}/users"),
        Some("ENV_VAR:API_URL:/users".to_string())
    );
    
    // Template literal with simple var
    assert_eq!(
        extractor.convert_to_env_var_format("${BASE_URL}/orders"),
        Some("ENV_VAR:BASE_URL:/orders".to_string())
    );
    
    // No env var - just path
    assert_eq!(
        extractor.convert_to_env_var_format("/users/${userId}"),
        Some("/users/:userId".to_string())
    );
    
    // process.env + string concat
    assert_eq!(
        extractor.convert_to_env_var_format("process.env.SERVICE_URL + \"/api/data\""),
        Some("ENV_VAR:SERVICE_URL:/api/data".to_string())
    );
}
```

---

## Issue 3: Query Parameters Creating Wrong Type Aliases

### Problem Description

URLs with query parameters generate incorrect type alias names:
- URL: `/orders?userId=:userId`
- Current alias: `GetOrdersUseridUseridResponseConsumerCall1`
- Expected alias: `GetOrdersResponseConsumerCall1`

The query string `?userId=:userId` is being parsed as path segments.

### Root Cause Analysis

The `sanitize_route_for_dynamic_paths()` function in `src/analyzer/mod.rs` splits the route by `/` without first stripping query parameters:

```rust
fn sanitize_route_for_dynamic_paths(route: &str) -> String {
    route
        .split('/')
        .filter(|segment| !segment.is_empty())
        .map(|segment| {
            // ... processes each segment
        })
        .collect()
}
```

When the route is `/orders?userId=:userId`, it splits into:
- `orders?userId=:userId` (single segment with query string embedded)

This segment contains `:userId` which gets converted to `ByUserid`.

### Files Involved

| File | Function | Role |
|------|----------|------|
| `src/analyzer/mod.rs` | `sanitize_route_for_dynamic_paths()` L438-464 | Converts route to PascalCase alias |
| `src/analyzer/mod.rs` | `generate_common_type_alias_name()` L317-335 | Generates type alias names |
| `src/analyzer/mod.rs` | `generate_unique_call_alias_name()` L340-360 | Generates unique call alias names |

### Proposed Fix

**Strip query parameters at the start of `sanitize_route_for_dynamic_paths()`**

```rust
fn sanitize_route_for_dynamic_paths(route: &str) -> String {
    // Strip query parameters first
    let route_without_query = if let Some(query_idx) = route.find('?') {
        &route[..query_idx]
    } else {
        route
    };
    
    route_without_query
        .split('/')
        .filter(|segment| !segment.is_empty())
        .map(|segment| {
            if let Some(param_name) = segment.strip_prefix(':') {
                format!("By{}", Self::to_pascal_case(param_name))
            } else if segment.starts_with("${") && segment.ends_with('}') {
                let inner = &segment[2..segment.len() - 1];
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

### Alternative: Strip in `normalize_route_for_type_name()`

If we want a single place for all route normalization, we could strip query params in `normalize_route_for_type_name()` instead:

```rust
fn normalize_route_for_type_name(route: &str) -> String {
    // Strip query parameters first
    let route = if let Some(query_idx) = route.find('?') {
        &route[..query_idx]
    } else {
        route
    };
    
    if route.contains("ENV_VAR:") {
        // ... existing env var handling
    } else {
        route.to_string()
    }
}
```

### Test Cases to Add

```rust
#[test]
fn test_sanitize_route_strips_query_params() {
    assert_eq!(
        Analyzer::sanitize_route_for_dynamic_paths("/orders?userId=123"),
        "Orders"
    );
    assert_eq!(
        Analyzer::sanitize_route_for_dynamic_paths("/users/:id?include=posts"),
        "UsersById"
    );
    assert_eq!(
        Analyzer::sanitize_route_for_dynamic_paths("/api/data?page=1&limit=10"),
        "ApiData"
    );
}

#[test]
fn test_generate_alias_with_query_params() {
    // Query params should NOT affect alias name
    let alias1 = Analyzer::generate_unique_call_alias_name(
        "/orders?userId=:userId",
        "GET",
        false,
        1,
        true,
    );
    let alias2 = Analyzer::generate_unique_call_alias_name(
        "/orders",
        "GET",
        false,
        1,
        true,
    );
    
    assert_eq!(alias1, alias2);
}
```

---

## Implementation Order

### Phase 1: Fix Query Parameter Issue (Low Risk, High Impact) ✅ COMPLETE
1. ✅ Update `sanitize_route_for_dynamic_paths()` to strip query params
2. ✅ Add test cases
3. ✅ Run existing tests to ensure no regressions

### Phase 2: Fix Configuration Suggestions Format (Medium Risk, High Impact) ✅ COMPLETE
1. ✅ Add helper functions `extract_env_var_name()` and `extract_path_from_env_var_route()` to `src/analyzer/mod.rs`
2. ✅ Update `analyze_matches_with_mount_graph()` to use new format
3. ✅ Verify formatter correctly parses new format
4. ✅ Add test cases

### Phase 3: Fix `expr_to_string()` to Preserve Nested Member Expressions (HIGH PRIORITY)

**Status**: 🔴 Required - This is the root cause of `[member_expr]` issue

**The Fix**:

Update `expr_to_string()` in `src/call_site_extractor.rs` to recursively handle nested member expressions:

```rust
fn expr_to_string(&self, expr: &Expr) -> String {
    match expr {
        Expr::Ident(ident) => ident.sym.to_string(),
        Expr::Member(member) => {
            // Recursively build the full member expression
            let obj_str = self.expr_to_string(&member.obj);
            let prop_str = match &member.prop {
                MemberProp::Ident(ident) => ident.sym.to_string(),
                MemberProp::Computed(computed) => {
                    format!("[{}]", self.expr_to_string(&computed.expr))
                }
                MemberProp::PrivateName(name) => format!("#{}", name.name),
            };
            format!("{}.{}", obj_str, prop_str)
        }
        Expr::Lit(Lit::Str(s)) => s.value.to_string(),
        Expr::Lit(Lit::Num(n)) => n.value.to_string(),
        _ => "...".to_string(),
    }
}
```

**Result**: 
- `process.env.API_URL` → `"process.env.API_URL"` (instead of `"member_expr"`)
- Template literals become `${process.env.API_URL}/users` (instead of `${member_expr}/users`)
- `extract_env_var_name()` can then extract `API_URL` correctly

**Risk**: Low - this is a pure improvement with no behavior change for simple expressions

### Phase 4: Improve Env Var Detection Logic (Medium Priority)

**Status**: 🟡 Recommended - Prevents false positives

**The Problem**:

Current check in `analyze_matches_with_mount_graph()`:
```rust
if call.route.contains("ENV_VAR:")
    || call.route.contains("process.env")
    || call.route.contains("${")  // TOO BROAD!
```

This flags `/users/${userId}` as an env var URL when it's actually a path parameter.

**The Fix**:

Add smarter detection that distinguishes base URL env vars from path parameters:

```rust
fn is_env_var_base_url(route: &str) -> bool {
    // Check for ENV_VAR: prefix format
    if route.starts_with("ENV_VAR:") {
        return true;
    }
    
    // Check for process.env at the start
    if route.starts_with("${process.env.") || route.contains("process.env.") && !route.starts_with('/') {
        return true;
    }
    
    // Check for ${UPPER_CASE_VAR} at the start (common env var pattern)
    if route.starts_with("${") {
        if let Some(end) = route.find('}') {
            let var_name = &route[2..end];
            // If it's all uppercase with underscores, likely an env var
            if var_name.chars().all(|c| c.is_uppercase() || c == '_' || c == '.') {
                return true;
            }
        }
    }
    
    false
}
```

**Risk**: Medium - changes which routes get flagged as env var URLs

### Phase 5: Preserve Env Var Info Through URL Extraction Pipeline (Lower Priority)

**Status**: 🟡 Deferred - Complex refactoring

This is the original Phase 3 plan. After fixing `expr_to_string()`, the template literals will correctly contain `${process.env.API_URL}` instead of `${member_expr}`. However, `extract_path_from_url()` still strips this prefix.

**Options**:

A. **Store both formats** - Add `original_template` field to `FetchCallInfo`
B. **Use ENV_VAR: format** - Convert `${process.env.X}/path` to `ENV_VAR:X:/path` during extraction
C. **Extract on demand** - Keep path-only in route, extract env var name only when needed for config suggestions

**Recommendation**: Option C is simplest - the current `extract_env_var_name()` helper can work with the route as-is once `expr_to_string()` is fixed.

---

## Testing Strategy

### Unit Tests
- Add tests for each new function
- Update existing tests that may be affected

### Integration Tests
- Run on test repos (repo-a, repo-c)
- Verify:
  - Configuration suggestions show actual env var names
  - Query params don't affect type alias matching
  - Type checking still works correctly

### Regression Tests
- Ensure existing passing tests still pass
- Run full test suite: `cargo test`
- Run clippy: `cargo clippy --all-targets -- -D warnings`

---

## Files to Modify

| File | Changes |
|------|---------|
| `src/analyzer/mod.rs` | Add `extract_env_var_name()`, `extract_path_from_env_var_route()`, update `sanitize_route_for_dynamic_paths()`, update env_var_calls format |
| `src/call_site_extractor.rs` | Add `extract_fetch_url_with_env_info()`, `convert_to_env_var_format()`, update usage in `visit_var_decl()` |
| `tests/url_alias_matching_test.rs` | Add tests for query param stripping |
| `src/analyzer/mod.rs` (tests section) | Add tests for env var extraction |

---

## Estimated Effort

| Phase | Effort | Risk | Status |
|-------|--------|------|--------|
| Phase 1: Query Params | 1 hour | Low | ✅ Complete |
| Phase 2: Config Format | 2 hours | Medium | ✅ Complete |
| Phase 3: Fix expr_to_string() | 1 hour | Low | ✅ Complete |
| Phase 4: Env Var Detection | 2 hours | Medium | ✅ Complete |
| Phase 5: Full Env Var Preservation | 4-6 hours | High | 🟡 Deferred |
| Testing & Verification | 2 hours | - | ✅ Complete |
| **Total (Phase 1-4)** | **8 hours** | - | ✅ Complete |
| **Total (all phases)** | **12-14 hours** | - | Phase 5 Remaining |

---

## Success Criteria

### Phase 1-2 (Complete)
1. ✅ **Query Parameters**: URL `/orders?userId=:userId` generates alias `GetOrdersResponseConsumerCall1`
2. ✅ **Configuration Message Format**: Messages now use format parseable by formatter
3. ✅ **Type Matching**: Consumers and producers correctly match when paths are equivalent (ignoring query params)
4. ✅ **All Tests Pass**: No regressions in existing functionality

### Phase 3 (Complete)
1. ✅ **Fix `[member_expr]`**: Configuration suggestions now show `[API_URL]` instead of `[member_expr]`
2. ✅ **Template literals preserve info**: `${process.env.API_URL}` now becomes `${process.env.API_URL}` not `${member_expr}`

### Phase 4 (Complete)
1. ✅ **No false positives**: `/users/${userId}` is no longer flagged as env var URL
2. ✅ **Correct detection**: Only routes with actual env var base URLs trigger config suggestions

### Phase 5 (Deferred)
1. 🟡 **Full env var names**: Show actual env var name like `[ORDER_SERVICE_URL]` in all cases
2. 🟡 **Consumer types**: Improve fetch correlation to extract types from more patterns

---

## Implementation Notes

### Changes Made (Phase 1-2)

**`src/analyzer/mod.rs`**:
- Updated `sanitize_route_for_dynamic_paths()` to strip query parameters before processing
- Added `extract_env_var_name()` helper function
- Added `extract_path_from_env_var_route()` helper function  
- Updated env_var_calls message format to: `"Environment variable endpoint: {} using env vars [{}] in ENV_VAR:{}:{}"`
- Added tests for all new functionality

### Phase 3 Implementation ✅ COMPLETE

**`src/call_site_extractor.rs`**:
- Fixed `expr_to_string()` to recursively handle nested member expressions
- Added `#[allow(clippy::only_used_in_recursion)]` attribute since `&self` is only used in recursive calls
- All existing tests continue to pass
- Template literal extraction now preserves `process.env.X` patterns

### Phase 4 Implementation ✅ COMPLETE

**`src/analyzer/mod.rs`**:
- Added `is_env_var_base_url()` helper function with smart detection logic
- Updated env var detection logic in `analyze_matches_with_mount_graph()` to use new helper
- Added comprehensive tests for `is_env_var_base_url()`

### Phase 5 Deferred Work

When implementing Phase 5, the following changes are needed:

1. **`src/call_site_extractor.rs`**:
   - Add `convert_url_preserving_env_var()` method 
   - Optionally add `original_template` field to `FetchCallInfo` struct

2. **Tests to update**:
   - `test_fetch_to_json_correlation_template_literal` - may need adjustment
   - `test_template_literal_with_multiple_dynamic_params` - may need adjustment