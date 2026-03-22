# Plan: Cross-Repo API Relationship Graph Webapp

## Overview

A polished standalone webapp that visualizes how services connect across repositories — which repos produce API endpoints and which repos consume them, with type compatibility status. Each analysis generates a unique shareable link. No authentication required.

## Architecture

```
┌──────────────────┐       ┌──────────────────────┐       ┌─────────────────────┐
│  Existing Infra  │       │   New Lambda          │       │   Webapp (SPA)      │
│                  │       │                       │       │                     │
│  DynamoDB        │──────▶│  GET /graph/{org}     │──────▶│  Next.js + shadcn/ui│
│  (CloudRepoData) │       │  (read-only, no auth) │       │  + Cytoscape.js     │
└──────────────────┘       └──────────────────────┘       │  (Cloudflare Pages) │
                                                          └─────────────────────┘
```

**Data already exists** — `CloudRepoData` in DynamoDB contains all the endpoints, calls, mounts, type manifests, and dependency info per repo. No new data collection is needed.

---

## Step 1: New Lambda — Graph Data API

**New file:** `lambdas/graph-api/index.mjs`

Add a read-only Lambda that queries DynamoDB for all repos in an org and returns a graph-ready JSON payload:

```json
{
  "id": "abc123",
  "org": "my-org",
  "generatedAt": "2026-03-21T...",
  "services": [
    {
      "id": "repo-a",
      "repoName": "repo-a",
      "serviceName": "user-service",
      "lastUpdated": "...",
      "commitHash": "abc123",
      "endpoints": [
        { "id": "repo-a::GET::/api/users/:id", "method": "GET", "path": "/api/users/:id", "handler": "getUser", "hasTypes": true }
      ],
      "calls": [
        { "id": "repo-a::GET::/api/payments", "method": "GET", "targetUrl": "/api/payments", "client": "axios" }
      ]
    }
  ],
  "connections": [
    {
      "from": "repo-b::GET::/api/users",
      "to": "repo-a::GET::/api/users/:id",
      "typeStatus": "compatible | mismatch | unknown",
      "typeDetail": { "requestMatch": true, "responseMatch": false, "producerType": "User", "consumerType": "UserDTO" }
    }
  ]
}
```

The webapp transforms this into Cytoscape compound elements:
- Each service → parent node
- Each endpoint/call → child node with `parent: serviceId`
- Each connection → edge between specific child nodes

**How edges are computed:** For each repo's `calls`, match against every other repo's `endpoints` using the existing URL normalizer logic (or a simplified JS version). If type manifests exist for both sides, report compatibility status.

### Unique Link Generation

Two new endpoints:

1. **`POST /graph/{org}/snapshot`** — Takes a snapshot of the current graph state, stores it in DynamoDB with a unique ID (nanoid), returns the ID
2. **`GET /graph/{org}/snapshot/{id}`** — Retrieves a stored snapshot by ID

This way each CI run or manual trigger can generate a unique URL like:
```
https://graph.carrick.dev/snapshot/abc123def
```

The snapshot captures the graph at a point in time, so the link is stable and shareable in PRs, Slack, etc.

**Terraform additions:**
- New Lambda resource (`graph-api`)
- New API Gateway routes: `GET /graph/{org}`, `POST /graph/{org}/snapshot`, `GET /graph/{org}/snapshot/{id}`
- IAM: DynamoDB read + write (for snapshots)
- CORS headers enabled (no auth)

---

## Step 2: Webapp — Next.js + shadcn/ui + Cytoscape.js

**New directory:** `webapp/`

### Tech stack

| Package | Purpose |
|---------|---------|
| **Next.js 15** | Framework — SSR for snapshot pages (good for link previews/SEO), static for app shell |
| **shadcn/ui** | Beautiful, accessible component library (built on Radix + Tailwind) |
| **Tailwind CSS 4** | Styling — consistent, professional look |
| **Cytoscape.js** | Graph visualization — compound nodes, force-directed layouts, pan/zoom, selection |
| **cytoscape-cose-bilkent** | Better layout algorithm for compound/network graphs |
| **@tanstack/react-query** | Data fetching with caching |
| **Deployed to:** | **Cloudflare Pages** (fast global CDN, free tier, simple Next.js support via `@cloudflare/next-on-pages`) |

### Graph visualization — Compound Nodes

Services are rendered as **compound (parent) nodes** containing their individual endpoints as child nodes. Edges connect specific endpoints across services.

```
┌─────────────────────────┐       ┌─────────────────────────┐
│  user-service           │       │  order-service           │
│  ┌───────────────────┐  │       │  ┌───────────────────┐  │
│  │ GET /api/users/:id │◀─────────│ │ fetch(/api/users)  │  │
│  └───────────────────┘  │       │  └───────────────────┘  │
│  ┌───────────────────┐  │       │  ┌───────────────────┐  │
│  │ POST /api/users    │  │       │  │ GET /api/orders    │  │
│  └───────────────────┘  │       │  └───────────────────┘  │
└─────────────────────────┘       └─────────────────────────┘
```

