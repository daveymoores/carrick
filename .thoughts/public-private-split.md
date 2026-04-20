# Public / Private Split

Where the line falls between the OSS scanner and the private infra — what moves, what stays, and why.

## The rule

**Rust = public (`carrick`, ELv2). Lambdas = private (`carrick-cloud`, proprietary, no public license).**

Mechanical boundary, no case-by-case debates. If a piece of functionality is worth protecting, it gets rewritten as lambda code in `carrick-cloud`. If it's written in Rust, it lives in `carrick`.

This rule happens to align with what's actually worth protecting — the accumulated LLM-handling experience (prompts, schema descriptions, rule generation) is mostly expressed today as Rust string literals and attributes, so "move it out of Rust" maps cleanly to "move it out of public."

## Why this split

1. **Mechanical, not philosophical.** Future PRs don't argue about what counts as sauce — the language is the gate.
2. **Matches current architecture.** Today the Rust agent code builds a prompt, POSTs to `lambdas/agent-proxy/`, which forwards to Gemini. The change is small in blast radius: relocate string literals and rule-generation logic; keep everything else.
3. **Protects what's worth protecting** without hollowing out the OSS. Scanner, mount graph, sidecar, analyzer — all stay public as legitimate engineering worth reading and contributing to.
4. **Prompts iterate on a different cadence than code.** Moving them server-side means prompt fixes don't require a scanner release, and A/B testing / model routing lives where it should.

## What moves

All of these are currently in Rust and get rewritten as TypeScript/Node in the `carrick-cloud`:

- **Prompt strings** — the system-prompt and user-prompt literals in `src/agents/file_analyzer_agent.rs` and `src/agents/framework_guidance_agent.rs`. Rewritten as prompt-building code in a new lambda (e.g. `lambdas/file-analyzer/`, `lambdas/framework-guidance/`) or folded into `lambdas/agent-proxy/`.
- **LLM-facing schema descriptions** — the `#[schemars(description = "...")]` text on fields in `src/agents/schemas.rs`. These are the per-field instructions that teach the LLM what to emit. Conceptually owned by the lambda; see **Schema ownership** below for the mechanical answer.
- **Wrapper rule generation** — `src/wrapper_registry.rs`. Rust currently hardcodes axios (and nothing else); the plan was always to have an LLM generate rules per-framework (see `framework-coverage.md` §9.4). Move that generation to a lambda; Rust receives rules as data.
- **Call-site classifier prompt** — `src/call_site_classifier.rs`. Legacy code, mostly; fold any still-active pieces into the file-analyzer lambda or delete outright.

## What stays in Rust

Unchanged from today, public in the `carrick` repo:

- **All scanner / parser / AST work** — `src/swc_scanner.rs`, `src/parser.rs`, `src/visitor.rs`, `src/extractor.rs`.
- **Symbol + import extraction** — `ImportSymbolExtractor`, `SymbolTable`.
- **Mount graph construction + URL normalizer** — under `src/analyzer/`.
- **Single-repo deterministic analyzer** — producer↔consumer correlation within one repo; no LLM needed.
- **ts-morph sidecar management** — the Rust side (`src/services/type_sidecar.rs`) that spawns the Node process.
- **Agent orchestrator structs** — `FileAnalyzerAgent`, `FrameworkGuidanceAgent`. They get thinner: today they build a prompt + POST it; tomorrow they build a structured payload + POST it. The retry / validation / response-parsing plumbing is still useful Rust code.
- **Rust schema types** — the Rust structs that deserialize LLM output. Rust still needs these to parse the response. Only the LLM-facing *descriptions* are in scope for relocation.
- **File walking, git-diff incremental mode.**
- **Action entry, CLI, MCP client plumbing.**
- **The Node-side `ts_check/` sidecar** — not Rust, but stays public because it needs access to the user's source + TypeScript compiler. Can't run server-side.

## Schema ownership — decision

Both sides need the schema. Rust needs types to deserialize; the lambda needs the schema to pass as `responseSchema` to Gemini's structured-output API. Options considered:

- **(a) Duplicate.** Rust has canonical types + descriptions. Lambda has its own mirrored schema. CI check catches drift.
- **(b) Rust ships schema (types + descriptions) in the request payload.** Lambda uses whatever Rust sends. Single source of truth.
- **(c) Hybrid.** Rust ships field shape; lambda injects descriptions by field name at request time.
- **(d) Define once in JSON Schema, codegen both sides.**

**Chosen: (b).** Zero duplication friction, matches how Gemini's structured-output API already works.

