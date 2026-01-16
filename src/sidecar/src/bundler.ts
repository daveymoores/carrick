/**
 * Type Bundler - Bundles TypeScript types using dts-bundle-generator
 *
 * This module creates bundled .d.ts files from specified symbols.
 * CRITICAL: Uses a PHYSICAL FILE strategy because dts-bundle-generator
 * requires physical files to resolve relative imports correctly.
 */

import { Project, type SourceFile } from 'ts-morph';
import { generateDtsBundle } from 'dts-bundle-generator';
import * as path from 'node:path';
import * as fs from 'node:fs';
import type {
  SymbolRequest,
  BundleResult,
  ManifestEntry,
  SymbolFailure,
} from './types.js';

/**
 * Virtual entrypoint filename - written to repo root
 */
const VIRTUAL_ENTRY_FILENAME = '.carrick_virtual_entry.ts';

/**
 * Options for TypeBundler construction
 */
export interface TypeBundlerOptions {
  /** The ts-morph Project instance */
  project: Project;
  /** The repository root directory (absolute path) */
  repoRoot: string;
}

/**
 * TypeBundler - Generates bundled .d.ts files for specified symbols
 *
 * Usage:
 *   const bundler = new TypeBundler({ project, repoRoot });
 *   const result = bundler.bundle(symbols);
 */
export class TypeBundler {
  private readonly project: Project;
  private readonly repoRoot: string;
  private readonly virtualEntryPath: string;

  constructor(options: TypeBundlerOptions) {
    this.project = options.project;
    this.repoRoot = options.repoRoot;
    this.virtualEntryPath = path.join(this.repoRoot, VIRTUAL_ENTRY_FILENAME);
  }

  /**
   * Bundle the requested symbols into a single .d.ts file
   *
   * @param symbols - Array of symbol requests to bundle
   * @returns BundleResult with the bundled content or errors
   */
  bundle(symbols: SymbolRequest[]): BundleResult {
    const manifest: ManifestEntry[] = [];
    const symbolFailures: SymbolFailure[] = [];
    const errors: string[] = [];

    // Phase 1: Validate all symbols exist
    const uniqueSymbols = this.dedupeSymbols(symbols);
    const validatedSymbols = this.validateSymbols(uniqueSymbols, symbolFailures);

    if (validatedSymbols.length === 0) {
      return {
        success: false,
        symbol_failures: symbolFailures,
        errors: symbolFailures.length > 0
          ? ['All requested symbols failed to resolve']
          : ['No symbols provided'],
      };
    }

    // Phase 2: Generate virtual entrypoint content
    const entryContent = this.generateVirtualEntrypoint(validatedSymbols);

    try {
      // Phase 3: Write physical file (CRITICAL for relative import resolution)
      this.writeVirtualEntry(entryContent);

      // Phase 4: Add to ts-morph project
      const entrySourceFile = this.project.addSourceFileAtPath(this.virtualEntryPath);

      try {
        // Phase 5: Generate bundled .d.ts using dts-bundle-generator
        const dtsContent = this.generateBundle();

        // Phase 6: Build manifest entries
        for (const symbol of validatedSymbols) {
          const typeString = this.extractTypeFromDts(
            dtsContent,
            symbol.alias || symbol.symbol_name
          );

          manifest.push({
            alias: symbol.alias || symbol.symbol_name,
            original_name: symbol.symbol_name,
            source_file: symbol.source_file,
            type_string: typeString || `/* Type ${symbol.symbol_name} */`,
            is_explicit: true,
          });
        }

        return {
          success: true,
          dts_content: dtsContent,
          manifest,
          symbol_failures: symbolFailures.length > 0 ? symbolFailures : undefined,
        };
      } finally {
        // Phase 7: Remove from ts-morph project
        this.project.removeSourceFile(entrySourceFile);
      }
    } catch (err) {
      const error = err instanceof Error ? err.message : String(err);
      this.logError(`Bundle generation failed: ${error}`);
      errors.push(error);

      return {
        success: false,
        symbol_failures: symbolFailures.length > 0 ? symbolFailures : undefined,
        errors,
      };
    } finally {
      // Phase 8: ALWAYS clean up physical file
      this.cleanupVirtualEntry();
    }
  }

