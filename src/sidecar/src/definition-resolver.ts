/**
 * DefinitionResolver - Resolves type aliases from bundled .d.ts content
 * using the TypeScript compiler.
 *
 * Two forms are produced per alias:
 *  - `definition`: the declaration *as written* (named refs preserved).
 *  - `expanded`: the fully *structural* form, with every named member type
 *    inlined to its member structure, recursively.
 *
 * `type.getText(node, NoTruncation)` does NOT inline named members — the
 * compiler prints a referenced type by its symbol name when that symbol is in
 * scope (`total: Money`, not `total: { amountCents: number; currency: string }`).
 * The structural form is produced by `expandTypeStructural` (shared with the
 * inference path in `type-inferrer.ts`), which walks the resolved `Type` and
 * rebuilds the inlined text.
 */

import { Project, type SourceFile } from 'ts-morph';
import { expandTypeStructural } from './type-structural-expander.js';

export interface ResolvedDefinition {
  type_alias: string;
  /** Original declaration text as written (preserves named types) */
  definition: string;
  /** Fully structural form: named member types inlined to their structure */
  expanded: string;
}

export class DefinitionResolver {
  private readonly project: Project;

  constructor(options: { project: Project }) {
    this.project = options.project;
  }

  /**
   * Resolve multiple type aliases from a bundled .d.ts string.
   * Returns both the original declaration and the structural form.
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
   * Resolve a single alias: the original text and the structural form.
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
      // Original declaration text as written.
      const definition = decl.getText();

      // Structural form — every named member inlined to its shape.
      const expanded = expandTypeStructural(decl.getType());

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