- **Parent nodes** = services (labeled with service/repo name, styled as containers)
- **Child nodes** = individual endpoints (producers) and calls (consumers)
- **Edges** = connect a consumer call in one service to the matching producer endpoint in another service
- **Edge color** by type compatibility: green=compatible, red=mismatch, gray=unknown
- **Layout**: `cose-bilkent` — handles compound graphs natively, keeps children inside parents
- Click a parent node → slide-out sheet with full service details
- Click a child node → endpoint/call detail with type info
- Click an edge → type compatibility breakdown
- Hover tooltips, dark mode via shadcn/ui theme

### Routes

| Route | Purpose |
|-------|---------|
| `/org/[orgName]` | Live graph — current state of all repos in org |
| `/snapshot/[id]` | Frozen snapshot — unique shareable link, shows graph as-of snapshot time |

### UI Layout

```
┌─────────────────────────────────────────────────────┐
│  Carrick Graph    [org selector]    [Share ▼] [⚙]  │
├────────────────────────────────────┬────────────────┤
│                                    │                │
│    ┌─────┐         ┌─────┐        │  Repo Detail   │
│    │Repo │────────▶│Repo │        │                │
│    │  A  │         │  B  │        │  Endpoints:    │
│    └──┬──┘         └─────┘        │  GET /users    │
│       │                           │  POST /orders  │
│       ▼                           │                │
│    ┌─────┐                        │  Calls:        │
│    │Repo │                        │  GET /payments │
│    │  C  │                        │                │
│    └─────┘                        │                │
│                                    │                │
│  [Filter: All ▼] [Status ▼]      │                │
├────────────────────────────────────┴────────────────┤
│  3 services · 8 connections · 2 mismatches          │
└─────────────────────────────────────────────────────┘
```

### shadcn/ui components used

- `Sheet` — slide-out detail panels
- `Card` — node info cards
- `Badge` — status indicators (compatible/mismatch)
- `Select` — org selector, filters
- `Tooltip` — hover info on graph elements
- `Button` — actions (share, refresh)
- `Separator`, `ScrollArea` — layout
- `Popover` — share link with copy button
- Dark/light mode toggle

### Minimal file structure

```
webapp/
├── next.config.ts
├── package.json
├── tailwind.config.ts
├── src/
│   ├── app/
│   │   ├── layout.tsx
│   │   ├── page.tsx                    # Redirect to /org/...
│   │   ├── org/[orgName]/page.tsx      # Live graph view
│   │   └── snapshot/[id]/page.tsx      # Snapshot view
│   ├── components/
│   │   ├── ui/                         # shadcn/ui components
│   │   ├── graph/
│   │   │   ├── GraphCanvas.tsx         # Cytoscape wrapper
│   │   │   ├── GraphControls.tsx       # Zoom, fit, layout buttons
│   │   │   └── NodeTooltip.tsx         # Hover tooltip
│   │   ├── detail/
│   │   │   ├── RepoSheet.tsx           # Repo detail slide-out
│   │   │   └── ConnectionSheet.tsx     # Edge detail slide-out
│   │   ├── FilterBar.tsx               # Status/method filters
│   │   ├── Legend.tsx                   # Color/size legend
│   │   └── SharePopover.tsx            # Copy shareable link
│   ├── lib/
│   │   ├── api.ts                      # API client
│   │   ├── graph-transform.ts          # API data → Cytoscape elements
│   │   └── utils.ts                    # cn() helper etc.
│   └── types/
│       └── graph.ts                    # TypeScript types
└── tsconfig.json
```

---

## Step 3: PR Integration (Unique Link)

Update the existing Carrick GitHub Action to:
1. After analysis, call `POST /graph/{org}/snapshot` to generate a snapshot
2. Include the unique graph link in the PR comment, e.g.:

```markdown
### 🪢 CARRICK: API Analysis Results
...existing output...

📊 [View API relationship graph →](https://graph.carrick.dev/snapshot/abc123def)
```

This is a small change to `action.yml` and the formatter output.

---

## Step 4: Deployment

### Webapp (Cloudflare Pages)
- Connect the `webapp/` directory to Cloudflare Pages
- Uses `@cloudflare/next-on-pages` for Next.js compatibility
- Auto-deploys on push to `main`
- Custom domain: `graph.carrick.dev` (optional)
- Free tier: unlimited bandwidth, 500 builds/month

### Lambda + API Gateway (Terraform)
- Add graph-api Lambda and routes to existing Terraform config
- Same DynamoDB table, just new access patterns

---

## Implementation Order

