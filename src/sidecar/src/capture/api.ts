/**
 * Wire contract for the v2 capture bundle ("tsc as the serializer").
 *
 * This file IS the seam (design doc: "seam, not split"): everything outside
 * src/sidecar/src/capture/ may import types from this file and the
 * `captureStub` entry point from ./index.js, and nothing else. Modules inside
 * capture/ import only node builtins, `typescript`, and each other. The
 * stdio `capture_v2` action is the only surface the Rust client sees.
 */

/**
 * How the anchor was produced upstream (design-doc amendment 2). Recorded so
 * the fidelity metric separates anchor-recall loss from serialization loss.
 * (Named anchor_origin because `provenance` is taken by the op-level
 * producer-provenance fields in src/eval_output.rs.)
 */
export type AnchorOrigin = 'llm-symbol' | 'deterministic-infer' | 'anchor-backfill';

/**
 * Serialization tier of a captured alias (design doc, Capture step 5):
 *  - emitted: compiler declaration emit of an addressable symbol (best)
 *  - node_builder: SymbolTracker-verified node-builder print of an anonymous
 *    inferred type
 *  - structural_fallback: the capture-native paths failed (guard failure,
 *    locator failure, inaccessible symbol during node-builder printing).
 *    In WP1 this tier emits `unknown` with a recorded reason -- visibly
 *    degraded, never a silently wrong .d.ts. Wiring the legacy structural
 *    expander text under this tier is a WP3 integration decision.
 */
export type SerializationTier = 'emitted' | 'node_builder' | 'structural_fallback';

/** Explicit exported symbol: `export type A = import('./m').Sym;` */
export interface SymbolAnchorRequest {
  kind: 'symbol';
  /** Manifest alias, e.g. Endpoint_abc123_Response */
  alias: string;
  /** Exported symbol name in the producer repo */
  symbol_name: string;
  /** Declaring module, repo-root-relative, e.g. src/types/stock.ts */
  source_file: string;
  anchor_origin: AnchorOrigin;
}

/**
 * Addressable handler: `export type A = Awaited<ReturnType<typeof
 * import('./m').fn>>;` -- guarded (design doc, Capture step 1): the symbol
 * must be exported, must not be an overload set (ReturnType silently resolves
 * the last overload), and must not be generic (type params erase). Guard
 * failures demote to structural_fallback with the reason recorded.
 */
export interface HandlerReturnAnchorRequest {
  kind: 'handler_return';
  alias: string;
  symbol_name: string;
  source_file: string;
  anchor_origin: AnchorOrigin;
}

/**
 * Anonymous inferred type at a source location (no addressable symbol). The
 * node is located by byte span when given, else by expression text on/after
 * a line, else by line. Its type is printed into the surface entry via the
 * compiler node builder with a real SymbolTracker (see node-builder.ts).
 */
export interface InferAnchorRequest {
  kind: 'infer';
  alias: string;
  source_file: string;
  anchor_origin: AnchorOrigin;
  /** Byte span of the target node (TS source positions). */
  span_start?: number;
  span_end?: number;
  /** 1-based line the target starts on (locator fallback + disambiguation). */
  line_number?: number;
  /** Exact source text of the target expression (locator fallback). */
  expression_text?: string;
  /**
   * Transport unwrapping applied to the located type before printing
   * (design doc, Capture step 6: machinery unwrapping stays at capture time).
   * Default 'awaited': Promise / thenable layers are unwrapped.
   */
  unwrap?: 'awaited' | 'none';
}

export type CaptureAnchorRequest =
  | SymbolAnchorRequest
  | HandlerReturnAnchorRequest
  | InferAnchorRequest;

export type SelfCheckOutcome = 'ok' | 'allowlisted_external' | 'decayed_internal';

export interface CaptureAliasRecord {
  alias: string;
  anchor_kind: CaptureAnchorRequest['kind'];
  symbol_name?: string;
  source_file: string;
  anchor_origin: AnchorOrigin;
  serialization: SerializationTier;
  self_check: SelfCheckOutcome;
  /** Human-readable reason when self_check is not 'ok'. */
  self_check_detail?: string;
  /**
   * Recorded when the alias never reached a capture-native tier (guard
   * failure, locator failure, inaccessible symbols during node-builder
   * printing). Always present when serialization === 'structural_fallback'.
   */
  capture_failure_reason?: string;
  /** True when the alias resolved to any/unknown/never during self-check.
   * With self_check === 'allowlisted_external' this is expected on a bare
   * checkout and is NOT a decay; the probe gates own the final verdict. */
  top_type_at_self_check: boolean;
}

/** Aggregate fidelity metric, emitted per capture (one service). */
export interface CaptureFidelity {
  total_aliases: number;
  by_serialization: Record<SerializationTier, number>;
  by_self_check: Record<SelfCheckOutcome, number>;
  by_anchor_origin: Record<AnchorOrigin, number>;
  /** Aliases whose capture is usable at check time (self_check ok or
   * allowlisted_external) over total. */
  usable_rate: number;
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
  fidelity: CaptureFidelity;
  /** Tree-relative paths of files included because they declare global or
   * module augmentations (design doc, Capture step 4). */
  augmentation_files: string[];
  /** Number of emitted specifiers rewritten by the post-emit pass
   * (tsconfig-paths mappings and absolute internal import types). */
  specifier_rewrites: number;
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
