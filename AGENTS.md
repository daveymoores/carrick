# Repository Guidelines

## Core Goals (Read First)
- Build a live, type-aware, intent-aware index of TypeScript services across a GitHub org. The index is the product surface; AI coding agents query it over MCP.
- The Rust scanner in this repo is the index-population component. Per scanned function it extracts structural facts, request/response types, and intent.
- The GitHub Action wraps the scanner and runs per repo. PR comments showing cross-repo drift are the demoable proof point of the type analysis, not the headline product.
- Build framework- and library-agnostic REST extraction for TypeScript services. The structural layer of the index depends on this; correctness in cross-repo matching takes priority over framework-specific heuristics.
- Remove legacy code paths; backwards compatibility is not required and dead code should not be kept.

## Project Structure & Module Organization
- `src/` is the main Rust crate (engine, parser, analysis, cloud storage).
- `tests/` holds Rust integration/unit tests; fixtures live in `tests/fixtures/`.
- `src/sidecar/` is the TypeScript type-extraction sidecar with tests in `src/sidecar/test/`.
- `ts_check/` runs the final cross-repo HTTP type-compatibility check. The scanner discovers it at runtime (`run-type-checking.ts`, via `discover_ts_check_path` in `src/main.rs`) and spawns it with `npx ts-node` to compare producer/consumer manifests. Type *extraction* now lives in `src/sidecar/`; `ts_check/` retains only the compatibility check. It is bundled into the released Action artifact and gated by the pre-commit hook — not legacy or optional.
- `scripts/` contains developer tooling (pre-commit hook installer).
- `docs/research/` stores architecture and research notes.
- `action.yml` defines the GitHub Action entrypoint.
- `examples/` contains reference Express services used as e2e fixtures by CI and as user-facing demos.

Lambdas, MCP server, AWS infrastructure, and the web dashboard live in the companion `carrick-cloud` repo, not here.

## Build, Test, and Development Commands
```bash
# Build and run
cargo build
cargo build --release
cargo run -- /path/to/repo
CARRICK_MOCK_ALL=1 cargo run -- examples/express-single

# Tests and checks
cargo test
cargo test --test integration_test
cargo fmt
cargo clippy

# Type sidecar (Node)
cd src/sidecar
npm install
npm run build
npm test
```
Install hooks once per clone: `./scripts/install-hooks.sh`.

## Coding Style & Naming Conventions
- Rust is formatted with `rustfmt`; keep code `cargo fmt` clean and `clippy`-warning free.
- Use `snake_case` for modules/functions/files, `CamelCase` for types, and `SCREAMING_SNAKE_CASE` for constants.
- TypeScript follows standard `camelCase`/`PascalCase` conventions; mirror nearby files.

## Testing Guidelines
- Rust tests live in `tests/*.rs` (files typically end in `_test.rs`); fixtures are under `tests/fixtures/`.
- Use `CARRICK_MOCK_ALL=1` for tests that should avoid real cloud storage.
- Sidecar tests are authored in `src/sidecar/test/` and run via `npm test` after `npm run build`.
- Follow TDD: add/adjust tests first, then implement changes.
- Tests, formatting, and build steps must pass after each phase of work.

## Commit & Pull Request Guidelines
- Follow Conventional Commits with optional scopes: `feat(sidecar): ...`, `fix: ...`, `docs: ...`, `refactor(phase4.1): ...`, `chore: ...`, `test: ...`.
- PRs should include a short summary, rationale, test commands run, and links to relevant issues. Include sample Action output when it changes.

## Configuration & Infrastructure Notes
- `carrick.json` is resolved by `Config::load_services` into one `Config` per service. A flat config (or none) is a single service rooted at the repo root; a `services` array fans out per directory (`directory`, `include`, `tsconfig` + the call-classification fields). The engine runs the analysis pipeline, per-service type extraction, and upload once per service. Multi-service index upload is gated on `CloudStorage::supports_multi_service`, driven by the cloud's `multiService` capability flag. See the README "Monorepos" section for the user-facing shape.
- Runtime env vars: `ACTIONS_ID_TOKEN_REQUEST_URL` / `ACTIONS_ID_TOKEN_REQUEST_TOKEN` (auto-set by GitHub Actions when the job grants `id-token: write`; the scanner mints an OIDC token from these and sends it as the `X-Carrick-OIDC` header — the cloud derives repo identity from the signed claims, so no API key is needed), `CARRICK_MOCK_ALL` (test-only, returns canned responses without hitting the cloud), `CARRICK_API_ENDPOINT` (override the default `https://api.carrick.tools` endpoint at build time; optional).
- Terraform, Lambdas, and dashboard code live in `carrick-cloud`. No infrastructure or server-side code belongs in this repo.

## Carrick

This repo is part of the **daveymoores / carrick-ci** Carrick
project. Carrick indexes every service in the project — exported functions
(with intent descriptions), dependencies, and API endpoints with real
request/response types.

### Connect the agent

```
claude mcp add --transport http carrick https://api.carrick.tools/mcp
```

One install serves every project in the workspace. On Carrick tool calls
from this repo, pass `project: "carrick-ci"` (or `repo: "<owner/repo>"`
from the git remote) so Carrick queries the right system.

### When to reach for Carrick

- Before writing a helper/parser/validator/formatter: `search_by_intent` to
  find an existing implementation in a sibling repo.
- Before calling another service's API: `get_api_endpoints` +
  `get_endpoint_types` instead of guessing the JSON shape.
- Before changing a response shape, removing an endpoint, or renaming a path:
  `check_compatibility` against each consumer.
- Before adding/bumping an npm dependency: `get_service_dependencies`.

Carrick is read-only; data reflects the most recent scan of each repo.
