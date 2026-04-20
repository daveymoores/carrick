# Framework Coverage — Follow-up Fix Requirements

Handoff doc for the next agent. Three fixes surfaced from the 2026-04-20 end-to-end fixture run (Koa / Fastify / Hapi / NestJS against real AWS + real LLM). Read `.thoughts/framework-coverage.md` §0 and §9 first — the scope guardrails there apply here without exception.

## Run evidence

Eight orgs written to production AWS on 2026-04-20 (v1 = run from fixture cwd, v2 = run from repo root so `ts_check/` resolves). Outputs were captured under `/tmp/carrick-fixture-runs/` on the developer's machine and summarized in the review thread.

| Fixture | Endpoints | Paths | Mount prefix | Consumer fetch | Verdict |
|---|---|---|---|---|---|
| nestjs-api | 3/3 | `/users`, `/users/:id`, `POST /users` | ✅ `@Controller('users')` applied once | n/a | **working** |
| fastify-api | 4/4 | correct incl. `/api/v1/status` | ✅ `app.register(plugin, { prefix })` resolved | ✅ | **working** |
| koa-api v1 | 4/4 | correct | ✅ | ✅ | works |
| koa-api v2 | 4/4 | `/api/v1/api/v1/status` — **doubled** | ⚠️ LLM re-applied the Router's constructor prefix at the mount site | ✅ | **Fix 1** |
| hapi-api | 4/4 | mounted route missing `/api/v1` prefix | ❌ `server.register({ plugin, routes: { prefix } })` unresolved | ✅ | **Fix 2** |

Same fixture, same binary, two different outputs for Koa — classic LLM non-determinism on how the mount edge gets described.

---

## Fix 1 — Koa mount-prefix non-determinism

**Status (2026-04-20):** ~~prompt edit shipped~~. `src/agents/file_analyzer_agent.rs` §1.C "Mount Path Attribution" adds framework-neutral rules teaching the LLM to attribute a prefix to exactly one of {child constructor, mount site} based on where the prefix literal actually appears. Acceptance criterion ("10 consecutive scans of koa-api all produce `/api/v1/status`") is LLM-dependent and has **not yet been run** — needs real AWS + LLM fixture runs once the human confirms scope.

### Problem
`tests/fixtures/koa-api/server.ts` constructs a sub-router with a constructor-level prefix:

```ts
const apiRouter = new Router({ prefix: '/api/v1' });
apiRouter.get('/status', async (ctx) => { ... });
app.use(apiRouter.routes());
```

Two consecutive scans produced `/api/v1/status` (correct) and `/api/v1/api/v1/status` (doubled). The `FileAnalyzerAgent` sometimes emits a mount edge that treats `/api/v1` as a mount-site prefix to apply on top of the router's already-prefixed endpoint.

### Root cause
The mount-semantics prompt section in `src/agents/file_analyzer_agent.rs` (around lines 430-450, "Variable & Alias Resolution" / "Mounts") doesn't tell the LLM that a child router constructed with an option object containing `prefix` already carries that prefix in its endpoint paths. The LLM guesses, and the guess is not stable.

