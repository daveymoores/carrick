# Monorepo Support (NX, Turborepo, pnpm/npm/Yarn workspaces)

**Status:** Design / Not yet implemented
**Last Updated:** 2026-05
**Supersedes:** PR #42 (`feat: monorepo workspace support`, branch `feat/monorepo-support`)

## Goal

Let Carrick analyze a monorepo by treating each **deployable app/service** inside
it as a separate "repo" from the index's standpoint. This lets the existing
cross-repo machinery raise TypeScript request/response mismatches **across apps
within the same monorepo**, exactly as it already does across separate GitHub
repos.

The defining decision (confirmed): the analysis unit is a **deployable app**, not
every workspace package. Shared libraries are *folded into* the analysis of each
app that depends on them (so their types resolve), **not** uploaded as their own
"repos". This avoids flooding the index with empty-endpoint library entries.

## Why this fits Carrick's existing architecture

Carrick already keys everything off `repo_name` and already performs cross-repo
matching with no notion of "same physical repo":

- `MountGraph::merge_from_repos` tags every endpoint with its `repo_name` and
  merges all repos into one graph (`src/mount_graph.rs`).
- The producer/consumer type manifests and `Analyzer::get_type_mismatches`
  (`src/analyzer/mod.rs`) compare across all loaded repos.
- Cloud storage identifies a scan purely by `repo_name`
  (`src/cloud_storage/`), and `download_all_repo_data` pulls every adjacent repo.

So a monorepo app can be modeled as a repo with a **composite identity**
`"<repo>::<app>"`. This flows through the entire cross-repo path unchanged — the
cloud, the merge, the manifest comparison, and the type checker all just see
"another repo". PR #42 already validated this composite-key approach.

What PR #42 got right (reuse): composite `repo::package` keys, a per-package
analysis loop, `package_name` on `CloudRepoData`, and git-diff path normalization
for package subdirectories.

What PR #42 got wrong / left out (the work below):

1. Only detects npm/Yarn `package.json#workspaces`. No pnpm, NX, or Turborepo.
2. Treats **every** workspace package as a repo (apps *and* libs) → noise.
3. Hand-rolled glob expansion (`trim_end_matches("/*")`) — can't do nested or
   negated globs.
4. Does **not** solve cross-package **type resolution** (the part that makes the
   feature actually useful). See "The hard part" below.
5. Is ~52 commits behind `main` and has signature drift
   (`discover_files_and_symbols` is now a 4-tuple; the PR makes it a 6-tuple;
   `upload_repo_data` signatures differ). It must be **redone on current main**,
   not rebased.

## Key insight: NX and Turborepo are both package-manager workspaces

Both can be enumerated **entirely by reading files** — there is no need to run
`nx` or `turbo` (their daemons/CLIs are caches over a file scan). This means we
need **one workspace abstraction** fed by a few detectors, not four code paths.

| Stack | "This is a monorepo" marker | Package list source | Internal edges |
|---|---|---|---|
| npm / Yarn | `package.json#workspaces` (array or `{packages:[]}`) | same globs | dep name-match |
| pnpm | `pnpm-workspace.yaml` | `packages:` in that YAML | `workspace:*` + name-match |
| Turborepo | `turbo.json` next to root `package.json` | *delegates to the PM above* | dep name-match |
| NX | `nx.json` at root | glob `**/project.json` ∪ workspace `package.json` | `tsconfig.base.json#paths` ∪ pkg names ∪ `implicitDependencies` |

Notes:

- **`turbo.json` is task-only.** It lists no packages and declares no
  inter-package dependencies — it is purely a marker that says "resolve packages
  via the package manager". (Watch for v1 `pipeline` vs v2 `tasks`, but neither
  matters for discovery.)
- **NX conventions are not authoritative.** `apps/` vs `libs/` and
  `nx.json#workspaceLayout` only tell *generators* where to scaffold; existing
  projects can live anywhere. Scan for `project.json` / workspace `package.json`
  markers — do not assume directory layout.

