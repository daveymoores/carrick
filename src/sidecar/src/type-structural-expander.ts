/**
 * Structural type expansion — renders a ts-morph `Type` as fully-inlined
 * structural text, with every named object/interface member expanded to its
 * member structure recursively.
 *
 * `Type.getText()` does NOT inline named members: the compiler prints a
 * referenced type by its symbol name when that symbol is in scope
 * (`total: Money`, not `total: { amountCents: number; currency: string }`).
 * That is fine inside a single project, but a cross-repo bundle carries only
 * the alias lines — no source declarations — so a named reference is a
 * dangling identifier that resolves to `any` downstream. Expanding the shape
 * structurally puts the real members in the bundle so the type checker can
 * compare them.
 *
 * Object/interface types are expanded to their members; primitives, literals,
 * library types (`Date`, `Promise`, tuples, …) and functions stay by name.
 * Bounded recursion + a per-branch cycle set guard against blow-ups; any type
 * that can't be safely expanded falls back to the non-expanded text rather
 * than throwing.
 *
 * Shared by `definition-resolver.ts` (bundle alias resolution) and
 * `type-inferrer.ts` (consumer-side inference), so both paths emit the same
 * structural form rather than a dangling name.
 */

import { type Symbol, type Type, ts } from 'ts-morph';

/**
 * Bound on the structural-expansion recursion. Deep enough for every realistic
 * request/response shape; a backstop against pathological/recursive types the
 * cycle set somehow misses.
 */
export const MAX_EXPANSION_DEPTH = 12;

/**
 * Recursively render a `Type` as fully-inlined structural text.
 *
 * Named object/interface types are expanded to their member structure;
 * primitives, literals, library types (`Date`, `Promise`, tuples, …) and
 * functions stay by name. The `seen` set (object type ids on the current
 * branch) breaks reference cycles; `depth` is a hard backstop.
 */
export function expandTypeStructural(
  type: Type,
  seen: Set<number> = new Set(),
  depth = 0,
): string {
  if (depth > MAX_EXPANSION_DEPTH) return namedText(type);

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
    return namedText(type);
  }

  // Unions / intersections: expand each member.
  if (type.isUnion()) {
    return type
      .getUnionTypes()
      .map((member) => expandTypeStructural(member, seen, depth + 1))
      .join(' | ');
  }
  if (type.isIntersection()) {
    return type
      .getIntersectionTypes()
      .map((member) => expandTypeStructural(member, seen, depth + 1))
      .join(' & ');
  }

  // Tuples are array-like but must keep their `[a, b]` shape, not be walked
  // as objects (which explodes into `Array.prototype`). Handle before arrays.
  if (isTuple(type)) {
    return namedText(type);
  }

  if (type.isArray()) {
    const element = type.getArrayElementType();
    if (!element) return namedText(type);
    const inner = expandTypeStructural(element, seen, depth + 1);
    // Parenthesise unions/intersections so `(A | B)[]` doesn't read as
    // `A | B[]`; object literals already self-delimit with braces.
    const needsParens = /[|&]/.test(inner) && !inner.startsWith('{');
    return needsParens ? `(${inner})[]` : `${inner}[]`;
  }

  // Library / built-in types (Date, Promise, RegExp, …): keep by name.
  if (isLibraryType(type)) {
    return namedText(type);
  }

  // Callable/constructable object types (functions): keep by name; their
  // structural form is the signature, which `getText` already renders.
  if (
    type.getCallSignatures().length > 0 ||
    type.getConstructSignatures().length > 0
  ) {
    return namedText(type);
  }

  if (type.isObject() || type.isInterface()) {
    const id = (type.compilerType as { id?: number }).id;
    if (id != null && seen.has(id)) return namedText(type);
    const nextSeen = id != null ? new Set(seen).add(id) : seen;

    const props = type.getProperties();
    if (props.length === 0) return namedText(type);

    const parts = props.map((prop) => expandProperty(prop, nextSeen, depth));
    return `{ ${parts.join('; ')}; }`;
  }

  return namedText(type);
}

/** Render a single property as `name[?]: <expanded>`. */
function expandProperty(prop: Symbol, seen: Set<number>, depth: number): string {
  const optional = (prop.getFlags() & ts.SymbolFlags.Optional) !== 0;

  const propDecl = prop.getDeclarations()[0];
  // Render the key from the declaration's name node so quoted/computed keys
  // ('x-y', "x y", [Symbol.iterator]) survive as valid TS text rather than
  // being unquoted into invalid output; fall back to the bare symbol name.
  const name = renderPropertyName(prop, propDecl);
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
        .map((member) => expandTypeStructural(member, seen, depth + 1))
        .join(' | ');
      return `${name}?: ${inner}`;
    }
  }

  const inner = expandTypeStructural(propType, seen, depth + 1);
  return `${name}${optional ? '?' : ''}: ${inner}`;
}

/**
 * The property key as valid TS text. Uses the declaration's name node so a
 * quoted (`'x-y'`) or computed (`[Symbol.iterator]`) key keeps its syntax;
 * `Symbol.getName()` would drop the quoting and emit invalid output. Falls
 * back to the bare symbol name when there's no usable name node.
 */
function renderPropertyName(prop: Symbol, decl: unknown): string {
  const node = decl as
    | { getNameNode?: () => { getText(): string } | undefined }
    | undefined;
  const text = node?.getNameNode?.()?.getText();
  return text && text.length > 0 ? text : prop.getName();
}

/** True for tuple types (`[a, b]`), which must not be walked as objects. */
function isTuple(type: Type): boolean {
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
function isLibraryType(type: Type): boolean {
  const symbol = type.getSymbol() ?? type.getAliasSymbol();
  if (!symbol) return false;
  const decls = symbol.getDeclarations();
  if (decls.length === 0) return false;
  return decls.some((decl) => {
    const sf = decl.getSourceFile();
    if (sf.isInNodeModules()) return true;
    return (
      sf.isDeclarationFile() &&
      // Normalize separators so a Windows `\\` path still matches lib.*.d.ts.
      /(^|\/)lib\.[^/]*\.d\.ts$/.test(sf.getFilePath().replace(/\\/g, '/'))
    );
  });
}

/**
 * Non-expanded text for a type. Passes `undefined` as the enclosing node so
 * the compiler can't throw on an invalid node context (tuples and some
 * generic instantiations do), falling back to the bare `getText()` and
 * finally to `unknown` so a single bad type never aborts the whole resolve.
 */
export function namedText(type: Type): string {
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
