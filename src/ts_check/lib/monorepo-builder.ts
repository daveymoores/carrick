/**
 * Monorepo Builder - Standalone module for building synthetic monorepo workspaces
 *
 * This is the ts_check library version of the monorepo builder, designed to be
 * used outside the sidecar context for direct TypeScript workspace manipulation.
 *
 * Architecture: "Synthetic Monorepo (Stub Snapshot)"
 * - Creates a workspace with stub packages for each repo
 * - Each stub package has isolated node_modules with pinned dependencies
 * - Path mappings resolve @carrick/{repoName}/{spec} to correct locations
 * - Checker package imports surfaces and asserts type compatibility
 *
 * Key constraints:
 * - No full repo copies - only captured artifacts
 * - Deterministic builds via pinned dependency snapshots
 * - Framework-agnostic type comparisons
 */

import * as fs from 'node:fs';
import * as path from 'node:path';
import { execSync, type ExecSyncOptions } from 'node:child_process';

// =============================================================================
// Types
// =============================================================================

/**
 * A map of package names to exact pinned versions.
 */
export interface PinnedDependencySnapshot {
  [packageName: string]: string;
}

/**
 * A normalized/closed tsconfig object where all extends chains have been resolved.
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

/**
 * Extraction rule for unwrapping machinery types
 */
export interface ExtractionRule {
  wrapperSymbols?: string[];
  machineryIndicators?: string[];
  originModuleGlobs?: string[];
  payloadGenericIndex?: number;
  payloadPropertyPath?: string[];
  unwrapRecursively?: boolean;
  maxDepth?: number;
}

/**
 * Configuration for payload extraction
 */
export interface ExtractionConfig {
  rules: ExtractionRule[];
}

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

  /** The emitted surface .d.ts content */
  surfaceContent?: string;
}

/**
 * A single compatibility check between two types
 */
export interface CompatibilityCheck {
  /** Source repo name */
  sourceRepo: string;
  /** Source payload alias */
  sourceAlias: string;
  /** Target repo name */
  targetRepo: string;
  /** Target payload alias */
  targetAlias: string;
  /** Direction of assignability check */
  direction: 'source_extends_target' | 'target_extends_source' | 'bidirectional';
}

/**
 * Result of a single compatibility check
 */
export interface CompatibilityResult {
  sourceRepo: string;
  sourceAlias: string;
  targetRepo: string;
  targetAlias: string;
  compatible: boolean;
  diagnostic?: string;
}

/**
 * Result from building the synthetic workspace
 */
export interface WorkspaceBuildResult {
  success: boolean;
  workspacePath?: string;
  stubPackages?: string[];
  checkerPath?: string;
  errors?: string[];
}

/**
 * Result from running compatibility checks
 */
export interface CompatibilityCheckResult {
  success: boolean;
  results?: CompatibilityResult[];
  diagnostics?: string[];
  errors?: string[];
}

/**
 * Options for MonorepoBuilder
 */
export interface MonorepoBuilderOptions {
  /** Working directory (defaults to cwd) */
  cwd?: string;
  /** Whether to use verbose logging */
  verbose?: boolean;
  /** Package manager to use (defaults to pnpm, falls back to npm) */
  packageManager?: 'pnpm' | 'npm' | 'yarn';
}

// =============================================================================
// Default Values
// =============================================================================

const DEFAULT_WORKSPACE_ROOT = '.carrick/workspace';

const DEFAULT_TSCONFIG_SNAPSHOT: TsconfigSnapshot = {
  compilerOptions: {
    module: 'NodeNext',
    moduleResolution: 'NodeNext',
    target: 'ES2022',
    lib: ['ES2022'],
    strict: true,
    esModuleInterop: true,
    skipLibCheck: true,
    declaration: true,
  },
};

// =============================================================================
// MonorepoBuilder Class
// =============================================================================

