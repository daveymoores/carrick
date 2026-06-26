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
 * To get the inlined form we walk the resolved `Type` ourselves and rebuild the
 * structural text, expanding named object/interface types into their members
 * while leaving primitives, literals and library types (e.g. `Date`, tuples) by
 * name. Bounded recursion + a per-branch cycle set guard against blow-ups; any
 * type that can't be safely expanded falls back to the non-expanded text rather
 * than throwing.
 */

import { Project, type SourceFile, type Symbol, type Type, ts } from 'ts-morph';

export interface ResolvedDefinition {
  type_alias: string;
  /** Original declaration text as written (preserves named types) */
  definition: string;
  /** Fully structural form: named member types inlined to their structure */
  expanded: string;
}

/**
 * Bound on the structural-expansion recursion. Deep enough for every realistic
 * request/response shape; a backstop against pathological/recursive types the
 * cycle set somehow misses.
 */
const MAX_EXPANSION_DEPTH = 12;

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
      const expanded = this.expandType(decl.getType(), new Set(), 0);

      return { type_alias: alias, definition, expanded };
    } catch (err) {
      this.logError(
        `Failed to resolve ${alias}: ${err instanceof Error ? err.message : String(err)}`,
      );
      return null;
    }
  }

  /**
   * Recursively render a `Type` as fully-inlined structural text.
   *
   * Named object/interface types are expanded to their member structure;
   * primitives, literals, library types (`Date`, `Promise`, tuples, …) and
   * functions stay by name. The `seen` set (object type ids on the current
   * branch) breaks reference cycles; `depth` is a hard backstop.
   */
  private expandType(type: Type, seen: Set<number>, depth: number): string {
    if (depth > MAX_EXPANSION_DEPTH) return this.namedText(type);

    // Primitives & literals: nothing to inline.
    if (
      type.isString() ||
      type.isNumber() ||
      type.isBoolean() ||
      type.isBooleanLiteral() ||
      type.isUndefined() ||
      type.isNull() ||
      type.isVoid() ||
      type.isAny() ||
      type.isUnknown() ||
      type.isNever() ||
      type.isStringLiteral() ||
      type.isNumberLiteral() ||
      type.isEnumLiteral()
    ) {
      return this.namedText(type);
    }

    // Unions / intersections: expand each member.
    if (type.isUnion()) {
      return type
        .getUnionTypes()
        .map((member) => this.expandType(member, seen, depth + 1))
        .join(' | ');
    }
    if (type.isIntersection()) {
      return type
        .getIntersectionTypes()
        .map((member) => this.expandType(member, seen, depth + 1))
        .join(' & ');
    }

    // Tuples are array-like but must keep their `[a, b]` shape, not be walked
    // as objects (which explodes into `Array.prototype`). Handle before arrays.
    if (this.isTuple(type)) {
      return this.namedText(type);
    }

    if (type.isArray()) {
      const element = type.getArrayElementType();
      if (!element) return this.namedText(type);
      const inner = this.expandType(element, seen, depth + 1);
      // Parenthesise unions/intersections so `(A | B)[]` doesn't read as
      // `A | B[]`; object literals already self-delimit with braces.
      const needsParens = /[|&]/.test(inner) && !inner.startsWith('{');
      return needsParens ? `(${inner})[]` : `${inner}[]`;
    }

    // Library / built-in types (Date, Promise, RegExp, …): keep by name.
    if (this.isLibraryType(type)) {
      return this.namedText(type);
    }

    // Callable/constructable object types (functions): keep by name; their
    // structural form is the signature, which `getText` already renders.
    if (
      type.getCallSignatures().length > 0 ||
      type.getConstructSignatures().length > 0
    ) {
      return this.namedText(type);
    }

    if (type.isObject() || type.isInterface()) {
      const id = (type.compilerType as { id?: number }).id;
      if (id != null && seen.has(id)) return this.namedText(type);
      const nextSeen = id != null ? new Set(seen).add(id) : seen;

      const props = type.getProperties();
      if (props.length === 0) return this.namedText(type);

      const parts = props.map((prop) =>
        this.expandProperty(prop, nextSeen, depth),
      );
      return `{ ${parts.join('; ')}; }`;
    }

    return this.namedText(type);
  }

  /** Render a single property as `name[?]: <expanded>`. */
  private expandProperty(
    prop: Symbol,
    seen: Set<number>,
    depth: number,
  ): string {
    const name = prop.getName();
    const optional = (prop.getFlags() & ts.SymbolFlags.Optional) !== 0;

    const propDecl = prop.getDeclarations()[0];
    let propType = propDecl
      ? prop.getTypeAtLocation(propDecl)
      : prop.getDeclaredType();

    // An optional property's type includes `undefined`; the structural label
    // drops it (`note?: string`, not `note?: string | undefined`).
    if (optional && propType.isUnion()) {
      const nonUndefined = propType
        .getUnionTypes()
        .filter((member) => !member.isUndefined());
      if (nonUndefined.length === 1) {
        propType = nonUndefined[0];
      } else if (nonUndefined.length > 1) {
        const inner = nonUndefined
          .map((member) => this.expandType(member, seen, depth + 1))
          .join(' | ');
        return `${name}?: ${inner}`;
      }
    }

    const inner = this.expandType(propType, seen, depth + 1);
    return `${name}${optional ? '?' : ''}: ${inner}`;
  }

  /** True for tuple types (`[a, b]`), which must not be walked as objects. */
  private isTuple(type: Type): boolean {
    const compiler = type.compilerType as {
      objectFlags?: number;
      target?: { objectFlags?: number };
    };
    const target = compiler.target ?? compiler;
    return ((target.objectFlags ?? 0) & ts.ObjectFlags.Tuple) !== 0;
  }

  /**
   * True for types declared in `node_modules` or a TS `lib.*.d.ts` (Date,
   * Promise, RegExp, …). These stay by name rather than being inlined.
   */
  private isLibraryType(type: Type): boolean {
    const symbol = type.getSymbol() ?? type.getAliasSymbol();
    if (!symbol) return false;
    const decls = symbol.getDeclarations();
    if (decls.length === 0) return false;
    return decls.some((decl) => {
      const sf = decl.getSourceFile();
      if (sf.isInNodeModules()) return true;
      return (
        sf.isDeclarationFile() && /(^|\/)lib\.[^/]*\.d\.ts$/.test(sf.getFilePath())
      );
    });
  }

  /**
   * Non-expanded text for a type. Passes `undefined` as the enclosing node so
   * the compiler can't throw on an invalid node context (tuples and some
   * generic instantiations do), falling back to the bare `getText()` and
   * finally to `unknown` so a single bad type never aborts the whole resolve.
   */
  private namedText(type: Type): string {
    try {
      return type.getText(
        undefined,
        ts.TypeFormatFlags.NoTruncation | ts.TypeFormatFlags.InTypeAlias,
      );
    } catch {
      try {
        return type.getText();
      } catch {
        return 'unknown';
      }
    }
  }

  private log(message: string): void {
    console.error(`[sidecar:definition-resolver] ${message}`);
  }

  private logError(message: string): void {
    console.error(`[sidecar:definition-resolver:error] ${message}`);
  }
}
