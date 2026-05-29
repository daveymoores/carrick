# Monorepo Support (NX, Turborepo, pnpm/npm/Yarn workspaces)

**Status:** Design / Not yet implemented
**Last Updated:** 2026-05
**Supersedes:** PR #42 (`feat: monorepo workspace support`, branch `feat/monorepo-support`)

## Goal

Let Carrick analyze a monorepo by treating each **deployable app/service** inside
it as a separate "repo" from the index's standpoint. This lets the existing
cross-repo machinery raise TypeScript request/response mismatches and missing/
orphaned routes **across apps**, and — more importantly — lets a monorepo backend
participate in true cross-repo matching against a **separate** frontend repo.

The defining decision (confirmed): the analysis unit is a **deployable app**, not
every workspace package. Shared libraries are *folded into* the analysis of each
app that depends on them (so their types resolve), **not** uploaded as their own
"repos".

## Why this is worth building (and why config-first)

The weak version of the pitch is "detect drift between apps inside one monorepo".
That's real but soft — colocated apps ship in one PR, so the drift window is
small.

The strong version, and the actual reason to build this: **a large fraction of
the cross-repo market Carrick already targets has a backend that lives in a
monorepo.** "Services backend in an Nx/Turborepo monorepo + React frontend in a
separate repo" is a very common topology. Without monorepo support, Carrick
cannot ingest that backend *at all*, so it can't do the headline cross-repo
matching (separate frontend → monorepo backend endpoint, with type comparison).
**Monorepo ingestion is therefore table-stakes for the existing product, not a
new feature.**

The risk is that full auto-detection + perfect cross-package type resolution is a
tar pit (Nx alone has three linking styles; every repo's tsconfig/`exports` setup
is a snowflake). So we **de-risk with configuration**: the user explicitly lists
their app roots in `carrick.json`, and Carrick analyzes each as a separate unit.
Auto-detection is reduced to a cheap *nudge* (below), not the ingestion path. We
can add full auto-detection later if demand justifies it.

## Mechanism 1 — Config-driven app roots (`projects` in `carrick.json`)

Add an optional `projects` field to the root `carrick.json`. It lists the
deployable apps to analyze, as paths or simple globs relative to the repo root:

```json
{
  "projects": ["apps/orders", "apps/billing", "services/*"]
}
```

Behavior:

- If `projects` is **absent** → current single-repo behavior, unchanged.
- If `projects` is **present** → Carrick expands the globs, and analyzes each
  resulting directory as its own analysis unit under a composite key
  `"<repo>::<app>"` (the `<app>` segment from the directory's `package.json#name`,
  falling back to the directory name).
- Each app may still carry its own `carrick.json` (for `serviceName`, env-var
  classification, etc.); root-level keys apply as defaults.

This composite-key model is exactly what PR #42 validated. It flows through the
entire cross-repo path unchanged — the cloud, `MountGraph::merge_from_repos`, the
producer/consumer manifests, and `Analyzer::get_type_mismatches` all just see
"another repo". No `carrick-cloud` change is needed: the cloud identifies scans
only by `repo_name`, so `repo::app` keys are transparent to storage/aggregation.

The config approach intentionally sidesteps the hard parts of auto-detection
(NX project-graph reconstruction, glob/negation edge cases, app-vs-lib
classification). The user tells us which directories are apps; we don't have to
infer it.

## Mechanism 2 — Monorepo detection as a warning/suggestion

Carrick should *cheaply* detect that a repo looks like a monorepo and, when no
`projects` is configured, surface a suggestion — the same pattern as the existing
"unclassified env var" configuration suggestion in the PR comment.

Detection is a shallow file check at the repo root (no graph building, no running
`nx`/`turbo`):

| Marker found at root | Implies |
|---|---|
| `nx.json` | Nx workspace |
| `turbo.json` | Turborepo |
| `pnpm-workspace.yaml` | pnpm workspaces |
| `package.json#workspaces` | npm / Yarn workspaces |

When a marker is present **and** `projects` is not configured **and** Carrick is
running in single-repo mode, emit a suggestion such as:

> Detected a Turborepo/Nx monorepo, but Carrick is only analyzing the repo root.
> Add a `projects` list to `carrick.json` (e.g. `"projects": ["apps/*"]`) to index
> each app as a separate service so cross-app and cross-repo drift is detected.

Where it surfaces:

- **CI / PR comment** — rendered by the existing formatter in this repo
  (`src/formatter/`), alongside the other configuration suggestions.
- **The app/dashboard** — this scanner uploads a detection flag (e.g. a
  `monorepo_hint` on the uploaded data); the dashboard rendering lives in
  `carrick-cloud` and is out of scope for this repo. This repo's job is to emit
  the signal.

This gives most of the conversion benefit of auto-detection (users discover the
feature exactly when it's relevant) at almost none of the cost, and keeps the
authoritative behavior config-driven and predictable.

## The hard part (still): cross-package type resolution

Config-first removes the *discovery* risk, not the *type-resolution* risk. When
analysis points at `apps/web` in isolation, the sidecar
(`TypeSidecar::start_init(repo_root, tsconfig)`) loads only that directory. An
import like `import { Order } from '@repo/orders'` then fails to resolve, because
the lib source lives in `packages/orders` and the path mapping lives in the
**root** `tsconfig.base.json#paths` (Nx) or root `package.json#exports` /
workspace symlinks (Turborepo). `src/sidecar/src/project-loader.ts` currently
looks for `tsconfig.json` only at the package root. Unresolved imports → `unknown`
types → no type mismatch.

This matters even for the "simple" separate-frontend case, because monorepo
backends frequently factor request/response DTOs into a shared package — that's
half the reason teams adopt monorepos.

Two options when we get to it:

- **(A) Root-context project, scoped extraction.** Initialize ts-morph's `Project`
  at the monorepo root so base `paths`/references/symlinks resolve, but scope
  endpoint/call extraction to the app dir + its transitive lib closure. Less code,
  most correct. **Preferred.**
- **(B) Hand-built alias table.** Compute alias→path ourselves (Nx: `paths` ∪
  package names; Turborepo: package `name` + `exports`, preferring the source/
  `types` condition over `dist`) and feed resolved lib sources into the per-app
  project.

The sidecar already has reusable scaffolding for loading a project from a custom
tsconfig snapshot (`project-loader.ts` "Priority 1" path) and `monorepo-builder.ts`
— it just isn't wired to real monorepo discovery.

## De-risked sequencing

1. **Route-level value first (cheap).** Endpoint/call extraction comes from the
   SWC/LLM mount-graph pass, *not* the type sidecar. So config-driven per-app
   analysis immediately enables cross-repo **missing/orphaned endpoint** warnings
   (separate frontend calls a route the monorepo backend doesn't expose, etc.)
   with **zero** cross-package type-resolution work.
2. **Add the detection nudge** (Mechanism 2) so users discover `projects`.
3. **Measure the type-resolution hit rate** on a real target (e.g.
   `optaxe-ts-monorepo`): how often does a backend endpoint's response type fail
   to resolve because it imports from a sibling package? Intra-package types
   already resolve when the sidecar is pointed at the app dir.
4. **Invest in cross-package type resolution** (the hard part) only if the data
   shows shared-DTO packages are common enough — and scope it to the linking style
   the target repos actually use, not all of Nx's variants.

Explicit non-goal: a faithful `nx graph` reconstruction. We need "analyze the
configured app dirs and resolve their type imports", not a general-purpose Nx
graph tool.

## Work breakdown (config-first)

**Phase 1 — Config + per-app loop**
- Add `projects: Option<Vec<String>>` to `Config` (`src/config.rs`); expand globs
  relative to repo root.
- Re-apply PR #42's per-app loop on **current `main`** (it is ~52 commits stale
  with signature drift — `discover_files_and_symbols` is a 4-tuple on main, the PR
  makes it 6; `upload_repo_data` signatures differ — so this is a redo, not a
  rebase): composite-key upload, `package_name` on `CloudRepoData`, git-diff
  subdir path filtering. Iterate over **configured apps**, not every package.

**Phase 2 — Detection nudge**
- Shallow root marker check (Mechanism 2); emit a configuration suggestion via
  `src/formatter/` and a `monorepo_hint` flag on uploaded data.

**Phase 3 — Cross-package type resolution** (gated on Phase-2 measurement)
- Sidecar root-context project + scoped extraction (`src/services/type_sidecar.rs`,
  `src/sidecar/src/project-loader.ts`).

**Phase 4 — Tests & fixtures** (`tests/fixtures/`)
- A pnpm/Turborepo and an Nx fixture, each with two apps where app A `fetch`es app
  B's endpoint with a deliberately mismatched type → assert the mismatch is raised.

**Cleanup (from PR #42's debt list)**
- Unify the partial `package.json` models (`PackageJson`, `RootPackageJson`,
  `PackageJsonSummary`); merge the monorepo/single-repo engine branches into one
  loop; `HashMap` lookup for previous data instead of linear scan.

## Boundary notes

- All cross-package type-checking work lives in the sidecar (TS) and `ts_check/` —
  both in this public repo. No prompts/Lambdas involved.
- The cloud knows repos only by `repo_name`, so composite `repo::app` keys need no
  `carrick-cloud` change. Dashboard rendering of the monorepo hint is a
  `carrick-cloud` concern; this repo only emits the flag.

---

## Appendix — How NX & Turborepo are structured (reference for the detection nudge and any future auto-detection)

Both are enumerable **entirely by reading files** — no need to run `nx`/`turbo`
(their daemons/CLIs are caches over a file scan). Both are fundamentally
package-manager workspaces.

| Stack | Marker | Package list source | Internal edges |
|---|---|---|---|
| npm / Yarn | `package.json#workspaces` (array or `{packages:[]}`) | same globs | dep name-match |
| pnpm | `pnpm-workspace.yaml` | `packages:` in that YAML | `workspace:*` + name-match |
| Turborepo | `turbo.json` next to root pkg.json | *delegates to the PM above* | dep name-match |
| NX | `nx.json` at root | glob `**/project.json` ∪ workspace `package.json` | `tsconfig.base.json#paths` ∪ pkg names ∪ `implicitDependencies` |

Notes for any future auto-detect:

- `turbo.json` is **task-only** — it lists no packages and declares no deps; it's
  purely a marker. (v1 uses `pipeline`, v2 uses `tasks`; irrelevant to discovery.)
- Nx conventions are not authoritative: `apps/`/`libs/` and
  `nx.json#workspaceLayout` only tell *generators* where to scaffold. Scan for
  `project.json` / workspace `package.json` markers; don't assume layout.
- App-vs-lib has no single reliable flag. Best heuristic: a package that is a
  **graph sink** (in-degree 0) **and** pulls in a server framework
  (express/fastify/next/nest/…) is a deployable app. Nx `projectType:
  "application"` corroborates when present.
- Import → source-file resolution: Nx via `tsconfig.base.json#paths` (alias → file
  → owning project) ∪ project `package.json#name`; Turborepo via package `name` +
  `exports` (prefer the source/`types` condition over `dist`), fallback
  `main`/`module`/`types` → `src/index.*`.

### Sources

- NX: `ProjectConfiguration`/`project.json` and `projectType`; `nx.json`
  (`workspaceLayout`); project graph (`ProjectGraph`, `ProjectGraphDependency`,
  `DependencyType`); TypeScript Project Linking. Authoritative shapes are the type
  defs in `nrwl/nx` `packages/nx/src/config/`. Docs: nx.dev project-configuration,
  nx-json, concepts/typescript-project-linking, concepts/decisions/folder-structure.
- Turborepo: discovery is package-manager-native (`package.json#workspaces` /
  `pnpm-workspace.yaml`); `turbo.json` task-only; internal packages + `exports`
  resolution; optional cross-checks `turbo ls --output=json` / `turbo query`
  (the `--graph=*.json` format is deprecated/removed in v3). Docs: turborepo.dev
  crafting-your-repository/structuring-a-repository, core-concepts/internal-packages,
  core-concepts/package-and-task-graph, reference/configuration.
