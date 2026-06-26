/**
 * Type Inferrer - Scope-based type inference with payload unwrapping
 *
 * This module extracts types even when developers don't write explicit
 * annotations. It uses span-based node lookup (no line windows) to target
 * precise expressions provided by the Rust/LLM pipeline.
 *
 * Key feature: Agent-informed payload unwrapping
 * - Extracts payload types from machinery wrappers (Response<T>, AxiosResponse<T>, etc.)
 * - Supports union/intersection composition
 * - Recursive unwrapping with depth limits
 *
 * Framework agnosticism: The LLM emits the payload subexpression directly (e.g.,
 * the `users` in `res.json(users)`, `ctx.body = users`, `h.response(users)`, or
 * `return users`). The sidecar resolves that node and reads its type — no
 * framework-specific method-name lists live here. For payload-less handlers
 * (redirects, 204s), the LLM emits null and we fall back to the containing
 * function's return type.
 */

import {
  Project,
  SourceFile,
  Node,
  SyntaxKind,
  type FunctionDeclaration,
  type ArrowFunction,
  type FunctionExpression,
  type MethodDeclaration,
  type CallExpression,
  type Type,
  type Symbol as TsSymbol,
  ts,
} from 'ts-morph';
import type {
  InferRequestItem,
  InferResult,
  InferredType,
  InferKind,
  SourceLocation,

  ExtractionConfig,
  ExtractionRule,
} from './types.js';
import { validateInferRequestItem } from './validators.js';
import { expandTypeStructural } from './type-structural-expander.js';

/**
 * Print a `Type` to its string form WITHOUT the compiler's default truncation.
 *
 * `Type.getText()` truncates large/anonymous object types to ~160 chars and inserts
 * `...`, which yields a syntactically-invalid or structurally-wrong surface that then
 * produces false type-drift verdicts downstream. `NoTruncation` disables that; this is
 * the same flag set `definition-resolver.ts` already uses for expanded definitions.
 */
const TYPE_TEXT_FLAGS =
  ts.TypeFormatFlags.NoTruncation | ts.TypeFormatFlags.InTypeAlias;

function typeText(type: Type, enclosingNode?: Node): string {
  return type.getText(enclosingNode, TYPE_TEXT_FLAGS);
}

/**
 * Union of all function-like nodes in ts-morph
 */
type FunctionLike =
  | FunctionDeclaration
  | ArrowFunction
  | FunctionExpression
  | MethodDeclaration;

/**
 * Options for TypeInferrer construction
 */
export interface TypeInferrerOptions {
  /** The ts-morph Project instance */
  project: Project;
}

/**
 * Result of unwrapping a type
 */
interface UnwrapResult {
  /** The unwrapped type string */
  typeString: string;
  /** Whether the original type had explicit annotation */
  isExplicit: boolean;
  /** Whether unwrapping was actually performed */
  wasUnwrapped: boolean;
}

/**
 * Outcome of trying one ExtractionRule against one type. A rule that verifies
 * wrapper identity but recovers no payload must neither win (stomping the
 * type) nor be indistinguishable from a non-match (the wrapper is verified
 * machinery, never the contract) — so the three cases are explicit.
 */
type RuleAttempt =
  | { kind: 'extracted'; result: UnwrapResult }
  | { kind: 'verified-no-payload' }
  | { kind: 'no-match' };

/**
 * TypeInferrer - Extracts types from source code, both explicit and inferred
 *
 * Usage:
 *   const inferrer = new TypeInferrer({ project });
 *   const result = inferrer.infer(requests, extractionConfig);
 */
export class TypeInferrer {
  private readonly project: Project;

  constructor(options: TypeInferrerOptions) {
    this.project = options.project;
  }

  /**
   * Infer types for the given requests
   *
   * @param requests - Array of inference requests
   * @param extractionConfig - Agent-generated extraction config for payload unwrapping
   * @returns InferResult with inferred types or errors
   */
  infer(
    requests: InferRequestItem[],
    extractionConfig?: ExtractionConfig
  ): InferResult {
    const inferredTypes: InferredType[] = [];
    const errors: string[] = [];

    for (const request of requests) {
      // Plain JavaScript has no type annotations to extract, and `checkJs` is
      // off, so inferring against a `.js` file yields nothing useful — it only
      // crashes deep in the compiler API on undefined symbols (`escapedName`,
      // `flags`) and floods the log with the resulting error strings. Skip it.
      // `allowJs` stays on so `.ts` files can still resolve `.js` imports.
      if (/\.(js|jsx|mjs|cjs)$/i.test(request.file_path)) {
        continue;
      }
      try {
        const loc = this.formatRequestLocation(request);
        const itemError = validateInferRequestItem(request);
        if (itemError) {
          errors.push(
            `Invalid infer item at ${request.file_path}:${loc} (${request.infer_kind}): ${itemError}`
          );
          continue;
        }
        const result = this.inferSingle(request, extractionConfig);
        if (result) {
          inferredTypes.push(result);
        } else {
          errors.push(
            `Could not infer type at ${request.file_path}:${loc} (${request.infer_kind})`
          );
        }
      } catch (err) {
        const error = err instanceof Error ? err.message : String(err);
        const loc = this.formatRequestLocation(request);
        errors.push(
          `Error inferring type at ${request.file_path}:${loc}: ${error}`
        );
      }
    }

    return {
      success: errors.length === 0 || inferredTypes.length > 0,
      inferred_types: inferredTypes.length > 0 ? inferredTypes : undefined,
      errors: errors.length > 0 ? errors : undefined,
    };
  }

  /**
   * Infer a single type from a request
   */
  private inferSingle(
    request: InferRequestItem,
    extractionConfig?: ExtractionConfig
  ): InferredType | null {
    const sourceFile = this.getSourceFile(request.file_path);
    if (!sourceFile) {
      this.logError(`Source file not found: ${request.file_path}`);
      return null;
    }

    switch (request.infer_kind) {
      case 'function_return':
        return this.inferFunctionReturn(sourceFile, request, extractionConfig);
      case 'response_body':
        return this.inferResponseBody(sourceFile, request, extractionConfig);
      case 'call_result':
        return this.inferCallResult(sourceFile, request, extractionConfig);
      case 'variable':
        return this.inferVariable(sourceFile, request, extractionConfig);
      case 'expression':
        return this.inferExpression(sourceFile, request, extractionConfig);
      case 'request_body':
        return this.inferRequestBody(sourceFile, request, extractionConfig);
      case 'signature_return':
        return this.inferSignatureReturn(sourceFile, request);
      case 'function_param':
        return this.inferFunctionParam(sourceFile, request);
      default:
        this.logError(`Unknown infer kind: ${request.infer_kind}`);
        return null;
    }
  }

  /**
   * Get or add a source file to the project
   */
  private getSourceFile(filePath: string): SourceFile | undefined {
    let sourceFile = this.project.getSourceFile(filePath);
    if (!sourceFile) {
      try {
        sourceFile = this.project.addSourceFileAtPath(filePath);
      } catch (err) {
        this.logError(
          `Failed to add source file: ${err instanceof Error ? err.message : String(err)}`
        );
        return undefined;
      }
    }
    return sourceFile;
  }

