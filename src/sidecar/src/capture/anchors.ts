/**
 * Anchor resolution for the surface entry: guards for addressable anchors
 * (symbol / handler_return) and the locator + node-builder path for
 * anonymous inferred types. Produces the final alias line for each anchor,
 * or a recorded demotion -- never a silently wrong line.
 */

import ts from 'typescript';
import * as path from 'node:path';
import type { CaptureAnchorRequest, InferAnchorRequest } from './api.js';
import { printTypeForDestination } from './node-builder.js';

export interface ResolvedAnchor {
  request: CaptureAnchorRequest;
  /** RHS of `export type <alias> = ...;` in the surface entry. */
  aliasText: string;
  serialization: 'emitted' | 'node_builder' | 'structural_fallback';
  /** Present exactly for demotions (the alias line is `unknown`). */
  failureReason?: string;
}

/** Repo-root-relative source file -> extensionless specifier from entryDir. */
export function entryRelativeSpecifier(
  entryDir: string,
  repoRoot: string,
  sourceFile: string
): string {
  const target = path
    .join(repoRoot, sourceFile)
    .replace(/\.(ts|tsx|mts|cts)$/, '');
  let rel = path.relative(entryDir, target).split(path.sep).join('/');
  if (!rel.startsWith('.')) rel = `./${rel}`;
  return rel;
}

function moduleExport(
  checker: ts.TypeChecker,
  sourceFile: ts.SourceFile,
  name: string
): ts.Symbol | undefined {
  const moduleSymbol = checker.getSymbolAtLocation(sourceFile);
  if (!moduleSymbol) return undefined;
  return checker
    .getExportsOfModule(moduleSymbol)
    .find((s) => s.getName() === name);
}

/**
 * Resolve one anchor against the analysis program. `placeholder` is the
 * anchor's own placeholder alias declaration in the entry file -- the
 * destination node the node builder anchors to (banked correction 1).
 */
export function resolveAnchor(
  program: ts.Program,
  request: CaptureAnchorRequest,
  args: {
    repoRoot: string;
    entryDir: string;
    placeholder: ts.TypeAliasDeclaration | undefined;
    /**
     * symbol_name -> entry-relative specifier for every sibling `symbol`
     * anchor. A literal anchor whose text is a bare identifier naming one of
     * these resolves through that module instead of dangling in the entry.
     */
    siblingSymbolSpecs?: Map<string, string>;
  }
): ResolvedAnchor {
  const checker = program.getTypeChecker();
  const demote = (reason: string): ResolvedAnchor => ({
    request,
    aliasText: 'unknown',
    serialization: 'structural_fallback',
    failureReason: reason,
  });

  if (request.kind === 'literal') {
    const text = request.type_text.trim();
    if (!text) {
      return demote('empty literal type text');
    }
    const bareIdentifier = /^[A-Za-z_$][A-Za-z0-9_$]*$/.test(text);
    const siblingSpec = bareIdentifier
      ? args.siblingSymbolSpecs?.get(text)
      : undefined;
    return {
      request,
      aliasText: siblingSpec ? `import('${siblingSpec}').${text}` : text,
      // Literal anchors ARE the legacy-text tier (WP3 wiring of the design's
      // structural_fallback): hand-produced type text riding the surface.
      // The self-check still classifies decay; the fidelity metric counts
      // them at this tier so the legacy dependence stays measurable and
      // ratchetable. Demotions are distinguished by failureReason.
      serialization: 'structural_fallback',
    };
  }

  const sourceAbs = path.join(args.repoRoot, request.source_file);
  const sourceFile = program.getSourceFile(sourceAbs);
  if (!sourceFile) {
    return demote(`source file not in program: ${request.source_file}`);
  }
  const spec = entryRelativeSpecifier(args.entryDir, args.repoRoot, request.source_file);

  if (request.kind === 'symbol') {
    const exported = moduleExport(checker, sourceFile, request.symbol_name);
    if (!exported) {
      return demote(
        `'${request.symbol_name}' is not an export of ${request.source_file}`
      );
    }
    // The anchor is the ELEMENT symbol; array_depth restores the use-site's
    // `[]` levels (#248/#306) so an array-vs-scalar mismatch stays visible.
    const arraySuffix = '[]'.repeat(Math.max(0, request.array_depth ?? 0));
    return {
      request,
      aliasText: `import('${spec}').${request.symbol_name}${arraySuffix}`,
      serialization: 'emitted',
    };
  }

  if (request.kind === 'handler_return') {
    // Design-doc Capture step 1 guards, all three verified failure modes.
    const exported = moduleExport(checker, sourceFile, request.symbol_name);
    if (!exported) {
      return demote(
        `handler '${request.symbol_name}' is not an export of ${request.source_file}`
      );
    }
    const declaration = exported.valueDeclaration ?? exported.declarations?.[0];
    const handlerType = checker.getTypeOfSymbolAtLocation(
      exported,
      declaration ?? sourceFile
    );
    const callSignatures = handlerType.getCallSignatures();
    if (callSignatures.length === 0) {
      return demote(`handler '${request.symbol_name}' has no call signatures`);
    }
    if (callSignatures.length > 1) {
      // ReturnType<> silently resolves the LAST overload only.
      return demote(
        `handler '${request.symbol_name}' is an overload set (${callSignatures.length} signatures)`
      );
    }
    if ((callSignatures[0].getTypeParameters() ?? []).length > 0) {
      // Type parameters erase to their constraint/unknown under ReturnType<>.
      return demote(`handler '${request.symbol_name}' is generic`);
    }
    return {
      request,
      aliasText: `Awaited<ReturnType<typeof import('${spec}').${request.symbol_name}>>`,
      serialization: 'emitted',
    };
  }

  // kind === 'infer'
  const located = locateNode(sourceFile, request);
  if (!located) {
    return demote(locatorFailureReason(request));
  }
  let type = checker.getTypeAtLocation(located);
  if ((request.unwrap ?? 'awaited') === 'awaited') {
    type = checker.getAwaitedType(type) ?? type;
  }
  // #433: the checker gave us nothing (whole top type) — try the syntactic
  // recovery for an inline literal type argument of an unresolvable generic
  // annotation. The literal is dependency-free source syntax; only the outer
  // generic needed the missing package.
  if (isTopType(type)) {
    const literalNode = recoverInlineLiteralTypeArgument(
      checker,
      sourceFile,
      request,
      located
    );
    if (literalNode) {
      const recovered = checker.getTypeAtLocation(literalNode);
      if (!isTopType(recovered)) type = recovered;
    }
  }
  if (!args.placeholder) {
    return demote('internal: no placeholder destination for infer anchor');
  }
  const printed = printTypeForDestination(program, type, args.placeholder);
  if (!printed.text) {
    return demote(printed.failure ?? 'node builder print failed');
  }
  return {
    request,
    aliasText: printed.text,
    serialization: 'node_builder',
  };
}

