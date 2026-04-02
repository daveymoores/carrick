/**
 * DefinitionResolver - Resolves type aliases from bundled .d.ts content
 * using the TypeScript compiler to produce fully expanded type definitions.
 *
 * Instead of walking AST nodes or regex-extracting types, we ask the compiler
 * "what is this type?" and get the fully resolved answer.
 */

import { Project, type SourceFile, ts } from 'ts-morph';

export interface ResolvedDefinition {
  type_alias: string;
  /** Original declaration text as written (preserves named types) */
  definition: string;
  /** Compiler-expanded form with all types fully inlined */
  expanded: string;
}

export class DefinitionResolver {
  private readonly project: Project;

  constructor(options: { project: Project }) {
    this.project = options.project;
  }

  /**
   * Resolve multiple type aliases from a bundled .d.ts string.
   * Returns both the original declaration and the compiler-expanded form.
   */
  resolve(bundledDts: string, aliases: string[]): ResolvedDefinition[] {
    const tempFileName = `__carrick_resolve_${Date.now()}.d.ts`;
    let tempFile: SourceFile | undefined;

    try {
      tempFile = this.project.createSourceFile(tempFileName, bundledDts, {
        overwrite: true,
      });

      const results: ResolvedDefinition[] = [];

      for (const alias of aliases) {
        const result = this.resolveAlias(tempFile, alias);
        if (result) {
          results.push(result);
        } else {
          this.log(`Could not resolve alias: ${alias}`);
        }
      }

      return results;
    } catch (err) {
      this.logError(
        `Resolution failed: ${err instanceof Error ? err.message : String(err)}`,
      );
      return [];
    } finally {
      if (tempFile) {
        this.project.removeSourceFile(tempFile);
      }
    }
  }

  /**
   * Resolve a single alias: get the original text and the compiler-expanded form.
   */
  private resolveAlias(
    sourceFile: SourceFile,
    alias: string,
  ): ResolvedDefinition | null {
    const decl =
      sourceFile.getTypeAlias(alias) ??
      sourceFile.getInterface(alias) ??
      sourceFile.getClass(alias) ??
      sourceFile.getEnum(alias);

    if (!decl) return null;

    try {
      // Original declaration text as written
      const definition = decl.getText();

      // Compiler-expanded form — fully resolved, no truncation
      const type = decl.getType();
      const expanded = type.getText(
        decl,
        ts.TypeFormatFlags.NoTruncation | ts.TypeFormatFlags.InTypeAlias,
      );

      return { type_alias: alias, definition, expanded };
    } catch (err) {
      this.logError(
        `Failed to resolve ${alias}: ${err instanceof Error ? err.message : String(err)}`,
      );
      return null;
    }
  }

  private log(message: string): void {
    console.error(`[sidecar:definition-resolver] ${message}`);
  }

  private logError(message: string): void {
    console.error(`[sidecar:definition-resolver:error] ${message}`);
  }
}