  // ===========================================================================
  // Inference Methods by Kind
  // ===========================================================================

  private inferFunctionReturn(
    sourceFile: SourceFile,
    request: InferRequestItem,
    extractionConfig?: ExtractionConfig
  ): InferredType | null {
    const func = this.resolveContainingFunction(sourceFile, request);

    if (!func) {
      this.log(`No function found for request at ${request.file_path}:${request.line_number}`);
      return null;
    }

    const returnTypeNode = func.getReturnTypeNode();
    const isExplicit = returnTypeNode !== undefined;
    let returnType = func.getReturnType();
    let typeString = typeText(returnType, func);

    // Apply the agent-generated extraction config to the AWAITED type: an
    // async handler's return is Promise<Wrapper<T>>, whose symbol is
    // `Promise` — a rule naming the wrapper could never match it, and the
    // textual unwrapPromise below runs too late for the rules to see T.
    const awaitedType = this.unwrapPromiseType(returnType);
    const unwrapResult = this.unwrapTypeWithConfig(awaitedType, func, extractionConfig);
    if (unwrapResult.wasUnwrapped) {
      typeString = unwrapResult.typeString;
    }

    typeString = this.unwrapPromise(typeString, returnType);

    return this.createInferredType(
      request,
      typeString,
      isExplicit,
      this.getNodeLocation(func),
      unwrapResult.wasUnwrapped ? unwrapResult.typeString : undefined
    );
  }

  /**
   * Infer a function's return type for the signature hint. Unlike
   * `inferFunctionReturn`, this does NOT unwrap Promise or apply wrapper
   * rules — a function that returns `Promise<AuthResult>` should show exactly
   * that in its signature. Used by the function-signature collection pass.
   */
  private inferSignatureReturn(
    sourceFile: SourceFile,
    request: InferRequestItem
  ): InferredType | null {
    const func = this.resolveContainingFunction(sourceFile, request);

    if (!func) {
      this.log(`No function found for request at ${request.file_path}:${request.line_number}`);
      return null;
    }

    const isExplicit = func.getReturnTypeNode() !== undefined;
    const typeString = typeText(func.getReturnType(), func);

    return this.createInferredType(
      request,
      typeString,
      isExplicit,
      this.getNodeLocation(func)
    );
  }

  /**
   * Infer the type of a single named parameter. `is_explicit` reflects whether
   * the parameter carries a source annotation; the type string is the
   * compiler's view either way (so contextually-typed callback params resolve
   * even without an annotation). Uses ts-morph's default `getText()` form,
   * which keeps named types as names and bounds depth via the compiler's own
   * truncation.
   */
  private inferFunctionParam(
    sourceFile: SourceFile,
    request: InferRequestItem
  ): InferredType | null {
    const func = this.resolveContainingFunction(sourceFile, request);

    if (!func) {
      this.log(`No function found for request at ${request.file_path}:${request.line_number}`);
      return null;
    }

    if (!request.param_name) {
      this.logError(`function_param request missing param_name at ${request.file_path}:${request.line_number}`);
      return null;
    }

    const param = func
      .getParameters()
      .find((p) => p.getName() === request.param_name);

    if (!param) {
      this.log(
        `Parameter "${request.param_name}" not found at ${request.file_path}:${request.line_number}`
      );
      return null;
    }

    const isExplicit = param.getTypeNode() !== undefined;
    const typeString = typeText(param.getType(), param);

    return this.createInferredType(
      request,
      typeString,
      isExplicit,
      this.getNodeLocation(param)
    );
  }

  private inferResponseBody(
    sourceFile: SourceFile,
    request: InferRequestItem,
    extractionConfig?: ExtractionConfig
  ): InferredType | null {
    const node = this.resolveTargetNode(sourceFile, request);

    if (!node) {
      // No locator, or locator didn't resolve — likely a payload-less handler
      // (redirect, 204, streaming). Infer the containing function's return type.
      this.log(
        `No payload node found for request at ${request.file_path}:${request.line_number}; falling back to function return`
      );
      return this.inferFunctionReturn(sourceFile, request, extractionConfig);
    }

    // The resolved node IS the payload subexpression in the MVP schema.
    // Transitional fallback: if a caller still supplies a bare call expression
    // (e.g., `res.json(users)`), drill to its first argument. No method-name list.
    let payloadNode: Node = node;
    if (Node.isCallExpression(node)) {
      const args = node.getArguments();
      // A call that receives a function is a callback registration (e.g. an
      // endpoint registration like `app.get('/path', handler)`) — its first
      // argument is the route path, not a payload. The span locator falls
      // back to exactly this shape when no payload expression was reported,
      // so drilling here would put the path literal's type in the manifest.
      const registersCallback = args.some(
        (arg) => Node.isArrowFunction(arg) || Node.isFunctionExpression(arg)
      );
      if (registersCallback) {
        this.log(
          `Span resolves to a callback-registration call at ${request.file_path}:${request.line_number}; no payload to infer`
        );
        return null;
      }
      if (args.length > 0) {
        payloadNode = args[0];
      }
    }

    const payloadType = payloadNode.getType();
    let typeString = typeText(payloadType, payloadNode);

    const unwrapResult = this.unwrapTypeWithConfig(
      payloadType,
      payloadNode,
      extractionConfig
    );
    if (unwrapResult.wasUnwrapped) {
      typeString = unwrapResult.typeString;
    }

    return this.createInferredType(
      request,
      typeString,
      false,
      this.getNodeLocation(payloadNode),
      unwrapResult.wasUnwrapped ? unwrapResult.typeString : undefined
    );
  }

  private inferCallResult(
    sourceFile: SourceFile,
    request: InferRequestItem,
    extractionConfig?: ExtractionConfig
  ): InferredType | null {
    const callExpr = this.resolveTargetCallExpression(sourceFile, request);

    if (!callExpr) {
      return this.inferExpression(sourceFile, request, extractionConfig);
    }

    // Walk up from the already-found call expression instead of re-searching
    const func = this.findContainingFunctionForNode(callExpr);
    const terminalNode = this.resolveCallResultTerminalNode(callExpr, func);
    const returnType = terminalNode.getType();
    let typeString = typeText(returnType, terminalNode);
    let isExplicit = false;

    const unwrapResult = this.unwrapTypeWithConfig(
      returnType,
      terminalNode,
      extractionConfig
    );

    if (unwrapResult.wasUnwrapped) {
      typeString = unwrapResult.typeString;
      isExplicit = unwrapResult.isExplicit;
    }

    const explicitType = this.extractExplicitTypeFromAncestor(terminalNode);
    if (explicitType) {
      typeString = explicitType;
      isExplicit = true;
    }

    typeString = this.unwrapPromise(typeString, returnType);

    return this.createInferredType(
      request,
      typeString,
      isExplicit,
      this.getNodeLocation(terminalNode),
      unwrapResult.wasUnwrapped ? unwrapResult.typeString : undefined
    );
  }