### Fix
In `src/agents/file_analyzer_agent.rs` system-prompt mount section, add this rule (framework-neutral wording — don't name Koa):

> When the child node was constructed with a prefix-carrying option object (e.g., `new Router({ prefix: '/api/v1' })`, `Hono().basePath('/api')`, etc.), the child's own endpoints already include that prefix. The mount point contributes no additional path prefix in that case — emit `mount_path: ''` (or null) for such mounts. Only contribute a mount-site prefix when the prefix is provided AT the mount site (e.g., `app.register(child, { prefix: '/api/v1' })`) and the child's endpoints are defined without it.

### Acceptance
- `koa-api` fixture: 10 consecutive scans all produce exactly `/api/v1/status` (no doubling, no dropping).
- `fastify-api` fixture: no regression — `/api/v1/status` continues to work (prefix at mount site, not constructor).
- A new unit-ish test in `tests/framework_coverage_test.rs` isn't practical for LLM output; gate this in the CI fixture harness (§10) once it exists.

### Scope guardrail
- **Prompt edit only.** Do not add Koa-specific, Router-specific, or prefix-key-name logic to Rust source.
- If the rule feels framework-specific as written, generalize it — the principle is "constructor-carried prefix vs mount-site prefix," not "Koa router."

### Defense-in-depth
Even with the prompt fix, LLM non-determinism is a category that won't disappear. Response caching per `framework-coverage.md` §10.4 is the proper CI defense — fold this into that workstream, not into source.

---

## Fix 2 — Hapi plugin-mount prefix extraction

**Status (2026-04-20):** ~~prompt edits shipped~~ in both agents.
- `src/agents/framework_guidance_agent.rs`: the MOUNT PATTERNS prompt now explicitly enumerates the four mount shapes (top-level path-string, top-level option-object key, nested option-object key, constructor-carried) and requires the `description` field to carry an extraction rule naming the dotted object-path where the prefix lives (e.g., `"Prefix is at options.routes.prefix (2nd argument)."`). Example count raised to 3–5 for mounts.
- `src/agents/file_analyzer_agent.rs` §1.C: the consumer-side rule now instructs the LLM to read the prefix from the dotted object-path given in the matching `mount_pattern.description`, rather than falling back to a top-level key.

No Hapi-specific strings landed in source. Acceptance criterion ("hapi-api fixture produces `/api/v1/status`") is LLM-dependent and **not yet verified** — needs a real AWS + LLM run.

### Problem
```ts
await server.register({ plugin: apiV1Plugin, routes: { prefix: '/api/v1' } });
```

The inner `/status` route registers without a prefix in the scan output — it should be `/api/v1/status`. The prefix lives inside a nested `routes` key, which differs from Fastify's top-level `prefix` key.

### Root cause
`FrameworkGuidanceAgent` generates `mount_patterns` per detected framework but does not currently surface Hapi's register-options shape explicitly. The `FileAnalyzerAgent` sees the candidate and classifies the literal `routes: { prefix }` as irrelevant configuration because nothing told it this is the mount-prefix carrier for Hapi.

### Fix
Per `framework-coverage.md` §9 direction of travel, **this is a guidance-output fix, not a source fix**.

In `src/agents/framework_guidance_agent.rs`, the `mount` pattern-generation prompt needs examples covering non-standard prefix-carrying shapes. The agent already generates framework-specific patterns per scan — the gap is that the prompt is too thin to prompt the LLM for edge shapes like Hapi's.

Concrete change in the mount-patterns system/context message (see `fetch_patterns("mount", …)`):
1. Add an explicit instruction: "Include mount patterns where the path prefix is nested inside an options object (e.g., `register({ plugin, routes: { prefix } })` for Hapi, `register(plugin, { prefix })` for Fastify). Name the exact object-path where the prefix lives."
2. The `PatternExample.description` field should carry the extraction rule ("The prefix is at `options.routes.prefix`") so `FileAnalyzerAgent` has enough to extract it.

Then enrich the `FileAnalyzerAgent` mount-extraction instructions to say: "If a `mount_pattern` description names an object-path where the prefix lives, read the prefix from that path in the call's argument object literal, not from a top-level key."

### Acceptance
- `hapi-api` fixture: `/api/v1/status` appears as an endpoint after scan.
- `fastify-api` fixture: no regression.
- `koa-api` fixture: no regression (the fix is additive at the prompt level).

### Scope guardrail
- **Must not** add Hapi-specific Rust or TS code.
- **Must not** hardcode the string `routes.prefix` anywhere in source.
- The Hapi shape enters the system via LLM-generated `PatternExample.description` at scan time. If the LLM's guidance is wrong, sharpen the guidance prompt — don't paper over it in source.

### On Perplexity for framework guidance

**Recommendation: rely on model training for MVP; do not add a Perplexity call yet.**

Reasoning:

- **The LLM already knows Hapi.** Gemini 2.5 Flash and Claude both know `server.register({ plugin, routes: { prefix } })` is a prefix-carrying shape. The guidance gap isn't *knowledge*, it's *elicitation* — the current prompt doesn't ask the model for these edge shapes. Sharpening the prompt is cheaper, faster, and deterministic-er than adding an external lookup.
- **External calls add failure modes**, latency (+1-3s per scan), rate-limit exposure, a new API key to manage, and another thing to mock in tests. At MVP scale this is net-negative.
- **The fixture harness (§10) is the correct forcing function.** If Hapi guidance is wrong, the fixture fails; you fix the guidance prompt; it stays fixed. Perplexity doesn't help here — the prompt is still the place where information enters the pipeline.

**Where Perplexity *might* earn its keep later (post-MVP):**
- **Bleeding-edge framework versions.** `FrameworkDetector` sees `fastify@5.0.0-rc3` and the LLM's training ends at Fastify 4. A narrow, conditional Perplexity call — only when the detected major version is newer than a threshold — gives a real win. But this is a <5% scenario at MVP and should wait until someone actually reports it.
- **Ecosystem coverage beyond what the LLM knows.** Frameworks with small footprints the LLM wasn't heavily trained on (e.g., `oak`, `Elysia`, some Go-style TS routers). Again, only if a user hits it.

**If you later decide to add it:**
- Call it from `FrameworkGuidanceAgent`, not the per-file agent. Guidance is per-scan; per-file LLM cost is hot-path.
- Request structured output (JSON schema Perplexity supports it via the `response_format` flag).
- Cache the response keyed on `(framework_name, major_version)` — Perplexity's output for "Fastify 5 routing patterns" doesn't change within a day.
- Add a timeout (2s) and treat failure as "use training fallback" — never block a scan on Perplexity availability.

For this fix specifically: **do not introduce Perplexity.** The Hapi gap is a prompt-elicitation problem, not a knowledge problem.

---

## Fix 3 — `ts_check/` distribution and CWD-relative lookup

**Layer A status (2026-04-20):** ~~shipped on `claude/carrick-demo-prep-qsSnA`~~. `discover_ts_check_path` added to `src/main.rs`; `Analyzer` grew a `ts_check_dir: Option<PathBuf>` field; the four hardcoded `"ts_check/..."` call sites in `src/analyzer/mod.rs` + the one in `src/engine/mod.rs::recreate_type_files_and_check` all read from the resolved path. `cargo build --release` + `cargo test --lib --release` clean (183 tests). End-to-end fixture verification (Layer A acceptance criterion "runs `carrick <abs-path>` from `$HOME` and finds ts_check") still pending — bundled with the Fix 1/2 fixture runs below. Layer B is unchanged.

### Problem (two layers)

**Layer A — CWD-relative path lookup (correctness bug).**
`src/analyzer/mod.rs:1193` looks up `"ts_check/run-type-checking.ts"` as a string relative to the process CWD. Seven other call sites in the same file and in `src/engine/mod.rs` reach into `ts_check/output/` the same way. When the binary runs from any directory other than one containing `ts_check/`, the type-checker silently skips with only a stderr note, and the scan reports zero type issues regardless of whether types actually mismatch. In the fixture run, running `carrick tests/fixtures/koa-api` from a fixture cwd produced the "Type checking script not found" error.

**Layer B — distribution channel (growth/adoption bug).**
`ts_check/` ships today inside `carrick-action-linux.tar.gz` on GitHub Releases (see `.github/workflows/release.yml`). That tarball is Linux-x86_64 only, pulled by `action.yml` into the CI workspace. For a free-tier user who wants to run `carrick` against their own repo locally — macOS, Windows, any platform — there is no install path. No `npx`, no Homebrew, no `cargo install carrick` (the API endpoint is baked in at build time via `CARRICK_API_ENDPOINT` build-script, so `cargo install` from crates.io would also fail without that var).

### Root cause — Layer A

The code assumes the working directory contains a `ts_check/` sibling to the binary. The GitHub Action works because its setup step extracts the tarball into cwd and then invokes `./carrick`. Nothing in the Rust code resolves paths relative to the binary location (contrast with `discover_sidecar_path` in `src/main.rs:206-240`, which *does* walk executable-relative candidates — that pattern works).

### Fix — Layer A

Mirror the `discover_sidecar_path` pattern for `ts_check/`. Add `fn discover_ts_check_path() -> Option<PathBuf>` in `src/main.rs` that tries, in order:

1. Relative to executable: `<exe_dir>/ts_check`, `<exe_dir>/../ts_check`, `<exe_dir>/../lib/ts_check`.
2. `CARGO_MANIFEST_DIR` at compile time (for `cargo run` during dev): `<manifest_dir>/ts_check`.
3. CWD fallback: `./ts_check` (preserves current behavior for the GitHub Action's extract-to-cwd setup).

Pass the resolved `PathBuf` through to whichever struct owns the analysis run (probably a new field on `Analyzer` or a parameter on `run_final_type_checking`). Replace every hardcoded `"ts_check/..."` string in `src/analyzer/mod.rs` and `src/engine/mod.rs` with `self.ts_check_dir.join(...)`.

If no candidate path resolves, the current silent-skip behavior is wrong — log a single clear error pointing at the expected layout, so users know the distribution broke rather than assuming their code is type-clean.

### Fix — Layer B (the free-tier question)

The canonical answer for a Rust-backed Node-ecosystem dev tool is **npm with per-platform binary subpackages**. Precedents: `@biomejs/biome`, `oxlint`, `turbo`, `@swc/core`, `esbuild`. All of them distribute a TypeScript-ecosystem experience (`npm i`, `npx`) wrapping a native binary.

Recommended shape:

- **`carrick` on npm** — a thin meta-package. Its `bin/carrick.js` detects platform+arch at install time and delegates to the matching `@carrick/linux-x64`, `@carrick/darwin-arm64`, etc. It also owns the bundled `ts_check/` and `sidecar/` (since those are platform-independent JS).
- **`@carrick/<platform>-<arch>`** — per-platform native binary packages, listed as `optionalDependencies` of `carrick`. npm's optional-dependency resolver pulls only the matching one. Example surface needed: `linux-x64`, `linux-arm64`, `darwin-x64`, `darwin-arm64`, `win32-x64`.
- **Release pipeline** — extend `.github/workflows/release.yml`:
  - Matrix-build the binary for each target triple (`cargo build --target <triple>`).
  - Publish each platform package with its binary.
  - Publish the meta-package last.
  - Continue publishing the existing Linux tarball for the GitHub Action's consumption (or migrate the Action to consume the npm package — cleaner long-term, more work now).
- **`CARRICK_API_ENDPOINT`** is currently baked in at build time. For an npm-distributed binary, either: (a) keep it baked in at release time to point at production, or (b) make it runtime-configurable with a sensible default. Pick (a) for MVP — users shouldn't need to know the endpoint exists.

**Homebrew** is a nice-to-have for Mac users and can come later via a tap. **`cargo install carrick`** would be a nice-to-have for Rust users but requires dropping the compile-time `CARRICK_API_ENDPOINT` env-var dependency first.

### Acceptance — Layer A

- Running `carrick <abs-path>` from an arbitrary cwd (e.g., `$HOME`) finds `ts_check/` and type checks successfully when the expected layout exists adjacent to the binary.
- Running from inside the repo (`cargo run --release`) continues to work via the `CARGO_MANIFEST_DIR` fallback.
- When no `ts_check/` exists anywhere reachable, the tool emits a single actionable error naming the expected install layout — not a silent skip.
- Existing GitHub Action continues to work unchanged (the cwd fallback preserves its behavior).

### Acceptance — Layer B

- `npx carrick /some/repo` works on macOS-arm64 and Linux-x64 without prior setup.
- The installed package includes `ts_check/` and the sidecar so Fix 1A's discovery picks them up from the adjacent package dir.
- GitHub Action either continues to consume the release tarball or migrates cleanly to the npm package.

### Scope guardrail — Fix 3

- **Layer A and Layer B are separable.** Land Layer A first — it's a correctness bug with a 30-line diff. Layer B is a packaging project.
- **Don't delete the tarball-based release flow while landing Layer B.** Parallel-publish; cut over the Action only after the npm package has been live for a release cycle.
- **Don't change `CARRICK_API_ENDPOINT` semantics** — keep build-time baking. Making it runtime-configurable is a separate decision.

---

## Sequencing suggestion

1. **Fix 3 Layer A** — smallest, fixes a real silent-failure in every non-CI run.
2. **Fix 1** — single prompt edit; run `koa-api` fixture 10 times to confirm determinism.
3. **Fix 2** — prompt edits in two agents; run `hapi-api` fixture to confirm `/api/v1/status` appears.
4. **Fix 3 Layer B** — multi-week packaging work; own PR per platform once the binary build works.

After 1–3, the framework-coverage §1 table can be honestly updated: Fastify and NestJS → tested-working; Koa → tested-working (deterministic); Hapi → tested-working. Hono remains untested until a fixture exists.

## Scope guardrails (repeat — these apply to all three fixes)

From `framework-coverage.md` §0 ("Defaults already decided — do not re-litigate"):

- **Never add framework-specific strings or name lists to Rust or TypeScript source.** If a fix seems to require one, the fix is wrong; re-read §9.
- **No external infrastructure** beyond what the three fixes above already require (for Fix 3 Layer B: npm + GitHub Releases, which is the delivery mechanism, not a new runtime dependency).
- **Every move should either kill a heuristic or push one into LLM-generated guidance.** If the proposed change adds a heuristic to source, reject it.
