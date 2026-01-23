/**
 * Project Loader - Loads TypeScript projects using ts-morph
 *
 * This module handles initialization of the ts-morph Project,
 * including finding and loading tsconfig.json files.
 *
 * Supports:
 * - Traditional tsconfig.json file loading
 * - Tsconfig snapshot (closed/merged extends chains) for synthetic monorepo
 * - Pinned dependency snapshots for deterministic builds
 */

import { Project, type CompilerOptions } from 'ts-morph';
import * as path from 'node:path';
import * as fs from 'node:fs';
import type { TsconfigSnapshot, PinnedDependencySnapshot } from './types.js';

/**
 * Options for ProjectLoader construction
 */
export interface ProjectLoaderOptions {
  /** The repository root directory (absolute path) */
  repoRoot: string;
  /** Optional path to tsconfig.json (relative to repo root or absolute) */
  tsconfigPath?: string;
  /** Optional tsconfig snapshot (closed/merged) - preferred over tsconfigPath */
  tsconfigSnapshot?: TsconfigSnapshot;
  /** Optional pinned dependencies for this repo */
  pinnedDependencies?: PinnedDependencySnapshot;
}

/**
 * Result of project loading
 */
export interface LoadResult {
  success: boolean;
  error?: string;
  initTimeMs?: number;
}

/**
 * Default compiler options used when no tsconfig.json is found
 */
const DEFAULT_COMPILER_OPTIONS: CompilerOptions = {
  target: 99, // ESNext
  module: 99, // ESNext
  moduleResolution: 100, // NodeNext
  strict: true,
  esModuleInterop: true,
  skipLibCheck: true,
  declaration: true,
  allowJs: true,
  checkJs: false,
  resolveJsonModule: true,
  isolatedModules: true,
};

/**
 * Map string module values to ts-morph enum values
 */
const MODULE_MAP: Record<string, number> = {
  'CommonJS': 1,
  'AMD': 2,
  'UMD': 3,
  'System': 4,
  'ES2015': 5,
  'ES2020': 6,
  'ES2022': 7,
  'ESNext': 99,
  'Node16': 100,
  'NodeNext': 199,
  'Preserve': 200,
};

/**
 * Map string moduleResolution values to ts-morph enum values
 */
const MODULE_RESOLUTION_MAP: Record<string, number> = {
  'Classic': 1,
  'Node': 2,
  'Node10': 2,
  'Node16': 3,
  'NodeNext': 99,
  'Bundler': 100,
};

/**
 * Map string target values to ts-morph enum values
 */
const TARGET_MAP: Record<string, number> = {
  'ES3': 0,
  'ES5': 1,
  'ES2015': 2,
  'ES2016': 3,
  'ES2017': 4,
  'ES2018': 5,
  'ES2019': 6,
  'ES2020': 7,
  'ES2021': 8,
  'ES2022': 9,
  'ES2023': 10,
  'ESNext': 99,
};

/**
 * ProjectLoader - Manages ts-morph Project initialization and access
 *
 * Usage:
 *   const loader = new ProjectLoader({ repoRoot: '/path/to/repo' });
 *   const result = loader.load();
 *   if (result.success) {
 *     const project = loader.getProject();
 *   }
 */
export class ProjectLoader {
  private project: Project | null = null;
  private readonly repoRoot: string;
  private readonly tsconfigPath: string | undefined;
  private readonly tsconfigSnapshot: TsconfigSnapshot | undefined;
  private readonly pinnedDependencies: PinnedDependencySnapshot | undefined;
  private initialized: boolean = false;
  private initError: string | null = null;
  private initTimeMs: number | null = null;

  constructor(options: ProjectLoaderOptions) {
    // Normalize the repo root to an absolute path
    this.repoRoot = path.isAbsolute(options.repoRoot)
      ? options.repoRoot
      : path.resolve(process.cwd(), options.repoRoot);

    // Resolve tsconfig path if provided
    if (options.tsconfigPath) {
      this.tsconfigPath = path.isAbsolute(options.tsconfigPath)
        ? options.tsconfigPath
        : path.resolve(this.repoRoot, options.tsconfigPath);
    }

    // Store snapshot if provided
    this.tsconfigSnapshot = options.tsconfigSnapshot;
    this.pinnedDependencies = options.pinnedDependencies;
  }