  private inferVariable(
    sourceFile: SourceFile,
    request: InferRequestItem,
    extractionConfig?: ExtractionConfig
  ): InferredType | null {
    const node = this.resolveTargetNode(sourceFile, request);

    if (!node) {
      return null;
    }

    const varDecl = Node.isVariableDeclaration(node)
      ? node
      : node.getFirstAncestorByKind(SyntaxKind.VariableDeclaration);

    if (!varDecl) {
      return this.inferExpression(sourceFile, request, extractionConfig);
    }

    const typeNode = varDecl.getTypeNode();
    const isExplicit = typeNode !== undefined;
    let varType = varDecl.getType();
    let typeString = typeText(varType, varDecl);

    // Apply extraction config
    const unwrapResult = this.unwrapTypeWithConfig(
      varType,
      varDecl,
      extractionConfig
    );
    if (unwrapResult.wasUnwrapped) {
      typeString = unwrapResult.typeString;
    }

    typeString = this.unwrapPromise(typeString, varType);

    return this.createInferredType(
      request,
      typeString,
      isExplicit,
      this.getNodeLocation(varDecl),
      unwrapResult.wasUnwrapped ? unwrapResult.typeString : undefined
    );
  }

  private inferExpression(
    sourceFile: SourceFile,
    request: InferRequestItem,
    extractionConfig?: ExtractionConfig
  ): InferredType | null {
    const node = this.resolveTargetNode(sourceFile, request);

    if (!node) {
      return null;
    }

    const type = node.getType();
    let typeString = typeText(type, node);

    // Apply extraction config
    const unwrapResult = this.unwrapTypeWithConfig(type, node, extractionConfig);
    if (unwrapResult.wasUnwrapped) {
      typeString = unwrapResult.typeString;
    }

    typeString = this.unwrapPromise(typeString, type);

    return this.createInferredType(
      request,
      typeString,
      false,
      this.getNodeLocation(node),
      unwrapResult.wasUnwrapped ? unwrapResult.typeString : undefined
    );
  }

  private inferRequestBody(
    sourceFile: SourceFile,
    request: InferRequestItem,
    extractionConfig?: ExtractionConfig
  ): InferredType | null {
    const located = this.resolveTargetNode(sourceFile, request);

    if (!located) {
      return null;
    }

    // Mirror inferCallResult: strip `await`/`as`/parens/`!` so the inner
    // expression's type (not the surrounding `Promise<any>`) is read.
    const node = this.unwrapExpressionNode(located);

    const payloadType = node.getType();
    let typeString = typeText(payloadType, node);
    let isExplicit = false;

    // Apply extraction config
    const unwrapResult = this.unwrapTypeWithConfig(
      payloadType,
      node,
      extractionConfig
    );
    if (unwrapResult.wasUnwrapped) {
      typeString = unwrapResult.typeString;
      isExplicit = unwrapResult.isExplicit;
    }

    // A declared type — an `as T` cast or a typed variable binding/annotation
    // on an ancestor — wins over the call's raw `Promise<any>` / `any`. Only
    // recover when one is genuinely present; untyped `request.formData()`
    // stays `FormData` / `any`.
    //
    // Use the unwrapped `node`, not `located`: when the locator lands ON the
    // `(...) as T` cast itself, the cast is the located node (not an ancestor),
    // so an ancestor walk from `located` would miss it. `node` is the inner
    // expression whose ancestors include the `as T`, so the cast is recovered;
    // the typed-binding and untyped-control cases are unaffected.
    const explicitType = this.extractExplicitTypeFromAncestor(node);
    if (explicitType) {
      typeString = explicitType;
      isExplicit = true;
    }

    typeString = this.unwrapPromise(typeString, payloadType);

    return this.createInferredType(
      request,
      typeString,
      isExplicit,
      this.getNodeLocation(node),
      unwrapResult.wasUnwrapped ? unwrapResult.typeString : undefined
    );
  }

  // ===========================================================================
  // Call Result Resolution
  // ===========================================================================

  private resolveCallResultTerminalNode(
    callExpr: CallExpression,
    func: FunctionLike | undefined
  ): Node {
    const returnStmt = callExpr.getFirstAncestorByKind(SyntaxKind.ReturnStatement);
    if (returnStmt) {
      const returnExpr = returnStmt.getExpression();
      if (returnExpr) {
        return returnExpr;
      }
    }

    const binding = this.extractBindingFromCall(callExpr);
    if (binding && func) {
      let currentNames = binding.names;
      let lastNode: Node = binding.node;
      const startPos = callExpr.getStart();
      const candidates = this.collectDefUseNodes(func);

      for (const expr of candidates) {
        if (expr.getStart() <= startPos) continue;

        if (Node.isVariableDeclaration(expr)) {
          const initializer = expr.getInitializer();
          if (
            initializer &&
            Node.isIdentifier(initializer) &&
            this.expressionUsesNames(initializer, currentNames)
          ) {
            const names = this.extractBindingNames(expr.getNameNode());
            currentNames = names;
            lastNode = expr;
          }
        }

        if (Node.isBinaryExpression(expr)) {
          const left = expr.getLeft();
          const right = expr.getRight();
          if (
            Node.isIdentifier(right) &&
            this.expressionUsesNames(right, currentNames)
          ) {
            if (Node.isIdentifier(left)) {
              const names = [left.getText()];
              currentNames = names;
              lastNode = expr;
            }
          }
        }

        if (this.expressionUsesNames(expr, currentNames)) {
          lastNode = expr;
        }
      }

      return lastNode;
    }

    return callExpr;
  }

  private extractBindingFromCall(callExpr: CallExpression): { names: string[]; node: Node } | null {
    const varDecl = callExpr.getFirstAncestorByKind(SyntaxKind.VariableDeclaration);
    if (varDecl) {
      const initializer = varDecl.getInitializer();
      if (initializer === callExpr || this.unwrapExpressionNode(initializer ?? callExpr) === callExpr) {
        const names = this.extractBindingNames(varDecl.getNameNode());
        const node = varDecl;
        return { names, node };
      }
    }

    const assignment = callExpr.getFirstAncestorByKind(SyntaxKind.BinaryExpression);
    if (assignment) {
      const right = assignment.getRight();
      if (
        right === callExpr ||
        this.unwrapExpressionNode(right) === callExpr
      ) {
        const left = assignment.getLeft();
        const names = Node.isIdentifier(left) ? [left.getText()] : [];
        const node = assignment;
        return { names, node };
      }
    }

    return null;
  }

  private extractBindingNames(nameNode: Node): string[] {
    if (Node.isIdentifier(nameNode)) {
      return [nameNode.getText()];
    }

    if (Node.isObjectBindingPattern(nameNode) || Node.isArrayBindingPattern(nameNode)) {
      const names: string[] = [];
      for (const element of nameNode.getElements()) {
        if (Node.isBindingElement(element)) {
          const elementName = element.getNameNode();
          names.push(...this.extractBindingNames(elementName));
        }
      }
      return names;
    }

    return [];
  }

