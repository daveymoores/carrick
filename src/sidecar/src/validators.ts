/**
 * Zod validators for the sidecar message protocol
 * These validators ensure incoming JSON messages are correctly typed
 */

import { z } from 'zod';
import type {
  SidecarRequest,
  InferKind,
  SymbolRequest,
  InferRequestItem,
} from './types.js';

// ============================================================================
// Inference Kind Schema
// ============================================================================

export const InferKindSchema = z.enum([
  'function_return',
  'expression',
  'call_result',
  'variable',
  'response_body',
  'request_body',
]);

// ============================================================================
// Extraction Config Schemas (New)
// ============================================================================

const ExtractionRuleSchema = z.object({
  wrapperSymbols: z.array(z.string()).optional(),
  machineryIndicators: z.array(z.string()).optional(),
  originModuleGlobs: z.array(z.string()).optional(),
  payloadGenericIndex: z.number().int().nonnegative().optional(),
  payloadPropertyPath: z.array(z.string()).optional(),
  unwrapRecursively: z.boolean().optional(),
  maxDepth: z.number().int().positive().optional(),
});

const ExtractionConfigSchema = z.object({
  rules: z.array(ExtractionRuleSchema),
});

// ============================================================================
// Tsconfig Snapshot Schema
// ============================================================================

const TsconfigSnapshotSchema = z.object({
  compilerOptions: z.object({
    module: z.string().optional(),
    moduleResolution: z.string().optional(),
    target: z.string().optional(),
    lib: z.array(z.string()).optional(),
    types: z.array(z.string()).optional(),
    typeRoots: z.array(z.string()).optional(),
    jsx: z.string().optional(),
    strict: z.boolean().optional(),
    esModuleInterop: z.boolean().optional(),
    skipLibCheck: z.boolean().optional(),
    declaration: z.boolean().optional(),
    declarationMap: z.boolean().optional(),
    paths: z.record(z.array(z.string())).optional(),
    baseUrl: z.string().optional(),
  }).passthrough(),
});

// ============================================================================
// Pinned Dependency Snapshot Schema
// ============================================================================

const PinnedDependencySnapshotSchema = z.record(z.string());

// ============================================================================
// Repo Metadata Schema
// ============================================================================

const RepoMetadataSchema = z.object({
  repoName: z.string().min(1, 'Repo name cannot be empty'),
  dependencies: PinnedDependencySnapshotSchema,
  tsconfig: TsconfigSnapshotSchema,
  extractionConfig: ExtractionConfigSchema.optional(),
  surfaceContent: z.string().optional(),
});

// ============================================================================
// Wrapper Registry Schemas (Legacy)
// ============================================================================

const WrapperUnwrapKindSchema = z.enum(['property', 'generic_param']);

const WrapperUnwrapRuleSchema = z
  .object({
    kind: WrapperUnwrapKindSchema,
    property: z.string().min(1, 'Property must be non-empty').optional(),
    index: z.number().int().nonnegative('Index must be non-negative').optional(),
  })
  .superRefine((value, ctx) => {
    if (value.kind === 'property' && !value.property) {
      ctx.addIssue({
        code: z.ZodIssueCode.custom,
        message: 'property is required for property unwrap rules',
        path: ['property'],
      });
    }
    if (value.kind === 'generic_param' && value.index === undefined) {
      ctx.addIssue({
        code: z.ZodIssueCode.custom,
        message: 'index is required for generic_param unwrap rules',
        path: ['index'],
      });
    }
  });

const WrapperRuleSchema = z.object({
  package: z.string().min(1, 'Package name cannot be empty'),
  type_name: z.string().min(1, 'Type name cannot be empty'),
  unwrap: WrapperUnwrapRuleSchema,
});

// ============================================================================
// Symbol Request Schema
// ============================================================================

