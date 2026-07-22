/**
 * Capture-time self-check (design doc, Capture step 8, amendment 1), with
 * per-alias closure attribution.
 *
 * The stub tree is typechecked standalone with `skipLibCheck: false` --
 * spike-verified as load-bearing: the tree is entirely .d.ts and skipLibCheck
 * skips declaration files wholesale, making the gate vacuous. When the
 * source repo has node_modules, resolution is pointed at it via a temporary
 * node_modules symlink inside the stub, so externals resolve exactly as they
 * will at check time against installed pins.
 *
 * Classification per alias (three-way, keyed on diagnostics, never on
 * printed type text):
 *  - ok: resolves to a concrete type.
 *  - allowlisted_external: resolution failed only through external
 *    specifiers pinned in the stub's dependencies, on a bare checkout. The
 *    alias KEEPS its serialization tier; the check-phase probe gates
 *    (any/unknown/never, both sides) are the backstop.
 *  - decayed_internal: a dangling internal specifier, an unpinned external,
 *    or a top-type resolution with no allowlisted explanation.
 *
 * Attribution is per-alias closure: failed specifiers are blamed on an alias
 * only if they occur in a file reachable from that alias's surface statement
 * (import-type seeds, then BFS over relative imports). The spike's
 * file-granularity shortcut is gone.
 */

import ts from 'typescript';
import * as fs from 'node:fs';
import * as path from 'node:path';
import type { CaptureAliasRecord, SelfCheckOutcome } from './api.js';
import type { ResolvedAnchor } from './anchors.js';
import { collectSpecifiers, isRelative, packageNameOf } from './specifiers.js';

export interface SelfCheckArgs {
  stubDir: string;
  surfaceAbsPath: string;
  resolved: ResolvedAnchor[];
  pinned: Record<string, string>;
  bareCheckout: boolean;
  /** Producer repo root; its node_modules (if any) backs resolution. */
  repoRoot: string;
}

interface FileFailures {
  externalPinned: Set<string>;
  internal: Set<string>;
}

export function selfCheckStub(args: SelfCheckArgs): CaptureAliasRecord[] {
  const typesDir = path.join(args.stubDir, 'types');
  const treeFiles: string[] = [];
  const walk = (dir: string) => {
    for (const entry of fs.readdirSync(dir, { withFileTypes: true })) {
      const p = path.join(dir, entry.name);
      if (entry.isDirectory()) walk(p);
      else if (entry.name.endsWith('.d.ts')) treeFiles.push(p);
    }
  };
  walk(typesDir);

  // Point resolution at the producer repo's installed deps when present.
  const repoNodeModules = path.join(args.repoRoot, 'node_modules');
  const linkPath = path.join(args.stubDir, 'node_modules');
  let linked = false;
  if (!args.bareCheckout && fs.existsSync(repoNodeModules) && !fs.existsSync(linkPath)) {
    fs.symlinkSync(repoNodeModules, linkPath, 'dir');
    linked = true;
  }

  try {
    return runSelfCheck(args, treeFiles);
  } finally {
    // unlinkSync, not rmSync: the link target is a directory and rmSync
    // refuses symlinks-to-directories with EISDIR.
    if (linked) fs.unlinkSync(linkPath);
  }
}

