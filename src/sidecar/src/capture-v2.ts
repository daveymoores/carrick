/**
 * SPIKE: type-compat v2 capture — "tsc as the serializer".
 *
 * Prototype of the v2 capture phase from
 * docs/research/type-compat-synthetic-monorepo.md ("Capture phase", steps
 * 1-2, 7, 8, plus the serialization/anchor-provenance tagging). Produces a
 * types-only stub package for one service:
 *
 *   @carrick/<service>/
 *   |- package.json            name, types entry, pinned deps (exact versions)
 *   |- tsconfig.snapshot.json
 *   |- carrick-manifest.json   per-alias records (spike stand-in for the
 *   |                          descriptor that will ride CloudRepoData)
 *   `- types/
 *      |- surface.d.ts         entry: export type <alias> = import(...).<Symbol>
 *      `- nested .d.ts tree    compiler-emitted declaration closure
 *
 * Deliberately NOT covered by the spike (see the sizing memo on #136):
 * Awaited<ReturnType<...>> implicit anchors and their guards, the
 * SymbolTracker node-builder fallback, tsconfig-paths specifier rewriting,
 * global-augmentation inclusion, machinery unwrapping, per-alias closure
 * attribution in the self-check (this file attributes at file granularity).
 *
 * The self-check here implements the amended rule from the design study:
 * external-specifier resolution failures whose package is pinned in the
 * stub's dependencies are ALLOWLISTED on bare checkouts (no node_modules at
 * capture time) instead of decaying the alias to Unknown; the decay rule is
 * kept for fully-internal failures. The check-phase probe gates
 * (any/unknown/never, both sides) remain the backstop for aliases the
 * allowlist lets through that still resolve to a top type at check time.
 */

import ts from 'typescript';
import * as fs from 'node:fs';
import * as path from 'node:path';
import * as os from 'node:os';

export type AnchorOrigin = 'llm-symbol' | 'deterministic-infer' | 'anchor-backfill';

export interface CaptureAnchorRequest {
  /** Manifest alias, e.g. Endpoint_abc123_Response */
  alias: string;
  /** Exported symbol name in the producer repo */
  symbol_name: string;
  /** Declaring module, repo-root-relative, e.g. src/types/stock.ts */
  source_file: string;
  /** How the anchor was produced (design-doc amendment: fidelity ratchet
   * must separate anchor-recall loss from serialization loss). */
  anchor_origin: AnchorOrigin;
}

export type SelfCheckOutcome = 'ok' | 'allowlisted_external' | 'decayed_internal';

export interface CaptureAliasRecord {
  alias: string;
  symbol_name: string;
  source_file: string;
  anchor_origin: AnchorOrigin;
  /** Serialization tier. The spike only exercises the primary tier. */
  serialization: 'emitted';
  self_check: SelfCheckOutcome;
  /** Human-readable reason when self_check is not 'ok'. */
  self_check_detail?: string;
  /** True when the alias resolved to any/unknown/never during self-check.
   * With self_check === 'allowlisted_external' this is expected on a bare
   * checkout and is NOT a decay; the probe gates own the final verdict. */
  top_type_at_self_check: boolean;
}

export interface CaptureStubResult {
  success: boolean;
  stub_dir: string;
  package_name: string;
  /** Stub-relative paths of the emitted declaration tree. */
  emitted_files: string[];
  /** Exact-version pins for external packages referenced by the tree. */
  pinned_dependencies: Record<string, string>;
  /** External specifiers referenced by the tree but absent from the lockfile. */
  unpinned_externals: string[];
  aliases: CaptureAliasRecord[];
  /** True when the source repo had no node_modules at capture time. */
  bare_checkout: boolean;
  ts_version: string;
  errors: string[];
}

export interface CaptureStubOptions {
  repoRoot: string;
  serviceName: string;
  anchors: CaptureAnchorRequest[];
  /** Directory the stub package is written into (created if missing). */
  outDir: string;
  tsconfigPath?: string;
}

const SURFACE_ENTRY_BASENAME = '__carrick_surface__';

/** Same normalization intent as bundle_file_stems on the Rust side. */
export function sanitizeServiceName(name: string): string {
  return name.toLowerCase().replace(/[^a-z0-9._-]+/g, '-').replace(/^-+|-+$/g, '');
}

function fail(stubDir: string, packageName: string, errors: string[]): CaptureStubResult {
  return {
    success: false,
    stub_dir: stubDir,
    package_name: packageName,
    emitted_files: [],
    pinned_dependencies: {},
    unpinned_externals: [],
    aliases: [],
    bare_checkout: false,
    ts_version: ts.version,
    errors,
  };
}

/** Extract every module specifier mentioned in a .d.ts text: `from "x"`,
 * `import "x"`, and `import("x")` type references. */