  private getPrimaryBindingNode(nameNode: Node): Node {
    if (Node.isIdentifier(nameNode)) {
      return nameNode;
    }

    if (Node.isObjectBindingPattern(nameNode) || Node.isArrayBindingPattern(nameNode)) {
      const elements = nameNode.getElements();
      if (elements.length > 0 && Node.isBindingElement(elements[0])) {
        const elementName = elements[0].getNameNode();
        const found = this.getPrimaryBindingNode(elementName);
        return found;
      }
    }

    return nameNode;
  }

  private collectDefUseNodes(func: FunctionLike): Node[] {
    const candidates: Node[] = [];
    func.forEachDescendant((node) => {
      if (
        Node.isIdentifier(node) ||
        Node.isVariableDeclaration(node) ||
        Node.isBinaryExpression(node)
      ) {
        candidates.push(node);
      }
    });
    return candidates;
  }

  private expressionUsesNames(expr: Node, names: string[]): boolean {
    const identifiers = expr.getDescendantsOfKind(SyntaxKind.Identifier);
    return identifiers.some((id) => this.isIdentifierUsage(id, names));
  }

  private isIdentifierUsage(id: Node, names: string[]): boolean {
    if (!Node.isIdentifier(id)) {
      return false;
    }

    const text = id.getText();
    if (!names.includes(text)) {
      return false;
    }

    const parent = id.getParent();
    if (
      parent &&
      Node.isVariableDeclaration(parent) &&
      parent.getNameNode() === id
    ) {
      return false;
    }
    if (
      parent &&
      Node.isBindingElement(parent) &&
      parent.getNameNode() === id
    ) {
      return false;
    }

    return true;
  }

  private isInFunctionScope(node: Node, func: FunctionLike): boolean {
    const ancestor = node.getFirstAncestor((candidate) =>
      Node.isFunctionDeclaration(candidate) ||
      Node.isArrowFunction(candidate) ||
      Node.isFunctionExpression(candidate) ||
      Node.isMethodDeclaration(candidate)
    );

    return ancestor === func;
  }

  // ===========================================================================
  // Extraction Config-based Payload Unwrapping (NEW)
  // ===========================================================================

  /** `Promise<T>` → `T` at the type level; any other type passes through. */
  private unwrapPromiseType(type: Type): Type {
    const symbolName = (type.getSymbol() || type.getAliasSymbol())?.getName();
    if (symbolName === 'Promise') {
      const args = type.getTypeArguments();
      if (args.length === 1) {
        return args[0];
      }
    }
    return type;
  }

  /** The "leave the type as it is" result every bail-out path shares. */
  private noUnwrap(type: Type, node: Node): UnwrapResult {
    return {
      typeString: typeText(type, node),
      isExplicit: false,
      wasUnwrapped: false,
    };
  }

  /**
   * Unwrap a type using the agent-generated ExtractionConfig.
   */
  private unwrapTypeWithConfig(
    type: Type,
    node: Node,
    extractionConfig?: ExtractionConfig
  ): UnwrapResult {
    if (!extractionConfig || extractionConfig.rules.length === 0) {
      return this.noUnwrap(type, node);
    }

    return this.unwrapType(type, node, extractionConfig, 0);
  }

  /**
   * Core unwrapping implementation with ExtractionConfig rules.
   *
   * Requirements:
   * 1. Exact wrapperSymbols match extracts (gated on originModuleGlobs when
   *    the rule carries them — names like `Response` are shared by the DOM,
   *    frameworks, and HTTP clients)
   * 2. machineryIndicators only trigger unwrap if originModuleGlobs also match
   * 3. Handle unions and intersections
   * 4. Support recursive unwrapping with depth limits
   * 5. A rule that matches but extracts nothing never blocks later rules;
   *    only after every rule has run does an origin-verified match with no
   *    recoverable payload collapse to `unknown`
   */
  private unwrapType(
    type: Type,
    node: Node,
    config: ExtractionConfig,
    depth: number
  ): UnwrapResult {
    const maxGlobalDepth = 10; // Safety limit
    if (depth >= maxGlobalDepth) {
      return this.noUnwrap(type, node);
    }

    // Handle union types: Response<A> | Response<B> → unwrap to A | B
    if (type.isUnion()) {
      const unionTypes = type.getUnionTypes();
      const unwrappedParts: string[] = [];
      let anyUnwrapped = false;

      for (const unionType of unionTypes) {
        const result = this.unwrapType(unionType, node, config, depth + 1);
        unwrappedParts.push(result.typeString);
        if (result.wasUnwrapped) {
          anyUnwrapped = true;
        }
      }

      if (anyUnwrapped) {
        // Dedupe and join. A member that collapsed to `unknown` (verified
        // machinery with no recoverable payload) must not pollute the join —
        // `unknown | User` would read downstream as a real composite type
        // instead of partially-unresolved.
        const unique = [...new Set(unwrappedParts)];
        const informative = unique.filter((part) => part !== 'unknown');
        const parts = informative.length > 0 ? informative : unique;
        return {
          typeString: parts.length === 1 ? parts[0] : parts.join(' | '),
          isExplicit: false,
          wasUnwrapped: true,
        };
      }
    }

    // Handle intersection types: Response<A> & X → try to unwrap Response<A>
    if (type.isIntersection()) {
      const intersectionTypes = type.getIntersectionTypes();
      for (const intersectType of intersectionTypes) {
        const result = this.unwrapType(intersectType, node, config, depth + 1);
        if (result.wasUnwrapped) {
          return result;
        }
      }
    }

    // Try each rule. A rule that verifies the wrapper's identity but cannot
    // recover a payload must not stop the loop — the model is encouraged to
    // emit several overlapping rules (e.g. a generic-index variant and a
    // property-path variant for the same wrapper), and a later one may still
    // extract. Only when every rule has had its chance does a verified match
    // collapse to `unknown`: the wrapper itself is never the contract, and
    // downstream treats `unknown` as unresolved instead of comparing it.
    let verifiedMachinery = false;
    for (const rule of config.rules) {
      const attempt = this.tryUnwrapWithRule(type, node, rule, config, depth);
      if (attempt.kind === 'extracted') {
        return attempt.result;
      }
      if (attempt.kind === 'verified-no-payload') {
        verifiedMachinery = true;
      }
    }

    if (verifiedMachinery) {
      return {
        typeString: 'unknown',
        isExplicit: false,
        wasUnwrapped: true,
      };
    }

    return this.noUnwrap(type, node);
  }