  /**
   * Validate that all requested symbols exist in their source files
   *
   * @param symbols - Symbols to validate
   * @param failures - Array to collect failures
   * @returns Array of validated symbols
   */
  private validateSymbols(
    symbols: SymbolRequest[],
    failures: SymbolFailure[]
  ): SymbolRequest[] {
    const validated: SymbolRequest[] = [];

    for (const symbol of symbols) {
      const result = this.validateSymbol(symbol);

      if (result.valid) {
        validated.push(symbol);
      } else {
        failures.push({
          symbol_name: symbol.symbol_name,
          source_file: symbol.source_file,
          reason: result.reason || 'Unknown error',
        });
      }
    }

    return validated;
  }

  private dedupeSymbols(symbols: SymbolRequest[]): SymbolRequest[] {
    const seen = new Set<string>();
    const unique: SymbolRequest[] = [];

    for (const symbol of symbols) {
      const key = `${symbol.source_file}::${symbol.symbol_name}::${symbol.alias || ''}`;
      if (seen.has(key)) {
        continue;
      }
      seen.add(key);
      unique.push(symbol);
    }

    return unique;
  }

  /**
   * Validate a single symbol exists in its source file
   */
  private validateSymbol(
    symbol: SymbolRequest
  ): { valid: boolean; reason?: string } {
    if (this.isPlaceholderSource(symbol.source_file)) {
      return {
        valid: false,
        reason: `Unsupported source placeholder: ${symbol.source_file}`,
      };
    }

    if (!this.isPathSource(symbol.source_file)) {
      return { valid: true };
    }

    // Resolve the source file path
    const absolutePath = path.isAbsolute(symbol.source_file)
      ? symbol.source_file
      : path.resolve(this.repoRoot, symbol.source_file);

    // Check if file exists
    if (!fs.existsSync(absolutePath)) {
      return {
        valid: false,
        reason: `Source file not found: ${symbol.source_file}`,
      };
    }

    // Get or add the source file to the project
    let sourceFile: SourceFile | undefined;
    try {
      sourceFile =
        this.project.getSourceFile(absolutePath) ||
        this.project.addSourceFileAtPath(absolutePath);
    } catch (err) {
      return {
        valid: false,
        reason: `Failed to load source file: ${err instanceof Error ? err.message : String(err)}`,
      };
    }

    // Look for the symbol in the file
    const symbolName = symbol.symbol_name;

    // Check interfaces
    const iface = sourceFile.getInterface(symbolName);
    if (iface) return { valid: true };

    // Check type aliases
    const typeAlias = sourceFile.getTypeAlias(symbolName);
    if (typeAlias) return { valid: true };

    // Check classes
    const classDecl = sourceFile.getClass(symbolName);
    if (classDecl) return { valid: true };

    // Check enums
    const enumDecl = sourceFile.getEnum(symbolName);
    if (enumDecl) return { valid: true };

    // Check exported variables (for const type definitions)
    const varDecl = sourceFile.getVariableDeclaration(symbolName);
    if (varDecl) return { valid: true };

    // Check functions
    const funcDecl = sourceFile.getFunction(symbolName);
    if (funcDecl) return { valid: true };

    // Symbol not found
    return {
      valid: false,
      reason: `Symbol '${symbolName}' not found in ${symbol.source_file}`,
    };
  }

  /**
   * Generate the virtual entrypoint content with export statements
   */
  private generateVirtualEntrypoint(symbols: SymbolRequest[]): string {
    const lines: string[] = [
      '// Generated virtual entrypoint for dts-bundle-generator',
      '// This file is auto-generated and should be deleted after bundling',
      '',
    ];

    for (const symbol of symbols) {
      if (this.isPlaceholderSource(symbol.source_file)) {
        continue;
      }

      const importSource = this.isPathSource(symbol.source_file)
        ? this.toRelativeImportPath(symbol.source_file)
        : symbol.source_file;

      // Generate export statement
      if (symbol.alias && symbol.alias !== symbol.symbol_name) {
        lines.push(
          `export type { ${symbol.symbol_name} as ${symbol.alias} } from '${importSource}';`
        );
      } else {
        lines.push(
          `export type { ${symbol.symbol_name} } from '${importSource}';`
        );
      }
    }

    return lines.join('\n');
  }