function collectSpecifiers(text: string): Set<string> {
  const specs = new Set<string>();
  const re = /(?:from\s+|import\s+|import\s*\(\s*)["']([^"']+)["']/g;
  let m: RegExpExecArray | null;
  while ((m = re.exec(text)) !== null) {
    specs.add(m[1]);
  }
  return specs;
}

function isRelative(spec: string): boolean {
  return spec.startsWith('./') || spec.startsWith('../') || spec.startsWith('/');
}

/** zod -> zod, @scope/pkg/sub -> @scope/pkg, pkg/sub -> pkg */
function packageNameOf(spec: string): string {
  const parts = spec.split('/');
  return spec.startsWith('@') ? parts.slice(0, 2).join('/') : parts[0];
}

/** Exact versions from package-lock.json (v2/v3 `packages` map, v1 fallback). */
function lockfileVersions(repoRoot: string): Map<string, string> {
  const versions = new Map<string, string>();
  const lockPath = path.join(repoRoot, 'package-lock.json');
  if (!fs.existsSync(lockPath)) return versions;
  try {
    const lock = JSON.parse(fs.readFileSync(lockPath, 'utf8'));
    if (lock.packages && typeof lock.packages === 'object') {
      for (const [key, entry] of Object.entries<{ version?: string }>(lock.packages)) {
        const idx = key.lastIndexOf('node_modules/');
        if (idx === -1) continue;
        const name = key.slice(idx + 'node_modules/'.length);
        if (entry?.version && !versions.has(name)) versions.set(name, entry.version);
      }
    } else if (lock.dependencies && typeof lock.dependencies === 'object') {
      for (const [name, entry] of Object.entries<{ version?: string }>(lock.dependencies)) {
        if (entry?.version) versions.set(name, entry.version);
      }
    }
  } catch {
    // Unparseable lockfile: every external ends up in unpinned_externals.
  }
  return versions;
}

export function captureStub(opts: CaptureStubOptions): CaptureStubResult {
  const repoRoot = path.resolve(opts.repoRoot);
  const packageName = `@carrick/${sanitizeServiceName(opts.serviceName)}`;
  const stubDir = path.resolve(opts.outDir);
  const errors: string[] = [];

  const configPath = opts.tsconfigPath
    ? path.resolve(opts.tsconfigPath)
    : path.join(repoRoot, 'tsconfig.json');
  if (!fs.existsSync(configPath)) {
    return fail(stubDir, packageName, [`tsconfig not found at ${configPath}`]);
  }

  const configHost: ts.ParseConfigFileHost = {
    ...ts.sys,
    onUnRecoverableConfigFileDiagnostic: (d) => {
      throw new Error(ts.flattenDiagnosticMessageText(d.messageText, '\n'));
    },
  };
  const parsed = ts.getParsedCommandLineOfConfigFile(configPath, {}, configHost);
  if (!parsed) {
    return fail(stubDir, packageName, [`failed to parse ${configPath}`]);
  }

  // The surface entry must live inside the effective rootDir (design doc
  // Capture step 1: an entry at repo root with rootDir "src" fails TS6059).
  const entryDir = parsed.options.rootDir
    ? path.resolve(path.dirname(configPath), parsed.options.rootDir)
    : repoRoot;
  const entryPath = path.join(entryDir, `${SURFACE_ENTRY_BASENAME}.ts`);

  const entryLines = ['// Generated by Carrick capture-v2 (spike). Deleted after emit.'];
  for (const anchor of opts.anchors) {
    const target = path.join(repoRoot, anchor.source_file).replace(/\.(ts|tsx|mts|cts)$/, '');
    let rel = path.relative(entryDir, target).split(path.sep).join('/');
    if (!rel.startsWith('.')) rel = `./${rel}`;
    entryLines.push(
      `export type ${anchor.alias} = import('${rel}').${anchor.symbol_name};`
    );
  }

  // Emit into a staging dir so output paths are unambiguous, then relocate
  // under <stub>/types/ with the entry renamed to surface.d.ts (same
  // directory level, so its relative import specifiers stay valid).
  const staging = fs.mkdtempSync(path.join(os.tmpdir(), 'carrick-capture-v2-'));
  const emitted = new Map<string, string>();

  try {
    fs.writeFileSync(entryPath, entryLines.join('\n') + '\n');
    const emitOptions: ts.CompilerOptions = {
      ...parsed.options,
      // The load-bearing trio: emit declarations without checking, so
      // type-error-laden and bare (no node_modules) checkouts still emit.
      noCheck: true,
      declaration: true,
      emitDeclarationOnly: true,
      noEmit: false,
      declarationMap: false,
      composite: false,
      incremental: false,
      outDir: staging,
      rootDir: entryDir,
    };
    const program = ts.createProgram([entryPath], emitOptions);
    const emitResult = program.emit(
      undefined,
      (fileName, text) => emitted.set(fileName, text),
      undefined,
      /* emitOnlyDtsFiles */ true
    );
    if (emitResult.emitSkipped) {
      return fail(stubDir, packageName, ['declaration emit was skipped']);
    }
    for (const d of emitResult.diagnostics) {
      errors.push(ts.flattenDiagnosticMessageText(d.messageText, '\n'));
    }
  } catch (err) {
    return fail(stubDir, packageName, [err instanceof Error ? err.message : String(err)]);
  } finally {
    if (fs.existsSync(entryPath)) fs.unlinkSync(entryPath);
    fs.rmSync(staging, { recursive: true, force: true });
  }

  // ---- Relocate the emitted tree into the stub package ----
  const typesDir = path.join(stubDir, 'types');
  fs.rmSync(stubDir, { recursive: true, force: true });
  fs.mkdirSync(typesDir, { recursive: true });

  const emittedFiles: string[] = [];
  const externalSpecs = new Set<string>();
  let surfaceAbsPath = '';

  for (const [fileName, text] of emitted) {
    let rel = path.relative(staging, fileName).split(path.sep).join('/');
    if (rel === `${SURFACE_ENTRY_BASENAME}.d.ts`) rel = 'surface.d.ts';
    const dest = path.join(typesDir, rel);
    fs.mkdirSync(path.dirname(dest), { recursive: true });
    fs.writeFileSync(dest, text);
    emittedFiles.push(`types/${rel}`);
    if (rel === 'surface.d.ts') surfaceAbsPath = dest;
    for (const spec of collectSpecifiers(text)) {
      if (!isRelative(spec) && !spec.startsWith('node:')) {
        externalSpecs.add(packageNameOf(spec));
      }
    }
  }
  if (!surfaceAbsPath) {
    return fail(stubDir, packageName, ['no surface.d.ts produced by emit']);
  }

  // ---- Pin external deps from the producer repo's lockfile ----
  const lockVersions = lockfileVersions(repoRoot);
  const pinned: Record<string, string> = {};
  const unpinned: string[] = [];
  for (const name of [...externalSpecs].sort()) {
    const version = lockVersions.get(name);
    if (version) pinned[name] = version;
    else unpinned.push(name);
  }

  fs.writeFileSync(
    path.join(stubDir, 'package.json'),
    JSON.stringify(
      {
        name: packageName,
        version: '0.0.0-carrick',
        private: true,
        types: './types/surface.d.ts',
        dependencies: pinned,
      },
      null,
      2
    ) + '\n'
  );
  fs.writeFileSync(
    path.join(stubDir, 'tsconfig.snapshot.json'),
    JSON.stringify(
      {
        ts_version: ts.version,
        strict: parsed.options.strict ?? false,
        strictNullChecks: parsed.options.strictNullChecks ?? parsed.options.strict ?? false,
        exactOptionalPropertyTypes: parsed.options.exactOptionalPropertyTypes ?? false,
        module: parsed.options.module !== undefined ? ts.ModuleKind[parsed.options.module] : undefined,
        target: parsed.options.target !== undefined ? ts.ScriptTarget[parsed.options.target] : undefined,
      },
      null,
      2
    ) + '\n'
  );

  // ---- Capture-time self-check (design doc Capture step 8, amended) ----
  const bareCheckout = !fs.existsSync(path.join(repoRoot, 'node_modules'));
  const aliases = selfCheck({
    stubDir,
    surfaceAbsPath,
    anchors: opts.anchors,
    pinned,
    bareCheckout,
  });

  fs.writeFileSync(
    path.join(stubDir, 'carrick-manifest.json'),
    JSON.stringify({ package_name: packageName, ts_version: ts.version, aliases }, null, 2) + '\n'
  );

  return {
    success: true,
    stub_dir: stubDir,
    package_name: packageName,
    emitted_files: emittedFiles.sort(),
    pinned_dependencies: pinned,
    unpinned_externals: unpinned,
    aliases,
    bare_checkout: bareCheckout,
    ts_version: ts.version,
    errors,
  };
}

/**
 * Standalone typecheck of the stub tree, classifying each alias:
 *
 *   ok                    resolved to a concrete type
 *   allowlisted_external  resolution failed only through external specifiers
 *                         that are pinned in the stub's dependencies (bare
 *                         checkout) — alias is KEPT at tier 'emitted'
 *   decayed_internal      a dangling internal specifier or a top-type
 *                         resolution not explained by a pinned external —
 *                         alias decays to Unknown at capture time
 *
 * Spike limitation: attribution is per emitted FILE, not per alias closure.
 * An alias is linked to an external failure if the file its surface entry
 * points at (or the surface itself) mentions a failed specifier.
 */
function selfCheck(args: {
  stubDir: string;
  surfaceAbsPath: string;
  anchors: CaptureAnchorRequest[];
  pinned: Record<string, string>;
  bareCheckout: boolean;
}): CaptureAliasRecord[] {
  const { stubDir, surfaceAbsPath, anchors, pinned, bareCheckout } = args;
  const typesDir = path.join(stubDir, 'types');
  const treeFiles: string[] = [];
  const walk = (dir: string) => {
    for (const entry of fs.readdirSync(dir, { withFileTypes: true })) {
      const p = path.join(dir, entry.name);
      if (entry.isDirectory()) walk(p);
      else if (entry.name.endsWith('.d.ts')) treeFiles.push(p);
    }
  };
  walk(typesDir);

  const program = ts.createProgram(treeFiles, {
    noEmit: true,
    strict: true,
    // MUST be false: the whole stub tree is .d.ts, and skipLibCheck skips
    // checking of declaration files entirely — with it on, unresolved
    // specifiers produce zero diagnostics and the self-check is vacuous.
    // (Same reason the design doc mandates skipLibCheck: false for stub
    // trees at check time.)
    skipLibCheck: false,
    module: ts.ModuleKind.ESNext,
    moduleResolution: ts.ModuleResolutionKind.Bundler,
    types: [],
  });
  const checker = program.getTypeChecker();
  const diagnostics = ts.getPreEmitDiagnostics(program);

  // Failed module specifiers, split external vs internal.
  const failedExternalPinned = new Set<string>();
  let internalFailure: string | undefined;
  for (const d of diagnostics) {
    if (d.code !== 2307 && d.code !== 2792) continue;
    const msg = ts.flattenDiagnosticMessageText(d.messageText, ' ');
    const m = /Cannot find module '([^']+)'/.exec(msg);
    if (!m) continue;
    const spec = m[1];
    if (isRelative(spec)) {
      internalFailure = internalFailure ?? spec;
    } else if (pinned[packageNameOf(spec)]) {
      failedExternalPinned.add(spec);
    } else {
      internalFailure = internalFailure ?? spec; // external but unpinned: treat as decay
    }
  }

  const surfaceSource = program.getSourceFile(surfaceAbsPath);
  const records: CaptureAliasRecord[] = [];

  for (const anchor of anchors) {
    let topType = true;
    let targetFileText = '';
    if (surfaceSource) {
      for (const stmt of surfaceSource.statements) {
        if (!ts.isTypeAliasDeclaration(stmt) || stmt.name.text !== anchor.alias) continue;
        const type = checker.getTypeAtLocation(stmt.name);
        topType = (type.flags & (ts.TypeFlags.Any | ts.TypeFlags.Unknown | ts.TypeFlags.Never)) !== 0;
        // Resolve the alias's import-type target file for coarse attribution.
        if (ts.isImportTypeNode(stmt.type) && ts.isLiteralTypeNode(stmt.type.argument)) {
          const lit = stmt.type.argument.literal;
          if (ts.isStringLiteral(lit)) {
            const base = path.resolve(path.dirname(surfaceAbsPath), lit.text);
            for (const cand of [`${base}.d.ts`, path.join(base, 'index.d.ts')]) {
              if (fs.existsSync(cand)) {
                targetFileText = fs.readFileSync(cand, 'utf8');
                break;
              }
            }
          }
        }
      }
    }

    let outcome: SelfCheckOutcome = 'ok';
    let detail: string | undefined;
    if (topType) {
      const closureText = targetFileText + fs.readFileSync(surfaceAbsPath, 'utf8');
      const blamedExternal = [...failedExternalPinned].find((spec) =>
        closureText.includes(`"${spec}"`) || closureText.includes(`'${spec}'`)
      );
      if (bareCheckout && blamedExternal && !internalFailure) {
        outcome = 'allowlisted_external';
        detail =
          `unresolved external '${blamedExternal}' is pinned ` +
          `(${packageNameOf(blamedExternal)}@${pinned[packageNameOf(blamedExternal)]}); ` +
          'kept at tier emitted on bare checkout; probe gates are the backstop';
      } else {
        outcome = 'decayed_internal';
        detail = internalFailure
          ? `dangling internal specifier '${internalFailure}'`
          : 'alias resolved to a top type with no allowlisted external failure';
      }
    }

    records.push({
      alias: anchor.alias,
      symbol_name: anchor.symbol_name,
      source_file: anchor.source_file,
      anchor_origin: anchor.anchor_origin,
      serialization: 'emitted',
      self_check: outcome,
      self_check_detail: detail,
      top_type_at_self_check: topType,
    });
  }

  return records;
}