export const SymbolRequestSchema = z.object({
  symbol_name: z.string().min(1, 'Symbol name cannot be empty'),
  source_file: z.string().min(1, 'Source file cannot be empty'),
  alias: z.string().optional(),
});

// ============================================================================
// Infer Request Item Schema
// ============================================================================

export const InferRequestItemSchema = z
  .object({
    file_path: z.string().min(1, 'File path cannot be empty'),
    line_number: z.number().int().positive('Line number must be positive'),
    span_start: z.number().int().nonnegative('Span start must be non-negative').optional(),
    span_end: z.number().int().nonnegative('Span end must be non-negative').optional(),
    expression_text: z.string().optional(),
    expression_line: z.number().int().positive('Expression line must be positive').optional(),
    infer_kind: InferKindSchema,
    alias: z.string().optional(),
  })
  .superRefine((value, ctx) => {
    if (
      value.span_start !== undefined &&
      value.span_end !== undefined &&
      value.span_end < value.span_start
    ) {
      ctx.addIssue({
        code: z.ZodIssueCode.custom,
        message: 'span_end must be greater than or equal to span_start',
        path: ['span_end'],
      });
    }
    const hasSpan = value.span_start !== undefined && value.span_end !== undefined;
    const hasText = value.expression_text !== undefined;
    if (!hasSpan && !hasText) {
      ctx.addIssue({
        code: z.ZodIssueCode.custom,
        message: 'At least one of (span_start + span_end) or expression_text must be provided',
        path: [],
      });
    }
  });

// ============================================================================
// Payload Definition Schema (New)
// ============================================================================

const SourceLocationSchema = z.object({
  file_path: z.string(),
  start_line: z.number(),
  end_line: z.number(),
  start_column: z.number().optional(),
  end_column: z.number().optional(),
});

const PayloadDefinitionSchema = z.object({
  alias: z.string().min(1, 'Alias cannot be empty'),
  type_string: z.string().min(1, 'Type string cannot be empty'),
  source_file: z.string().optional(),
  source_location: SourceLocationSchema.optional(),
});

// ============================================================================
// Compatibility Check Schema (New)
// ============================================================================

const CompatibilityCheckSchema = z.object({
  source_repo: z.string().min(1, 'Source repo cannot be empty'),
  source_alias: z.string().min(1, 'Source alias cannot be empty'),
  target_repo: z.string().min(1, 'Target repo cannot be empty'),
  target_alias: z.string().min(1, 'Target alias cannot be empty'),
  direction: z.enum(['source_extends_target', 'target_extends_source', 'bidirectional']),
});

// ============================================================================
// Action-specific Request Schemas
// ============================================================================

const BaseRequestSchema = z.object({
  request_id: z.string().min(1, 'Request ID cannot be empty'),
});

export const InitRequestSchema = BaseRequestSchema.extend({
  action: z.literal('init'),
  repo_root: z.string().min(1, 'Repo root cannot be empty'),
  tsconfig_path: z.string().optional(),
  tsconfig_snapshot: TsconfigSnapshotSchema.optional(),
  pinned_dependencies: PinnedDependencySnapshotSchema.optional(),
});

export const BundleRequestSchema = BaseRequestSchema.extend({
  action: z.literal('bundle'),
  symbols: z.array(SymbolRequestSchema).min(1, 'At least one symbol is required'),
});

export const EmitSurfaceRequestSchema = BaseRequestSchema.extend({
  action: z.literal('emit_surface'),
  repo_name: z.string().min(1, 'Repo name cannot be empty'),
  payloads: z.array(PayloadDefinitionSchema).min(1, 'At least one payload is required'),
  output_path: z.string().min(1, 'Output path cannot be empty'),
});

export const InferRequestSchema = BaseRequestSchema.extend({
  action: z.literal('infer'),
  requests: z.array(InferRequestItemSchema).min(1, 'At least one infer request is required'),
  wrappers: z.array(WrapperRuleSchema).optional(),
  extraction_config: ExtractionConfigSchema.optional(),
});