function runSelfCheck(args: SelfCheckArgs, treeFiles: string[]): CaptureAliasRecord[] {
  const program = ts.createProgram(treeFiles, {
    noEmit: true,
    strict: true,
    // MUST be false: the whole stub tree is .d.ts (see module header).
    skipLibCheck: false,
    module: ts.ModuleKind.ESNext,
    moduleResolution: ts.ModuleResolutionKind.Bundler,
    types: [],
  });
  const checker = program.getTypeChecker();
  const diagnostics = ts.getPreEmitDiagnostics(program);

  // Failed module specifiers, split external-pinned vs internal, per FILE.
  const failuresByFile = new Map<string, FileFailures>();
  const failuresFor = (fileName: string): FileFailures => {
    let entry = failuresByFile.get(fileName);
    if (!entry) {
      entry = { externalPinned: new Set(), internal: new Set() };
      failuresByFile.set(fileName, entry);
    }
    return entry;
  };
  for (const d of diagnostics) {
    if ((d.code !== 2307 && d.code !== 2792) || !d.file) continue;
    const msg = ts.flattenDiagnosticMessageText(d.messageText, ' ');
    const m = /Cannot find module '([^']+)'/.exec(msg);
    if (!m) continue;
    const spec = m[1];
    const bucket = failuresFor(path.resolve(d.file.fileName));
    if (!isRelative(spec) && args.pinned[packageNameOf(spec)]) {
      bucket.externalPinned.add(spec);
    } else {
      bucket.internal.add(spec);
    }
  }

  // Per-file relative-import adjacency for closure walks.
  const adjacency = new Map<string, string[]>();
  const treeSet = new Set(treeFiles.map((f) => path.resolve(f)));
  for (const file of treeFiles) {
    const abs = path.resolve(file);
    const text = fs.readFileSync(abs, 'utf8');
    const neighbors: string[] = [];
    for (const spec of collectSpecifiers(text)) {
      const resolved = resolveTreeSpecifier(abs, spec, treeSet);
      if (resolved) neighbors.push(resolved);
    }
    adjacency.set(abs, neighbors);
  }

  const surfaceAbs = path.resolve(args.surfaceAbsPath);
  const surfaceSource = program.getSourceFile(surfaceAbs);

  const records: CaptureAliasRecord[] = [];
  for (const anchor of args.resolved) {
    // Demotions (failureReason present) never reached the surface with a
    // real type; everything else — including successful literal anchors,
    // which sit at the structural_fallback TIER but did produce text — is
    // classified by the real self-check.
    records.push(
      anchor.failureReason !== undefined
        ? demotedRecord(anchor)
        : checkedRecord(anchor, {
            args,
            program,
            checker,
            surfaceAbs,
            surfaceSource,
            adjacency,
            failuresByFile,
          })
    );
  }
  return records;
}

/** An alias that never reached a capture-native tier: the failure reason was
 * recorded at demotion time; the surface line is `unknown` by construction. */
function demotedRecord(anchor: ResolvedAnchor): CaptureAliasRecord {
  return {
    alias: anchor.request.alias,
    anchor_kind: anchor.request.kind,
    symbol_name: 'symbol_name' in anchor.request ? anchor.request.symbol_name : undefined,
    source_file: 'source_file' in anchor.request ? anchor.request.source_file : '<inline>',
    anchor_origin: anchor.request.anchor_origin,
    serialization: 'structural_fallback',
    self_check: 'decayed_internal',
    self_check_detail: anchor.failureReason,
    capture_failure_reason: anchor.failureReason,
    top_type_at_self_check: true,
  };
}

/** A disqualifying finding below the root of a captured type: an author-baked
 * `any`/`unknown`, or `budget_exhausted` — a subtree the walk could not finish
 * within its depth/node budget, treated as unverifiable (fail closed) rather
 * than silently clean. */
interface DeepTopType {
  kind: 'any' | 'unknown' | 'budget_exhausted';
  /** Human-readable member path, e.g. `metadata`, `items<0>.meta`, `[index]`. */
  path: string;
}

/**
 * Depth-bounded structural walk for a disqualifying top type at ANY depth:
 * a member, array element, index signature, type argument, or callable RETURN
 * that resolved to `any` (bidirectionally assignable — an arbitrary
 * counterparty shape reads compatible) or `unknown` (a failed-inference bake;
 * carries no shape information). The check phase's probe gates are WHOLE-type
 * only, so this walk owns the depths they cannot see. It is a genuine SUPERSET
 * of v1's text-scan disqualifier (`contains_disqualifying_top_type`) at ALL
 * depths/widths — so removing the `type_state == Unknown` pre-verdict (carrick
 * #448) never turns a shape v1 would have abstained on into a false-compatible.
 * The superset holds unconditionally because budget EXHAUSTION FAILS CLOSED
 * (returns the `budget_exhausted` sentinel, not "clean"): a subtree too deep or
 * wide to finish within budget is unverifiable, never silently compatible. A
 * bigger bound would only relocate the fail-open cliff; failing closed removes
 * it. v1's text scan is itself unbounded, so any disqualifier it would flag
 * that lies past this walk's finite budget is still caught — as
 * `budget_exhausted` rather than its exact kind.
 *
 * Cycle-safe. Two deliberate exceptions to "flag any/unknown anywhere":
 *  - callable PARAMETER types are NOT descended (only return types): a
 *    parameter `any` is contravariant and genuinely permissive (`(x: any) =>
 *    void` safely accepts a stricter counterparty), so it is not a masked
 *    mismatch and demoting it would over-demote a sound shape;
 *  - TypeScript's unresolved-reference `error` placeholder (`intrinsicName ===
 *    'error'`) is excluded (see `flagOf`): it heals when the check installs the
 *    pinned external, so it is a healable decay, not an author-baked `any`.
 * A type the walk cannot cheaply finish is NOT flagged — over-demoting a
 * legitimately fully-resolved type is the failure mode this guard must not have.
 */
