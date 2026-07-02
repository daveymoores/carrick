# Type Compatibility v2 — Compiler-Native Artifacts in a Synthetic Monorepo

**Status:** Proposed (2026-07)
**Supersedes (on completion):** the flat-bundle + `ts_check` comparison path
**Builds on:** `docs/research/compiler-sidecar-architecture/` (Phase 3, "Synthetic
Monorepo / Stub Snapshot"), `docs/research/type-checking-flow.md`

## Executive summary

The comparison engine is not the problem. `Type.isAssignableTo()` on real
compiler `Type` objects (`ts_check/lib/type-checker.ts:456`) is the right
verdict function, and the manifest matcher (method + normalized path +
type_kind, plus exact operation keys for socket/graphql/pubsub) is sound.

What makes type compat persistently hard is upstream of the verdict: **the
cross-repo artifact is a hand-serialized string.** Types are captured by
printing them to text with a custom printer (`expandTypeStructural`), shipped
as one flattened `.d.ts` string per service (`CloudRepoData.bundled_types`),
and re-parsed at check time in a synthetic flat project where the original
imports do not exist. Every hop is lossy, and nearly every hard-won fix in the
current system is compensation for that loss:

| Compensation | Root cause it patches |
|---|---|
| `expandTypeStructural` (custom recursive printer, depth cap 12) | named refs dangle in a flattened bundle → `any` |
| `append_missing_aliases` + `// carrick:missing-alias` marker (byte-identical across Rust and TS, `engine/mod.rs` ↔ `type-checker.ts:34`) | unresolved anchors must be distinguishable from real `unknown` (#244) |
| double `any`/`unknown` guards in `compareTypes` (raw types, then comparands) | dangling names resolve to top types → vacuously "compatible" |
| `cleanupPaths` regex in `run-type-checking.ts` | absolute `import("...")` specifiers leak into error text |
| merged `package.json` + `npm install --legacy-peer-deps` | library types kept by-name must resolve *somewhere* at check time |
| verdict join by re-parsing endpoint label strings (`analyzer/mod.rs`, `parse_compat_endpoint`) | no stable pair identity crosses the Rust/TS boundary |

We are incrementally reimplementing TypeScript's declaration emitter — one of
the hairiest parts of the compiler — one regression at a time (#149, #226,
#233, #240, #244, #253, #260 are all inline-documented instances).

**Proposal:** finish the synthetic-monorepo architecture that is already ~70%
built but unreachable from Rust (`MonorepoBuilder`, `SurfaceEmitter`,
tsconfig/dependency snapshots — the sidecar handles `emit_surface`,
`build_workspace`, `check_compatibility`, but `SidecarRequest` in
`src/services/type_sidecar.rs` never sends them), with two corrections to its
current design:

1. **The artifact becomes a compiler-emitted declaration tree** (a types-only
   package per service), not a flattened structurally-expanded string. Let
   `tsc` be the serializer.
2. **The checker becomes per-pair probe files** checked by `tsc --noEmit` in
   the workspace, with diagnostics attributed by filename. Let `tsc` be the
   judge, and let its elaborated errors be the user-facing mismatch report.

Everything the pipeline currently does to type *text* by hand should either
become a real file the compiler emitted or a real probe the compiler checked.

## Framing

Cross-repo type checking at different scan times is the problem npm publishing
already solves: a types-only package with pinned dependencies, installed and
consumed later by a different project. Each repo's scan should behave like
publishing `@carrick/<service>` — a package whose public API is the service's
endpoint contract. The cross-repo check is then "install all captured packages
into one workspace and typecheck generated probes."

This is the same insight as the existing Phase-3 stub-snapshot design; the
correction is *what goes inside the stub package*.

## Current state (what actually runs today)

Per service, at scan time (all in the source repo, full `node_modules`
available):

1. LLM file analysis emits type anchors per endpoint/call
   (`primary_type_symbol`, `type_import_source`, expression text, SWC spans).
2. `FileOrchestrator::collect_type_requests` turns anchors into sidecar
   requests: explicit `SymbolRequest`s, inline aliases, or `infer` requests.
3. The sidecar resolves them with ts-morph and serializes each result to
   structural text (`expandTypeStructural`: objects inlined recursively,
   library types kept by name). `TypeBundler` — `@deprecated` but live —
   renames declarations via first-occurrence `String.replace`.
4. Rust concatenates the results into one `.d.ts` string
   (`resolve_all_types`, `append_inline_aliases`), enriches the manifest by
   alias-string join, uploads `bundled_types` + `type_manifest`.

At check time (any repo's CI, after downloading all `CloudRepoData`):

5. `recreate_type_files_and_check` writes one `<stem>_types.d.ts` per service
   (padded with `= unknown // carrick:missing-alias` for unresolved aliases),
   merges every repo's dependencies into one `package.json`, runs
   `npm install --legacy-peer-deps`, and spawns
   `npx ts-node ts_check/run-type-checking.ts`.
6. `ts_check` matches manifests, loads every `.d.ts` into one flat ts-morph
   project, gates unverifiable pairs, and calls `isAssignableTo` with
   per-protocol direction (HTTP/GraphQL: `producer ⊑ consumer`;
   socket/pubsub inverted; GraphQL producers get a structural envelope
   unwrap).
7. Verdicts come back as JSON keyed by human-readable endpoint labels, which
   Rust re-parses to join onto `CrossRepoMatch.type_compatible`.

### Failure classes this structure produces

- **Dangling-name decay.** Anything the expander can't inline (depth cap,
  unexpandable shapes, expander bugs) ships as a bare name, resolves to
  `any`/`unknown` at check time, and surfaces as "unverifiable" far from the
  cause. The guard stack in `compareTypes` exists because top types make
  `isAssignableTo` vacuously true (the `graphql|subscription|orderUpdated`
  false positive needed a *second* guard on the post-unwrap comparands).
- **Version-conflict soundness hole.** The merged install resolves conflicting
  majors arbitrarily, so one repo's types are checked against the wrong
  library version. `xrepo-corpus-1` deliberately ships a zod 3.x/4.x conflict;
  the current checker resolves it by coin flip.
- **Semantic loss.** Eager structural expansion erases generic structure,
  nominal-ish semantics (branded types, classes with privates, enums,
  declaration merging), and type *names* — so even correct mismatch verdicts
  print anonymous `{ ... }` soup instead of "`OrderUpdate` requires `note`".
- **String-protocol coupling.** The marker comment, the alias-definition
  regexes duplicated in Rust (`dts_defines_alias` in `type_sidecar.rs` and
  `engine/mod.rs`), socket direction labels, GraphQL kind casing, and the
  verdict-label re-parse must all stay byte-compatible across two languages.
- **Latent direction bug (verify with an eval case).** `compareTypes` keys
  direction on protocol only, but pairs match per `type_kind`. For an HTTP
  `request` pair, data flows consumer → producer (the caller sends the body
  the endpoint must accept), so the check should be `consumer ⊑ producer`;
  the code runs `producer ⊑ consumer` for everything HTTP. If request-body
  types ever resolve to real shapes, widening/narrowing verdicts on them are
  inverted.

## Target architecture

### Capture phase (sidecar, per service, at scan time)

**Output artifact per service: a types-only stub package**, replacing the
single `bundled_types` string:

```
@carrick/<service>/
├── package.json          # name, types entry, pinned deps (from lockfile)
├── tsconfig.snapshot.json
└── types/
    ├── surface.d.ts      # entry: export type Endpoint_<hash>_Response = ...
    └── **/*.d.ts         # compiler-emitted declaration tree (closure)
```

Generation:

1. Write a **surface entry `.ts` file inside the real repo project** that
   aliases each manifest anchor:
   - Explicit symbol: `export type Endpoint_abc_Response =
     import('./src/types/order').OrderResponse;`
   - Addressable handler (implicit): `export type Endpoint_def_Response =
     Awaited<ReturnType<typeof import('./src/routes/orders').getOrder>>;`
     (after machinery unwrap — see below).
2. Run **the compiler's declaration emit** rooted at that entry
   (`declaration: true, emitDeclarationOnly: true`) and capture the emitted
   `.d.ts` **tree**. The compiler computes the transitive closure of local
   declarations and prints external references as real imports. No flattening;
   no custom printer for anything the compiler can address.
3. **Anonymous inferred types** (inline handlers with no addressable symbol —
   the reason the current design serializes everything): print via the
   compiler's node builder (`checker.typeToTypeNode` /
   `typeToString` with declaration-emit flags, `NoTruncation`), which emits
   `import("pkg").T` / `import("./x").T` references that now *resolve* because
   the tree and pinned deps travel with them. `expandTypeStructural` survives
   only as a last-resort fallback for shapes the node builder refuses, and its
   use is recorded per-alias in the manifest (`serialization: emitted |
   node_builder | structural_fallback`) so fidelity is measurable.
4. **Machinery unwrapping stays at capture time** where the real `Type` is in
   hand: transport generics (`Promise`, async iterables), agent-generated
   `ExtractionConfig` wrapper rules, and — moved here from the checker — the
   GraphQL resolver envelope unwrap. The checker should never guess at
   payload shape.
5. `package.json` dependencies are **pinned exact versions resolved from the
   repo's lockfile**, pruned to specifiers actually referenced by the emitted
   tree (the `SurfaceEmitter` already tracks referenced specifiers). tsconfig
   snapshot records the flags that affect assignability (`strict*`,
   `exactOptionalPropertyTypes`, …) plus the TypeScript version used.
6. **Capture-time self-check (new gate):** the stub package must typecheck
   standalone, and no exported alias may resolve to `any`. Failures downgrade
   the alias to `type_state: Unknown` *with a recorded reason* at capture
   time — converting today's silent degradation (discovered at match time as
   a confusing "unverifiable") into a per-repo, per-alias capture error.

`CloudRepoData` changes: `bundled_types: Option<String>` →
`type_surface: Option<TypeSurface>` where `TypeSurface = { files:
BTreeMap<RelPath, String>, pinned_deps: BTreeMap<String, String>,
tsconfig_snapshot: Value, ts_version: String }`. Per the no-backwards-compat
rule, the old field is deleted in the same commit; `cache_version` bumps.

### Check phase (synthetic monorepo, at cross-repo analysis time)

Reuses `MonorepoBuilder`'s workspace shape (pnpm, `node-linker=isolated`,
per-stub `node_modules` — this correctly fixes the version-conflict hole),
with these corrections:

1. **Stub packages carry the declaration tree**, not a single `surface.d.ts`
   of expanded text. `main`/`types` point at `types/surface.d.ts`; internal
   relative imports resolve within the tree; external imports resolve against
   the stub's own pinned `node_modules`. The `@carrick/{repo}/*` tsconfig
   path-mapping gymnastics in `createCheckerTsconfig` become unnecessary for
   type resolution (each stub resolves its own deps), remaining only for the
   checker package to import surfaces.
2. **One probe file per matched pair**, named by the pair's stable ID (reuse
   the FNV pair hash). Matching itself is unchanged (manifest matcher —
   whether it stays TS or moves to Rust is orthogonal). Probe content:

   ```ts
   // pair_<fnv-hash>.ts — <protocol> <method> <path> (<type_kind>)
   import type { Endpoint_abc_Response as Sent } from '@carrick/orders/surface';
   import type { Call_def_Response as Expected } from '@carrick/web/surface';

   type IsAny<T> = 0 extends 1 & T ? true : false;
   type Not<T extends boolean> = T extends true ? false : true;
   type Assert<T extends true> = T;

   // Top-type gates: a side that decayed to any/unknown must read
   // UNVERIFIABLE (its probe errors on these lines), never compatible.
   type _SentNotTop = Assert<Not<IsAny<Sent>>>;
   type _ExpectedNotTop = Assert<Not<IsAny<Expected>>>;

   // Value-level assignability in the data-flow direction.
   declare const sent: Sent;
   const expected: Expected = sent;
   ```

   Value-level assignment, not `[X] extends [Y]` conditional types: the
   conditional-type relation differs subtly from assignability (notably
   around `any`), and the compiler's **elaborated errors** — "Property 'note'
   is missing in type 'OrderUpdate' but required in type …" — become the
   user-facing mismatch report for free, with real type *names* preserved by
   the declaration tree.
