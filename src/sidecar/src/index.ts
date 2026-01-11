/**
 * Main entry point for the type-sidecar
 *
 * This module implements a message loop that:
 * 1. Listens on stdin for JSON requests
 * 2. Processes each request (init, bundle, infer, health, shutdown)
 * 3. Writes JSON responses to stdout
 *
 * IMPORTANT:
 * - stdout is ONLY for JSON responses
 * - stderr is for logging
 * - Process stays alive between requests (warm standby)
 */

import * as readline from 'node:readline';
import { parseRequest } from './validators.js';
import { ProjectLoader } from './project-loader.js';
import { TypeBundler } from './bundler.js';
import { TypeInferrer } from './type-inferrer.js';
import type {
  SidecarRequest,
  SidecarResponse,
  InitResponse,
  BundleResponse,
  InferResponse,
  HealthResponse,
  ShutdownResponse,
  ErrorResponse,
} from './types.js';

// ===========================================================================
// Module-level state
// ===========================================================================

let projectLoader: ProjectLoader | null = null;
let typeBundler: TypeBundler | null = null;
let typeInferrer: TypeInferrer | null = null;
let initTimeMs: number | null = null;

// ===========================================================================
// Request Handlers
// ===========================================================================

/**
 * Handle the 'init' action - initialize the TypeScript project
 */
function handleInit(request: SidecarRequest & { action: 'init' }): InitResponse {
  const startTime = performance.now();

  try {
    log(`Initializing with repo_root: ${request.repo_root}`);

    projectLoader = new ProjectLoader({
      repoRoot: request.repo_root,
      tsconfigPath: request.tsconfig_path,
    });

    const result = projectLoader.load();

    if (!result.success) {
      return {
        request_id: request.request_id,
        status: 'error',
        errors: [result.error || 'Unknown initialization error'],
        init_time_ms: result.initTimeMs,
      };
    }

    // Initialize bundler and inferrer
    const project = projectLoader.getProject();
    typeBundler = new TypeBundler({
      project,
      repoRoot: projectLoader.getRepoRoot(),
    });
    typeInferrer = new TypeInferrer({ project });

    initTimeMs = result.initTimeMs || Math.round(performance.now() - startTime);

    log(`Initialization complete in ${initTimeMs}ms`);

    return {
      request_id: request.request_id,
      status: 'ready',
      init_time_ms: initTimeMs,
    };
  } catch (err) {
    const error = err instanceof Error ? err.message : String(err);
    logError(`Initialization failed: ${error}`);

    return {
      request_id: request.request_id,
      status: 'error',
      errors: [error],
      init_time_ms: Math.round(performance.now() - startTime),
    };
  }
}

/**
 * Handle the 'bundle' action - bundle explicit types
 */
function handleBundle(request: SidecarRequest & { action: 'bundle' }): BundleResponse {
  if (!projectLoader?.isInitialized() || !typeBundler) {
    return {
      request_id: request.request_id,
      status: 'error',
      errors: ['Sidecar not initialized. Call init first.'],
    };
  }

  try {
    log(`Bundling ${request.symbols.length} symbol(s)`);

    const result = typeBundler.bundle(request.symbols);

    if (!result.success) {
      return {
        request_id: request.request_id,
        status: 'error',
        dts_content: result.dts_content,
        manifest: result.manifest,
        symbol_failures: result.symbol_failures,
        errors: result.errors,
      };
    }

    return {
      request_id: request.request_id,
      status: 'success',
      dts_content: result.dts_content,
      manifest: result.manifest,
      symbol_failures: result.symbol_failures,
    };
  } catch (err) {
    const error = err instanceof Error ? err.message : String(err);
    logError(`Bundle failed: ${error}`);

    return {
      request_id: request.request_id,
      status: 'error',
      errors: [error],
    };
  }
}

/**
 * Handle the 'infer' action - infer implicit types
 */
function handleInfer(request: SidecarRequest & { action: 'infer' }): InferResponse {
  if (!projectLoader?.isInitialized() || !typeInferrer) {
    return {
      request_id: request.request_id,
      status: 'error',
      errors: ['Sidecar not initialized. Call init first.'],
    };
  }

  try {
    log(`Inferring ${request.requests.length} type(s)`);

    const result = typeInferrer.infer(request.requests);

    return {
      request_id: request.request_id,
      status: result.success ? 'success' : 'error',
      inferred_types: result.inferred_types,
      errors: result.errors,
    };
  } catch (err) {
    const error = err instanceof Error ? err.message : String(err);
    logError(`Inference failed: ${error}`);

    return {
      request_id: request.request_id,
      status: 'error',
      errors: [error],
    };
  }
}

