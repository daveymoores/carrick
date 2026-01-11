/**
 * Project Loader - Loads TypeScript projects using ts-morph
 *
 * This module handles initialization of the ts-morph Project,
 * including finding and loading tsconfig.json files.
 */

import { Project, type CompilerOptions } from 'ts-morph';
import * as path from 'node:path';
import * as fs from 'node:fs';

/**
 * Options for ProjectLoader construction
 */
export interface ProjectLoaderOptions {
  /** The repository root directory (absolute path) */
  repoRoot: string;
  /** Optional path to tsconfig.json (relative to repo root or absolute) */
  tsconfigPath?: string;
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

      // Find tsconfig.json
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