## Detecting deployable apps (vs libraries)

Neither NX `projectType` nor Turborepo gives a fully reliable "this is an app"
flag, so combine heuristics (highest-confidence first):

1. **Graph sink** — the package has in-degree 0 in the internal dependency graph
   (no other workspace package imports it). Libraries are depended upon; apps are
   not.
2. **Server/framework dependency present** — `express`, `fastify`, `next`,
   `@nestjs/*`, `koa`, `hapi`, etc. in its `dependencies`.
3. **NX `projectType: "application"`** (when `project.json` exists) corroborates.
4. **No `exports` map** + has a `build`/`start`/`dev` script that emits a
   deployable artifact.

A package satisfying (1)+(2) is treated as a deployable app = one analysis unit.
Everything else is a library, folded into the closure of apps that depend on it.

## The hard part: cross-package type resolution

This is the risk that decides whether the feature works, and PR #42 does not
address it.

When analysis points at `apps/web` in isolation, the sidecar
(`TypeSidecar::start_init(repo_root, tsconfig)`,
`src/services/type_sidecar.rs`) loads only that directory. An import like
`import { Order } from '@repo/orders'` then **fails to resolve**, because:

- The lib source lives in `packages/orders`, outside the app directory, and
- The mapping that connects them lives in the **root** `tsconfig.base.json#paths`
  (NX) or the root `package.json#exports` / workspace symlinks (Turborepo) — not
  in the app's own `tsconfig.json`.

`src/sidecar/src/project-loader.ts` currently looks for `tsconfig.json` only at
the package root and otherwise globs that single directory's `src/**`. Result:
unresolved imports → missing/`unknown` types → no usable cross-app mismatch,
which defeats the purpose.

Two viable approaches:

- **(A) Root-context project, scoped extraction.** Initialize ts-morph's
  `Project` at the **monorepo root** so `tsconfig.base.json` `paths`, project
  references, and workspace symlinks all resolve, but scope endpoint/call
  extraction to the app directory + its transitive lib closure. Less code, most
  correct (uses TS's own resolver). **Preferred.**
- **(B) Hand-built alias table.** Compute the alias→path table ourselves
  (NX: `tsconfig.base.json#paths` ∪ package names; Turborepo: each package's
  `name` + `exports`, preferring the `types`/source condition over `dist`) and
  feed resolved lib source files into the per-app project. More control, more
  code; matches the "file-only" philosophy.

Useful existing scaffolding: the sidecar already supports loading a project from
a `tsconfigSnapshot` with custom compiler options/paths
(`project-loader.ts` "Priority 1" path) and has `monorepo-builder.ts`. That
plumbing is reusable; it just isn't wired to real monorepo discovery yet. (Note:
`monorepo-builder.ts` builds a *synthetic* workspace for cross-repo type
checking — a different concern from discovering the user's real packages.)

## Import resolution / dependency-graph algorithms (file-only)

Both reduce to: build a `name/alias → package-dir` table, then resolve each
import specifier against it; a hit in another package is an internal edge.

**NX**

1. Require `nx.json` at root.
2. Determine PM workspace globs (`package.json#workspaces` or
   `pnpm-workspace.yaml`).
3. Project roots = dirs containing `project.json` (glob `**/project.json`,
   excluding `node_modules`/`dist`/`.nx`) ∪ workspace `package.json` dirs.
4. Per project, merge `project.json` + `package.json` into one config
   (`project.json` wins; name resolves project.json.name → package.json.name →
   derived from dir). `projectType` distinguishes app vs lib.
5. Alias table = `tsconfig.base.json#compilerOptions.paths` (alias → file →
   owning project) ∪ every project `package.json#name`.
6. Edges = static/dynamic imports resolved via the alias table ∪
   `implicitDependencies` ∪ workspace deps in `package.json`. (Optionally
   corroborate with each project `tsconfig.json#references`.)

**Turborepo / PM workspaces**

1. Detect PM (`packageManager` field / lockfile): pnpm → `pnpm-workspace.yaml`,
   else `package.json#workspaces` (handle array and `{packages:[]}` forms).
2. Expand globs (real glob crate; honor `!` negation; defensively handle `**`),
   skip `node_modules`/`.git`/`dist`/etc.
3. A matched dir is a package only if it has a `package.json` **with a `name`**.
4. `nameIndex = name → package`. Edges = for each package, intersect
   `dependencies ∪ devDependencies ∪ peerDependencies ∪ optionalDependencies`
   keys with `nameIndex`. `workspace:*` confirms (pnpm/bun); npm/Yarn rely on
   name-match alone.
5. Resolve `@repo/ui/button` → `packages/ui`'s `exports["./button"]` (prefer the
   `types`/source condition over `default`/`dist`); fall back to
   `main`/`module`/`types` → `src/index.*`.

