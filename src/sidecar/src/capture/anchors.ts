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
  /** Present when serialization is structural_fallback. */
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
  }
): ResolvedAnchor {
  const checker = program.getTypeChecker();
  const demote = (reason: string): ResolvedAnchor => ({
    request,
    aliasText: 'unknown',
    serialization: 'structural_fallback',
    failureReason: reason,
  });

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
    return {
      request,
      aliasText: `import('${spec}').${request.symbol_name}`,
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
