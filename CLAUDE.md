# CLAUDE.md — `carrick`

This is the **public** Rust scanner for Carrick. Companion to the private `carrick-cloud` repo (Lambdas + Terraform + dashboard).

## Hard rules

- **Never run `terraform` shell commands.** Not `init`, not `plan`, not `apply`, not `import`, not `state mv`. The user runs these. Editing files in `terraform/` (writing new `.tf` files, modifying existing ones) is fine — only invoking the `terraform` CLI is forbidden. (See also `AGENTS.md` line 60.)
- **No LLM prompts in this repo.** Per the public/private split, prompt strings live in `carrick-cloud/lambdas/*/`. CI workflow `prompt-leak-guard.yml` enforces this — if a PR adds matches for `You are`, `Extract ONLY`, `responseSchema`, `system_instruction`, `prompt:\s*"`, `Identify all frameworks`, or `"frameworks":` to `src/`, the PR fails.
- **No backwards compatibility / no users.** When refactoring, ship the new shape and delete the old shape in the same commit. No feature flags, no deprecation cycles, no parallel old/new code paths.

## Boundary

- Public (this repo): Rust scanner, AST/parser, agent orchestrators (thin), MCP server source, `ts_check/`, `src/sidecar/`, GitHub Action.
- Private (`carrick-cloud`): all Lambdas, Terraform, prompts, wrapper-rule generation, future web dashboard.

If you need to touch a Lambda, Terraform, or a prompt, the change goes in `carrick-cloud`.

## Where things are

`AGENTS.md` is the canonical repo-guidelines doc — read it for project structure, build commands, testing conventions, and commit style.

The current migration plan (Carrick → carrick-cloud split, Phases 0+1) lives at `/Users/davidjonathanmoores/.claude-personal/plans/can-you-research-the-jiggly-lampson.md`.
