/**
 * Framework-machinery detection for the v2 capture path (carrick#371).
 *
 * A producer response anchor whose resolved type IS or CONTAINS HTTP transport
 * machinery — a fetch/DOM `Response`/`Request`, a Node `http.ServerResponse`, a
 * wrapper envelope `{ response: Response; error?: undefined } | { ...; error }`,
 * or a wrapper function `(req: Request) => Promise<Response>` — must never be
 * emitted as a comparable contract. Comparing it against the consumer's real
 * payload manufactures a false compat mismatch (the bug this module fixes).
 *
 * This is the raw-`ts` seam mirror of `type-inferrer.ts`'s
 * `typeIsOrContainsResponseMachinery` (ts-morph): the capture/ seam forbids
 * importing a module from outside it, so the canonical indicator set and the
 * detection shape are duplicated here in lockstep — the same pattern by which
 * `BUILTIN_ANCHOR_SYMBOLS` mirrors `socket_io.rs`. Detection is STRUCTURAL and
 * framework-agnostic: no framework NAME appears, only the shared HTTP-message
 * member surface, gated by a lib / `node_modules` declaration origin so a user
 * payload that merely shares a member name can never trip it.
 */

import ts from 'typescript';

/**
 * Strongly-discriminating member names of HTTP transport machinery. Kept
 * identical to `MACHINERY_MEMBER_INDICATORS` in `type-inferrer.ts`. These are
 * the names no JSON payload carries (`ok`, `redirected`, `bodyUsed`,
 * `writeHead`, ...), so the origin gate + threshold never fire on real data.
 * Exported so a drift-guard test (`machinery-indicator-mirror.test.ts`) asserts
 * it stays equal to the `type-inferrer.ts` copy — nothing else enforces the
 * lockstep, and if the two drift one path silently stops abstaining.
 */
export const MACHINERY_MEMBER_INDICATORS = new Set<string>([
  // fetch / DOM Response & Request body-consumer surface
  'ok',
  'redirected',
  'bodyUsed',
  'arrayBuffer',
  'blob',
  'formData',
  'clone',
  'json',
  'statusText',
  // Node http ServerResponse / reply-object surface
  'statusCode',
  'statusMessage',
  'setHeader',
  'getHeader',
  'removeHeader',
  'writeHead',
  'flushHeaders',
]);

/** Machinery needs at least this many indicator members to be recognized. */
const MACHINERY_INDICATOR_THRESHOLD = 3;

/**
 * True when `type`, resolved against `node`, IS or CONTAINS framework
 * machinery.
 *
 * DETECTS, exactly:
 *   1. the type itself is machinery (`isFrameworkMachinery`);
 *   2. a union/intersection member is machinery (the envelope union);
 *   3. a DIRECT property whose type is machinery (`{ response: Response }`),
 *      ONE level of descent only;
 *   4. a CALL SIGNATURE whose AWAITED return type is machinery — the wrapper
 *      function `(req) => Promise<Response>` the Infer fallback resolves to.
 *      (Only the call return is awaited; property types below are not.)
 *
 * DELIBERATELY NOT DETECTED — stated so this comment never overstates the
 * guarantee (mirrors the same list in `type-inferrer.ts`). Each is a
 * non-regression (pre-existing verdict unchanged, never a new wrong one), a
 * tracked follow-up:
 *   - machinery nested deeper than one property level;
 *   - a PROPERTY typed `Promise<Response>` (property types are not awaited /
 *     Promise-unwrapped before the check — only call-signature returns are);
 *   - an array element type: `Response[]` is not descended to its element;
 *   - `interface X extends Response` declared in USER source — the origin gate
 *     is lib/`node_modules` only, so a user-declared subtype reads as a real
 *     contract.
 *
 * The depth cap + origin gate keep a legitimate payload that merely references a
 * machinery type far inside from over-abstaining.
 */
export function typeIsOrContainsMachinery(
  checker: ts.TypeChecker,
  type: ts.Type,
  node: ts.Node
): boolean {
  return isOrContains(checker, type, node, 0);
}

function isOrContains(
  checker: ts.TypeChecker,
  type: ts.Type,
  node: ts.Node,
  depth: number
): boolean {
  if (isFrameworkMachinery(checker, type)) {
    return true;
  }

  if (type.isUnion() || type.isIntersection()) {
    return (type as ts.UnionOrIntersectionType).types.some((part) =>
      isOrContains(checker, part, node, depth)
    );
  }

  if (depth >= 1) {
    return false;
  }

  // A wrapper function value: descend into its (awaited) return type.
  for (const sig of checker.getSignaturesOfType(type, ts.SignatureKind.Call)) {
    const returnType = sig.getReturnType();
    const awaited = checker.getAwaitedType?.(returnType) ?? returnType;
    if (isOrContains(checker, awaited, node, depth + 1)) {
      return true;
    }
  }

  // Direct properties: the `{ response: Response; error }` envelope shape.
  for (const prop of checker.getPropertiesOfType(type)) {
    const propType = checker.getTypeOfSymbolAtLocation(prop, node);
    if (isOrContains(checker, propType, node, depth + 1)) {
      return true;
    }
  }

  return false;
}

/**
 * True when `type` itself is an HTTP-machinery type: it structurally carries at
 * least `MACHINERY_INDICATOR_THRESHOLD` of the indicator members AND its symbol
 * is declared in a lib (`lib.*.d.ts`) or `node_modules` origin.
 */
function isFrameworkMachinery(checker: ts.TypeChecker, type: ts.Type): boolean {
  let hits = 0;
  for (const prop of checker.getPropertiesOfType(type)) {
    if (MACHINERY_MEMBER_INDICATORS.has(prop.getName())) {
      hits += 1;
      if (hits >= MACHINERY_INDICATOR_THRESHOLD) break;
    }
  }
  if (hits < MACHINERY_INDICATOR_THRESHOLD) {
    return false;
  }
  return symbolIsLibOrExternalOrigin(checker, type.getSymbol() ?? type.aliasSymbol);
}

/**
 * True when the symbol is declared in a TS lib file (`lib.*.d.ts`) or under
 * `node_modules`. Works on a bare checkout: the DOM `Response`/`Request`
 * resolve from the bundled `lib.dom.d.ts` even with no installed dependencies.
 */
function symbolIsLibOrExternalOrigin(
  checker: ts.TypeChecker,
  symbol: ts.Symbol | undefined
): boolean {
  if (!symbol) {
    return false;
  }
  const isExternalPath = (filePath: string): boolean => {
    const normalized = filePath.replace(/\\/g, '/');
    return (
      normalized.includes('/node_modules/') ||
      /\/lib\.[^/]*\.d\.ts$/.test(normalized)
    );
  };
  for (const decl of symbol.getDeclarations() ?? []) {
    if (isExternalPath(decl.getSourceFile().fileName)) {
      return true;
    }
  }
  if (symbol.flags & ts.SymbolFlags.Alias) {
    try {
      const aliased = checker.getAliasedSymbol(symbol);
      if (aliased && aliased !== symbol) {
        return symbolIsLibOrExternalOrigin(checker, aliased);
      }
    } catch {
      // Ignore errors when resolving the aliased symbol.
    }
  }
  return false;
}
