/**
 * Monorepo Builder - Builds synthetic monorepo workspaces for type compatibility checking
 *
 * This module implements the "Synthetic Monorepo (Stub Snapshot)" architecture:
 * - Creates a workspace with stub packages for each repo
 * - Each stub package has its own isolated node_modules with pinned dependencies
 * - Generates path mappings so @carrick/{repoName}/{spec} resolves correctly
 * - Creates a checker package that imports surfaces and asserts type compatibility
 *
 * Key design decisions:
 * - Uses pnpm for better isolation between packages
 * - Does NOT copy full repos - only uses captured artifacts
 * - Deterministic: uses pinned dependency snapshots
 */

import * as fs from 'node:fs';
import * as path from 'node:path';
import { execSync } from 'node:child_process';
import type {
  RepoMetadata,
  WorkspaceBuildResult,
  CompatibilityCheck,
  CompatibilityCheckResult,
  CompatibilityResult,
  TsconfigSnapshot,
} from './types.js';

/**
 * Default workspace root path
 */
const DEFAULT_WORKSPACE_ROOT = '.carrick/workspace';

/**
 * MonorepoBuilder - Creates synthetic workspaces for cross-repo type checking
 *
 * Usage:
 *   const builder = new MonorepoBuilder();
 *   const result = builder.build(repos);
 *   const checkResult = builder.checkCompatibility(workspacePath, checks);
 */