  /**
   * Try to unwrap a type using a single ExtractionRule.
   */
  private tryUnwrapWithRule(
    type: Type,
    node: Node,
    rule: ExtractionRule,
    config: ExtractionConfig,
    depth: number
  ): RuleAttempt {
    const maxDepth = rule.maxDepth ?? 4;
    if (depth >= maxDepth) {
      return { kind: 'no-match' };
    }

    const symbol = type.getSymbol() || type.getAliasSymbol();
    const symbolName = symbol?.getName();

    // 1. Check exact wrapperSymbols match. When the rule also carries
    // originModuleGlobs, the symbol's declaration must come from a matching
    // module — names like `Response` are shared by the DOM, frameworks, and
    // HTTP clients, so a bare name match would unwrap unrelated types.
    if (rule.wrapperSymbols && symbolName && rule.wrapperSymbols.includes(symbolName)) {
      const originGated = !!(rule.originModuleGlobs && rule.originModuleGlobs.length > 0);
      if (!originGated || this.symbolOriginatesFromModules(symbol, rule.originModuleGlobs!)) {
        const extracted = this.extractPayloadFromWrapper(type, node, rule, config, depth);
        if (extracted) {
          return { kind: 'extracted', result: extracted };
        }
        // A name-only match is not proof of machinery: a local type that
        // happens to share the name must keep its real structural type when
        // nothing was extracted. Only origin-verified matches may collapse
        // to `unknown`.
        return originGated ? { kind: 'verified-no-payload' } : { kind: 'no-match' };
      }
      return { kind: 'no-match' };
    }

    // 2. Check machineryIndicators + originModuleGlobs. Indicators alone are
    // too many false positives, so the origin gate is mandatory here — which
    // also means a match in this branch is always origin-verified.
    if (rule.machineryIndicators && rule.machineryIndicators.length > 0) {
      if (!rule.originModuleGlobs || rule.originModuleGlobs.length === 0) {
        return { kind: 'no-match' };
      }

      if (!this.typeHasMachineryIndicators(type, rule.machineryIndicators)) {
        return { kind: 'no-match' };
      }

      if (!this.symbolOriginatesFromModules(symbol, rule.originModuleGlobs)) {
        return { kind: 'no-match' };
      }

      const extracted = this.extractPayloadFromWrapper(type, node, rule, config, depth);
      if (extracted) {
        return { kind: 'extracted', result: extracted };
      }
      return { kind: 'verified-no-payload' };
    }

    return { kind: 'no-match' };
  }

  /**
   * Extract the payload type from a matched wrapper. Returns null when the
   * rule matched the wrapper but no payload is recoverable from generics or
   * property paths — the caller decides what a payload-less match means
   * (verified machinery collapses to `unknown` after every rule has run;
   * a name-only match leaves the type untouched).
   */
  private extractPayloadFromWrapper(
    type: Type,
    node: Node,
    rule: ExtractionRule,
    config: ExtractionConfig,
    depth: number
  ): UnwrapResult | null {
    // The outer extraction already succeeded on the paths below; a recursive
    // inner pass that finds nothing more must not demote the result back to
    // "not unwrapped" (which would discard the recovered payload).
    const recurse = (payload: Type): UnwrapResult => ({
      ...this.unwrapType(payload, node, config, depth + 1),
      wasUnwrapped: true,
    });

    // 1. Try generic type argument at payloadGenericIndex
    const genericIndex = rule.payloadGenericIndex ?? 0;
    const typeArgs = type.getTypeArguments();

    if (typeArgs.length > genericIndex) {
      const payloadArg = typeArgs[genericIndex];

      // Check if it's a useful type (not any/unknown/never)
      const argText = typeText(payloadArg, node);
      if (!this.isUselessType(argText)) {
        // Recursive unwrap if configured
        if (rule.unwrapRecursively) {
          return recurse(payloadArg);
        }
        return {
          typeString: argText,
          isExplicit: true,
          wasUnwrapped: true,
        };
      }

      // Try "first useful generic" heuristic
      for (let i = 0; i < typeArgs.length; i++) {
        const argType = typeArgs[i];
        const text = typeText(argType, node);
        if (!this.isUselessType(text)) {
          if (rule.unwrapRecursively) {
            return recurse(argType);
          }
          return {
            typeString: text,
            isExplicit: true,
            wasUnwrapped: true,
          };
        }
      }
    }

    // 2. Try payloadPropertyPath
    if (rule.payloadPropertyPath && rule.payloadPropertyPath.length > 0) {
      let currentType = type;

      for (const propName of rule.payloadPropertyPath) {
        const prop = currentType.getProperty(propName);
        if (!prop) {
          break;
        }
        const propType = prop.getTypeAtLocation(node);
        currentType = propType;
      }

      if (currentType !== type) {
        const propText = typeText(currentType, node);
        if (!this.isUselessType(propText)) {
          if (rule.unwrapRecursively) {
            return recurse(currentType);
          }
          return {
            typeString: propText,
            isExplicit: false,
            wasUnwrapped: true,
          };
        }
      }
    }

    return null;
  }

  /**
   * Check if a type has machinery indicator properties/methods.
   */
  private typeHasMachineryIndicators(type: Type, indicators: string[]): boolean {
    const properties = type.getProperties();
    const propertyNames = properties.map((p) => p.getName());

    for (const indicator of indicators) {
      if (propertyNames.includes(indicator)) {
        return true;
      }
    }

    // Also check apparent properties (for interfaces, etc.)
    const apparentProperties = type.getApparentProperties();
    const apparentNames = apparentProperties.map((p) => p.getName());

    for (const indicator of indicators) {
      if (apparentNames.includes(indicator)) {
        return true;
      }
    }

    return false;
  }

  /**
   * Check if a symbol's declarations originate from modules matching the globs.
   */
  private symbolOriginatesFromModules(symbol: TsSymbol | undefined, moduleGlobs: string[]): boolean {
    if (!symbol) {
      return false;
    }

    const declarations = symbol.getDeclarations();
    for (const decl of declarations) {
      const sourceFile = decl.getSourceFile();
      const filePath = sourceFile.getFilePath();

      for (const glob of moduleGlobs) {
        if (this.filePathMatchesModuleGlob(filePath, glob)) {
          return true;
        }
      }
    }

    // Also check aliased symbol
    try {
      const aliased = symbol.getAliasedSymbol?.();
      if (aliased && aliased !== symbol) {
        return this.symbolOriginatesFromModules(aliased, moduleGlobs);
      }
    } catch {
      // Ignore errors when getting aliased symbol
    }

    return false;
  }

  /**
   * Simple glob matching for module paths.
   * Supports: exact match, a trailing "*" wildcard, and "package/*" patterns.
   *
   * Matches are segment-bounded: the glob names a package (or package
   * subpath) under node_modules, and the match must end at a path-segment
   * boundary — `got` matches `node_modules/got/...` but never
   * `node_modules/got-scraping/...`. This matters because the exact-symbol
   * origin gate routes shared names like `Response` through here.
   */
  private filePathMatchesModuleGlob(filePath: string, glob: string): boolean {
    const normalizedPath = filePath.replace(/\\/g, '/');

    const candidates = [glob];
    if (!glob.startsWith('@types/')) {
      // Auto-try the DefinitelyTyped variant: pkg → @types/pkg,
      // @scope/pkg → @types/scope__pkg.
      candidates.push(
        glob.startsWith('@')
          ? `@types/${glob.slice(1).replace('/', '__')}`
          : `@types/${glob}`
      );
    }

    return candidates.some((candidate) => {
      const base = candidate.replace(/\/?\*+$/, '').replace(/\*/g, '');
      if (base === '') {
        return false;
      }
      const needle = `node_modules/${base}`;
      let idx = normalizedPath.indexOf(needle);
      while (idx !== -1) {
        const next = normalizedPath[idx + needle.length];
        if (next === undefined || next === '/') {
          return true;
        }
        idx = normalizedPath.indexOf(needle, idx + 1);
      }
      return false;
    });
  }

