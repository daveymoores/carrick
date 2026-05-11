# CLAUDE.md — `carrick`

This is the **public** Rust scanner for Carrick. Companion to the private `carrick-cloud` repo (Lambdas + Terraform + dashboard).

## Hard rules

- **Never run `terraform` shell commands.** Terraform and the rest of the AWS infrastructure live in `carrick-cloud`, not here. If a task needs infra changes, switch to that repo.
- **No LLM system instructions in this repo.** Per the public/private split, system-prompt strings live in `carrick-cloud/lambdas/*/system_prompt.txt`. User-message templates that interpolate scan-time data may live in Rust because they need access to the data structures the scanner produces (e.g. `src/agents/file_analyzer_agent.rs`). CI workflow `prompt-leak-guard.yml` enforces this as a ratchet against `.github/prompt-leak-baseline.txt`: counts may shrink but never grow. It scans every `*.rs` file under `src/`, `build.rs`, and `tests/` (excluding `tests/fixtures/`) for the patterns `You are `, `You describe `, `You analyze `, `Extract ONLY`, `responseSchema`, `system_instruction`, `prompt:[[:space:]]*"`, `Identify all frameworks`, and `"frameworks":`.
- **No backwards compatibility / no users.** When refactoring, ship the new shape and delete the old shape in the same commit. No feature flags, no deprecation cycles, no parallel old/new code paths.

## Boundary

- Public (this repo): Rust scanner, AST/parser, agent orchestrators (thin), `ts_check/`, `src/sidecar/`, GitHub Action.
- Private (`carrick-cloud`): all Lambdas, MCP server + tools, Terraform, prompts, wrapper-rule generation, future web dashboard.

MCP is exposed exclusively as an HTTP endpoint at `https://api.carrick.tools/mcp`. Users add Carrick to their AI agent via `claude mcp add --transport http carrick https://api.carrick.tools/mcp`. There is no local-stdio install — the MCP tool implementations live in `carrick-cloud/lambdas/mcp-server/`.

If you need to touch a Lambda, Terraform, or a prompt, the change goes in `carrick-cloud`.

## Where things are

`AGENTS.md` is the canonical repo-guidelines doc — read it for project structure, build commands, testing conventions, and commit style.

The Carrick → carrick-cloud split landed in 2026-05. Follow-up work (OAuth dashboard, ELv2 relicense + flip public, etc.) is tracked as GitHub issues.