3. **Direction is a table in the probe generator**, keyed on
   `(protocol, type_kind)` — one place, in Rust:

   | protocol | type_kind | sent | expected |
   |---|---|---|---|
   | http, graphql | response | producer | consumer |
   | http | request | consumer | producer |
   | socket, pubsub | both | consumer (emitter/publisher) | producer (listener/subscriber) |

   This structurally fixes the request-direction inversion and replaces the
   60-line direction comment in `compareTypes`.
4. **Unresolved anchors generate no probe.** The pair is recorded
   unverifiable in Rust, carrying the capture-time reason. No `= unknown`
   padding, no marker comments, no placeholder taxonomy.
5. **Run `tsc --noEmit` once over the checker package; attribute diagnostics
   by probe filename.** This kills the weakest part of the current
   `MonorepoBuilder`: `parseCheckResult` computes an `isRelated` regex match
   against tsc output and then ignores it, marking every check incompatible
   on any failure. Verdict classification per probe:
   - errors only on `_*NotTop` lines → **unverifiable** (with which side);
   - error on the assignment line → **incompatible**, diagnostic text is the
     report;
   - no errors → **compatible**.
6. **Verdicts return keyed by pair ID**, not by endpoint label strings.
   `apply_compat_verdicts` joins by ID; `parse_compat_endpoint` /
   `parse_producer_key` are deleted.