/**
 * MonorepoBuilder - Creates synthetic workspaces for cross-repo type checking
 *
 * @example
 * const builder = new MonorepoBuilder();
 *
 * // Build workspace with repo metadata
 * const buildResult = await builder.build([
 *   {
 *     repoName: 'api-server',
 *     dependencies: { 'zod': '3.23.8', '@types/node': '20.11.30' },
 *     tsconfig: { compilerOptions: { module: 'NodeNext' } },
 *     surfaceContent: 'export type UserPayload = { id: string; name: string };',
 *   },
 *   {
 *     repoName: 'web-client',
 *     dependencies: { 'zod': '3.22.0', '@types/node': '18.0.0' },
 *     tsconfig: { compilerOptions: { module: 'ES2022' } },
 *     surfaceContent: 'export type UserPayload = { id: string; name: string; email?: string };',
 *   },
 * ]);
 *
 * // Run compatibility checks
 * const checkResult = await builder.checkCompatibility(buildResult.workspacePath!, [
 *   {
 *     sourceRepo: 'api-server',
 *     sourceAlias: 'UserPayload',
 *     targetRepo: 'web-client',
 *     targetAlias: 'UserPayload',
 *     direction: 'source_extends_target',
 *   },
 * ]);
 */
export class MonorepoBuilder {
  private readonly cwd: string;
  private readonly verbose: boolean;
  private readonly packageManager: 'pnpm' | 'npm' | 'yarn';

  constructor(options: MonorepoBuilderOptions = {}) {
    this.cwd = options.cwd || process.cwd();
    this.verbose = options.verbose || false;
    this.packageManager = options.packageManager || 'pnpm';
  }

  // ===========================================================================
  // Public API
  // ===========================================================================

  /**
   * Build the synthetic monorepo workspace
   *
   * @param repos - Metadata for each repository to include
   * @param workspaceRoot - Root directory for the workspace (relative to cwd or absolute)
   * @returns Promise<WorkspaceBuildResult>
   */
  async build(
    repos: RepoMetadata[],
    workspaceRoot?: string
  ): Promise<WorkspaceBuildResult> {
    const root = this.resolveWorkspaceRoot(workspaceRoot);
    const errors: string[] = [];
    const stubPackages: string[] = [];

    try {
      // Phase 1: Clean and create workspace root
      this.log(`Creating workspace at: ${root}`);
      this.ensureCleanDirectory(root);

      // Phase 2: Create root workspace files
      this.createRootPackageJson(root);
      this.createWorkspaceConfig(root);
      this.createNpmrc(root);

      // Phase 3: Create stub packages for each repo
      for (const repo of repos) {
        const packagePath = this.createStubPackage(root, repo);
        stubPackages.push(packagePath);
      }

      // Phase 4: Create checker package
      const checkerPath = this.createCheckerPackage(root, repos);

      // Phase 5: Install dependencies
      await this.installDependencies(root);

      this.log(`Workspace built successfully at: ${root}`);

      return {
        success: true,
        workspacePath: root,
        stubPackages,
        checkerPath,
      };
    } catch (err) {
      const error = err instanceof Error ? err.message : String(err);
      this.logError(`Workspace build failed: ${error}`);
      errors.push(error);

      return {
        success: false,
        errors,
      };
    }
  }