  private isPathSource(sourceFile: string): boolean {
    if (
      sourceFile.startsWith('.') ||
      sourceFile.startsWith('/') ||
      path.isAbsolute(sourceFile)
    ) {
      return true;
    }

    const basePath = path.resolve(this.repoRoot, sourceFile);
    if (fs.existsSync(basePath)) {
      return true;
    }

    if (path.extname(sourceFile) !== '') {
      return false;
    }

    const extensions = ['.ts', '.tsx', '.js', '.jsx', '.d.ts'];
    return extensions.some((ext) => fs.existsSync(basePath + ext));
  }

  private isPlaceholderSource(sourceFile: string): boolean {
    const trimmed = sourceFile.trim();
    return trimmed === '' || trimmed === '%none%' || trimmed === '%inline%';
  }

  private toRelativeImportPath(sourceFile: string): string {
    const absolutePath = path.isAbsolute(sourceFile)
      ? sourceFile
      : path.resolve(this.repoRoot, sourceFile);

    let relativePath = path.relative(this.repoRoot, absolutePath);

    if (!relativePath.startsWith('.')) {
      relativePath = './' + relativePath;
    }

    return relativePath.replace(/\.tsx?$/, '');
  }

  /**
   * Write the virtual entrypoint to a physical file
   */
  private writeVirtualEntry(content: string): void {
    this.log(`Writing virtual entry to: ${this.virtualEntryPath}`);
    fs.writeFileSync(this.virtualEntryPath, content, 'utf-8');
  }

  /**
   * Clean up the virtual entrypoint file
   */
  private cleanupVirtualEntry(): void {
    try {
      if (fs.existsSync(this.virtualEntryPath)) {
        fs.unlinkSync(this.virtualEntryPath);
        this.log('Cleaned up virtual entry file');
      }
    } catch (err) {
      this.logError(
        `Failed to clean up virtual entry: ${err instanceof Error ? err.message : String(err)}`
      );
    }
  }

  /**
   * Generate the bundled .d.ts content using dts-bundle-generator
   */
  private generateBundle(): string {
    this.log('Generating .d.ts bundle');

    const result = generateDtsBundle([
      {
        filePath: this.virtualEntryPath,
        output: {
          noBanner: true,
          exportReferencedTypes: true,
        },
      },
    ]);

    if (!result || result.length === 0) {
      throw new Error('dts-bundle-generator returned empty result');
    }

    return result[0];
  }

  /**
   * Extract a specific type definition from the bundled .d.ts content
   *
   * @param dtsContent - The full bundled .d.ts content
   * @param typeName - The name of the type to extract
   * @returns The type definition string, or undefined if not found
   */
  private extractTypeFromDts(
    dtsContent: string,
    typeName: string
  ): string | undefined {
    // Match export interface, export type, or export class
    const patterns = [
      // Interface
      new RegExp(
        `export\\s+interface\\s+${this.escapeRegex(typeName)}\\s*\\{[^}]*\\}`,
        's'
      ),
      // Type alias (simple)
      new RegExp(
        `export\\s+type\\s+${this.escapeRegex(typeName)}\\s*=\\s*[^;]+;`,
        's'
      ),
      // Type alias (with generics)
      new RegExp(
        `export\\s+type\\s+${this.escapeRegex(typeName)}\\s*<[^>]*>\\s*=\\s*[^;]+;`,
        's'
      ),
      // Class
      new RegExp(
        `export\\s+(?:declare\\s+)?class\\s+${this.escapeRegex(typeName)}[^{]*\\{[^}]*\\}`,
        's'
      ),
      // Enum
      new RegExp(
        `export\\s+(?:declare\\s+)?(?:const\\s+)?enum\\s+${this.escapeRegex(typeName)}\\s*\\{[^}]*\\}`,
        's'
      ),
    ];

    for (const pattern of patterns) {
      const match = dtsContent.match(pattern);
      if (match) {
        return match[0];
      }
    }

    return undefined;
  }

  /**
   * Escape special regex characters in a string
   */
  private escapeRegex(str: string): string {
    return str.replace(/[.*+?^${}()|[\]\\]/g, '\\$&');
  }

  /**
   * Log a message to stderr
   */
  private log(message: string): void {
    console.error(`[sidecar:bundler] ${message}`);
  }

  /**
   * Log an error to stderr
   */
  private logError(message: string): void {
    console.error(`[sidecar:bundler:error] ${message}`);
  }
}