## Work breakdown

**Phase 1 — Generalize workspace detection** (`src/workspace.rs`, additive)
- Keep `WorkspaceInfo`/`WorkspacePackage`; add `kind: App | Lib` and
  `internal_deps: Vec<String>`.
- Detectors: `pnpm-workspace.yaml`, `turbo.json`-as-marker, `nx.json` +
  `project.json` scan, plus the existing `package.json#workspaces`.
- Use a real glob crate (nested + negated patterns).
- Build the internal dependency graph and classify apps as framework-bearing
  graph sinks.

**Phase 2 — Refresh the engine loop** (`src/engine/mod.rs`)
- Re-apply PR #42's monorepo branch on **current main** (the 6-tuple
  `discover_files_and_symbols` refactor, `package_name` on `CloudRepoData`,
  composite-key upload, git-diff subdir path filtering). This is a re-do.
- Iterate over **apps** (not every package); fold each app's transitive lib
  closure into its analysis scope.

**Phase 3 — Cross-package type resolution** (`src/services/type_sidecar.rs`,
`src/sidecar/src/project-loader.ts`) — the hard part
- Init the ts-morph project with monorepo-root context / resolved `paths`; scope
  extraction to app + lib closure. Prototype against a real repo early
  (e.g. `optaxe-ts-monorepo`).

**Phase 4 — Tests & fixtures** (`tests/fixtures/`)
- One pnpm/Turborepo monorepo and one NX workspace, each with two apps where app
  A `fetch`es app B's endpoint with a deliberately mismatched request/response
  type → assert the cross-app mismatch is raised.

**Cleanup (from PR #42's own debt list)**
- Unify the three partial `package.json` models (`PackageJson` in `packages.rs`,
  `RootPackageJson` in `workspace.rs`, `PackageJsonSummary` in
  `framework_detector.rs`).
- Merge the monorepo/single-repo engine branches into one loop.
- `HashMap<String, CloudRepoData>` keyed by `repo_name` for previous-data lookup
  instead of a linear scan.

## Scope / boundary notes

- All the cross-package type-checking work lives in the **sidecar (TS)** and
  `ts_check/` — both in this public repo. No prompts or Lambdas are involved, so
  none of this work belongs in `carrick-cloud`.
- The cloud knows repos only by `repo_name`, so composite keys `repo::app` are
  transparent to storage/aggregation — no `carrick-cloud` change is needed.

## References

- Existing PR: `daveymoores/carrick#42` (`feat/monorepo-support`).
- NX: project config / `project.json` (`ProjectConfiguration`, `projectType`);
  `nx.json` (`workspaceLayout`); project graph (`ProjectGraph`,
  `ProjectGraphDependency`, `DependencyType`); TypeScript Project Linking
  (`tsconfig.base.json#paths` vs workspaces + project references). Authoritative
  shapes are the type defs in `nrwl/nx` `packages/nx/src/config/`.
- Turborepo: workspace discovery is package-manager-native
  (`package.json#workspaces` / `pnpm-workspace.yaml`); `turbo.json` is task-only;
  internal packages + `exports` resolution; `turbo ls --output=json` /
  `turbo query` as optional cross-checks (not required).