  /**
   * Run type compatibility checks in the workspace
   *
   * @param workspaceRoot - Path to the workspace root
   * @param checks - Compatibility checks to run
   * @returns Promise<CompatibilityCheckResult>
   */
  async checkCompatibility(
    workspaceRoot: string,
    checks: CompatibilityCheck[]
  ): Promise<CompatibilityCheckResult> {
    const errors: string[] = [];
    const results: CompatibilityResult[] = [];
    const diagnostics: string[] = [];

    try {
      const root = this.resolveWorkspaceRoot(workspaceRoot);
      const checkerDir = path.join(root, 'checker');

      // Generate check assertions file
      const checkFilePath = path.join(checkerDir, 'src', 'checks.ts');
      this.generateCheckFile(checkFilePath, checks);

      // Run tsc on the checker package
      const tscResult = await this.runTypeCheck(checkerDir);

      // Parse results
      for (const check of checks) {
        const result = this.parseCheckResult(check, tscResult);
        results.push(result);

        if (!result.compatible && result.diagnostic) {
          diagnostics.push(result.diagnostic);
        }
      }

      return {
        success: true,
        results,
        diagnostics: diagnostics.length > 0 ? diagnostics : undefined,
      };
    } catch (err) {
      const error = err instanceof Error ? err.message : String(err);
      this.logError(`Compatibility check failed: ${error}`);
      errors.push(error);

      return {
        success: false,
        errors,
      };
    }
  }

  /**
   * Update the surface file for a specific repo without rebuilding the entire workspace
   */
  async updateSurface(
    workspaceRoot: string,
    repoName: string,
    surfaceContent: string
  ): Promise<{ success: boolean; error?: string }> {
    try {
      const root = this.resolveWorkspaceRoot(workspaceRoot);
      const surfacePath = path.join(root, 'packages', repoName, 'src', 'surface.d.ts');

      if (!fs.existsSync(path.dirname(surfacePath))) {
        return {
          success: false,
          error: `Package directory for '${repoName}' does not exist`,
        };
      }

      fs.writeFileSync(surfacePath, surfaceContent, 'utf-8');
      this.log(`Updated surface for ${repoName}`);

      return { success: true };
    } catch (err) {
      const error = err instanceof Error ? err.message : String(err);
      return { success: false, error };
    }
  }

  /**
   * Clean up the workspace directory
   */
  async cleanup(workspaceRoot?: string): Promise<void> {
    const root = this.resolveWorkspaceRoot(workspaceRoot);
    if (fs.existsSync(root)) {
      fs.rmSync(root, { recursive: true, force: true });
      this.log(`Cleaned up workspace: ${root}`);
    }
  }

  // ===========================================================================
  // Workspace Creation
  // ===========================================================================

  private resolveWorkspaceRoot(workspaceRoot?: string): string {
    const root = workspaceRoot || DEFAULT_WORKSPACE_ROOT;
    return path.isAbsolute(root) ? root : path.join(this.cwd, root);
  }

  private ensureCleanDirectory(dirPath: string): void {
    if (fs.existsSync(dirPath)) {
      fs.rmSync(dirPath, { recursive: true, force: true });
    }
    fs.mkdirSync(dirPath, { recursive: true });
  }

  private createRootPackageJson(root: string): void {
    const packageJson = {
      name: '@carrick/workspace',
      version: '0.0.0',
      private: true,
      description: 'Synthetic workspace for Carrick type compatibility checking',
      workspaces: ['packages/*', 'checker'],
    };

    const filePath = path.join(root, 'package.json');
    fs.writeFileSync(filePath, JSON.stringify(packageJson, null, 2), 'utf-8');
    this.log(`Created: ${filePath}`);
  }

  private createWorkspaceConfig(root: string): void {
    if (this.packageManager === 'pnpm') {
      const content = `# Carrick synthetic workspace
packages:
  - 'packages/*'
  - 'checker'
`;
      const filePath = path.join(root, 'pnpm-workspace.yaml');
      fs.writeFileSync(filePath, content, 'utf-8');
      this.log(`Created: ${filePath}`);
    }
    // npm and yarn use the workspaces field in package.json
  }

  private createNpmrc(root: string): void {
    // Configuration to prevent hoisting that would blur package boundaries
    const content = `# Prevent hoisting to maintain per-package isolation
node-linker=isolated
hoist=false
shamefully-hoist=false
`;

    const filePath = path.join(root, '.npmrc');
    fs.writeFileSync(filePath, content, 'utf-8');
    this.log(`Created: ${filePath}`);
  }