  /**
   * Check if a type string is "useless" for payload purposes.
   */
  private isUselessType(typeString: string): boolean {
    const useless = ['any', 'unknown', 'never', 'void', 'undefined', 'null', 'object', '{}'];
    const trimmed = typeString.trim();
    return useless.includes(trimmed) || trimmed === '';
  }

  private unwrapExpressionNode(node: Node | undefined): Node {
    let current = node;
    while (current) {
      if (Node.isParenthesizedExpression(current)) {
        current = current.getExpression();
        continue;
      }
      if (Node.isAwaitExpression(current)) {
        current = current.getExpression();
        continue;
      }
      if (Node.isAsExpression(current)) {
        current = current.getExpression();
        continue;
      }
      if (Node.isNonNullExpression(current)) {
        current = current.getExpression();
        continue;
      }
      break;
    }
    return current ?? node!;
  }

  private extractExplicitTypeFromAncestor(node: Node): string | null {
    const varDecl = node.getFirstAncestorByKind(SyntaxKind.VariableDeclaration);
    if (varDecl) {
      const typeNode = varDecl.getTypeNode();
      if (typeNode) {
        return this.expandAnnotationTypeNode(typeNode);
      }
    }

    // Consider the node ITSELF as well as its ancestors: the `call_result`
    // path's terminal node for `return res.json() as Promise<T>` IS the
    // `as` cast (an ancestor walk from it would miss it), so the #257 consumer
    // shape would never be recovered. `as T` and `<T>x` assertions both apply.
    const asExpr = Node.isAsExpression(node)
      ? node
      : node.getFirstAncestorByKind(SyntaxKind.AsExpression);
    if (asExpr) {
      const typeNode = asExpr.getTypeNode();
      if (typeNode) {
        return this.expandAnnotationTypeNode(typeNode);
      }
    }

    const typeAssertion = Node.isTypeAssertion(node)
      ? node
      : node.getFirstAncestorByKind(SyntaxKind.TypeAssertionExpression);
    if (typeAssertion) {
      const typeNode = typeAssertion.getTypeNode();
      if (typeNode) {
        return this.expandAnnotationTypeNode(typeNode);
      }
    }

    return null;
  }

  /**
   * Render an explicit annotation (`as T`, `<T>`, or a typed binding) as
   * fully-structural text.
   *
   * `typeNode.getText()` keeps a named type as its bare identifier
   * (`OrderView`, `Promise<Payment>`). A bare name is fine inside the source
   * project but becomes a dangling reference in the cross-repo `.d.ts` bundle,
   * which carries only alias lines and no source declarations — it resolves to
   * `any` and the comparison reads `unverifiable`. Resolving the annotation to
   * its `Type`, stripping `Promise<…>` at the type level, and expanding the
   * object structurally (shared with `definition-resolver.ts`) lands the real
   * shape (`{ id: string; currency: string }`) in the bundle so the consumer
   * can actually be compared.
   *
   * Falls back to the bare annotation text when the resolved type can't be
   * expanded to a structural form (primitives, library types, unresolvable
   * references), so a non-object annotation behaves exactly as before.
   */
  private expandAnnotationTypeNode(typeNode: Node): string {
    const fallback = typeNode.getText();
    try {
      const annotationType = this.unwrapPromiseType(typeNode.getType());
      const expanded = expandTypeStructural(annotationType);
      // Only prefer the structural form when expansion actually inlined an
      // object shape; otherwise keep the annotation text (e.g. a bare
      // primitive or a library type the expander leaves by name).
      return expanded.startsWith('{') ? expanded : fallback;
    } catch {
      return fallback;
    }
  }

  // ===========================================================================
  // Node Finding
  // ===========================================================================

  // ===========================================================================
  // Text-Based Node Resolution (Gemini expression text + line)
  // ===========================================================================

  /**
   * Resolve the target node using the best available locator:
   * 1. If span_start + span_end present → findNodeAtSpan (SWC byte offsets)
   * 2. If expression_text present → findNodeByText (Gemini text + line)
   * 3. Otherwise → undefined
   */
  private resolveTargetNode(
    sourceFile: SourceFile,
    request: InferRequestItem
  ): Node | undefined {
    if (request.span_start !== undefined && request.span_end !== undefined) {
      return this.findNodeAtSpan(sourceFile, request.span_start, request.span_end);
    }
    if (request.expression_text) {
      return this.findNodeByText(sourceFile, request.expression_text, request.expression_line);
    }
    return undefined;
  }

  /**
   * Resolve to a CallExpression using the best available locator.
   */
  private resolveTargetCallExpression(
    sourceFile: SourceFile,
    request: InferRequestItem
  ): CallExpression | undefined {
    if (request.span_start !== undefined && request.span_end !== undefined) {
      return this.findCallExpressionAtSpan(sourceFile, request.span_start, request.span_end);
    }
    if (request.expression_text) {
      return this.findCallExpressionByText(sourceFile, request.expression_text, request.expression_line);
    }
    return undefined;
  }

  /**
   * Resolve to a containing function using the best available locator.
   */
  private resolveContainingFunction(
    sourceFile: SourceFile,
    request: InferRequestItem
  ): FunctionLike | undefined {
    if (request.span_start !== undefined && request.span_end !== undefined) {
      return this.findContainingFunctionBySpan(sourceFile, request.span_start, request.span_end);
    }
    if (request.expression_text) {
      const node = this.findNodeByText(sourceFile, request.expression_text, request.expression_line);
      if (!node) return undefined;
      return this.findContainingFunctionForNode(node);
    }
    // Signature requests carry only the function's start line (FunctionDefinition
    // has no byte span). Fall back to locating the function by that line.
    return this.findFunctionByLine(sourceFile, request.line_number);
  }