/**
 * Handle the 'health' action - report initialization status
 */
function handleHealth(request: SidecarRequest & { action: 'health' }): HealthResponse {
  const isReady = projectLoader?.isInitialized() ?? false;

  return {
    request_id: request.request_id,
    status: isReady ? 'ready' : 'not_ready',
    init_time_ms: initTimeMs ?? undefined,
  };
}

/**
 * Handle the 'shutdown' action - exit gracefully
 */
function handleShutdown(request: SidecarRequest & { action: 'shutdown' }): ShutdownResponse {
  log('Shutdown requested');

  // Schedule exit after response is sent
  setImmediate(() => {
    log('Exiting');
    process.exit(0);
  });

  return {
    request_id: request.request_id,
    status: 'success',
  };
}

// ===========================================================================
// Request Router
// ===========================================================================

/**
 * Route a request to the appropriate handler
 */
function handleRequest(request: SidecarRequest): SidecarResponse {
  switch (request.action) {
    case 'init':
      return handleInit(request);
    case 'bundle':
      return handleBundle(request);
    case 'infer':
      return handleInfer(request);
    case 'health':
      return handleHealth(request);
    case 'shutdown':
      return handleShutdown(request);
    default:
      // TypeScript should catch this, but just in case
      return {
        request_id: (request as { request_id: string }).request_id || 'unknown',
        status: 'error',
        errors: [`Unknown action: ${(request as { action: string }).action}`],
      } as ErrorResponse;
  }
}

// ===========================================================================
// Response Writer
// ===========================================================================

/**
 * Write a JSON response to stdout
 */
function writeResponse(response: SidecarResponse): void {
  const json = JSON.stringify(response);
  process.stdout.write(json + '\n');
}

/**
 * Write an error response for an invalid request
 */
function writeErrorResponse(requestId: string, error: string): void {
  const response: ErrorResponse = {
    request_id: requestId,
    status: 'error',
    errors: [error],
  };
  writeResponse(response);
}

// ===========================================================================
// Logging (to stderr only)
// ===========================================================================

function log(message: string): void {
  console.error(`[sidecar] ${message}`);
}

function logError(message: string): void {
  console.error(`[sidecar:error] ${message}`);
}

// ===========================================================================
// Main Entry Point
// ===========================================================================

/**
 * Process a single line of input
 */
function processLine(line: string): void {
  // Skip empty lines
  const trimmed = line.trim();
  if (!trimmed) return;

  // Parse JSON
  let json: unknown;
  try {
    json = JSON.parse(trimmed);
  } catch (err) {
    logError(`Invalid JSON: ${trimmed}`);
    writeErrorResponse('unknown', `Invalid JSON: ${err instanceof Error ? err.message : String(err)}`);
    return;
  }

  // Validate request
  const parseResult = parseRequest(json);
  if (!parseResult.success) {
    logError(`Invalid request: ${parseResult.error}`);
    writeErrorResponse(
      (json as { request_id?: string })?.request_id || 'unknown',
      parseResult.error
    );
    return;
  }

  // Handle request
  const response = handleRequest(parseResult.request);
  writeResponse(response);
}

/**
 * Start the message loop
 */
function main(): void {
  log('Process started');

  const rl = readline.createInterface({
    input: process.stdin,
    output: process.stdout,
    terminal: false,
  });

  rl.on('line', processLine);

  rl.on('close', () => {
    log('stdin closed, exiting');
    process.exit(0);
  });

  // Handle process signals
  process.on('SIGINT', () => {
    log('SIGINT received, exiting');
    process.exit(0);
  });

  process.on('SIGTERM', () => {
    log('SIGTERM received, exiting');
    process.exit(0);
  });

  // Handle uncaught errors
  process.on('uncaughtException', (err) => {
    logError(`Uncaught exception: ${err.message}`);
    logError(err.stack || '');
    // Keep running, but log the error
  });

  process.on('unhandledRejection', (reason) => {
    logError(`Unhandled rejection: ${reason}`);
    // Keep running, but log the error
  });
}

// Start the sidecar
main();