  // ===========================================================================
  // Stub Package Creation
  // ===========================================================================

  private createStubPackage(root: string, repo: RepoMetadata): string {
    const packageDir = path.join(root, 'packages', repo.repoName);
    const srcDir = path.join(packageDir, 'src');

    // Create directories
    fs.mkdirSync(srcDir, { recursive: true });

    // Create package.json from pinned dependencies
    this.createStubPackageJson(packageDir, repo);

    // Create tsconfig.json from snapshot
    this.createStubTsconfig(packageDir, repo);

    // Create surface.d.ts
    if (repo.surfaceContent) {
      this.createSurfaceFile(srcDir, repo.surfaceContent);
    } else {
      this.createPlaceholderSurface(srcDir, repo.repoName);
    }

    this.log(`Created stub package: ${packageDir}`);
    return packageDir;
  }

  private createStubPackageJson(packageDir: string, repo: RepoMetadata): void {
    const packageJson: Record<string, unknown> = {
      name: `@carrick/${repo.repoName}`,
      version: '0.0.0',
      private: true,
      description: `Stub package for ${repo.repoName} type checking`,
      main: './src/surface.d.ts',
      types: './src/surface.d.ts',
      dependencies: { ...repo.dependencies },
    };

    const filePath = path.join(packageDir, 'package.json');
    fs.writeFileSync(filePath, JSON.stringify(packageJson, null, 2), 'utf-8');
  }

  private createStubTsconfig(packageDir: string, repo: RepoMetadata): void {
    const snapshot = repo.tsconfig || DEFAULT_TSCONFIG_SNAPSHOT;

    const tsconfig = {
      compilerOptions: {
        ...snapshot.compilerOptions,
        // Override for surface checking
        declaration: true,
        declarationMap: false,
        emitDeclarationOnly: true,
        noEmit: false,
        skipLibCheck: true,
        rootDir: './src',
        outDir: './dist',
      },
      include: ['src/**/*'],
      exclude: ['node_modules'],
    };

    const filePath = path.join(packageDir, 'tsconfig.json');
    fs.writeFileSync(filePath, JSON.stringify(tsconfig, null, 2), 'utf-8');
  }

  private createSurfaceFile(srcDir: string, content: string): void {
    const filePath = path.join(srcDir, 'surface.d.ts');
    fs.writeFileSync(filePath, content, 'utf-8');
  }

  private createPlaceholderSurface(srcDir: string, repoName: string): void {
    const content = `// Placeholder surface for ${repoName}
// This file should be replaced with actual extracted types

export type Placeholder = unknown;
`;

    const filePath = path.join(srcDir, 'surface.d.ts');
    fs.writeFileSync(filePath, content, 'utf-8');
  }

  // ===========================================================================
  // Checker Package Creation
  // ===========================================================================

  private createCheckerPackage(root: string, repos: RepoMetadata[]): string {
    const checkerDir = path.join(root, 'checker');
    const srcDir = path.join(checkerDir, 'src');

    // Create directories
    fs.mkdirSync(srcDir, { recursive: true });

    // Create package.json
    this.createCheckerPackageJson(checkerDir, repos);

    // Create tsconfig.json with path mappings
    this.createCheckerTsconfig(checkerDir, repos);

    // Create initial checks file
    this.createInitialChecksFile(srcDir);

    this.log(`Created checker package: ${checkerDir}`);
    return checkerDir;
  }

  private createCheckerPackageJson(checkerDir: string, repos: RepoMetadata[]): void {
    const dependencies: Record<string, string> = {};

    // Add workspace dependencies to each stub package
    for (const repo of repos) {
      dependencies[`@carrick/${repo.repoName}`] = 'workspace:*';
    }

    const packageJson = {
      name: '@carrick/checker',
      version: '0.0.0',
      private: true,
      description: 'Type compatibility checker',
      dependencies,
      devDependencies: {
        typescript: '^5.0.0',
      },
      scripts: {
        check: 'tsc --noEmit',
      },
    };

    const filePath = path.join(checkerDir, 'package.json');
    fs.writeFileSync(filePath, JSON.stringify(packageJson, null, 2), 'utf-8');
  }

