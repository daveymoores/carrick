/**
 * Main entry point for the type-sidecar
 *
 * This module implements a message loop that:
 * 1. Listens on stdin for JSON requests
 * 2. Processes each request (init, bundle, emit_surface, infer, build_workspace, check_compatibility, health, shutdown)
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
import { TypeBundler, SurfaceEmitter } from './bundler.js';
import { TypeInferrer } from './type-inferrer.js';
import { MonorepoBuilder } from './monorepo-builder.js';
import type {
  SidecarRequest,
  SidecarResponse,
  InitResponse,
  BundleResponse,
  EmitSurfaceResponse,
  InferResponse,
  BuildWorkspaceResponse,
  CheckCompatibilityResponse,
  HealthResponse,
  ShutdownResponse,
  ErrorResponse,
} from './types.js';

// ===========================================================================
// Module-level state
// ===========================================================================

let projectLoader: ProjectLoader | null = null;
let typeBundler: TypeBundler | null = null;
let surfaceEmitter: SurfaceEmitter | null = null;
let typeInferrer: TypeInferrer | null = null;
let monorepoBuilder: MonorepoBuilder | null = null;
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
      tsconfigSnapshot: request.tsconfig_snapshot,
      pinnedDependencies: request.pinned_dependencies,
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

    // Initialize bundler, surface emitter, and inferrer
    const project = projectLoader.getProject();
    const repoRoot = projectLoader.getRepoRoot();

    typeBundler = new TypeBundler({
      project,
      repoRoot,
    });

    surfaceEmitter = new SurfaceEmitter({
      project,
      repoRoot,
    });

    typeInferrer = new TypeInferrer({ project });

    // Initialize monorepo builder (doesn't need project)
    monorepoBuilder = new MonorepoBuilder();

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
 * Handle the 'bundle' action - bundle explicit types (legacy)
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
 * Handle the 'emit_surface' action - emit a surface .d.ts with rewritten specifiers
 */
function handleEmitSurface(request: SidecarRequest & { action: 'emit_surface' }): EmitSurfaceResponse {
  if (!projectLoader?.isInitialized() || !surfaceEmitter) {
    return {
      request_id: request.request_id,
      status: 'error',
      errors: ['Sidecar not initialized. Call init first.'],
    };
  }

  try {
    log(`Emitting surface for repo '${request.repo_name}' with ${request.payloads.length} payload(s)`);

    const result = surfaceEmitter.emit(
      request.repo_name,
      request.payloads,
      request.output_path
    );

    if (!result.success) {
      return {
        request_id: request.request_id,
        status: 'error',
        errors: result.errors,
      };
    }

    return {
      request_id: request.request_id,
      status: 'success',
      output_path: result.output_path,
      surface_content: result.surface_content,
      manifest: result.manifest,
    };
  } catch (err) {
    const error = err instanceof Error ? err.message : String(err);
    logError(`Surface emission failed: ${error}`);

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

    const result = typeInferrer.infer(
      request.requests,
      request.wrappers || [],
      request.extraction_config
    );

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
 * Handle the 'build_workspace' action - build synthetic monorepo workspace
 */
function handleBuildWorkspace(request: SidecarRequest & { action: 'build_workspace' }): BuildWorkspaceResponse {
  // MonorepoBuilder doesn't require project initialization
  if (!monorepoBuilder) {
    monorepoBuilder = new MonorepoBuilder();
  }

  try {
    log(`Building synthetic workspace with ${request.repos.length} repo(s)`);

    const result = monorepoBuilder.build(request.repos, request.workspace_root);

    if (!result.success) {
      return {
        request_id: request.request_id,
        status: 'error',
        errors: result.errors,
      };
    }

    return {
      request_id: request.request_id,
      status: 'success',
      workspace_path: result.workspace_path,
      stub_packages: result.stub_packages,
      checker_path: result.checker_path,
    };
  } catch (err) {
    const error = err instanceof Error ? err.message : String(err);
    logError(`Workspace build failed: ${error}`);

    return {
      request_id: request.request_id,
      status: 'error',
      errors: [error],
    };
  }
}

/**
 * Handle the 'check_compatibility' action - run type compatibility checks
 */
function handleCheckCompatibility(request: SidecarRequest & { action: 'check_compatibility' }): CheckCompatibilityResponse {
  if (!monorepoBuilder) {
    monorepoBuilder = new MonorepoBuilder();
  }

  try {
    log(`Running compatibility checks: ${request.checks.length} check(s)`);

    const result = monorepoBuilder.checkCompatibility(
      request.workspace_root,
      request.checks
    );

    if (!result.success) {
      return {
        request_id: request.request_id,
        status: 'error',
        errors: result.errors,
      };
    }

    return {
      request_id: request.request_id,
      status: 'success',
      results: result.results,
      diagnostics: result.diagnostics,
    };
  } catch (err) {
    const error = err instanceof Error ? err.message : String(err);
    logError(`Compatibility check failed: ${error}`);

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
    case 'emit_surface':
      return handleEmitSurface(request);
    case 'infer':
      return handleInfer(request);
    case 'build_workspace':
      return handleBuildWorkspace(request);
    case 'check_compatibility':
      return handleCheckCompatibility(request);
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
