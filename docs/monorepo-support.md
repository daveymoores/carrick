# Monorepo Support

## Requirements

For Carrick to detect and analyze a monorepo, the repository must meet these requirements:

### 1. Root `package.json` with a `workspaces` field

The root `package.json` must declare workspaces using one of the standard formats:

**Array format** (Yarn / npm):
```json
{
  "name": "my-monorepo",
  "workspaces": ["apps/*", "libs/*"]
}
```

**Object format** (Yarn classic):
```json
{
  "name": "my-monorepo",
  "workspaces": {
    "packages": ["apps/*", "libs/*"]
  }
}
```

### 2. Simple glob patterns only

Workspace patterns must be simple `<directory>/*` or `<directory>/**` globs. Carrick expands these by listing immediate child directories of the base path. It does **not** support:

- Multi-level globs like `packages/group-*/app-*`
- Literal paths like `packages/specific-app` (treated as a directory to list children of)
- Negation patterns like `!packages/ignored`

### 3. Each package must have its own `package.json`

Each workspace package directory must contain a `package.json`. Directories without one are silently skipped.

The `name` field is used as the package identifier. If absent, the directory name is used as a fallback.

```
my-monorepo/
  package.json          <- root, declares workspaces
  apps/
    service-a/
      package.json      <- {"name": "service-a", ...}
      src/
        index.ts
    service-b/
      package.json      <- {"name": "service-b", ...}
      src/
        app.ts
```

### 4. No special tooling required

Carrick does not depend on NX, Lerna, Turborepo, or any specific monorepo tool. It reads the `workspaces` field directly from `package.json`. Any monorepo that uses Yarn workspaces, npm workspaces, or pnpm workspaces (with a compatible `package.json`) will work.

pnpm workspaces declared only in `pnpm-workspace.yaml` (without a `workspaces` field in `package.json`) are **not** currently detected.

## How it works

1. Carrick reads the root `package.json` and checks for a `workspaces` field
2. If found, it expands each pattern into concrete package directories
3. Each package is analyzed independently — its own framework detection, file discovery, mount graph, and `CloudRepoData`
4. Results are stored with a composite key: `<repo-name>::<package-name>`
5. Cross-repo comparison works automatically — packages within the same monorepo participate alongside packages from other repos
6. If no `workspaces` field is found, Carrick falls back to single-repo mode with no behavior change

## Limitations and known gaps

These are known issues that may be addressed in future work.

### Parallel package analysis

Packages are currently analyzed sequentially. For a monorepo with many packages, this means wall-clock time scales linearly with the number of packages. Each package triggers independent LLM calls (framework detection, per-file analysis) that could run concurrently.

### Monorepo / single-repo code duplication

The monorepo and single-repo orchestration paths in `engine/mod.rs` share similar logic (lookup previous data, analyze, upload, cross-repo assembly). These could be unified into a single loop with one entry for single-repo mode.

### `build_cross_repo_analyzer` signature

The function takes a distinguished `current_repo_data` parameter, but internally just pushes it into the list with all other repos. In monorepo mode, the "current" package is arbitrarily chosen (first alphabetically). If this function is ever changed to treat `current_repo_data` specially, the monorepo path would need updating.

### Linear scan for previous data

Previous analysis data is looked up by scanning the full list of downloaded repos per package. Converting to a `HashMap<String, CloudRepoData>` keyed by `repo_name` would be more efficient and would also avoid deep-cloning each match.

### Multiple partial `PackageJson` models

The codebase has several structs that partially model `package.json`:

| Struct | File | Fields |
|--------|------|--------|
| `PackageJson` | `packages.rs` | `name`, `version`, `dependencies`, `devDependencies`, `peerDependencies` |
| `RootPackageJson` | `workspace.rs` | `workspaces` |
| `PackageJsonSummary` | `framework_detector.rs` | `dependencies`, `dev_dependencies` |

These could be consolidated into a single canonical struct with `#[serde(default)]` on all fields.

### pnpm workspace detection

Monorepos using pnpm that declare workspaces only in `pnpm-workspace.yaml` (not in `package.json`) are not detected. Adding support would require parsing the YAML file as a secondary detection path.

### Complex glob patterns

Workspace patterns like `packages/group-*/app-*` or negation patterns are not supported. Only simple `dir/*` and `dir/**` patterns are expanded. Using a proper glob crate would handle these correctly.