function isTopType(type: ts.Type): boolean {
  return (
    (type.flags & (ts.TypeFlags.Any | ts.TypeFlags.Unknown | ts.TypeFlags.Never)) !== 0
  );
}

/**
 * The checker's intrinsic ERROR type: resolution of the reference FAILED
 * (missing package on a bare checkout, unresolvable symbol). Distinct from a
 * legitimate `any`, whose intrinsic name is `any` — an author-written or
 * alias-resolved `any` must never trigger the syntactic recovery.
 * `intrinsicName` is internal but stable since TS 1.x (same standing as the
 * internals node-builder.ts already mirrors).
 */
function isErrorType(type: ts.Type): boolean {
  if (!(type.flags & ts.TypeFlags.Any)) return false;
  const name = (type as unknown as { intrinsicName?: string }).intrinsicName;
  return name === 'error' || name === 'unresolved';
}

/**
 * #433 syntactic recovery: an infer anchor whose checker type decayed to a
 * whole top type may sit under a DECLARED annotation of the form
 * `SomeGeneric<{ ...literal... }>` where only the outer generic failed to
 * resolve (error-typed; its package is absent on a bare checkout) while the
 * literal type argument is dependency-free source syntax referencing only
 * locally-resolvable symbols. Trigger is purely structural — no framework
 * names anywhere:
 *
 *  1. Data-flow path: the payload node (the located node, or the anchor's
 *     expression_text relocated) is an argument of a call — or IS a send
 *     call — whose callee chain roots at an identifier whose declaration
 *     carries the annotation (`res.json(payload)` -> `res`).
 *  2. Registration path: the located node is a call registering inline
 *     callbacks; the callbacks' parameter annotations are scanned.
 *
 * Either path recovers ONLY when exactly one literal type argument is in
 * scope — ambiguity refuses recovery and the alias keeps decaying honestly.
 * The recovered literal node's own type is printed through the normal
 * node-builder path, so the alias still faces the real self-check.
 */
