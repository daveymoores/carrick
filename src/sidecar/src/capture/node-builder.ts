/**
 * SymbolTracker-backed node-builder printing for anonymous inferred types
 * (design doc, Capture step 5). This is WP1's risk concentration, built
 * failure-visibility-first: any tracked symbol that is not plainly
 * accessible from the destination file demotes the alias to the
 * structural_fallback tier with a recorded reason -- the bundle never ships
 * a silently wrong .d.ts.
 *
 * The four banked corrections from the 2026-07-02 derisk sweep, applied
 * verbatim:
 *  1. enclosingDeclaration anchors in the DESTINATION surface file (the
 *     entry's placeholder alias for this anchor), never undefined.
 *  2. Demotion triggers on accessibility !== Accessible -- that includes
 *     CannotBeNamed (2), not just NotAccessible (1).
 *  3. Detection drives off the trackSymbol callback; the
 *     reportInaccessibleUniqueSymbolError / reportInaccessibleThisError
 *     callbacks never fire for these shapes and are implemented only as
 *     belt-and-braces recorders.
 *  4. The tracker is passed as the 5th argument of the (internal)
 *     typeToTypeNode signature: (type, enclosingDeclaration, flags,
 *     internalFlags, tracker).
 */

import ts from 'typescript';

export interface NodeBuilderPrintResult {
  /** Printed type text, present only when the print is trusted. */
  text?: string;
  /** Symbols the tracker flagged as not accessible from the destination. */
  inaccessible: string[];
  /** Failure description when text is absent. */
  failure?: string;
}

/**
 * SymbolAccessibility is internal to the compiler (stable since TS 1.x):
 * Accessible = 0, NotAccessible = 1, CannotBeNamed = 2. Mirrored here
 * because the public .d.ts does not export it; correction 2 depends on the
 * distinction between the two failure members.
 */
const ACCESSIBLE = 0;

interface InternalTypeChecker extends ts.TypeChecker {
  // Internal APIs: isSymbolAccessible is not in the public TypeChecker
  // surface, and the public typeToTypeNode overload has no tracker
  // parameter at all -- the tracker must ride the 5th slot (correction 4).
  isSymbolAccessible(
    symbol: ts.Symbol,
    enclosingDeclaration: ts.Node | undefined,
    meaning: ts.SymbolFlags,
    shouldComputeAliasesToMakeVisible: boolean
  ): { accessibility: number };
  typeToTypeNode(
    type: ts.Type,
    enclosingDeclaration: ts.Node | undefined,
    flags: ts.NodeBuilderFlags | undefined,
    internalFlags?: number,
    tracker?: unknown
  ): ts.TypeNode | undefined;
}

/**
 * Print `type` as a type node anchored at `destination` (a declaration inside
 * the surface entry file). Returns untrusted-failure instead of text whenever
 * any referenced symbol is not plainly accessible from the destination.
 */
export function printTypeForDestination(
  checker: ts.TypeChecker,
  type: ts.Type,
  destination: ts.Node
): NodeBuilderPrintResult {
  const inaccessible: string[] = [];
  const seen = new Set<ts.Symbol>();

  const record = (symbol: ts.Symbol, meaning: ts.SymbolFlags | undefined) => {
    if (seen.has(symbol)) return;
    seen.add(symbol);
    const accessibility = (checker as InternalTypeChecker).isSymbolAccessible(
      symbol,
      destination, // correction 1: destination file, never undefined
      meaning ?? ts.SymbolFlags.Type,
      /* shouldComputeAliasesToMakeVisible */ false
    ).accessibility;
    // Correction 2: anything other than Accessible demotes -- CannotBeNamed
    // (2) is a distinct enum member that a `=== NotAccessible` check misses.
    if (accessibility !== ACCESSIBLE) {
      inaccessible.push(symbol.getName());
    }
  };

  // Correction 3: trackSymbol is the callback that actually fires. The
  // report* callbacks are dead for the verified failure shapes (unexported
  // local interfaces, unique-symbol keys, local recursive aliases) but are
  // kept as recorders in case other shapes reach them.
  const tracker = {
    trackSymbol: (symbol: ts.Symbol, enclosing: ts.Node | undefined, meaning: ts.SymbolFlags) => {
      void enclosing;
      record(symbol, meaning);
      // Returning false tells the builder no error was reported here; the
      // demotion decision is ours, made after the walk completes.
      return false;
    },
    reportInaccessibleThisError: () => {
      inaccessible.push('this');
    },
    reportInaccessibleUniqueSymbolError: () => {
      inaccessible.push('(unique symbol)');
    },
    reportPrivateInBaseOfClassExpression: (propertyName: string) => {
      inaccessible.push(propertyName);
    },
  };

  let node: ts.TypeNode | undefined;
  try {
    node = (checker as InternalTypeChecker).typeToTypeNode(
      type,
      destination,
      ts.NodeBuilderFlags.NoTruncation |
        ts.NodeBuilderFlags.UseStructuralFallback |
        ts.NodeBuilderFlags.InTypeAlias,
      /* internalFlags */ undefined,
      tracker // correction 4: 5th argument
    );
  } catch (err) {
    return {
      inaccessible,
      failure: `node builder threw: ${err instanceof Error ? err.message : String(err)}`,
    };
  }

  if (!node) {
    return { inaccessible, failure: 'node builder returned no type node' };
  }
  if (inaccessible.length > 0) {
    return {
      inaccessible,
      failure: `symbols not accessible from the surface entry: ${[...new Set(inaccessible)].join(', ')}`,
    };
  }

  const printer = ts.createPrinter({ removeComments: true });
  const text = printer.printNode(
    ts.EmitHint.Unspecified,
    node,
    destination.getSourceFile()
  );
  return { text, inaccessible };
}
