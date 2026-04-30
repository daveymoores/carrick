# Service Map Visualization — Implementation Brief

Spec for porting the marketing-site dependency graph into the actual Carrick service-map pages.

The reference implementation lives at `carrick-site/src/components/DependencyGraph.jsx` (~170 lines, React + raw `<canvas>`). It uses hardcoded demo data; the real version wires to analyzer output. **Match the visual style exactly — the look is what we want to preserve.**

## Where this lives

Implementation target is `carrick-cloud/app/` (the web dashboard) per the public/private split — see [`growth-playbook.md`](./growth-playbook.md) §product surface §4 and [`public-private-split.md`](./public-private-split.md). The spec lives in this repo for now because `carrick-cloud` doesn't exist yet.

## Reference

Read `carrick-site/src/components/DependencyGraph.jsx` first. The component renders to a single `<canvas>` element, no SVG, no D3, no react-flow. Every magic number below is taken directly from that file — don't round, don't "improve."

## Design tokens (load-bearing — do not drift)

```ts
// Container
const CONTAINER  = { width: "100%", height: 520 /* px */ };
const BACKGROUND = "transparent"; // page provides dark bg
const NODE_FILL  = "rgba(12,12,16,0.85)"; // near-black, lets halo show through

// Service color palette — assign deterministically by hash(serviceId) % 5
const PALETTE = [
  { r: 167, g: 139, b: 250 }, // violet
  { r:  96, g: 165, b: 250 }, // blue
  { r: 251, g: 191, b:  36 }, // amber
  { r:  52, g: 211, b: 153 }, // green
  { r: 244, g: 114, b: 182 }, // pink — reserve for `consumer` type (frontends)
];

// Node geometry by type
const NODE = {
  service:  { radius: 24, strokeWidth: 1.5, strokeAlpha: 0.55, labelSize: 10.5, labelAlpha: 0.6  },
  consumer: { radius: 28, strokeWidth: 1.5, strokeAlpha: 0.55, labelSize: 10.5, labelAlpha: 0.6  },
  endpoint: { radius: 14, strokeWidth: 1.0, strokeAlpha: 0.30, labelSize:  9.0, labelAlpha: 0.35 },
};
const LABEL_FONT       = "JetBrains Mono, monospace";
const LABEL_OFFSET     = 14;   // px below node: y + radius + 14
const HALO_RADIUS_MULT = 2.5;  // halo extends to radius * 2.5
const HALO_INNER_ALPHA = 0.1;  // rgba(r,g,b,0.1) → rgba(r,g,b,0)

// Edges
const EDGE_LINE_COLOR = "rgba(255,255,255,0.05)";
const EDGE_LINE_WIDTH = 1;
const PACKET = {
  radius:    1.5,
  alpha:     0.35,  // colored in the source node's color
  periodMs:  2800,  // t = (Date.now()/periodMs + i*phaseStep) % 1
  phaseStep: 0.065, // per-edge phase offset, indexed by edge order
};

// Monorepo cluster glow
const CLUSTER = {
  paddingPx: 70, // gradient extends to maxDist + 70 from centroid
  stops: [
    [0,   "rgba(167,139,250,0.045)"],
    [0.7, "rgba(167,139,250,0.015)"],
    [1,   "rgba(167,139,250,0)"],
  ],
  label: {
    size:                9.5,
    alpha:               0.3,
    color:               "rgba(167,139,250,0.3)",
    offsetAboveTopNode:  24, // y = topNode.y - topNode.radius - 24
    format:              (monorepo: string) => `${monorepo} · monorepo`,
  },
};

// Physics (per-frame, 60fps via requestAnimationFrame)
const PHYSICS = {
  springK:      0.015, // v += (home - pos) * springK
  damping:      0.92,  // v *= damping each frame
  cursorRadius: 150,   // px — repulsion falloff
  cursorForce:  30,    // peak push magnitude
  cursorScale:  0.02,  // applied to velocity
};
const CURSOR_OFFSCREEN = { x: -1000, y: -1000 }; // on mouseleave

// Rendering
const USE_DPR = true; // canvas.width = rect.width * devicePixelRatio; ctx.setTransform(dpr,0,0,dpr,0,0)
```

## Visual design

A dark-background, force-directed-feeling graph rendered to a single `<canvas>`.

**Three node types**, visually distinct:

- `service` — backend services
- `consumer` — frontends (always assigned the pink palette slot)
- `endpoint` — HTTP routes (e.g. `GET /users/:id`); inherits the parent service's color

**Per-node rendering, in order:**