  /**
   * Create tsconfig.json with path mappings for the checker
   */
  private createCheckerTsconfig(checkerDir: string, repos: RepoMetadata[]): void {
    const paths: Record<string, string[]> = {};

    for (const repo of repos) {
      const repoName = repo.repoName;

      // Surface import
      paths[`@carrick/${repoName}/surface`] = [
        `../packages/${repoName}/src/surface.d.ts`,
      ];

      // Unscoped packages
      paths[`@carrick/${repoName}/*`] = [
        `../packages/${repoName}/node_modules/*`,
        `../packages/${repoName}/node_modules/*/index.d.ts`,
        `../packages/${repoName}/node_modules/@types/*`,
      ];

      // Scoped packages
      paths[`@carrick/${repoName}/@*/*`] = [
        `../packages/${repoName}/node_modules/@*/*`,
        `../packages/${repoName}/node_modules/@*/*/index.d.ts`,
      ];
    }

    const tsconfig = {
      compilerOptions: {
        module: 'NodeNext',
        moduleResolution: 'NodeNext',
        target: 'ES2022',
        lib: ['ES2022'],
        strict: true,
        esModuleInterop: true,
        skipLibCheck: true,
        noEmit: true,
        baseUrl: '.',
        paths,
      },
      include: ['src/**/*'],
      exclude: ['node_modules'],
    };

    const filePath = path.join(checkerDir, 'tsconfig.json');
    fs.writeFileSync(filePath, JSON.stringify(tsconfig, null, 2), 'utf-8');
  }

  private createInitialChecksFile(srcDir: string): void {
    const content = `// Carrick Type Compatibility Checks
// This file is auto-generated - do not edit manually

// Type assertion helpers
type Assert<T extends true> = T;
type IsAssignable<X, Y> = [X] extends [Y] ? true : false;
type IsBidirectional<X, Y> = [X] extends [Y] ? ([Y] extends [X] ? true : false) : false;

// Checks will be generated here
export {};
`;

    const filePath = path.join(srcDir, 'checks.ts');
    fs.writeFileSync(filePath, content, 'utf-8');
  }

  // ===========================================================================
  // Dependency Installation
  // ===========================================================================

  private async installDependencies(root: string): Promise<void> {
    this.log(`Installing dependencies with ${this.packageManager}...`);

    const execOptions: ExecSyncOptions = {
      cwd: root,
      stdio: this.verbose ? 'inherit' : 'pipe',
      env: { ...process.env, CI: 'false' },
    };

    try {
      switch (this.packageManager) {
        case 'pnpm':
          await this.installWithPnpm(root, execOptions);
          break;
        case 'yarn':
          await this.installWithYarn(root, execOptions);
          break;
        case 'npm':
        default:
          await this.installWithNpm(root, execOptions);
          break;
      }
      this.log('Dependencies installed successfully');
    } catch {
      // Try fallback to npm
      if (this.packageManager !== 'npm') {
        this.log(`${this.packageManager} failed, falling back to npm...`);
        await this.installWithNpm(root, execOptions);
        this.log('Dependencies installed successfully with npm');
      } else {
        throw new Error('Failed to install dependencies');
      }
    }
  }

  private async installWithPnpm(root: string, options: ExecSyncOptions): Promise<void> {
    try {
      execSync('pnpm --version', { ...options, stdio: 'pipe' });
    } catch {
      throw new Error('pnpm is not available');
    }

    try {
      execSync('pnpm install --frozen-lockfile=false', options);
    } catch {
      // If frozen lockfile fails, try without it
      execSync('pnpm install', options);
    }
  }