7. **Workspace caching:** key the installed workspace on a hash of all stubs'
   pinned dep sets; pnpm's content-addressable store makes warm installs
   cheap in CI. Record `tsc` version in results (assignability is stable
   across patch versions, but the artifact should say what judged it).

### What this deletes

- `type-structural-expander.ts` as a primary path (fallback only, measured);
  `definition-resolver.ts`'s expansion duplication.
- `append_missing_aliases`, `MISSING_ALIAS_MARKER` (both sides), both
  `dts_defines_alias` regex copies, placeholder-vs-real-`unknown`
  disambiguation (#244 machinery).
- The `any`/`unknown` imperative guard stack in `compareTypes` (replaced by
  declarative per-probe gates).
- `cleanupPaths`, the merged `package.json` + `--legacy-peer-deps` install,
  `create_dynamic_tsconfig`'s `*-types` path mapping.
- `ts_check/lib/type-checker.ts`'s comparison core and the endpoint-label
  verdict round-trip in `analyzer/mod.rs`.
- The deprecated-but-live `TypeBundler` and its `String.replace` renames.

### What stays unchanged

- LLM anchor extraction, SWC span plumbing, `ExtractionConfig` unwrap rules.
- The manifest, deterministic alias hashing (`type_manifest.rs`), and
  operation keys.
- The manifest matcher's normalization and per-protocol matching.
- The warm sidecar process model and stdio protocol (new actions, same
  transport).
- The eval corpus and scorer contract — verdict semantics
  (`compatible / incompatible / unverifiable`, `None` never read as
  compatible) are preserved.

## Risks and open questions

1. **Declaration emit on real-world repos.** `emitDeclarationOnly` requires
   the program to be declaration-emittable; repos with type errors or
   `isolatedDeclarations`-hostile patterns may fail. Mitigation: emit is
   rooted at the surface entry (only its closure matters); on failure, fall
   back per-alias to the node-builder path and record it. The self-check gate
   makes fallback quality visible instead of silent. Node-builder output for
   complex inferred types can still reference unexported local symbols
   (`TS4023`-class issues) — those aliases degrade to `structural_fallback`,
   which is today's behavior, now measured.
2. **Artifact size.** A declaration tree is larger than one flattened string.
   Trees are trivially compressible and pruned to the entry's closure; S3
   payloads stay small relative to `file_results` already stored. Measure in
   the eval fixtures before committing to limits.
3. **Private-source leakage.** The tree ships real declaration text (as the
   expanded bundle already does, structurally). Types-only, no
   implementation bodies; same trust model as today, but worth stating in
   the artifact docs.
4. **tsconfig heterogeneity.** Producer and consumer may compile under
   different strictness. Decide and document one policy: the checker package
   compiles probes under `strict: true` (recommended — the check asserts
   wire-contract compatibility, not either repo's local flags), with stub
   snapshots retained for diagnosis. `exactOptionalPropertyTypes` mismatches
   are the sharpest edge; add an eval case.
5. **pnpm availability in the GitHub Action runtime.** `MonorepoBuilder`
   already falls back to npm; isolated linking is the property that matters,
   so document that npm fallback loses per-stub isolation and prefer
   `corepack`-pinned pnpm in the action image.
6. **Install cost per check.** Bounded by pruned dep sets + workspace
   caching; the current path already pays an uncached
   `npm install --legacy-peer-deps` on every run, so this should be neutral
   or better warm. Benchmark on `xrepo-corpus-1/2`.

## Future hook: wire-truth vs type-truth

TS assignability is a proxy for what Carrick actually claims: **wire
compatibility of JSON payloads**. `Date` (producer) vs `string` (consumer) is
wire-compatible but TS-incompatible; branded types are wire-identical but
TS-distinct. Because v2 checks run in real tsc probes, a type-level
serialization model becomes possible:

```ts
const expected: Serialize<Expected> = serializedSent; // Serialize<T> models JSON.stringify
```

where `Serialize<T>` maps `Date → string`, respects `toJSON()`, and drops
functions/`undefined`. Impossible to bolt onto `isAssignableTo` over
pre-serialized text; a natural follow-up experiment once probes are the
substrate. Not in scope for the initial migration.

## Eval leverage

The migration decomposes today's hardest-to-debug metric (verdict accuracy)
into independently measurable stages:

1. **Anchor resolution rate** (exists): manifest entries reaching
   `Explicit`/`Implicit`.
2. **Surface fidelity (new):** stub package self-check pass rate +
   per-alias `serialization` histogram (`emitted` > `node_builder` >
   `structural_fallback`). Ratchet this in the Tier-A baseline.
3. **Match F1** (exists, unchanged).
4. **Verdict accuracy** (exists): now attributable — a wrong verdict with a
   clean stage-2 is a checker bug; with a dirty stage-2, a capture bug.

New eval cases to add with the migration: HTTP request-body direction
(widening and narrowing, both orders), `exactOptionalPropertyTypes` skew,
conflicting dependency majors where the *types* differ across versions
(upgrade the zod 3/4 fixture from dependency-report-only to a
type-verdict assertion), and a `Date`-serialization pair (expected
incompatible today; flips when `Serialize<T>` lands).

## Migration plan

The engine seams already exist (`bundled_types` artifact per service; manifest
enrichment; `ts_check_dir` injection). Order:

1. **Wire `emit_surface` v2** in the sidecar: surface entry generation,
   declaration-emit tree capture, node-builder fallback, self-check gate,
   `serialization` tagging. Ship `TypeSurface` in `CloudRepoData` (replacing
   `bundled_types`; bump `cache_version`).
2. **Wire `build_workspace` + probe generation** from matched pairs; verdict
   classification by probe filename; ID-keyed verdict join. Run the full
   xrepo eval against the Tier-A baseline.
3. **Delete the old path** (flat bundles, padding, expander-as-primary,
   `ts_check` comparator, label re-parse) in the same change that flips the
   default — no parallel code paths, per repo policy.

Each step is eval-gated: step 1 lands when surface fidelity ≥ current
resolution rate on the corpus; step 2/3 land when verdict accuracy is ≥
baseline with no new false-compatibles (the scorer's §7 guard already fails
loud if compat data goes silently absent).