1. **Lambda + Terraform** — graph data API endpoint + snapshot storage
2. **Webapp scaffold** — Next.js + shadcn/ui + Tailwind setup
3. **Graph rendering** — Cytoscape canvas with nodes, edges, layout
4. **Detail panels** — shadcn Sheet components for repo/connection details
5. **Snapshot + sharing** — unique link generation and share UI
6. **PR integration** — add graph link to formatter output
7. **Deploy** — Cloudflare Pages for webapp, Terraform apply for Lambda

## Why these packages

| Choice | Rationale |
|--------|-----------|
| **Next.js** | SSR for snapshot pages (link previews work in Slack/GitHub), great DX |
| **shadcn/ui** | Beautiful defaults, fully customizable, no heavy runtime |
| **Cytoscape.js** | Native compound node support, force-directed layouts handle service→endpoint hierarchy, 10K+ node scale |
| **Cloudflare Pages** | Fast global CDN, generous free tier, simpler than self-hosting, good Next.js support |
| **@tanstack/react-query** | Handles loading/error/cache states cleanly |

## Scope boundaries

- No auth (per user request)
- Read-only — no write operations from the webapp (except snapshot creation from CI)
- Snapshots are immutable once created
- No real-time updates (refresh or regenerate snapshot)

---

## Appendix: Actual Data Shapes

This section documents the real data structures output by the Carrick tool, as they exist in the codebase today. The graph API Lambda must transform these into the graph-ready format above.

### Source of truth: `CloudRepoData` (Rust → JSON)

This is what gets serialized and stored in DynamoDB's `cloudRepoData` field per repo. Defined in `src/cloud_storage/mod.rs`:

```jsonc
{
  "repo_name": "my-api",
  "service_name": "user-service",           // Optional — from carrick.json serviceName
  "endpoints": [                            // Producer endpoints (Vec<ApiEndpointDetails>)
    {
      "owner": { "App": "app" },            // OwnerType enum: App(String) | Router(String) | null for calls
      "route": "/api/users/:id",            // The resolved full path
      "method": "GET",
      "params": [],                         // Extracted route params
      "request_body": null,                 // Optional Json enum (Null | Boolean | Number | String | Array | Object)
      "response_body": null,                // Optional Json enum
      "handler_name": "getUser",            // Optional handler function name
      "request_type": null,                 // Optional TypeReference (see below)
      "response_type": null,                // Optional TypeReference
      "file_path": "src/routes/users.ts"
    }
  ],
  "calls": [                                // Consumer API calls (same Vec<ApiEndpointDetails> shape)
    {
      "owner": null,                        // Calls have no owner
      "route": "/api/payments",             // Target URL
      "method": "GET",
      "params": [],
      "request_body": null,
      "response_body": null,
      "handler_name": "axios",              // Client library used (fetch, axios, got, etc.)
      "request_type": null,
      "response_type": null,
      "file_path": "src/services/payment.ts"
    }
  ],
  "mounts": [                               // Router mount relationships
    {
      "parent": { "App": "app" },
      "child": { "Router": "userRouter" },
      "prefix": "/api"
    }
  ],
  "apps": {},                               // HashMap<String, AppContext> — { name: String }
  "imported_handlers": [                    // Vec<(route, method, handler_name, source)>
    ["/users", "GET", "getUser", "./controllers/users"]
  ],
  "function_definitions": {},               // HashMap<String, FunctionDefinition>
  "config_json": "{...}",                   // Raw carrick.json content (optional)
  "package_json": "{...}",                  // Raw package.json content (optional)
  "packages": {                             // Optional structured package data
    "package_jsons": [
      {
        "name": "my-api",
        "version": "1.0.0",
        "dependencies": { "express": "^4.18.0" },
        "dev_dependencies": { "typescript": "^5.0.0" },
        "peer_dependencies": {}
      }
    ],
    "source_paths": ["package.json"],
    "merged_dependencies": {
      "express": { "name": "express", "version": "^4.18.0", "source_path": "package.json" }
    }
  },
  "last_updated": "2026-03-21T10:30:00Z",  // DateTime<Utc>
  "commit_hash": "abc123def",
  "mount_graph": {                          // Optional MountGraph — framework-agnostic analysis
    "nodes": {
      "app": {
        "name": "app",
        "node_type": "Root",                // Root | Mountable | Unknown
        "creation_site": "const app = express()",
        "file_location": "src/index.ts:3"
      }
    },
    "mounts": [
      {
        "parent": "app",
        "child": "userRouter",
        "path_prefix": "/api",
        "middleware_stack": ["authMiddleware"]
      }
    ],
    "endpoints": [                          // ResolvedEndpoint — with computed full_path
      {
        "method": "GET",
        "path": "/:id",                     // Local path on the router
        "full_path": "/api/users/:id",      // Computed full path including mount prefixes
        "handler": "getUser",
        "owner": "userRouter",
        "file_location": "src/routes/users.ts:15",
        "middleware_chain": ["authMiddleware"],
        "repo_name": null                   // Optional — for cross-repo matching
      }
    ],
    "data_calls": [                         // DataFetchingCall
      {
        "method": "GET",
        "target_url": "/api/payments",
        "client": "axios",
        "file_location": "src/services/payment.ts:42"
      }
    ]
  },
  "bundled_types": "declare type Endpoint_abc_Response = { id: number; name: string; };",  // Optional .d.ts content
  "type_manifest": [                        // Optional Vec<TypeManifestEntry>
    {
      "method": "GET",
      "path": "/api/users/:id",
      "role": "producer",                   // "producer" | "consumer"
      "type_kind": "response",              // "request" | "response"
      "type_alias": "Endpoint_abc123_Response",  // Alias in the bundled .d.ts
      "file_path": "src/routes/users.ts",
      "line_number": 15,
      "is_explicit": true,                  // Was the type explicitly annotated?
      "type_state": "explicit",             // "explicit" | "implicit" | "unknown"
      "evidence": {
        "file_path": "src/routes/users.ts",
        "span_start": 450,                  // Byte offset (optional)
        "span_end": 520,                    // Byte offset (optional)
        "line_number": 15,
        "infer_kind": "FunctionReturn",     // FunctionReturn | Expression | CallResult | Variable | ResponseBody | RequestBody
        "is_explicit": true,
        "type_state": "explicit"
      }
    }
  ]
}
```

