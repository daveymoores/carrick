# Repository Guidelines

## Core Goals (Read First)
- Build a framework- and library-agnostic REST analysis tool for TypeScript services.
- Validate producer/consumer request/response/body compatibility across disparate repositories.
- Prioritize correctness in cross-repo matching over framework-specific heuristics.
- Remove legacy code paths; backwards compatibility is not required and dead code should not be kept.

## Project Structure & Module Organization
- `src/` is the main Rust crate (engine, parser, analysis, cloud storage).
- `tests/` holds Rust integration/unit tests; fixtures live in `tests/fixtures/`.
- `src/sidecar/` is the TypeScript type-extraction sidecar with tests in `src/sidecar/test/`.
- `ts_check/` contains legacy/experimental TypeScript utilities.
- `lambdas/` and `terraform/` cover AWS helpers and infrastructure.
- `scripts/` contains developer tooling (pre-commit hook installer).
- `docs/research/` stores architecture and research notes.
- `action.yml` defines the GitHub Action entrypoint.

## Build, Test, and Development Commands
```bash
# Build and run
cargo build
cargo build --release
cargo run -- /path/to/repo
CARRICK_MOCK_ALL=1 cargo run -- test-repo

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
- Runtime env vars: `CARRICK_API_KEY` (required when calling the cloud; org is derived server-side from this key), `CARRICK_MOCK_ALL` (test-only, returns canned responses without hitting the cloud), `CARRICK_API_ENDPOINT` (override the default `https://api.carrick.tools` endpoint at build time; optional).
- Never run Terraform commands in this repository.
