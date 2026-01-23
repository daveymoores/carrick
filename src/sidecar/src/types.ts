/**
 * Type definitions for the sidecar message protocol
 * These types define the JSON messages exchanged between Rust and the Node.js sidecar
 */

// ============================================================================
// Inference Kind Enum
// ============================================================================

/**
 * The kind of type inference to perform
 */
export type InferKind =
  | 'function_return'   // Get return type of a function
  | 'expression'        // Get type of an expression
  | 'call_result'       // Get return type of a call expression
  | 'variable'          // Get type of a variable declaration
  | 'response_body'     // Find response body (.json()/.send()/ctx.body)
  | 'request_body';     // Find request body (req.body/ctx.request.body or call payloads)

// ============================================================================
// Extraction Config Types (Agent-Informed Payload Unwrapping)
// ============================================================================

/**
 * A rule for unwrapping machinery/wrapper types to extract payload types.
 *
 * The unwrapping logic follows these priorities:
 * 1. Exact wrapperSymbols match wins immediately
 * 2. machineryIndicators only trigger unwrap if originModuleGlobs also match
 * 3. Payload extraction: prefer generic args, then property paths
 */
export interface ExtractionRule {
  /**
   * Exact wrapper type/symbol names to unwrap.
   * Examples: ["Response", "AxiosResponse", "Promise", "Observable"]
   */
  wrapperSymbols?: string[];

  /**
   * Method/property indicators that suggest a wrapper type.
   * Examples: ["status", "json", "send", "header", "cookie"]
   * Note: Only used in conjunction with originModuleGlobs to avoid false positives.
   */
  machineryIndicators?: string[];

  /**
   * Glob patterns for module origins. Only unwrap if the symbol's declarations
   * come from modules matching these patterns.
   * Examples: ["express", "express/*", "@types/express/*", "axios", "axios/*"]
   */
  originModuleGlobs?: string[];

  /**
   * Index of the generic type argument containing the payload.
   * Defaults to 0 (first type arg).
   * Examples:
   *   - Response<T> → index 0
   *   - Map<K, V> → index 1 for values
   */
  payloadGenericIndex?: number;

  /**
   * Property path to extract payload when generics aren't available.
   * Examples: ["data"] for AxiosResponse.data, ["body"] for Response.body
   */
  payloadPropertyPath?: string[];

  /**
   * Whether to recursively unwrap nested wrappers.
   * Example: Promise<Response<T>> → unwrap both layers to get T
   */
  unwrapRecursively?: boolean;

  /**
   * Maximum unwrap depth when unwrapRecursively is true.
   * Defaults to 4 to prevent infinite loops.
   */
  maxDepth?: number;
}

/**
 * Configuration for extracting payload types from machinery wrappers.
 * Provided by the main Carrick process based on agent analysis.
 */
export interface ExtractionConfig {
  rules: ExtractionRule[];
}

// ============================================================================
// Pinned Dependency Snapshot Types
// ============================================================================

/**
 * A map of package names to exact pinned versions.
 * Used to ensure deterministic typechecking across CI runs.
 */
export interface PinnedDependencySnapshot {
  [packageName: string]: string;
}

// ============================================================================
// Tsconfig Snapshot Types
// ============================================================================

/**
 * A normalized/closed tsconfig object where all `extends` chains have been resolved.
 * Contains only the compiler options needed for surface checking.
 */
export interface TsconfigSnapshot {
  compilerOptions: {
    module?: string;
    moduleResolution?: string;
    target?: string;
    lib?: string[];
    types?: string[];
    typeRoots?: string[];
    jsx?: string;
    strict?: boolean;
    esModuleInterop?: boolean;
    skipLibCheck?: boolean;
    declaration?: boolean;
    declarationMap?: boolean;
    paths?: Record<string, string[]>;
    baseUrl?: string;
    [key: string]: unknown;
  };
}

// ============================================================================
// Repo Metadata Types
// ============================================================================

