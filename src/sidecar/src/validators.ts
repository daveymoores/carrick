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

export const InferRequestItemSchema = z.object({
  file_path: z.string().min(1, 'File path cannot be empty'),
  line_number: z.number().int().positive('Line number must be positive'),
  infer_kind: InferKindSchema,
  alias: z.string().optional(),
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
});

export const BundleRequestSchema = BaseRequestSchema.extend({
  action: z.literal('bundle'),
  symbols: z.array(SymbolRequestSchema).min(1, 'At least one symbol is required'),
});

export const InferRequestSchema = BaseRequestSchema.extend({
  action: z.literal('infer'),
  requests: z.array(InferRequestItemSchema).min(1, 'At least one infer request is required'),
});

export const HealthRequestSchema = BaseRequestSchema.extend({
  action: z.literal('health'),
});

export const ShutdownRequestSchema = BaseRequestSchema.extend({
  action: z.literal('shutdown'),
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
  InferRequestSchema,
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
export type ValidatedInferRequest = z.infer<typeof InferRequestSchema>;
export type ValidatedHealthRequest = z.infer<typeof HealthRequestSchema>;
export type ValidatedShutdownRequest = z.infer<typeof ShutdownRequestSchema>;
export type ValidatedSidecarRequest = z.infer<typeof SidecarRequestSchema>;