  /**
   * Load the TypeScript project
   *
   * @returns LoadResult indicating success or failure
   */
  load(): LoadResult {
    const startTime = performance.now();

    try {
      // Validate repo root exists
      if (!fs.existsSync(this.repoRoot)) {
        const error = `Repository root does not exist: ${this.repoRoot}`;
        this.logError(error);
        this.initError = error;
        return { success: false, error };
      }

      if (!fs.statSync(this.repoRoot).isDirectory()) {
        const error = `Repository root is not a directory: ${this.repoRoot}`;
        this.logError(error);
        this.initError = error;
        return { success: false, error };
      }

      // Priority 1: Use tsconfig snapshot if provided (for synthetic monorepo)
      if (this.tsconfigSnapshot) {
        this.log('Loading project with tsconfig snapshot');
        const compilerOptions = this.snapshotToCompilerOptions(this.tsconfigSnapshot);
        this.project = new Project({
          compilerOptions,
          skipAddingFilesFromTsConfig: true,
        });
        this.addDefaultSourceFiles();
      }
      // Priority 2: Use tsconfig.json file
      else {
        const tsconfigPath = this.findTsConfig();

        if (tsconfigPath) {
          this.log(`Loading project with tsconfig: ${tsconfigPath}`);
          this.project = new Project({
            tsConfigFilePath: tsconfigPath,
            skipAddingFilesFromTsConfig: false,
          });
        } else {
          this.log('No tsconfig.json found, using default compiler options');
          this.project = new Project({
            compilerOptions: DEFAULT_COMPILER_OPTIONS,
            skipAddingFilesFromTsConfig: true,
          });

          // Add source files from common locations
          this.addDefaultSourceFiles();
        }
      }

      // Log pinned dependencies if provided
      if (this.pinnedDependencies) {
        const depCount = Object.keys(this.pinnedDependencies).length;
        this.log(`Using ${depCount} pinned dependencies`);
      }

      this.initialized = true;
      this.initTimeMs = Math.round(performance.now() - startTime);
      this.log(`Project loaded successfully in ${this.initTimeMs}ms`);

      return {
        success: true,
        initTimeMs: this.initTimeMs,
      };
    } catch (err) {
      const error = err instanceof Error ? err.message : String(err);
      this.logError(`Failed to load project: ${error}`);
      this.initError = error;

      return {
        success: false,
        error,
        initTimeMs: Math.round(performance.now() - startTime),
      };
    }
  }

  /**
   * Convert a TsconfigSnapshot to ts-morph CompilerOptions
   */
  private snapshotToCompilerOptions(snapshot: TsconfigSnapshot): CompilerOptions {
    const opts = snapshot.compilerOptions;
    const result: CompilerOptions = {};

    // Map module
    if (opts.module) {
      const moduleValue = MODULE_MAP[opts.module];
      if (moduleValue !== undefined) {
        result.module = moduleValue;
      }
    }

    // Map moduleResolution
    if (opts.moduleResolution) {
      const moduleResValue = MODULE_RESOLUTION_MAP[opts.moduleResolution];
      if (moduleResValue !== undefined) {
        result.moduleResolution = moduleResValue;
      }
    }

    // Map target
    if (opts.target) {
      const targetValue = TARGET_MAP[opts.target];
      if (targetValue !== undefined) {
        result.target = targetValue;
      }
    }

    // Pass through other options directly
    if (opts.lib) result.lib = opts.lib;
    if (opts.types) result.types = opts.types;
    if (opts.typeRoots) result.typeRoots = opts.typeRoots;
    if (opts.strict !== undefined) result.strict = opts.strict;
    if (opts.esModuleInterop !== undefined) result.esModuleInterop = opts.esModuleInterop;
    if (opts.skipLibCheck !== undefined) result.skipLibCheck = opts.skipLibCheck;
    if (opts.declaration !== undefined) result.declaration = opts.declaration;
    if (opts.declarationMap !== undefined) result.declarationMap = opts.declarationMap;
    if (opts.paths) result.paths = opts.paths;
    if (opts.baseUrl) result.baseUrl = opts.baseUrl;

    // Map jsx if present
    if (opts.jsx) {
      const jsxMap: Record<string, number> = {
        'preserve': 1,
        'react': 2,
        'react-native': 3,
        'react-jsx': 4,
        'react-jsxdev': 5,
      };
      const jsxValue = jsxMap[opts.jsx.toLowerCase()];
      if (jsxValue !== undefined) {
        result.jsx = jsxValue;
      }
    }

    return result;
  }