function recoverInlineLiteralTypeArgument(
  checker: ts.TypeChecker,
  sourceFile: ts.SourceFile,
  request: InferAnchorRequest,
  located: ts.Node
): ts.TypeLiteralNode | undefined {
  const candidates: ts.Node[] = [located];
  if (request.expression_text) {
    const byText = nodeByExpressionText(
      sourceFile,
      request.expression_text,
      request.line_number
    );
    if (byText && byText !== located) candidates.push(byText);
  }

  // Data-flow first: the payload's own call names the annotated receiver.
  for (const node of candidates) {
    const call = governingCall(node);
    if (!call) continue;
    const root = calleeRootIdentifier(call.expression);
    if (!root) continue;
    const annotation = declaredAnnotationOf(checker, root);
    if (!annotation || !isErroredGenericReference(checker, annotation)) continue;
    // The payload carrier is identified; its annotation's verdict is FINAL —
    // never fall through to the registration scan, which could pick a
    // different parameter's literal for this payload.
    const literals = literalTypeArguments(annotation);
    if (literals.length !== 1) return undefined;
    return literalResolvesLocally(checker, literals[0]) ? literals[0] : undefined;
  }

  // Registration shape: the located call passes inline callbacks whose
  // parameters carry the annotations. Ambiguity is counted BEFORE any
  // resolvability filtering: two literal-carrying annotations refuse even
  // when one of them would be rejected later — filtering first could crown
  // the wrong parameter's literal.
  for (const node of candidates) {
    if (!ts.isCallExpression(node)) continue;
    const literals: ts.TypeLiteralNode[] = [];
    for (const arg of node.arguments) {
      if (!isFunctionArgument(arg)) continue;
      for (const param of arg.parameters) {
        if (param.type && isErroredGenericReference(checker, param.type)) {
          literals.push(...literalTypeArguments(param.type));
        }
      }
    }
    if (literals.length !== 1) continue;
    return literalResolvesLocally(checker, literals[0]) ? literals[0] : undefined;
  }
  return undefined;
}

/**
 * The call whose ARGUMENT list carries `node` (payload in argument position),
 * or `node` itself when it is a call (the send call). Calls that register
 * callbacks are excluded: their receiver is the framework app/router value,
 * not a payload carrier — those go through the registration path instead.
 */
function governingCall(node: ts.Node): ts.CallExpression | undefined {
  if (ts.isCallExpression(node)) {
    return node.arguments.some(isFunctionArgument) ? undefined : node;
  }
  const parent = node.parent;
  if (
    parent !== undefined &&
    ts.isCallExpression(parent) &&
    node !== parent.expression &&
    (parent.arguments as readonly ts.Node[]).includes(node)
  ) {
    return parent.arguments.some(isFunctionArgument) ? undefined : parent;
  }
  return undefined;
}

function isFunctionArgument(
  arg: ts.Expression
): arg is ts.ArrowFunction | ts.FunctionExpression {
  return ts.isArrowFunction(arg) || ts.isFunctionExpression(arg);
}

/** Root identifier of a callee chain: `res.status(500).json` -> `res`. */
function calleeRootIdentifier(expr: ts.Expression): ts.Identifier | undefined {
  let current: ts.Expression = expr;
  while (
    ts.isPropertyAccessExpression(current) ||
    ts.isElementAccessExpression(current) ||
    ts.isCallExpression(current) ||
    ts.isNonNullExpression(current) ||
    ts.isParenthesizedExpression(current)
  ) {
    current = current.expression;
  }
  return ts.isIdentifier(current) ? current : undefined;
}

/** The explicit type annotation on the identifier's declaration, if any. */
function declaredAnnotationOf(
  checker: ts.TypeChecker,
  identifier: ts.Identifier
): ts.TypeNode | undefined {
  const symbol = checker.getSymbolAtLocation(identifier);
  const decl = symbol?.valueDeclaration ?? symbol?.declarations?.[0];
  if (!decl) return undefined;
  if (
    ts.isParameter(decl) ||
    ts.isVariableDeclaration(decl) ||
    ts.isPropertySignature(decl) ||
    ts.isPropertyDeclaration(decl)
  ) {
    return decl.type;
  }
  return undefined;
}

/**
 * A generic type reference whose OUTER resolution failed: the annotation is
 * `Name<...args>` and its own type is the checker's error type. A resolvable
 * generic (however it resolves) never triggers recovery; its checker answer
 * is the truth.
 */
function isErroredGenericReference(
  checker: ts.TypeChecker,
  annotation: ts.TypeNode
): annotation is ts.TypeReferenceNode {
  return (
    ts.isTypeReferenceNode(annotation) &&
    (annotation.typeArguments?.length ?? 0) > 0 &&
    isErrorType(checker.getTypeFromTypeNode(annotation))
  );
}