function findDisqualifyingTopType(
  root: ts.Type,
  program: ts.Program,
  checker: ts.TypeChecker,
  location: ts.Node
): DeepTopType | undefined {
  // Cover v1's inline-expander reach with margin so this structural walk is a
  // genuine superset of v1's text-scan disqualifier AT DEPTH: anything v1 could
  // expand-and-flag as `any`/`unknown`, this walk reaches too. v1's expander
  // (`MAX_EXPANSION_DEPTH` in ../type-structural-expander.ts) reaches 12; 16
  // clears it with margin. That constant is NOT imported: the capture bundle
  // seam (`capture-v2-seam.test.ts`) forbids capture/ from importing across the
  // boundary, so the value is pinned here and kept ">= MAX_EXPANSION_DEPTH" by
  // that contract. The node budget scales with the deeper bound so a real
  // deep-but-narrow type cannot exhaust it before the walk reaches a buried
  // `any` — running out early would fail OPEN (read compatible).
  const MAX_DEPTH = 16;
  const MAX_VISITED = 4096;
  const seen = new Set<ts.Type>();
  let visited = 0;

  const flagOf = (t: ts.Type): 'any' | 'unknown' | undefined => {
    if (t.flags & ts.TypeFlags.Any) {
      // TypeScript's unresolved-reference placeholder (e.g. `import('ext').Foo`
      // on a bare checkout) carries `TypeFlags.Any` but `intrinsicName ===
      // 'error'` — NOT an author-baked `any`. It resolves to the real type once
      // the check phase installs the pinned external, so it must not count as a
      // disqualifier: treating it as `any` would demote a healable external
      // reference. Genuine author `any` carries `intrinsicName === 'any'`.
      // (`intrinsicName` is internal but stable since TS 1.x — same standing as
      // its use in anchors.ts.)
      //
      // The `error` placeholder also stands in for NON-healable causes (TS2304
      // undefined name, TS2315 wrong-arity generic, a dangling internal
      // specifier). Excluding those here is not a hole: each emits a diagnostic
      // in the alias's own closure, so the closure-failure classification
      // (`internalFailure` -> decayed_internal) or the check-phase POISON rule
      // — NOT this deep walk — is their backstop, and both fail closed.
      const name = (t as unknown as { intrinsicName?: string }).intrinsicName;
      return name === 'error' ? undefined : 'any';
    }
    return t.flags & ts.TypeFlags.Unknown ? 'unknown' : undefined;
  };

  const walk = (t: ts.Type, path: string, depth: number): DeepTopType | undefined => {
    // Genuine cycle handling — NOT fail-open. `t` is already on the walk stack
    // (or was fully explored earlier), so the owning frame completes it;
    // returning "clean" here is sound because a type reaches `seen` ONLY after a
    // visit that fully completed clean. A visit that hit the budget below
    // returns the `budget_exhausted` sentinel, which bubbles up (every frame
    // propagates a truthy child return) and terminates the whole walk before
    // any shallower re-entry — so `seen` never memoizes a truncated visit as
    // clean. And a type fully explored clean at depth d1 is clean at any d2 < d1
    // (the shallower reach is a superset of the deeper), so reusing it is safe.
    if (seen.has(t)) return undefined;
    // Budget exhaustion FAILS CLOSED. `return undefined` (clean) here would let
    // an `any` buried past the depth/node budget read compatible — the
    // fail-open cliff a bigger number only relocates. Instead abstain: the
    // alias demotes to unverifiable, over-abstaining on a legitimately clean
    // type deeper/wider than budget rather than ever false-compatible.
    if (depth > MAX_DEPTH || visited > MAX_VISITED) {
      return { kind: 'budget_exhausted', path: path === '' ? '<root>' : path };
    }
    seen.add(t);
    visited++;

    // Root-level top types are the caller's whole-type check (and the check
    // phase's probe gates catch them); this walk owns depth > 0.
    if (depth > 0) {
      const kind = flagOf(t);
      if (kind) return { kind, path };
    }

    if (t.flags & (ts.TypeFlags.Union | ts.TypeFlags.Intersection)) {
      for (const part of (t as ts.UnionOrIntersectionType).types) {
        const found = walk(part, path, depth + 1);
        if (found) return found;
      }
      return undefined;
    }

    if (!(t.flags & ts.TypeFlags.Object)) return undefined;

    // Callable members: descend the RETURN type of every call/construct
    // signature — a return is a COVARIANT wire position, so `() => any`
    // covariantly widens `() => string` and an `any` there masks a real
    // mismatch exactly as a plain-member `any` does (v1's text scan flags it).
    // PARAMETER types are deliberately NOT descended: a parameter `any` is
    // contravariant and genuinely permissive (`(x: any) => void` safely
    // accepts a stricter counterparty), so demoting it would over-demote a
    // sound shape. Fall through afterwards so a hybrid callable
    // (`{ (): T; data: any }`) still has its own members walked below.
    for (const sig of [...t.getCallSignatures(), ...t.getConstructSignatures()]) {
      const found = walk(sig.getReturnType(), `${path}()`, depth + 1);
      if (found) return found;
    }

    // Type arguments: arrays, tuples, Promise<T>, Map<K, V>, ...
    if ((t as ts.ObjectType).objectFlags & ts.ObjectFlags.Reference) {
      const args = checker.getTypeArguments(t as ts.TypeReference);
      for (let i = 0; i < args.length; i++) {
        const found = walk(args[i], `${path}<${i}>`, depth + 1);
        if (found) return found;
      }
    }

    // Index signatures: { [k: string]: T }, Record<string, T>.
    for (const info of checker.getIndexInfosOfType(t)) {
      const found = walk(info.type, `${path}[index]`, depth + 1);
      if (found) return found;
    }

    for (const prop of t.getProperties()) {
      // Skip the built-in method suite (`Array#map`, `Promise#then`, ...): a
      // lib-declared member is machinery, never a wire payload, and descending
      // its return type recurses (map -> U[] -> map -> ...) until it exhausts
      // the budget — which pre-fail-closed silently read "clean" and now would
      // over-abstain on every ordinary `T[]`. The element/value type is still
      // covered via the type-argument branch above; a USER function-typed
      // member (`getData: () => any`) is not lib-declared, so it is still walked.
      const decl = prop.valueDeclaration ?? prop.declarations?.[0];
      if (decl && program.isSourceFileDefaultLibrary(decl.getSourceFile())) {
        continue;
      }
      const propType = checker.getTypeOfSymbolAtLocation(prop, location);
      const found = walk(
        propType,
        path === '' ? prop.getName() : `${path}.${prop.getName()}`,
        depth + 1
      );
      if (found) return found;
    }
    return undefined;
  };

  return walk(root, '', 0);
}

