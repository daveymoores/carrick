# Mock Mode Improvements - Progress Update

## Issues Identified

From the user's feedback, several problems were identified with the mock mode output:

1. ❌ **Routes showing as "unknown"** - API calls not extracting URLs
2. ❌ **"N/A" appearing** - Mock endpoints showing invalid method/path combinations
3. ❌ **Duplicate output** - Analysis results printed twice  
4. ✅ **No mount relationships** - FIXED: Now detecting 3 mounts correctly
5. ❌ **Router endpoints not being analyzed** - Endpoints defined in router files not detected

## Fixes Applied

### 1. Router Mount Detection ✅

**Problem**: Triage was classifying all `app.use()` calls as `Middleware` instead of distinguishing between:
- `app.use(express.json())` → Middleware (1 arg or non-router arg)
- `app.use('/users', userRouter)` → RouterMount (path + router)

**Solution**: 
- Modified `generate_mock_triage_response()` to check argument types
- Check if `app.use()` has 2 args where first is `StringLiteral` and second is `Identifier`
- Modified triage agent to pass full `CallSite` objects in mock mode (not lean)

**Result**: Now correctly detecting 3 router mounts:
```
Classification breakdown: {"HttpEndpoint": 10, "RouterMount": 3, "Middleware": 10, "Irrelevant": 3}
Built mount graph:
  - 7 nodes
  - 3 mounts
  - 10 endpoints
```

## Remaining Issues

### 1. Router Endpoints Not Detected ❌

**Problem**: The test fixture has routers in separate files:
- `routes/users.ts`: Defines `router.get('/:id')`, `router.post('/')`, `router.get('/')`
- `routes/api.ts`: Defines `router.get('/posts')`, `router.post('/posts')`, etc.
- `routes/health.ts`: Defines `router.get('/status')`, `router.get('/ping')`, etc.

Mock generator only detects endpoints where `callee_object == "get" | "post" | ...`, but these routers use `router.get()` not `app.get()`.

**Expected**: Should detect ~13 total endpoints across all files
**Actual**: Only detecting 10 endpoints (probably just from app.ts)

**Fix Needed**: Mock endpoint generator should accept any `callee_object`, not just check for HTTP methods in `callee_property`.

### 2. "unknown" and "N/A" in Output ❌

**Problem**: Output shows:
```
6 Missing Endpoints: all showing "unknown"
8 Orphaned Endpoints: including "/N/A" and "N/A"
```

**Root Causes**:
- Consumer mock generator returns `url: null` →converts to "unknown"
- Endpoint mock generator uses "/" as default → could become "N/A" somewhere
- Path resolution might not be working for all endpoints

### 3. Duplicate Output ❌

**Problem**: The Carrick output appears twice:
```
### 🪢 CARRICK: API Analysis Results
Analyzed **10 endpoints**...
(full output)

Analyzed current repo: imported-routers
...

### 🪢 CARRICK: API Analysis Results  
Analyzed **10 endpoints**...
(same output again)
```

**Root Cause**: The formatter is being called twice:
1. Once in `analyze_current_repo()` after local analysis
2. Once in cross-repo analysis after downloading other repos

**Fix Needed**: Only print results once, probably at the end of cross-repo analysis.

## Next Steps

### Priority 1: Fix Router Endpoint Detection

Modify `generate_mock_endpoint_response()` to detect endpoints on any object (router, app, fastify, etc.):

```rust
// Current: Only matches if callee_property is an HTTP method
if matches!(callee_property, "get" | "post" | "put" | "delete" | "patch") {
    // ...
}

// Should be: Match any callee_object with HTTP method property
if matches!(callee_property, "get" | "post" | "put" | "delete" | "patch") {
    // Works for both app.get() and router.get()
    Some(serde_json::json!({
        "method": callee_property.to_uppercase(),
        "path": path,
        "handler": "handler",
        "node_name": callee_object,  // This will be "router" or "app"
        // ...
    }))
}
```

### Priority 2: Fix Duplicate Output

Find where `print_results()` is being called and ensure it only runs once:
- Option A: Remove the call in `analyze_current_repo()`
- Option B: Add a flag to skip printing in local-only mode

### Priority 3: Fix "unknown" and "N/A"

- Review path resolution in mount graph
- Check why some endpoints have invalid method/path combinations
- Ensure consumer mock returns valid URLs (even if placeholder)

## Test Expectations

For `tests/fixtures/imported-routers`, we should see:

**Mounts** (3 total): ✅ WORKING
- app → userRouter at `/users`
- app → apiRouter at `/api/v1`
- app → healthRouter at `/health`

**Endpoints** (13 total): ❌ ONLY 10 DETECTED
- From users.ts (3): GET /:id, POST /, GET /
  - Should resolve to: GET /users/:id, POST /users, GET /users
- From api.ts (4): GET /posts, POST /posts, GET /stats, DELETE /posts/:id
  - Should resolve to: GET /api/v1/posts, POST /api/v1/posts, GET /api/v1/stats, DELETE /api/v1/posts/:id
- From health.ts (3): GET /status, GET /ping, GET /ready
  - Should resolve to: GET /health/status, GET /health/ping, GET /health/ready
- From app.ts (3): Middleware calls (not HTTP endpoints)

**Output**: ❌ NEEDS FIX
- Should show 13 endpoints with proper paths
- Should show 0 API calls (no fetch/axios in this fixture)
- Should show 3 router mounts
- Should appear only ONCE

## Code Locations

- Triage mock: `src/gemini_service.rs:generate_mock_triage_response()`
- Endpoint mock: `src/gemini_service.rs:generate_mock_endpoint_response()`
- Mount mock: `src/gemini_service.rs:generate_mock_mount_response()`
- Duplicate output: `src/engine/mod.rs:analyze_current_repo()` and `build_cross_repo_analyzer()`
- Path resolution: `src/mount_graph.rs:resolve_endpoint_paths()`