export class MonorepoBuilder {
  /**
   * Build the synthetic monorepo workspace
   *
   * @param repos - Metadata for each repository to include
   * @param workspaceRoot - Root directory for the workspace (defaults to .carrick/workspace)
   * @returns WorkspaceBuildResult
   */
  build(repos: RepoMetadata[], workspaceRoot?: string): WorkspaceBuildResult {
    const root = workspaceRoot || DEFAULT_WORKSPACE_ROOT;
    const errors: string[] = [];
    const stubPackages: string[] = [];

    try {
      // Phase 1: Clean and create workspace root
      this.log(`Creating workspace at: ${root}`);
      this.ensureCleanDirectory(root);

      // Phase 2: Create root workspace files
      this.createRootPackageJson(root);
      this.createPnpmWorkspaceYaml(root);
      this.createNpmrc(root);

      // Phase 3: Create stub packages for each repo
      for (const repo of repos) {
        const packagePath = this.createStubPackage(root, repo);
        stubPackages.push(packagePath);
      }

      // Phase 4: Create checker package
      const checkerPath = this.createCheckerPackage(root, repos);

      // Phase 5: Install dependencies
      this.installDependencies(root);

      this.log(`Workspace built successfully at: ${root}`);

      return {
        success: true,
        workspace_path: path.resolve(root),
        stub_packages: stubPackages.map((p) => path.resolve(p)),
        checker_path: path.resolve(checkerPath),
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
   * @returns CompatibilityCheckResult
   */
  checkCompatibility(
    workspaceRoot: string,
    checks: CompatibilityCheck[]
  ): CompatibilityCheckResult {
    const errors: string[] = [];
    const results: CompatibilityResult[] = [];
    const diagnostics: string[] = [];

    try {
      // Generate check assertions file
      const checkerDir = path.join(workspaceRoot, 'checker');
      const checkFilePath = path.join(checkerDir, 'src', 'checks.ts');

      this.generateCheckFile(checkFilePath, checks);

      // Run tsc on the checker package
      const tscResult = this.runTypeCheck(checkerDir);

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

  // ===========================================================================
  // Workspace Creation
  // ===========================================================================

  /**
   * Ensure a clean directory exists at the given path
   */
  private ensureCleanDirectory(dirPath: string): void {
    if (fs.existsSync(dirPath)) {
      fs.rmSync(dirPath, { recursive: true, force: true });
    }
    fs.mkdirSync(dirPath, { recursive: true });
  }

  /**
   * Create the root package.json for the workspace
   */
  private createRootPackageJson(root: string): void {
    const packageJson = {
      name: '@carrick/workspace',
      version: '0.0.0',
      private: true,
      description: 'Synthetic workspace for Carrick type compatibility checking',
      workspaces: [
        'packages/*',
        'checker',
      ],
    };

    const filePath = path.join(root, 'package.json');
    fs.writeFileSync(filePath, JSON.stringify(packageJson, null, 2), 'utf-8');
    this.log(`Created: ${filePath}`);
  }

  /**
   * Create pnpm-workspace.yaml
   */
  private createPnpmWorkspaceYaml(root: string): void {
    const content = `# Carrick synthetic workspace
packages:
  - 'packages/*'
  - 'checker'
`;

    const filePath = path.join(root, 'pnpm-workspace.yaml');
    fs.writeFileSync(filePath, content, 'utf-8');
    this.log(`Created: ${filePath}`);
  }

  /**
   * Create .npmrc to prevent hoisting that would blur package boundaries
   */
  private createNpmrc(root: string): void {
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

  /**
   * Create a stub package for a repository
   *
   * @returns Path to the created package
   */
  private createStubPackage(root: string, repo: RepoMetadata): string {
    const packageDir = path.join(root, 'packages', repo.repoName);
    const srcDir = path.join(packageDir, 'src');

    // Create directories
    fs.mkdirSync(srcDir, { recursive: true });

    // Create package.json from pinned dependencies
    this.createStubPackageJson(packageDir, repo);

    // Create tsconfig.json from snapshot
    this.createStubTsconfig(packageDir, repo);

    // Create surface.d.ts if provided
    if (repo.surfaceContent) {
      this.createSurfaceFile(srcDir, repo.surfaceContent);
    } else {
      // Create placeholder
      this.createPlaceholderSurface(srcDir, repo.repoName);
    }

    this.log(`Created stub package: ${packageDir}`);
    return packageDir;
  }

  /**
   * Create package.json for a stub package
   */
  private createStubPackageJson(packageDir: string, repo: RepoMetadata): void {
    const packageJson: Record<string, unknown> = {
      name: `@carrick/${repo.repoName}`,
      version: '0.0.0',
      private: true,
      description: `Stub package for ${repo.repoName} type checking`,
      main: './src/surface.d.ts',
      types: './src/surface.d.ts',
      dependencies: {},
    };

    // Add pinned dependencies
    if (repo.dependencies) {
      packageJson.dependencies = { ...repo.dependencies };
    }

    const filePath = path.join(packageDir, 'package.json');
    fs.writeFileSync(filePath, JSON.stringify(packageJson, null, 2), 'utf-8');
  }

  /**
   * Create tsconfig.json for a stub package from the snapshot
   */
  private createStubTsconfig(packageDir: string, repo: RepoMetadata): void {
    const snapshot = repo.tsconfig || this.getDefaultTsconfigSnapshot();

    const tsconfig = {
      compilerOptions: {
        ...snapshot.compilerOptions,
        // Override for surface checking
        declaration: true,
        declarationMap: false,
        emitDeclarationOnly: true,
        noEmit: false,
        skipLibCheck: true,
        // Ensure we can import the surface
        rootDir: './src',
        outDir: './dist',
      },
      include: ['src/**/*'],
      exclude: ['node_modules'],
    };

    const filePath = path.join(packageDir, 'tsconfig.json');
    fs.writeFileSync(filePath, JSON.stringify(tsconfig, null, 2), 'utf-8');
  }

  /**
   * Create the surface.d.ts file
   */
  private createSurfaceFile(srcDir: string, content: string): void {
    const filePath = path.join(srcDir, 'surface.d.ts');
    fs.writeFileSync(filePath, content, 'utf-8');
  }

  /**
   * Create a placeholder surface file
   */
  private createPlaceholderSurface(srcDir: string, repoName: string): void {
    const content = `// Placeholder surface for ${repoName}
// This file should be replaced with actual extracted types

export type Placeholder = unknown;
`;

    const filePath = path.join(srcDir, 'surface.d.ts');
    fs.writeFileSync(filePath, content, 'utf-8');
  }

  /**
   * Get default tsconfig snapshot
   */
  private getDefaultTsconfigSnapshot(): TsconfigSnapshot {
    return {
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
  }

  // ===========================================================================
  // Checker Package Creation
  // ===========================================================================

  /**
   * Create the checker package that runs type compatibility checks
   *
   * @returns Path to the checker package
   */
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

  /**
   * Create package.json for the checker package
   */
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
   * Create tsconfig.json for the checker package with path mappings
   */
  private createCheckerTsconfig(checkerDir: string, repos: RepoMetadata[]): void {
    const paths: Record<string, string[]> = {};

    for (const repo of repos) {
      const repoName = repo.repoName;

      // Surface import: @carrick/{repoName}/surface -> packages/{repoName}/src/surface.d.ts
      paths[`@carrick/${repoName}/surface`] = [
        `../packages/${repoName}/src/surface.d.ts`,
      ];

      // Unscoped packages: @carrick/{repoName}/* -> packages/{repoName}/node_modules/*
      paths[`@carrick/${repoName}/*`] = [
        `../packages/${repoName}/node_modules/*`,
        `../packages/${repoName}/node_modules/*/index.d.ts`,
        `../packages/${repoName}/node_modules/@types/*`,
      ];

      // Scoped packages: @carrick/{repoName}/@*/* -> packages/{repoName}/node_modules/@*/*
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

  /**
   * Create initial checks.ts file
   */
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

  /**
   * Install dependencies using pnpm
   */
  private installDependencies(root: string): void {
    this.log('Installing dependencies with pnpm...');

    try {
      // Check if pnpm is available
      execSync('pnpm --version', { stdio: 'pipe' });
    } catch {
      this.log('pnpm not found, trying npm...');
      this.installWithNpm(root);
      return;
    }

    try {
      execSync('pnpm install --frozen-lockfile=false', {
        cwd: root,
        stdio: 'pipe',
        env: { ...process.env, CI: 'false' },
      });
      this.log('Dependencies installed successfully');
    } catch {
      // If frozen lockfile fails, try without it
      try {
        execSync('pnpm install', {
          cwd: root,
          stdio: 'pipe',
          env: { ...process.env, CI: 'false' },
        });
        this.log('Dependencies installed successfully');
      } catch (installErr) {
        const error = installErr instanceof Error ? installErr.message : String(installErr);
        throw new Error(`Failed to install dependencies: ${error}`);
      }
    }
  }

  /**
   * Fallback: Install dependencies using npm
   */
  private installWithNpm(root: string): void {
    try {
      // Remove pnpm-workspace.yaml since npm doesn't understand it
      const workspaceFile = path.join(root, 'pnpm-workspace.yaml');
      if (fs.existsSync(workspaceFile)) {
        fs.unlinkSync(workspaceFile);
      }

      execSync('npm install', {
        cwd: root,
        stdio: 'pipe',
        env: { ...process.env, CI: 'false' },
      });
      this.log('Dependencies installed successfully with npm');
    } catch (err) {
      const error = err instanceof Error ? err.message : String(err);
      throw new Error(`Failed to install dependencies with npm: ${error}`);
    }
  }

  // ===========================================================================
  // Compatibility Checking
  // ===========================================================================

  /**
   * Generate the checks.ts file with type assertions
   */
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
      if (!repoImports.has(check.source_repo)) {
        repoImports.set(check.source_repo, new Set());
      }
      repoImports.get(check.source_repo)!.add(check.source_alias);

      if (!repoImports.has(check.target_repo)) {
        repoImports.set(check.target_repo, new Set());
      }
      repoImports.get(check.target_repo)!.add(check.target_alias);
    }

    for (const [repo, aliases] of repoImports) {
      const aliasesArray = Array.from(aliases);
      // Create unique import names to avoid conflicts
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
      const sourceType = this.makeImportName(check.source_repo, check.source_alias);
      const targetType = this.makeImportName(check.target_repo, check.target_alias);
      const checkName = `_check${i}`;

      lines.push('');
      lines.push(`// Check: ${check.source_repo}/${check.source_alias} vs ${check.target_repo}/${check.target_alias}`);

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

  /**
   * Create a unique import name for a repo/alias combination
   */
  private makeImportName(repo: string, alias: string): string {
    const safeRepo = repo.replace(/[^a-zA-Z0-9]/g, '_');
    const safeAlias = alias.replace(/[^a-zA-Z0-9]/g, '_');
    return `${safeRepo}_${safeAlias}`;
  }

  /**
   * Run TypeScript type checking on the checker package
   *
   * @returns Object with success status and any error output
   */
  private runTypeCheck(checkerDir: string): { success: boolean; output: string } {
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

  /**
   * Parse the result of a type check for a specific compatibility check
   */
  private parseCheckResult(
    check: CompatibilityCheck,
    tscResult: { success: boolean; output: string }
  ): CompatibilityResult {
    // If tsc succeeded, all checks passed
    if (tscResult.success) {
      return {
        source_repo: check.source_repo,
        source_alias: check.source_alias,
        target_repo: check.target_repo,
        target_alias: check.target_alias,
        compatible: true,
      };
    }

    // Look for errors related to this specific check
    const escSrc = check.source_alias.replace(/[.*+?^${}()|[\]\\]/g, '\\$&');
    const escTgt = check.target_alias.replace(/[.*+?^${}()|[\]\\]/g, '\\$&');
    const checkPattern = new RegExp(
      `${escSrc}.*${escTgt}|${escTgt}.*${escSrc}`,
      'i'
    );

    const isRelated = checkPattern.test(tscResult.output);

    // If tsc failed but the error doesn't mention our aliases, treat as
    // incompatible rather than silently reporting compatible (the check
    // was not actually validated).
    return {
      source_repo: check.source_repo,
      source_alias: check.source_alias,
      target_repo: check.target_repo,
      target_alias: check.target_alias,
      compatible: false,
      diagnostic: tscResult.output,
    };
  }

  // ===========================================================================
  // Logging
  // ===========================================================================

  private log(message: string): void {
    console.error(`[sidecar:monorepo-builder] ${message}`);
  }

  private logError(message: string): void {
    console.error(`[sidecar:monorepo-builder:error] ${message}`);
  }
}