  private async installWithYarn(root: string, options: ExecSyncOptions): Promise<void> {
    try {
      execSync('yarn --version', { ...options, stdio: 'pipe' });
    } catch {
      throw new Error('yarn is not available');
    }

    execSync('yarn install', options);
  }

  private async installWithNpm(root: string, options: ExecSyncOptions): Promise<void> {
    // Remove pnpm-workspace.yaml if it exists (npm doesn't understand it)
    const workspaceFile = path.join(root, 'pnpm-workspace.yaml');
    if (fs.existsSync(workspaceFile)) {
      fs.unlinkSync(workspaceFile);
    }

    execSync('npm install', options);
  }

  // ===========================================================================
  // Compatibility Checking
  // ===========================================================================

  private generateCheckFile(filePath: string, checks: CompatibilityCheck[]): void {
    const lines: string[] = [
      '// Carrick Type Compatibility Checks',
      '// Auto-generated - do not edit manually',
      '',
      '// Type assertion helpers',
      'type Assert<T extends true> = T;',
      'type IsAssignable<X, Y> = [X] extends [Y] ? true : false;',
      'type IsBidirectional<X, Y> = [X] extends [Y] ? ([Y] extends [X] ? true : false) : false;',
      '',
    ];

    // Generate imports
    const repoImports = new Map<string, Set<string>>();
    for (const check of checks) {
      if (!repoImports.has(check.sourceRepo)) {
        repoImports.set(check.sourceRepo, new Set());
      }
      repoImports.get(check.sourceRepo)!.add(check.sourceAlias);

      if (!repoImports.has(check.targetRepo)) {
        repoImports.set(check.targetRepo, new Set());
      }
      repoImports.get(check.targetRepo)!.add(check.targetAlias);
    }

    for (const [repo, aliases] of repoImports) {
      const aliasesArray = Array.from(aliases);
      const importNames = aliasesArray.map(
        (alias) => `${alias} as ${this.makeImportName(repo, alias)}`
      );
      lines.push(
        `import type { ${importNames.join(', ')} } from '@carrick/${repo}/surface';`
      );
    }

    lines.push('');
    lines.push('// Compatibility checks');

    // Generate type assertions
    for (let i = 0; i < checks.length; i++) {
      const check = checks[i];
      const sourceType = this.makeImportName(check.sourceRepo, check.sourceAlias);
      const targetType = this.makeImportName(check.targetRepo, check.targetAlias);
      const checkName = `_check${i}`;

      lines.push('');
      lines.push(
        `// Check: ${check.sourceRepo}/${check.sourceAlias} vs ${check.targetRepo}/${check.targetAlias}`
      );

      switch (check.direction) {
        case 'source_extends_target':
          lines.push(`type ${checkName} = Assert<IsAssignable<${sourceType}, ${targetType}>>;`);
          break;
        case 'target_extends_source':
          lines.push(`type ${checkName} = Assert<IsAssignable<${targetType}, ${sourceType}>>;`);
          break;
        case 'bidirectional':
          lines.push(`type ${checkName} = Assert<IsBidirectional<${sourceType}, ${targetType}>>;`);
          break;
      }
    }

    lines.push('');
    lines.push('export {};');

    const content = lines.join('\n');
    fs.writeFileSync(filePath, content, 'utf-8');
    this.log(`Generated check file: ${filePath}`);
  }

  private makeImportName(repo: string, alias: string): string {
    const safeRepo = repo.replace(/[^a-zA-Z0-9]/g, '_');
    const safeAlias = alias.replace(/[^a-zA-Z0-9]/g, '_');
    return `${safeRepo}_${safeAlias}`;
  }

  private async runTypeCheck(
    checkerDir: string
  ): Promise<{ success: boolean; output: string }> {
    try {
      execSync('npx tsc --noEmit', {
        cwd: checkerDir,
        stdio: 'pipe',
        encoding: 'utf-8',
      });
      return { success: true, output: '' };
    } catch (err: unknown) {
      const error = err as { stdout?: string; stderr?: string; message?: string };
      const output = error.stdout || error.stderr || error.message || 'Unknown error';
      return { success: false, output };
    }
  }