export const BuildWorkspaceRequestSchema = BaseRequestSchema.extend({
  action: z.literal('build_workspace'),
  repos: z.array(RepoMetadataSchema).min(1, 'At least one repo is required'),
  workspace_root: z.string().optional(),
});

export const CheckCompatibilityRequestSchema = BaseRequestSchema.extend({
  action: z.literal('check_compatibility'),
  workspace_root: z.string().min(1, 'Workspace root cannot be empty'),
  checks: z.array(CompatibilityCheckSchema).min(1, 'At least one check is required'),
});

export const HealthRequestSchema = BaseRequestSchema.extend({
  action: z.literal('health'),
});

export const ShutdownRequestSchema = BaseRequestSchema.extend({
  action: z.literal('shutdown'),
});

export const ResolveDefinitionsRequestSchema = BaseRequestSchema.extend({
  action: z.literal('resolve_definitions'),
  bundled_dts: z.string().min(1, 'Bundled .d.ts content cannot be empty'),
  aliases: z.array(z.string().min(1)).min(1, 'At least one alias is required'),
});

// ============================================================================
// Discriminated Union Schema
// ============================================================================

/**
 * The main sidecar request schema - a discriminated union on the 'action' field
 */
export const SidecarRequestSchema = z.discriminatedUnion('action', [
  InitRequestSchema,
  BundleRequestSchema,
  EmitSurfaceRequestSchema,
  InferRequestSchema,
  BuildWorkspaceRequestSchema,
  CheckCompatibilityRequestSchema,
  ResolveDefinitionsRequestSchema,
  HealthRequestSchema,
  ShutdownRequestSchema,
]);

// ============================================================================
// Parse Function
// ============================================================================

/**
 * Result type for parseRequest
 */
export type ParseResult =
  | { success: true; request: SidecarRequest }
  | { success: false; error: string };

/**
 * Validates and parses incoming JSON into a typed SidecarRequest
 *
 * @param json - The raw JSON value to parse
 * @returns ParseResult with either the typed request or an error message
 */
export function parseRequest(json: unknown): ParseResult {
  const result = SidecarRequestSchema.safeParse(json);

  if (result.success) {
    return {
      success: true,
      request: result.data as SidecarRequest,
    };
  }

  // Format Zod errors nicely
  const errorMessages = result.error.issues.map((issue) => {
    const path = issue.path.join('.');
    return path ? `${path}: ${issue.message}` : issue.message;
  });

  return {
    success: false,
    error: `Invalid request: ${errorMessages.join('; ')}`,
  };
}

/**
 * Validates and parses incoming JSON, throwing on error
 *
 * @param json - The raw JSON value to parse
 * @returns The typed SidecarRequest
 * @throws Error if validation fails
 */
export function parseRequestOrThrow(json: unknown): SidecarRequest {
  const result = parseRequest(json);
  if (!result.success) {
    throw new Error(result.error);
  }
  return result.request;
}

// ============================================================================
// Type exports (inferred from schemas)
// ============================================================================

export type ValidatedInitRequest = z.infer<typeof InitRequestSchema>;
export type ValidatedBundleRequest = z.infer<typeof BundleRequestSchema>;
export type ValidatedEmitSurfaceRequest = z.infer<typeof EmitSurfaceRequestSchema>;
export type ValidatedInferRequest = z.infer<typeof InferRequestSchema>;
export type ValidatedBuildWorkspaceRequest = z.infer<typeof BuildWorkspaceRequestSchema>;
export type ValidatedCheckCompatibilityRequest = z.infer<typeof CheckCompatibilityRequestSchema>;
export type ValidatedHealthRequest = z.infer<typeof HealthRequestSchema>;
export type ValidatedShutdownRequest = z.infer<typeof ShutdownRequestSchema>;
export type ValidatedSidecarRequest = z.infer<typeof SidecarRequestSchema>;
