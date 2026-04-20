# Task: Build a per-PR framework-coverage fixture harness for Carrick

## Why this matters (one paragraph so you understand the stakes)

Carrick's scanner pipeline relies heavily on LLM-generated guidance to stay framework-agnostic. Small prompt edits can silently regress output in ways unit tests can't catch — e.g., during the 2026-04-20 work, a mid-session prompt simplification took Koa fixture determinism from 15/15 to 9/10 and was only caught because a human was running scans by hand. Today the project has zero automated signal for "did the latest prompt change break framework X?" This harness is the bottom of the test pyramid for everything LLM-driven.

## Required reading (in order, before touching code)

1. `.thoughts/framework-coverage.md` — the audit. Focus on §0 "Hand-off orientation" (guardrails), §7 Step 6 (what shipped), and all of §10 "Framework fixtures in CI." The §10.2 `expected.json` shape is your spec; §10.4 response caching is explicitly out of scope for now (§10.4 says "don't build speculatively").
2. `.thoughts/framework-coverage-followup.md` — for the 2026-04-20 context and the exact acceptance criteria the fixes were verified against.
3. `tests/fixtures/{koa,fastify,hapi,nestjs}-api/server.ts` — the four fixtures you'll wrap.
4. `src/main.rs` — the CLI entry point, env vars, and how the scanner is invoked.
5. `src/formatter/mod.rs` — the Markdown output format that your harness will parse (look for `CARRICK_OUTPUT_START`/`CARRICK_OUTPUT_END` anchors and the endpoint table).

## What exists today

**Fixtures** at `tests/fixtures/{koa,fastify,hapi,nestjs}-api/` — each is a single `server.ts` + `package.json` defining 3–4 endpoints and (except NestJS) 1 outbound `fetch` to `http://comment-service/api/comments`. These fixtures are already used by the scanner-level test at `tests/framework_coverage_test.rs`; don't move them, don't rename them.

**Scanner invocation** (this is what the harness runs):
```bash
# Build once
cd src/sidecar && npm install && npm run build && cd ../..
CARRICK_API_ENDPOINT="https://api.carrick.tools" cargo build --release

# Invoke per fixture
CARRICK_API_KEY=<from GitHub secret> \
CARRICK_API_ENDPOINT="https://api.carrick.tools" \
CARRICK_USE_SYSTEM_PROXY=1 \
CARRICK_ORG="carrick-ci-<pr#>-<sha>-<fixture>" \
./target/release/carrick <absolute-path-to-fixture-dir>
```

**Output format** — stdout is Markdown between HTML-comment anchors:
```
<!-- CARRICK_OUTPUT_START -->
<!-- CARRICK_ISSUE_COUNT:5 -->
### 🪢 CARRICK: API Analysis Results

Analyzed **4 endpoints** and **1 API calls** across all repositories.
...
| Method | Path |
| :--- | :--- |
| `GET` | `/users` |
| `GET` | `/users/:id` |
| `POST` | `/orders` |
| `GET` | `/api/v1/status` |
...
<!-- CARRICK_OUTPUT_END -->
```

Stable anchors you can parse: `CARRICK_OUTPUT_START`/`END`, the "Analyzed **N endpoints** and **M API calls**" line, and the `` | `METHOD` | `path` | `` table rows.

## Ground truth (verified against real AWS+LLM on 2026-04-20, commit `4e4a5c3`)

| Fixture | Expected endpoints | Expected outbound calls |
|---|---|---|
| koa-api | `GET /users`, `GET /users/:id`, `POST /orders`, `GET /api/v1/status` | 1 GET to host containing `comment-service` and path containing `/api/comments` |
| fastify-api | `GET /users`, `GET /users/:id`, `POST /orders`, `GET /api/v1/status` | same |
| hapi-api | `GET /users`, `GET /users/{id}`, `POST /orders`, `GET /api/v1/status` | same |
| nestjs-api | `GET /users`, `GET /users/:id`, `POST /users` | (none) |

Path format notes: Hapi uses `{id}`, Express/Koa/Fastify/NestJS use `:id` — your predicates must tolerate this (either normalize, or allow both).

## What to build

### 1. `expected.json` per fixture

Place at `tests/fixtures/<name>-api/expected.json`. Shape (from audit §10.2, tolerant predicates only — no exact string matches on handler names, issue wording, or IDs):

```json
{
  "min_endpoints": 4,
  "endpoints_contain": [
    { "method": "GET",  "path_matches": "/users" },
    { "method": "GET",  "path_matches": "/users/[:{]?id[}]?" },
    { "method": "POST", "path_matches": "/orders" },
    { "method": "GET",  "path_matches": "/api/v1/status" }
  ],
  "min_data_calls": 1,
  "data_calls_contain": [
    { "method": "GET", "target_contains": "comment-service" }
  ]
}
```

NestJS fixture's `min_data_calls` is 0. `path_matches` uses a regex so Hapi `{id}` and Express-style `:id` both pass.

### 2. Rust integration test

Create `tests/framework_fixtures_e2e.rs` (sibling to the existing `framework_coverage_test.rs`). One `#[test]` per fixture, each of which:

1. Reads `expected.json`.
2. Spawns `./target/release/carrick` against the fixture's absolute path with the env vars above. Gate on `CARRICK_API_KEY` being present — if absent, skip early with a clear log line (use `eprintln!` + early `return`), so local `cargo test` without a key still works.
3. Captures stdout, extracts the region between `CARRICK_OUTPUT_START`/`END`.
4. Parses endpoint rows and the "Analyzed **N endpoints** and **M API calls**" line.
5. Asserts every predicate in `expected.json`.
6. On failure, prints the full captured stdout for debugging, plus the `CARRICK_ORG` used so it can be purged.