  /**
   * Get the loaded ts-morph Project
   *
   * @returns The Project instance
   * @throws Error if project hasn't been successfully loaded
   */
  getProject(): Project {
    if (!this.project || !this.initialized) {
      throw new Error(
        'Project not initialized. Call load() first and ensure it succeeds.'
      );
    }
    return this.project;
  }

  /**
   * Check if the project has been successfully initialized
   */
  isInitialized(): boolean {
    return this.initialized;
  }

  /**
   * Get initialization time in milliseconds
   */
  getInitTimeMs(): number | null {
    return this.initTimeMs;
  }

  /**
   * Get the last initialization error, if any
   */
  getInitError(): string | null {
    return this.initError;
  }

  /**
   * Get the repository root path
   */
  getRepoRoot(): string {
    return this.repoRoot;
  }

  /**
   * Get the pinned dependencies, if any
   */
  getPinnedDependencies(): PinnedDependencySnapshot | undefined {
    return this.pinnedDependencies;
  }

  /**
   * Find the tsconfig.json file to use
   *
   * @returns Absolute path to tsconfig.json, or undefined if not found
   */
  private findTsConfig(): string | undefined {
    // If a specific path was provided, try to use it
    if (this.tsconfigPath) {
      if (fs.existsSync(this.tsconfigPath)) {
        return this.tsconfigPath;
      }
      this.log(`Specified tsconfig not found: ${this.tsconfigPath}`);
    }

    // Try common tsconfig locations
    const candidates = [
      path.join(this.repoRoot, 'tsconfig.json'),
      path.join(this.repoRoot, 'tsconfig.build.json'),
      path.join(this.repoRoot, 'tsconfig.app.json'),
    ];

    for (const candidate of candidates) {
      if (fs.existsSync(candidate)) {
        return candidate;
      }
    }

    return undefined;
  }

  /**
   * Add source files from common project locations when no tsconfig is found
   */
  private addDefaultSourceFiles(): void {
    if (!this.project) return;

    const patterns = [
      path.join(this.repoRoot, 'src/**/*.ts'),
      path.join(this.repoRoot, 'src/**/*.tsx'),
      path.join(this.repoRoot, 'lib/**/*.ts'),
      path.join(this.repoRoot, 'app/**/*.ts'),
      path.join(this.repoRoot, 'app/**/*.tsx'),
      path.join(this.repoRoot, '*.ts'),
    ];

    // Filter patterns to only include directories that exist
    for (const pattern of patterns) {
      try {
        this.project.addSourceFilesAtPaths(pattern);
      } catch {
        // Ignore errors for non-existent paths
      }
    }

    const fileCount = this.project.getSourceFiles().length;
    this.log(`Added ${fileCount} source files from default patterns`);
  }

  /**
   * Log a message to stderr (stdout is reserved for JSON responses)
   */
  private log(message: string): void {
    console.error(`[sidecar:project-loader] ${message}`);
  }

  /**
   * Log an error message to stderr
   */
  private logError(message: string): void {
    console.error(`[sidecar:project-loader:error] ${message}`);
  }
}
