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