  /**
   * Find the function whose declaration starts at (or within a couple of lines
   * of) the given line. Used for signature inference, where the only locator is
   * the function's start line as recorded by the scanner. Ties break toward the
   * innermost (smallest) function. Returns undefined if nothing is close enough,
   * to avoid binding to an unrelated function.
   */
  private findFunctionByLine(
    sourceFile: SourceFile,
    line: number
  ): FunctionLike | undefined {
    const LINE_TOLERANCE = 2;
    const functions = sourceFile.getDescendants().filter(
      (node): node is FunctionLike =>
        Node.isFunctionDeclaration(node) ||
        Node.isArrowFunction(node) ||
        Node.isFunctionExpression(node) ||
        Node.isMethodDeclaration(node)
    );

    let best: FunctionLike | undefined;
    let bestDelta = Infinity;
    for (const fn of functions) {
      const delta = Math.abs(fn.getStartLineNumber() - line);
      if (delta > LINE_TOLERANCE) continue;
      const isCloser = delta < bestDelta;
      const isInnermostTie =
        delta === bestDelta &&
        best !== undefined &&
        fn.getEnd() - fn.getStart() < best.getEnd() - best.getStart();
      if (isCloser || isInnermostTie) {
        best = fn;
        bestDelta = delta;
      }
    }
    return best;
  }

  /**
   * Walk up from a node to find its innermost containing function.
   */
  private findContainingFunctionForNode(node: Node): FunctionLike | undefined {
    return node.getFirstAncestor(
      (n): n is FunctionLike =>
        Node.isFunctionDeclaration(n) ||
        Node.isArrowFunction(n) ||
        Node.isFunctionExpression(n) ||
        Node.isMethodDeclaration(n)
    );
  }

  /**
   * Find a node by matching expression text near a target line.
   *
   * Strategy:
   * 1. Get all descendant nodes within [lineNumber - searchRadius, lineNumber + searchRadius]
   * 2. Normalize whitespace for comparison
   * 3. Try exact match first (after normalization), prefer closest to target line
   * 4. Fall back to substring match (LLM text in node text, or vice versa)
   * 5. Return smallest matching node closest to target line
   */
  private findNodeByText(
    sourceFile: SourceFile,
    expressionText: string,
    lineNumber?: number,
    searchRadius: number = 5
  ): Node | undefined {
    const allNodes = sourceFile.getDescendants().filter((node) => {
      if (Node.isSourceFile(node)) return false;
      if (node.getKind() === SyntaxKind.SyntaxList) return false;
      return true;
    });
    return this.matchByText(allNodes, expressionText, lineNumber, searchRadius);
  }

  /**
   * Find a CallExpression by matching expression text near a target line.
   */
  private findCallExpressionByText(
    sourceFile: SourceFile,
    expressionText: string,
    lineNumber?: number,
    searchRadius: number = 5
  ): CallExpression | undefined {
    const callExpressions = sourceFile.getDescendantsOfKind(SyntaxKind.CallExpression);
    return this.matchByText(callExpressions, expressionText, lineNumber, searchRadius) as
      | CallExpression
      | undefined;
  }

  /**
   * Shared text-matching logic for node resolution.
   * Normalizes whitespace once per candidate, then tries exact match,
   * then substring match (preferring containing matches).
   */
  private matchByText<T extends Node>(
    nodes: T[],
    expressionText: string,
    lineNumber?: number,
    searchRadius: number = 5
  ): T | undefined {
    const normalizedTarget = this.normalizeWhitespace(expressionText);
    if (!normalizedTarget) return undefined;

    // Filter to nodes within the search window and pre-compute normalized text
    const candidates = (
      lineNumber
        ? nodes.filter((node) => {
            const nodeLine = node.getStartLineNumber();
            return nodeLine >= lineNumber - searchRadius && nodeLine <= lineNumber + searchRadius;
          })
        : nodes
    ).map((node) => ({ node, text: this.normalizeWhitespace(node.getText()) }));

    if (candidates.length === 0) return undefined;

    // Try exact match (normalized whitespace)
    const exactMatches = candidates.filter((c) => c.text === normalizedTarget);
    if (exactMatches.length > 0) {
      return this.pickBestMatch(exactMatches.map((c) => c.node), lineNumber) as T;
    }

    // A bare identifier target must match a node exactly: substring matching
    // would bind `users` to `usersCsv` (or to any enclosing node that merely
    // contains the identifier somewhere) and report a confidently wrong type.
    // Failing here is correct — the caller records an error and the alias
    // pads to `unknown` downstream.
    if (/^[A-Za-z_$][A-Za-z0-9_$]*$/.test(normalizedTarget)) {
      return undefined;
    }

    // Fall back to substring match
    // For the reverse direction (target contains node text), require a minimum node text
    // length to avoid matching tiny identifiers like "res" or "body" too broadly
    const MIN_REVERSE_MATCH_LEN = 8;
    const substringMatches = candidates.filter(
      (c) =>
        c.text.includes(normalizedTarget) ||
        (c.text.length >= MIN_REVERSE_MATCH_LEN && normalizedTarget.includes(c.text))
    );

    if (substringMatches.length > 0) {
      // Prefer nodes where the LLM text is contained in the node text
      const containingMatches = substringMatches.filter((c) =>
        c.text.includes(normalizedTarget)
      );

      if (containingMatches.length > 0) {
        return this.pickBestMatch(containingMatches.map((c) => c.node), lineNumber) as T;
      }

      return this.pickBestMatch(substringMatches.map((c) => c.node), lineNumber) as T;
    }

    return undefined;
  }

  /**
   * Pick the best match from a set of candidate nodes:
   * smallest range, then closest to target line.
   */
  private pickBestMatch(nodes: Node[], targetLine?: number): Node {
    return nodes.reduce((best, current) => {
      const bestRange = best.getEnd() - best.getStart();
      const currentRange = current.getEnd() - current.getStart();

      // Prefer smaller nodes
      if (currentRange !== bestRange) {
        return currentRange < bestRange ? current : best;
      }

      // Tie-break by proximity to target line
      if (targetLine !== undefined) {
        const bestDist = Math.abs(best.getStartLineNumber() - targetLine);
        const currentDist = Math.abs(current.getStartLineNumber() - targetLine);
        return currentDist < bestDist ? current : best;
      }

      return best;
    });
  }

  /**
   * Normalize whitespace for text comparison:
   * collapse runs of whitespace into single spaces, trim.
   */
  private normalizeWhitespace(text: string): string {
    return text.replace(/\s+/g, ' ').trim();
  }

  /**
   * Format a human-readable location string for error messages.
   */
  private formatRequestLocation(request: InferRequestItem): string {
    return request.expression_text
      ? `text="${request.expression_text}" line=${request.expression_line ?? '?'}`
      : `${request.span_start}-${request.span_end}`;
  }

  // ===========================================================================
  // Span-Based Node Lookup (SWC byte offsets)
  // ===========================================================================

  private findContainingFunctionBySpan(
    sourceFile: SourceFile,
    spanStart: number,
    spanEnd: number
  ): FunctionLike | undefined {
    const functions = sourceFile.getDescendants().filter(
      (node): node is FunctionLike =>
        Node.isFunctionDeclaration(node) ||
        Node.isArrowFunction(node) ||
        Node.isFunctionExpression(node) ||
        Node.isMethodDeclaration(node)
    );

    const containing = functions.filter((func) => {
      const start = func.getStart();
      const end = func.getEnd();
      return start <= spanStart && spanEnd <= end;
    });

    if (containing.length === 0) {
      return undefined;
    }

    // Return innermost function
    return containing.reduce((innermost, current) => {
      const innermostRange = innermost.getEnd() - innermost.getStart();
      const currentRange = current.getEnd() - current.getStart();
      return currentRange < innermostRange ? current : innermost;
    });
  }

