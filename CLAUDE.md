# CLAUDE.md — `carrick`

This is the **public** Rust scanner for Carrick. Companion to the private `carrick-cloud` repo (Lambdas + Terraform + dashboard).

## Hard rules

- **Never run `terraform` shell commands.** Not `init`, not `plan`, not `apply`, not `import`, not `state mv`. The user runs these. Editing files in `terraform/` (writing new `.tf` files, modifying existing ones) is fine — only invoking the `terraform` CLI is forbidden. (See also `AGENTS.md` line 60.)
- **No LLM prompts in this repo.** Per the public/private split, prompt strings live in `carrick-cloud/lambdas/*/`. CI workflow `prompt-leak-guard.yml` enforces this — if a PR adds matches for `You are`, `Extract ONLY`, `responseSchema`, `system_instruction`, `prompt:\s*"`, `Identify all frameworks`, or `"frameworks":` to `src/`, the PR fails.
- **No backwards compatibility / no users.** When refactoring, ship the new shape and delete the old shape in the same commit. No feature flags, no deprecation cycles, no parallel old/new code paths.

## Boundary

- Public (this repo): Rust scanner, AST/parser, agent orchestrators (thin), `ts_check/`, `src/sidecar/`, GitHub Action.
- Private (`carrick-cloud`): all Lambdas, MCP server + tools, Terraform, prompts, wrapper-rule generation, future web dashboard.

MCP is exposed exclusively as an HTTP endpoint at `https://api.carrick.tools/mcp`. Users add Carrick to their AI agent via `claude mcp add --transport http carrick https://api.carrick.tools/mcp`. There is no local-stdio install — the MCP tool implementations live in `carrick-cloud/lambdas/mcp-server/`.

If you need to touch a Lambda, Terraform, or a prompt, the change goes in `carrick-cloud`.

## Where things are

`AGENTS.md` is the canonical repo-guidelines doc — read it for project structure, build commands, testing conventions, and commit style.

The Carrick → carrick-cloud split landed in 2026-05. Follow-up work (OAuth dashboard, ELv2 relicense + flip public, etc.) is tracked as GitHub issues.