/**
 * Metadata for a single repository in the synthetic monorepo.
 */
export interface RepoMetadata {
  /** Unique name for this repo (used in @carrick/{repoName}/...) */
  repoName: string;

  /** Pinned dependency versions for this repo */
  dependencies: PinnedDependencySnapshot;

  /** Closed tsconfig snapshot for this repo */
  tsconfig: TsconfigSnapshot;

  /** Extraction config for unwrapping machinery types */
  extractionConfig?: ExtractionConfig;

  /** The emitted surface .d.ts content (after Task 2) */
  surfaceContent?: string;
}

// ============================================================================
// Legacy Wrapper Registry Types (kept for backwards compat during migration)
// ============================================================================

export type WrapperUnwrapKind = 'property' | 'generic_param';

export interface WrapperUnwrapRule {
  kind: WrapperUnwrapKind;
  property?: string;
  index?: number;
}

export interface WrapperRule {
  package: string;
  type_name: string;
  unwrap: WrapperUnwrapRule;
}

// ============================================================================
// Request Types
// ============================================================================

/**
 * Base fields present in all requests
 */
interface BaseRequest {
  request_id: string;
}

/**
 * Initialize the sidecar with a repository root
 */
export interface InitRequest extends BaseRequest {
  action: 'init';
  repo_root: string;
  tsconfig_path?: string;
  /** Optional tsconfig snapshot (closed/merged) - preferred over tsconfig_path */
  tsconfig_snapshot?: TsconfigSnapshot;
  /** Optional pinned dependencies for this repo */
  pinned_dependencies?: PinnedDependencySnapshot;
}

/**
 * Request to bundle explicit types from source files
 * @deprecated Use emit_surface instead for the new architecture
 */
export interface BundleRequest extends BaseRequest {
  action: 'bundle';
  symbols: SymbolRequest[];
}

/**
 * Request to emit a surface .d.ts file with rewritten module specifiers
 */
export interface EmitSurfaceRequest extends BaseRequest {
  action: 'emit_surface';
  /** The repo name for specifier rewriting (@carrick/{repoName}/...) */
  repo_name: string;
  /** Payload types to include in the surface */
  payloads: PayloadDefinition[];
  /** Output path for the surface .d.ts file */
  output_path: string;
}

/**
 * Definition of a payload type to emit
 */
export interface PayloadDefinition {
  /** Alias/name for this payload in the surface */
  alias: string;
  /** The type string (already unwrapped from machinery) */
  type_string: string;
  /** Optional source information */
  source_file?: string;
  source_location?: SourceLocation;
}

/**
 * Request to infer implicit types at specific locations
 */
export interface InferRequest extends BaseRequest {
  action: 'infer';
  requests: InferRequestItem[];
  wrappers?: WrapperRule[];
  /** New extraction config (preferred over wrappers) */
  extraction_config?: ExtractionConfig;
}

/**
 * Request to build the synthetic monorepo workspace
 */
export interface BuildWorkspaceRequest extends BaseRequest {
  action: 'build_workspace';
  repos: RepoMetadata[];
  /** Root directory for the workspace (defaults to .carrick/workspace) */
  workspace_root?: string;
}

/**
 * Request to run type compatibility checks
 */
export interface CheckCompatibilityRequest extends BaseRequest {
  action: 'check_compatibility';
  /** Path to the workspace root */
  workspace_root: string;
  /** Pairs of types to check for compatibility */
  checks: CompatibilityCheck[];
}

/**
 * A single compatibility check between two types
 */
export interface CompatibilityCheck {
  /** Source repo name */
  source_repo: string;
  /** Source payload alias */
  source_alias: string;
  /** Target repo name */
  target_repo: string;
  /** Target payload alias */
  target_alias: string;
  /** Direction: 'source_extends_target' or 'target_extends_source' or 'bidirectional' */
  direction: 'source_extends_target' | 'target_extends_source' | 'bidirectional';
}

/**
 * Health check request
 */
export interface HealthRequest extends BaseRequest {
  action: 'health';
}