### DynamoDB item shape (what the Lambda reads)

The `get-cross-repo-data` action in `lambdas/check-or-upload/index.js` scans DynamoDB and returns:

```jsonc
// Response from action: "get-cross-repo-data"
{
  "repos": [
    {
      "repo": "my-api",                     // Extracted from pk: "repo#org/my-api"
      "hash": "abc123def",
      "s3Url": "https://bucket.s3.amazonaws.com/org/my-api/abc123def/output.json",
      "filename": "output.json",
      "metadata": { /* CloudRepoData JSON — full shape above */ },
      "lastUpdated": "2026-03-21T10:30:00Z"
    }
  ],
  "processing_errors": []                   // Only present if errors occurred
}
```

**DynamoDB key schema** (table: `CarrickTypeFiles`):
- **pk** (partition key): `repo#${org}/${repo}` (e.g., `repo#my-org/my-api`)
- **sk** (sort key): always `types`

### Formatter output (GitHub PR comment)

The tool's final markdown output (`src/formatter/mod.rs`) is wrapped in machine-readable delimiters:

```
<!-- CARRICK_OUTPUT_START -->
<!-- CARRICK_ISSUE_COUNT:5 -->
### 🪢 CARRICK: API Analysis Results

Analyzed **12 endpoints** and **8 API calls** across all repositories.

Found **5 total issues**: **2 critical mismatches**, **1 connectivity issues**,
**1 dependency conflicts**, and **1 configuration suggestions**.

<details>
<summary><strong>2 Critical: API Mismatches</strong></summary>
  - Type compatibility issues (TypeScript compiler errors, request body mismatches)
  - Grouped by endpoint with producer/consumer type details
</details>

<details>
<summary><strong>1 Connectivity Issues</strong></summary>
  - Missing endpoints (called but not defined) — table of method + path
  - Orphaned endpoints (defined but never called) — table of method + path
</details>

<details>
<summary><strong>1 Dependency Conflicts</strong></summary>
  - Grouped by severity: Critical (major), Warning (minor), Info (patch)
  - Table of repo + version + source for each conflict
</details>

<details>
<summary><strong>1 Configuration Suggestions</strong></summary>
  - Env var calls that need classifying in carrick.json
</details>
<!-- CARRICK_OUTPUT_END -->
```

### Key observations for the graph API

1. **`mount_graph` is the richest source** — it has resolved full paths, file locations, middleware chains, and the mount hierarchy. Prefer this over the flat `endpoints`/`calls` arrays when available.
2. **`type_manifest` + `bundled_types`** together enable type compatibility checks. The manifest maps endpoints to type aliases; the bundled `.d.ts` contains the actual type definitions.
3. **`calls[].route`** contains the target URL (what the consumer calls). **`endpoints[].route`** contains the served path. URL normalization is needed to match them (e.g., `/api/users/:id` vs `/api/users/${userId}`).
4. **`calls[].handler_name`** holds the HTTP client name (axios, fetch, got), not a function name.
5. **`service_name`** comes from `carrick.json` — it's the human-readable service name. Falls back to `repo_name` if absent.
6. **`owner`** is a tagged enum: `{ "App": "app" }` or `{ "Router": "userRouter" }` — not a plain string.