**Tradeoff accepted:** the schema descriptions are part of the sauce — the per-field instructions teach the LLM what to emit (e.g. "emit the payload subexpression: for `res.json(x)` emit `x`"). Under (b) they stay in the Rust repo, which means they're public. Honest weighting: prompts are ~70% of the sauce, schema descriptions ~20%, rule generation ~10%. At MVP we accept reduced-but-not-catastrophic protection in exchange for shipping faster.

**Escape hatch if a competitor appears:** switch to (c). Keep Rust types as shape-only, have the lambda inject descriptions at request time by field name. More moving parts, but keeps descriptions private without duplicating the type definition. Don't pre-build.

## Protocol shape

Roughly:

```
Rust scanner (per file):
  builds { structured_facts, response_schema, repo_context, framework_hints }
  POST → lambda (e.g. /analyze-file, /framework-guidance)

Lambda:
  builds prompt (from structured_facts + hints + server-side prompt template)
  calls Gemini with prompt + response_schema
  validates response, retries on schema violation
  returns structured result

Rust scanner:
  deserializes into Rust types
  plumbs into MountGraph + Analyzer (unchanged downstream)
```

One request per unit of work (per-file, per-scan-guidance). Parallelism is unchanged — Rust already orchestrates per-file work in parallel.

## Contract versioning

Once prompts live server-side, scanner and lambda evolve independently. A stable versioned contract between them is a hard requirement before cutover — not a "nice to have."

Requirements:
- **Scanner sends its version** (`X-Carrick-Scanner-Version: 1.4.2`) in every request.
- **Lambda declares supported scanner version range.** Rejects incompatible versions with a structured error the scanner surfaces clearly: "Your Carrick version is too old — update the Action."
- **Additive schema evolution.** Lambda adds fields to the response; old scanners ignore unknown fields. Never rename or remove fields without a deprecation window.
- **Bidirectional schema drift check in CI.** A test that starts both the scanner and the lambda locally, runs a representative payload, and asserts the scanner can deserialize the lambda's response. Catches drift before it ships.

## What doesn't move, and why

Being explicit so the OSS story isn't hollow:

- **SWC scanner and AST work** — understanding TS/JS AST, writing visitors that correctly capture call sites, imports, and decorators is legitimate engineering worth reading.
- **The ts-morph sidecar architecture** (manifest-based symbol matching, dts-bundle-generator integration) is novel and documented in `docs/research/compiler-sidecar-architecture/`. Shareable.
- **Mount graph + URL normalizer** encode how routes compose across files in real frameworks. Reusable reference implementation.
- **The Rust orchestrator** shows how to structure an LLM-backed analysis pipeline with retries, validation, parallelism. Useful even without the prompts.

What the public repo *doesn't* give you:
- The prompts that make the scanner produce correct output on real codebases.
- Wrapper / framework rules generated per-scan.
- Cross-repo correlation, drift detection, stored service maps.

Scanner-alone-running-locally is explicitly not a supported path (see `growth-playbook.md` — Option C, GitHub Action only). Anyone who forks the public repo and points it at their own Gemini key gets a partially-working tool: no cross-repo, no hosted maps, prompts they have to write themselves. That's the intended story.

## Migration plan

Rough order. Each step is independent and shippable.

1. **Stand up contract versioning + CI drift check first.** Before anything moves, get the versioning plumbing in place so the cutover is safe.
2. **Move the file-analyzer prompt.** Single highest-value, largest chunk of sauce. New lambda endpoint; Rust `FileAnalyzerAgent` sends structured payload + schema instead of constructing a prompt. Keep the old code path behind a feature flag for one release to catch regressions.
3. **Move the framework-guidance prompt.** Same shape; smaller blast radius.
4. **Move wrapper rule generation.** Per `framework-coverage.md` §9.4 this was always planned as LLM-generated; just generate in a lambda instead of Rust.
5. **Delete or fold the call-site classifier.** Mostly legacy; verify then remove.
6. **Relicense the public repo to ELv2** — do this before the prompts move out, so the repo is already source-available-not-MIT when the OSS conversation starts. Keeps everyone honest.

## Cross-references

- `.thoughts/growth-playbook.md` — the overall launch plan. This doc implements the "public / private boundary" promised there.
- `.thoughts/framework-coverage.md` §9 — direction of travel toward LLM-generated framework rules. Step 4 above operationalizes §9.4 ("wrapper types — a later move").
- `docs/research/compiler-sidecar-architecture/` — why the ts-morph sidecar stays client-side.