/**
 * Shutdown the sidecar process
 */
export interface ShutdownRequest extends BaseRequest {
  action: 'shutdown';
}

/**
 * Union type for all possible sidecar requests
 */
export type SidecarRequest =
  | InitRequest
  | BundleRequest
  | EmitSurfaceRequest
  | InferRequest
  | BuildWorkspaceRequest
  | CheckCompatibilityRequest
  | HealthRequest
  | ShutdownRequest;

/**
 * Request for a specific symbol to be bundled
 */
export interface SymbolRequest {
  /** The name of the symbol (type, interface, class, etc.) */
  symbol_name: string;
  /** The source file path (relative to repo root) */
  source_file: string;
  /** Optional alias for the exported type */
  alias?: string;
}

/**
 * Request for type inference at a specific location
 */
export interface InferRequestItem {
  /** Path to the file (relative to repo root) */
  file_path: string;
  /** Line number (1-based) for context and alias generation */
  line_number: number;
  /** Start byte offset of the target expression */
  span_start: number;
  /** End byte offset of the target expression */
  span_end: number;
  /** The kind of inference to perform */
  infer_kind: InferKind;
  /** Optional alias for the inferred type */
  alias?: string;
}

// ============================================================================
// Response Types
// ============================================================================

/**
 * Response status
 */
export type ResponseStatus = 'success' | 'error' | 'ready' | 'not_ready';

/**
 * Base response fields
 */
interface BaseResponse {
  request_id: string;
  status: ResponseStatus;
}

/**
 * Response for init action
 */
export interface InitResponse extends BaseResponse {
  status: 'ready' | 'error';
  init_time_ms?: number;
  errors?: string[];
}

/**
 * Response for bundle action
 * @deprecated Use EmitSurfaceResponse instead
 */
export interface BundleResponse extends BaseResponse {
  /** The bundled .d.ts content */
  dts_content?: string;
  /** Manifest mapping aliases to their type strings */
  manifest?: ManifestEntry[];
  /** Individual symbol failures */
  symbol_failures?: SymbolFailure[];
  /** General errors */
  errors?: string[];
}

/**
 * Response for emit_surface action
 */
export interface EmitSurfaceResponse extends BaseResponse {
  /** Path to the emitted surface file */
  output_path?: string;
  /** The emitted .d.ts content */
  surface_content?: string;
  /** Manifest of emitted payloads */
  manifest?: SurfaceManifestEntry[];
  /** Errors during emission */
  errors?: string[];
}

/**
 * Entry in the surface manifest
 */
export interface SurfaceManifestEntry {
  alias: string;
  type_string: string;
  rewritten_imports: string[];
}

/**
 * Response for infer action
 */
export interface InferResponse extends BaseResponse {
  /** Successfully inferred types */
  inferred_types?: InferredType[];
  /** General errors */
  errors?: string[];
}

/**
 * Response for build_workspace action
 */
export interface BuildWorkspaceResponse extends BaseResponse {
  /** Path to the created workspace */
  workspace_path?: string;
  /** Paths to generated stub packages */
  stub_packages?: string[];
  /** Path to the checker package */
  checker_path?: string;
  /** Errors during workspace creation */
  errors?: string[];
}

/**
 * Response for check_compatibility action
 */
export interface CheckCompatibilityResponse extends BaseResponse {
  /** Results of each compatibility check */
  results?: CompatibilityResult[];
  /** TypeScript compiler diagnostics */
  diagnostics?: string[];
  /** Errors during checking */
  errors?: string[];
}

/**
 * Result of a single compatibility check
 */
export interface CompatibilityResult {
  source_repo: string;
  source_alias: string;
  target_repo: string;
  target_alias: string;
  compatible: boolean;
  /** Diagnostic message if not compatible */
  diagnostic?: string;
}

/**
 * Response for health action
 */
export interface HealthResponse extends BaseResponse {
  status: 'ready' | 'not_ready';
  init_time_ms?: number;
}