1. Soft radial-gradient halo at `radius * HALO_RADIUS_MULT`, color stops `rgba(r,g,b,HALO_INNER_ALPHA)` → `rgba(r,g,b,0)`.
2. Filled circle at `radius`, fill `NODE_FILL` (near-black so the halo bleeds through).
3. Stroke at the node's color, width and alpha per `NODE[type]`.
4. Label in `LABEL_FONT` centered at `(node.x, node.y + radius + LABEL_OFFSET)`, white at `NODE[type].labelAlpha`.

**Edges:** straight line in `EDGE_LINE_COLOR` at `EDGE_LINE_WIDTH`, plus a single bright "packet" dot animating along each edge using the `PACKET` constants, filled in the source node's color. **No arrowheads.**

**Monorepo cluster glow:** services sharing a `monorepo` value get one shared halo. Compute centroid + max distance over the cluster's services and their endpoints, draw a radial gradient at `maxDist + CLUSTER.paddingPx` using `CLUSTER.stops`, and place `CLUSTER.label.format(monorepo)` above the topmost node in the cluster.

## Physics / interaction

Lightweight pseudo-verlet simulation, 60fps via `requestAnimationFrame`. **No real layout engine.**

- Each node has a home position (`homeX`, `homeY`) — the layout is computed once on mount/resize.
- Per frame: spring back to home using `PHYSICS.springK`, damped by `PHYSICS.damping`.
- Cursor repulsion: within `PHYSICS.cursorRadius`, push away with `force = (1 - dist/cursorRadius) * cursorForce`, applied to velocity at `* PHYSICS.cursorScale`. On `mouseleave`, set the mouse position to `CURSOR_OFFSCREEN`.
- DPR-aware (see `USE_DPR`). Re-init nodes on resize.

This gives a subtle "alive" feel without real layout work. **Don't swap in a force-directed library — the cheapness is part of the look.**

## Layout

The marketing version hardcodes positions as fractions of width/height. The real tool needs a deterministic layout from arbitrary input:

- **Services** on a coarse grid or ring, keyed by stable hash of service id, so the same project lays out the same way every render.
- **Endpoints** in a small fan/arc near their parent service.
- **Consumers** (frontends) lower-center.
- **Cluster monorepo siblings** closer together so the shared halo encloses them naturally.

Don't optimize layout — the spring-back + cursor jitter hides minor overlap.

## Data shape

Replace hardcoded `EDGES` / nodes / `MONOREPO_SERVICES` with:

```ts
type ServiceMap = {
  services: {
    id: string;                 // "user-service"
    type: "service" | "consumer";
    monorepo?: string;          // services sharing this value get the cluster glow
  }[];
  endpoints: {
    id: string;                 // "GET /users/:id"
    serviceId: string;          // owner — inherits color
    method: string;             // "GET" | "POST" | ...
    path: string;               // "/users/:id"
  }[];
  edges: {
    from: string;               // service or consumer id
    to: string;                 // endpoint id (caller → endpoint)
  }[];
};
```

Pull these from Carrick's analyzer output (`CloudRepoData` per repo, joined across an org — see the data-shape appendix in `docs/plan.md` on `claude/repo-metadata-graph-1oxKm` for the upstream JSON shape).

**Color assignment:** deterministic `hash(service.id) % PALETTE.length`, with one rule — `consumer` nodes always use the pink slot (`PALETTE[4]`).

## What to keep vs. change

**Keep exactly** — every value in the design-tokens block, canvas-only rendering, halo + dark fill + colored stroke style, animated packet dots, monorepo cluster glow, cursor-repulsion + spring-home physics.

**Change** — source from analyzer output not const arrays; deterministic layout function; deterministic palette assignment; scale to larger node counts (consider hiding endpoint labels when total endpoints > 40, or shrinking endpoint radius further).

**Out of scope** — click/drag, zoom/pan, tooltips, legends, filters, or a graph library. The marketing version has none of these and the aesthetic depends on the restraint.

## Deliverable

A drop-in component (matching whatever framework `carrick-cloud/app/` uses) that takes a `ServiceMap` prop and renders the same look as `carrick-site/src/components/DependencyGraph.jsx`, scaled to real data. Side-by-side with the marketing site, the two should be visually indistinguishable for equivalent inputs.

## Cross-references

- [`growth-playbook.md`](./growth-playbook.md) §product surface §4 — service map pages and visibility/auth rules.
- [`public-private-split.md`](./public-private-split.md) — why this lives in `carrick-cloud`, not `carrick`.
- `carrick-site/src/components/DependencyGraph.jsx` — reference implementation.
- `docs/plan.md` on branch `claude/repo-metadata-graph-1oxKm` — earlier exploration (Cytoscape-based; superseded by this spec).