function checkedRecord(
  anchor: ResolvedAnchor,
  ctx: {
    args: SelfCheckArgs;
    program: ts.Program;
    checker: ts.TypeChecker;
    surfaceAbs: string;
    surfaceSource: ts.SourceFile | undefined;
    adjacency: Map<string, string[]>;
    failuresByFile: Map<string, FileFailures>;
  }
): CaptureAliasRecord {
  const alias = anchor.request.alias;
  let topType = true;
  let deep: DeepTopType | undefined;
  const seeds: string[] = [];

  if (ctx.surfaceSource) {
    for (const stmt of ctx.surfaceSource.statements) {
      if (!ts.isTypeAliasDeclaration(stmt) || stmt.name.text !== alias) continue;
      const type = ctx.checker.getTypeAtLocation(stmt.name);
      topType =
        (type.flags & (ts.TypeFlags.Any | ts.TypeFlags.Unknown | ts.TypeFlags.Never)) !== 0;
      if (!topType) {
        deep = findDisqualifyingTopType(type, ctx.program, ctx.checker, stmt.name);
      }
      // Seed the closure with the alias's own import-type targets.
      const visit = (node: ts.Node) => {
        if (ts.isImportTypeNode(node) && ts.isLiteralTypeNode(node.argument)) {
          const lit = node.argument.literal;
          if (ts.isStringLiteral(lit) && isRelative(lit.text)) {
            const resolved = resolveTreeSpecifier(
              ctx.surfaceAbs,
              lit.text,
              new Set(ctx.adjacency.keys())
            );
            if (resolved) seeds.push(resolved);
          }
        }
        node.forEachChild(visit);
      };
      visit(stmt);
    }
  }

  // Closure = surface itself + BFS from the alias's import-type seeds.
  const closure = new Set<string>([ctx.surfaceAbs]);
  const queue = [...seeds];
  while (queue.length > 0) {
    const file = queue.pop()!;
    if (closure.has(file)) continue;
    closure.add(file);
    for (const next of ctx.adjacency.get(file) ?? []) queue.push(next);
  }

  let blamedExternal: string | undefined;
  let internalFailure: string | undefined;
  for (const file of closure) {
    const failures = ctx.failuresByFile.get(file);
    if (!failures) continue;
    if (!blamedExternal) blamedExternal = [...failures.externalPinned][0];
    if (!internalFailure) internalFailure = [...failures.internal][0];
  }

  // Classification consults the closure failures REGARDLESS of the root
  // type: a dangling internal specifier means part of this alias's closure
  // is missing and the emitted tree is silently wrong even when the root
  // resolves to a concrete shape (adversarial-review finding 1).
  let outcome: SelfCheckOutcome = 'ok';
  let detail: string | undefined;
  const allowlisted = (): void => {
    const pkg = packageNameOf(blamedExternal!);
    outcome = 'allowlisted_external';
    detail =
      `unresolved external '${blamedExternal}' is pinned ` +
      `(${pkg}@${ctx.args.pinned[pkg]}); kept at tier ${anchor.serialization} ` +
      'on bare checkout; probe gates are the backstop';
  };
  // `unexplainedDeep`: a member-level author-baked any/unknown. The deep walk
  // excludes TypeScript's unresolved-reference `error` placeholder (see
  // `flagOf`), so `deep` is ALWAYS an author-baked disqualifier that survives
  // into the emitted `.d.ts` text — never a healable external decay. The check
  // phase must pre-gate exactly these: its probe gates are whole-type only and
  // cannot see a member-level any.
  let unexplainedDeep: DeepTopType | undefined;
  if (internalFailure) {
    outcome = 'decayed_internal';
    detail = `dangling internal specifier '${internalFailure}'`;
  } else if (topType || deep) {
    // `deep` is only computed when `!topType`, so the two are mutually
    // exclusive here.
    if (deep) {
      // A member-level disqualifier. Either an author-baked any/unknown (baked
      // into the emitted text; does NOT heal when the check installs pins) or a
      // `budget_exhausted` sentinel (a subtree too deep/wide to finish). Record
      // it even when the closure ALSO carries a failing pinned external (which
      // would otherwise allowlist the alias) — fail closed.
      unexplainedDeep = deep;
      outcome = 'decayed_internal';
      detail =
        deep.kind === 'budget_exhausted'
          ? `type too deep or wide to verify within the capture budget at ` +
            `'${deep.path}'; abstaining (fail closed) rather than risk a buried ` +
            'any reading compatible'
          : `type carries '${deep.kind}' at '${deep.path}'; an arbitrary ` +
            'counterparty shape would read compatible';
    } else if (blamedExternal) {
      // Root top type from a pinned external decay: heals when the check
      // installs the pin.
      if (ctx.args.bareCheckout) {
        allowlisted();
      } else {
        outcome = 'decayed_internal';
        detail = `unresolved pinned external '${blamedExternal}' on an installed checkout`;
      }
    } else {
      // Root top type with no external explanation.
      outcome = 'decayed_internal';
      detail = 'alias resolved to a top type with no allowlisted external failure';
    }
  } else if (blamedExternal) {
    // The alias's own type is fully concrete, but its closure has failing
    // pinned-external specifiers — previously invisible behind the topType
    // gate. On a bare checkout that is the expected amendment-1 shape; on an
    // installed checkout it means the tree references something the repo's
    // own node_modules could not resolve.
    if (ctx.args.bareCheckout) {
      allowlisted();
    } else {
      outcome = 'decayed_internal';
      detail = `unresolved pinned external '${blamedExternal}' on an installed checkout`;
    }
  }

  return {
    alias,
    anchor_kind: anchor.request.kind,
    symbol_name: 'symbol_name' in anchor.request ? anchor.request.symbol_name : undefined,
    source_file: 'source_file' in anchor.request ? anchor.request.source_file : '<inline>',
    anchor_origin: anchor.request.anchor_origin,
    serialization: anchor.serialization,
    self_check: outcome,
    // A #438/#439 re-aim note is provenance only; a real decay detail always
    // wins, so it surfaces exactly when the re-aimed alias self-checks clean.
    self_check_detail: detail ?? anchor.reaimNote,
    top_type_at_self_check: topType,
    ...(unexplainedDeep
      ? {
          deep_top_type_kind: unexplainedDeep.kind,
          deep_top_type_path: unexplainedDeep.path,
        }
      : {}),
  };
}

/** Resolve a relative specifier from `fromAbs` to a tree file, if present. */
function resolveTreeSpecifier(
  fromAbs: string,
  spec: string,
  tree: Set<string>
): string | undefined {
  if (!isRelative(spec)) return undefined;
  const base = path.resolve(path.dirname(fromAbs), spec);
  const candidates = [
    `${base}.d.ts`,
    path.join(base, 'index.d.ts'),
    base.endsWith('.js') ? `${base.slice(0, -3)}.d.ts` : undefined,
    base, // already .d.ts
  ].filter((c): c is string => c !== undefined);
  for (const candidate of candidates) {
    if (tree.has(candidate)) return candidate;
  }
  return undefined;
}