/** Type-literal type arguments of a generic reference (parens unwrapped). */
function literalTypeArguments(annotation: ts.TypeReferenceNode): ts.TypeLiteralNode[] {
  const literals: ts.TypeLiteralNode[] = [];
  for (const arg of annotation.typeArguments ?? []) {
    let node: ts.TypeNode = arg;
    while (ts.isParenthesizedTypeNode(node)) node = node.type;
    if (ts.isTypeLiteralNode(node)) literals.push(node);
  }
  return literals;
}

/**
 * Every type reference inside the literal must itself RESOLVE. A literal
 * leaning on a third-party type (`{ thing: LibThing }` with the lib absent)
 * is not the tractable class — recovering it would bake `any` at a member;
 * it keeps decaying honestly instead. Author-written `any`/`unknown`
 * keywords are allowed through: they are the source's truth, and the
 * self-check's deep walk still owns that verdict.
 */
function literalResolvesLocally(
  checker: ts.TypeChecker,
  literal: ts.TypeLiteralNode
): boolean {
  let resolves = true;
  const visit = (node: ts.Node): void => {
    if (!resolves) return;
    if (
      (ts.isTypeReferenceNode(node) ||
        ts.isImportTypeNode(node) ||
        ts.isTypeQueryNode(node) ||
        ts.isExpressionWithTypeArguments(node)) &&
      isErrorType(checker.getTypeFromTypeNode(node))
    ) {
      resolves = false;
      return;
    }
    node.forEachChild(visit);
  };
  visit(literal);
  return resolves;
}

function locatorFailureReason(request: InferAnchorRequest): string {
  const hints: string[] = [];
  if (request.span_start !== undefined) hints.push(`span ${request.span_start}-${request.span_end}`);
  if (request.expression_text) hints.push(`expression '${request.expression_text.slice(0, 40)}'`);
  if (request.line_number !== undefined) hints.push(`line ${request.line_number}`);
  return `no node located in ${request.source_file} (${hints.join(', ') || 'no locator hints'})`;
}

/**
 * Locate the target node: tightest expression covering the byte span when
 * given; else the expression matching expression_text on/after line_number;
 * else the first expression starting on line_number.
 */
export function locateNode(
  sourceFile: ts.SourceFile,
  request: InferAnchorRequest
): ts.Node | undefined {
  if (request.span_start !== undefined && request.span_end !== undefined) {
    const bySpan = tightestCoveringNode(sourceFile, request.span_start, request.span_end);
    if (bySpan) return bySpan;
  }
  if (request.expression_text) {
    const byText = nodeByExpressionText(sourceFile, request.expression_text, request.line_number);
    if (byText) return byText;
  }
  if (request.line_number !== undefined) {
    return firstExpressionOnLine(sourceFile, request.line_number);
  }
  return undefined;
}

function isPreferredTarget(node: ts.Node): boolean {
  return (
    ts.isExpression(node) ||
    ts.isVariableDeclaration(node) ||
    ts.isParameter(node) ||
    ts.isPropertyAssignment(node)
  );
}

function tightestCoveringNode(
  sourceFile: ts.SourceFile,
  start: number,
  end: number
): ts.Node | undefined {
  let best: ts.Node | undefined;
  const visit = (node: ts.Node) => {
    if (node.getStart(sourceFile) <= start && node.getEnd() >= end) {
      if (isPreferredTarget(node)) best = node;
      node.forEachChild(visit);
    } else {
      node.forEachChild(visit);
    }
  };
  visit(sourceFile);
  return best;
}

function nodeByExpressionText(
  sourceFile: ts.SourceFile,
  text: string,
  fromLine?: number
): ts.Node | undefined {
  const wanted = text.replace(/\s+/g, ' ').trim();
  let best: ts.Node | undefined;
  const visit = (node: ts.Node) => {
    if (best) return;
    if (isPreferredTarget(node)) {
      const nodeText = node.getText(sourceFile).replace(/\s+/g, ' ').trim();
      if (nodeText === wanted) {
        const line =
          sourceFile.getLineAndCharacterOfPosition(node.getStart(sourceFile)).line + 1;
        if (fromLine === undefined || line >= fromLine) {
          best = node;
          return;
        }
      }
    }
    node.forEachChild(visit);
  };
  visit(sourceFile);
  return best;
}

function firstExpressionOnLine(
  sourceFile: ts.SourceFile,
  line: number
): ts.Node | undefined {
  let best: ts.Node | undefined;
  const visit = (node: ts.Node) => {
    const nodeLine =
      sourceFile.getLineAndCharacterOfPosition(node.getStart(sourceFile)).line + 1;
    if (nodeLine === line && isPreferredTarget(node) && !best) {
      best = node;
      return;
    }
    node.forEachChild(visit);
  };
  visit(sourceFile);
  return best;
}