Retry once on empty output (known transient failure mode — the scanner silently produces zero-byte stdout on occasional AWS/LLM hiccups). Any other failure fails the test immediately.

Use `std::process::Command`, not a shell script. Parse Markdown with simple line splitting + regex; don't pull in a full Markdown parser.

### 3. GitHub Actions workflow

Create `.github/workflows/framework-fixtures.yml`. Trigger: `pull_request` (the user has explicitly chosen per-PR over nightly and accepts the cost). Steps:

1. Check out the PR.
2. Install Node (check `src/sidecar/package.json` for the `engines.node` version — it's `>=22`).
3. `cd src/sidecar && npm install && npm run build`.
4. Install Rust (stable).
5. `CARRICK_API_ENDPOINT="https://api.carrick.tools" cargo build --release`.
6. `CARRICK_API_KEY=${{ secrets.CARRICK_API_KEY }} CARRICK_API_ENDPOINT="https://api.carrick.tools" cargo test --release --test framework_fixtures_e2e -- --test-threads=1 --nocapture`.

Run fixtures sequentially (`--test-threads=1`) — parallel runs hit real AWS rate limits and produce empty-output failures. Expected runtime: ~2 minutes total (four scans × ~30s each).

### 4. `CARRICK_ORG` naming

Use `carrick-ci-${PR_NUMBER}-${SHORT_SHA}-${FIXTURE_NAME}`. Never reuse. Never collide with production orgs (prefix `carrick-ci-` is the namespace).

## Design decisions already made — do not re-litigate

- **Per-PR, not nightly.** The user has accepted the LLM cost at current commit volume. Do not propose nightly.
- **Fixtures stay at `tests/fixtures/`.** The audit §10.1 proposed a new `tests/framework-fixtures/` dir; ignore that — reuse what exists.
- **Rust integration test, not bash.** `cargo test` integration, runnable locally with a key.
- **Predicate-only assertions.** No exact string matches on handler names, issue IDs, or LLM-worded sentences.
- **No response caching (audit §10.4).** Build it only if the harness is proven flaky in practice.
- **No new fixtures in this scope.** The four existing fixtures are the MVP.
- **No `--json` output flag for the scanner.** Tempting but scope creep; parse the Markdown anchors. Revisit if parsing proves fragile.
- **Parse the `CARRICK_OUTPUT_START`/`END` region only.** Everything outside those anchors is log noise that may change.

## Open decisions you need from the human

- **Adding `CARRICK_API_KEY` as a GitHub secret.** This is a human-only step; flag it in the PR description with exact instructions (Settings → Secrets and variables → Actions → New repository secret; value: same key used for the 2026-04-20 verification).
- **Whether to auto-purge `CARRICK_ORG` entries after the CI run.** There's no built-in purge API in Carrick — a purge script would have to go through `AwsStorage` directly. Propose, don't implement without sign-off.

## Non-goals

- Don't add new fixtures.
- Don't change the scanner's output format.
- Don't build response caching.
- Don't touch Fix 3 Layer B (npm packaging).
- Don't add Hono, Express-5, or any audit §10.5 follow-on fixtures.
- Don't refactor existing scanner-level tests (`tests/framework_coverage_test.rs`).

## Acceptance

1. A PR opened against this branch with a no-op change (e.g., a comment edit in a docs file) triggers the new workflow and it goes green.
2. Locally: `CARRICK_API_KEY=<key> cargo test --release --test framework_fixtures_e2e -- --test-threads=1 --nocapture` passes all four fixtures.
3. Without `CARRICK_API_KEY`: tests skip (don't fail) with a clear log message.
4. A deliberate regression — e.g., revert commit `4e4a5c3` to bring back the plugin-closure bug — makes the `hapi-api` test go red with a useful failure message naming which predicate mismatched.
5. `expected.json` files committed and reviewable.
6. Workflow file committed and runnable.
7. README section (in `tests/fixtures/README.md` or append to `.thoughts/framework-coverage.md` §10) documenting how to add a new fixture once this harness exists.

## Gotchas

- **LLM non-determinism exists.** Predicate-only assertions mitigate most of it. If a fixture goes red flakily (not content-wrong, just wording-drift), investigate whether the predicate is too strict before declaring the scanner broken.
- **Empty-output transients.** The scanner occasionally exits 0 with zero-byte stdout under AWS/LLM strain. Retry once. If both attempts empty, fail with a clear diagnostic — this is an infrastructure signal, not a scanner bug.
- **Build-time env var.** `cargo build` reads `CARRICK_API_ENDPOINT` at compile time via `build.rs`. Forgetting to set it in CI will produce a binary that points at localhost and silently fails.
- **Sidecar must be built.** If `src/sidecar/dist/src/index.js` doesn't exist, the sidecar silently doesn't start, type extraction is skipped, and the scan still "succeeds" with degraded output. Your CI step must build the sidecar.
- **Existing pre-commit hook** in this repo runs `cargo fmt`, `cargo clippy`, and the full Rust + ts_check test suites. Keep your new test fast — under ~3 minutes — or consider marking it `#[ignore]` by default and enabling via a CI-only env var if the hook starts timing out.
- **Release process.** `release-please` drives versioning; if you add a crate or change `Cargo.toml`, check `.release-please-config.json` so you don't break the release bot.

## Scope guardrail

If you find yourself writing more than ~300 lines of Rust for the test runner, stop and ask — you're probably over-engineering. This is a "run 4 subprocesses and grep stdout" harness, not a framework.

Report back when (1) the workflow runs green on an opened PR and (2) a deliberate regression produces a useful red.