  private parseCheckResult(
    check: CompatibilityCheck,
    tscResult: { success: boolean; output: string }
  ): CompatibilityResult {
    // If tsc succeeded, all checks passed
    if (tscResult.success) {
      return {
        sourceRepo: check.sourceRepo,
        sourceAlias: check.sourceAlias,
        targetRepo: check.targetRepo,
        targetAlias: check.targetAlias,
        compatible: true,
      };
    }

    // Look for errors related to this specific check
    const checkPattern = new RegExp(
      `${check.sourceAlias}.*${check.targetAlias}|${check.targetAlias}.*${check.sourceAlias}`,
      'i'
    );

    const isRelated = checkPattern.test(tscResult.output);

    return {
      sourceRepo: check.sourceRepo,
      sourceAlias: check.sourceAlias,
      targetRepo: check.targetRepo,
      targetAlias: check.targetAlias,
      compatible: !isRelated,
      diagnostic: isRelated ? tscResult.output : undefined,
    };
  }

  // ===========================================================================
  // Logging
  // ===========================================================================

  private log(message: string): void {
    if (this.verbose) {
      console.error(`[monorepo-builder] ${message}`);
    }
  }

  private logError(message: string): void {
    console.error(`[monorepo-builder:error] ${message}`);
  }
}

// =============================================================================
// Utility Functions
// =============================================================================

/**
 * Create a default extraction config for common frameworks
 */
export function createDefaultExtractionConfig(): ExtractionConfig {
  return {
    rules: [
      // Express/Koa/Fastify Response types
      {
        wrapperSymbols: ['Response', 'ServerResponse'],
        machineryIndicators: ['status', 'json', 'send', 'header', 'cookie', 'redirect'],
        originModuleGlobs: ['express', '@types/express', 'koa', 'fastify'],
        payloadGenericIndex: 0,
        unwrapRecursively: true,
        maxDepth: 4,
      },
      // Axios Response
      {
        wrapperSymbols: ['AxiosResponse'],
        originModuleGlobs: ['axios'],
        payloadPropertyPath: ['data'],
        unwrapRecursively: true,
        maxDepth: 4,
      },
      // Promise wrapper
      {
        wrapperSymbols: ['Promise', 'PromiseLike'],
        payloadGenericIndex: 0,
        unwrapRecursively: true,
        maxDepth: 4,
      },
      // Observable (RxJS)
      {
        wrapperSymbols: ['Observable'],
        originModuleGlobs: ['rxjs'],
        payloadGenericIndex: 0,
        unwrapRecursively: true,
        maxDepth: 4,
      },
    ],
  };
}

/**
 * Merge multiple tsconfig snapshots (later ones override earlier)
 */
export function mergeTsconfigSnapshots(
  ...snapshots: (TsconfigSnapshot | undefined)[]
): TsconfigSnapshot {
  const result: TsconfigSnapshot = {
    compilerOptions: {},
  };

  for (const snapshot of snapshots) {
    if (snapshot) {
      result.compilerOptions = {
        ...result.compilerOptions,
        ...snapshot.compilerOptions,
      };
    }
  }

  return result;
}

/**
 * Create a minimal tsconfig snapshot from key options
 */
export function createTsconfigSnapshot(options: {
  module?: string;
  moduleResolution?: string;
  target?: string;
  strict?: boolean;
}): TsconfigSnapshot {
  return {
    compilerOptions: {
      module: options.module || 'NodeNext',
      moduleResolution: options.moduleResolution || 'NodeNext',
      target: options.target || 'ES2022',
      lib: ['ES2022'],
      strict: options.strict ?? true,
      esModuleInterop: true,
      skipLibCheck: true,
      declaration: true,
    },
  };
}