/**
 * Response for shutdown action
 */
export interface ShutdownResponse extends BaseResponse {
  status: 'success';
}

/**
 * Error response
 */
export interface ErrorResponse extends BaseResponse {
  status: 'error';
  errors: string[];
}

/**
 * Union type for all possible sidecar responses
 */
export type SidecarResponse =
  | InitResponse
  | BundleResponse
  | EmitSurfaceResponse
  | InferResponse
  | BuildWorkspaceResponse
  | CheckCompatibilityResponse
  | HealthResponse
  | ShutdownResponse
  | ErrorResponse;

/**
 * An entry in the type manifest
 */
export interface ManifestEntry {
  /** The alias or original name of the type */
  alias: string;
  /** The original symbol name */
  original_name: string;
  /** The source file where the type was found */
  source_file: string;
  /** The full type definition string */
  type_string: string;
  /** Whether this was an explicit annotation or inferred */
  is_explicit: boolean;
}

/**
 * An inferred type result
 */
export interface InferredType {
  /** The alias for this type (generated if not provided) */
  alias: string;
  /** The full TypeScript type string */
  type_string: string;
  /** Whether the type was explicitly annotated in source */
  is_explicit: boolean;
  /** Source location information */
  source_location: SourceLocation;
  /** The kind of inference that was performed */
  infer_kind: InferKind;
  /** The unwrapped/extracted payload type (if different from type_string) */
  payload_type_string?: string;
}

/**
 * Source location information for a type
 */
export interface SourceLocation {
  /** File path relative to repo root */
  file_path: string;
  /** Start line (1-based) */
  start_line: number;
  /** End line (1-based) */
  end_line: number;
  /** Start column (0-based) */
  start_column?: number;
  /** End column (0-based) */
  end_column?: number;
}

/**
 * Information about a symbol that failed to resolve
 */
export interface SymbolFailure {
  /** The symbol that failed */
  symbol_name: string;
  /** The source file where it was supposed to be */
  source_file: string;
  /** Reason for the failure */
  reason: string;
}

// ============================================================================
// Bundle Result (internal)
// ============================================================================

/**
 * Internal result from the bundler
 * @deprecated Use SurfaceEmitResult instead
 */
export interface BundleResult {
  /** Whether bundling was successful */
  success: boolean;
  /** The bundled .d.ts content */
  dts_content?: string;
  /** Manifest entries for successfully bundled types */
  manifest?: ManifestEntry[];
  /** Failures for individual symbols */
  symbol_failures?: SymbolFailure[];
  /** General error messages */
  errors?: string[];
}

/**
 * Internal result from surface emission
 */
export interface SurfaceEmitResult {
  /** Whether emission was successful */
  success: boolean;
  /** The emitted .d.ts content */
  surface_content?: string;
  /** Output path where content was written */
  output_path?: string;
  /** Manifest of emitted payloads */
  manifest?: SurfaceManifestEntry[];
  /** General error messages */
  errors?: string[];
}

/**
 * Internal result from the type inferrer
 */
export interface InferResult {
  /** Whether inference was successful */
  success: boolean;
  /** Successfully inferred types */
  inferred_types?: InferredType[];
  /** General error messages */
  errors?: string[];
}

/**
 * Result from building the synthetic workspace
 */
export interface WorkspaceBuildResult {
  /** Whether build was successful */
  success: boolean;
  /** Path to the workspace root */
  workspace_path?: string;
  /** Paths to stub packages */
  stub_packages?: string[];
  /** Path to the checker package */
  checker_path?: string;
  /** Error messages */
  errors?: string[];
}

/**
 * Result from running compatibility checks
 */
export interface CompatibilityCheckResult {
  /** Whether checks ran successfully (not whether types are compatible) */
  success: boolean;
  /** Individual check results */
  results?: CompatibilityResult[];
  /** TypeScript diagnostics */
  diagnostics?: string[];
  /** Error messages */
  errors?: string[];
}