  private findNodeAtSpan(
    sourceFile: SourceFile,
    spanStart: number,
    spanEnd: number
  ): Node | undefined {
    const allNodes = sourceFile.getDescendants();
    const containing = allNodes.filter((node) => {
      if (Node.isSourceFile(node)) return false;
      // Skip SyntaxList nodes as they have unreliable types
      if (node.getKind() === SyntaxKind.SyntaxList) return false;
      const start = node.getStart();
      const end = node.getEnd();
      return start <= spanStart && spanEnd <= end;
    });

    if (containing.length === 0) {
      return undefined;
    }

    return containing.reduce((best, current) => {
      const bestRange = best.getEnd() - best.getStart();
      const currentRange = current.getEnd() - current.getStart();

      // Prefer exact matches, then smallest containing
      const bestDelta = Math.abs(bestRange - (spanEnd - spanStart));
      const currentDelta = Math.abs(currentRange - (spanEnd - spanStart));

      return currentDelta < bestDelta ? current : best;
    });
  }

  private findCallExpressionAtSpan(
    sourceFile: SourceFile,
    spanStart: number,
    spanEnd: number
  ): CallExpression | undefined {
    const callExpressions = sourceFile
      .getDescendantsOfKind(SyntaxKind.CallExpression);
    const candidates = callExpressions.filter((expr) => {
      const start = expr.getStart();
      const end = expr.getEnd();
      return start <= spanStart && spanEnd <= end;
    });

    if (candidates.length === 0) {
      return undefined;
    }

    return candidates.reduce((best, current) => {
      const bestRange = best.getEnd() - best.getStart();
      const currentRange = current.getEnd() - current.getStart();

      const bestDelta = Math.abs(bestRange - (spanEnd - spanStart));
      const currentDelta = Math.abs(currentRange - (spanEnd - spanStart));

      return currentDelta < bestDelta ? current : best;
    });
  }

  // ===========================================================================
  // Type Utilities
  // ===========================================================================

  private unwrapPromise(typeString: string, type: Type): string {
    // Operate per top-level union member: a naive `^Promise<(.+)>$` regex
    // matches the WHOLE of `Promise<A> | Promise<B>` and produces the
    // mangled capture `A> | Promise<B`.
    const parts = this.splitTopLevelUnion(typeString);
    const unwrapped = parts.map((part) => this.unwrapPromiseText(part));
    if (unwrapped.some((u, i) => u !== parts[i])) {
      return [...new Set(unwrapped)].join(' | ');
    }

    // Handle nested Promise via type arguments
    const typeArguments = type.getTypeArguments();
    if (typeArguments.length > 0 && typeString.startsWith('Promise<')) {
      return typeText(typeArguments[0]);
    }

    return typeString;
  }

  /**
   * Unwrap a single `Promise<...>` type string, only when the inner text is
   * bracket-balanced (so `Promise<A> | B` is left alone for the caller's
   * union handling rather than mangled).
   */
  private unwrapPromiseText(part: string): string {
    if (!part.startsWith('Promise<') || !part.endsWith('>')) {
      return part;
    }
    const inner = part.slice('Promise<'.length, -1);
    return this.isBracketBalanced(inner) ? inner : part;
  }

  /**
   * Split a type string on `|` at bracket depth 0. `=>` is not treated as a
   * closing bracket.
   */
  private splitTopLevelUnion(typeString: string): string[] {
    const parts: string[] = [];
    let depth = 0;
    let start = 0;
    for (let i = 0; i < typeString.length; i++) {
      const ch = typeString[i];
      if (ch === '>' && typeString[i - 1] === '=') continue;
      if (ch === '<' || ch === '(' || ch === '[' || ch === '{') depth++;
      else if (ch === '>' || ch === ')' || ch === ']' || ch === '}') depth--;
      else if (ch === '|' && depth === 0) {
        parts.push(typeString.slice(start, i).trim());
        start = i + 1;
      }
    }
    parts.push(typeString.slice(start).trim());
    return parts;
  }

  private isBracketBalanced(text: string): boolean {
    let depth = 0;
    for (let i = 0; i < text.length; i++) {
      const ch = text[i];
      if (ch === '>' && text[i - 1] === '=') continue;
      if (ch === '<' || ch === '(' || ch === '[' || ch === '{') depth++;
      else if (ch === '>' || ch === ')' || ch === ']' || ch === '}') depth--;
      if (depth < 0) return false;
    }
    return depth === 0;
  }

  // ===========================================================================
  // Result Building
  // ===========================================================================

  private getNodeLocation(node: Node): SourceLocation {
    const startLinePos = node.getStartLineNumber();
    const endLinePos = node.getEndLineNumber();

    return {
      file_path: node.getSourceFile().getFilePath(),
      start_line: startLinePos,
      end_line: endLinePos,
      start_column: node.getStart() - node.getStartLinePos(),
      end_column: node.getEnd() - node.getStartLinePos(),
    };
  }

  private createInferredType(
    request: InferRequestItem,
    typeString: string,
    isExplicit: boolean,
    sourceLocation: SourceLocation,
    payloadTypeString?: string
  ): InferredType {
    const alias =
      request.alias ||
      this.generateAlias(request.file_path, request.line_number, request.infer_kind);

    return {
      alias,
      type_string: typeString,
      is_explicit: isExplicit,
      source_location: sourceLocation,
      infer_kind: request.infer_kind,
      payload_type_string: payloadTypeString,
    };
  }

  private generateAlias(
    filePath: string,
    lineNumber: number,
    inferKind: InferKind
  ): string {
    const fileName = filePath
      .split('/')
      .pop()
      ?.replace(/\.(ts|tsx|js|jsx)$/, '') || 'unknown';

    const pascalName = fileName
      .split(/[-_.]/)
      .map((part) => part.charAt(0).toUpperCase() + part.slice(1))
      .join('');

    const suffix = this.inferKindSuffix(inferKind);
    return `${pascalName}${suffix}L${lineNumber}`;
  }

  private inferKindSuffix(inferKind: InferKind): string {
    switch (inferKind) {
      case 'function_return':
        return 'Return';
      case 'response_body':
        return 'Response';
      case 'call_result':
        return 'Result';
      case 'variable':
        return 'Var';
      case 'expression':
        return 'Expr';
      case 'request_body':
        return 'Request';
      case 'signature_return':
        return 'SigReturn';
      case 'function_param':
        return 'Param';
      default:
        return 'Type';
    }
  }

  // ===========================================================================
  // Logging
  // ===========================================================================

  private log(message: string): void {
    console.error(`[sidecar:type-inferrer] ${message}`);
  }

  private logError(message: string): void {
    console.error(`[sidecar:type-inferrer:error] ${message}`);
  }
}
