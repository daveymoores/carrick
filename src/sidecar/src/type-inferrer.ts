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
  type PropertyAccessExpression,
  type TypeReferenceNode,
  type Type,
  type Symbol as TsSymbol,
} from 'ts-morph';
import type {
  InferRequestItem,
  InferResult,
  InferredType,
  InferKind,
  SourceLocation,
  WrapperRule,
  ExtractionConfig,
  ExtractionRule,
} from './types.js';

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
 * TypeInferrer - Extracts types from source code, both explicit and inferred
 *
 * Usage:
 *   const inferrer = new TypeInferrer({ project });
 *   const result = inferrer.infer(requests, undefined, extractionConfig);
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
   * @param wrappers - Legacy wrapper rules (deprecated, use extractionConfig)
   * @param extractionConfig - New extraction config for payload unwrapping
   * @returns InferResult with inferred types or errors
   */
  infer(
    requests: InferRequestItem[],
    wrappers: WrapperRule[] = [],
    extractionConfig?: ExtractionConfig
  ): InferResult {
    const inferredTypes: InferredType[] = [];
    const errors: string[] = [];

    for (const request of requests) {
      try {
        const loc = this.formatRequestLocation(request);
        const result = this.inferSingle(request, wrappers, extractionConfig);
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
    wrappers: WrapperRule[],
    extractionConfig?: ExtractionConfig
  ): InferredType | null {
    const sourceFile = this.getSourceFile(request.file_path);
    if (!sourceFile) {
      this.logError(`Source file not found: ${request.file_path}`);
      return null;
    }

    switch (request.infer_kind) {
      case 'function_return':
        return this.inferFunctionReturn(sourceFile, request, wrappers, extractionConfig);
      case 'response_body':
        return this.inferResponseBody(sourceFile, request, wrappers, extractionConfig);
      case 'call_result':
        return this.inferCallResult(sourceFile, request, wrappers, extractionConfig);
      case 'variable':
        return this.inferVariable(sourceFile, request, wrappers, extractionConfig);
      case 'expression':
        return this.inferExpression(sourceFile, request, wrappers, extractionConfig);
      case 'request_body':
        return this.inferRequestBody(sourceFile, request, wrappers, extractionConfig);
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
    wrappers: WrapperRule[],
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
    let typeString = returnType.getText(func);

    // Apply extraction config or legacy wrappers
    const unwrapResult = this.unwrapTypeWithConfig(
      returnType,
      func,
      extractionConfig,
      wrappers
    );
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

  private inferResponseBody(
    sourceFile: SourceFile,
    request: InferRequestItem,
    wrappers: WrapperRule[],
    extractionConfig?: ExtractionConfig
  ): InferredType | null {
    const node = this.resolveTargetNode(sourceFile, request);

    if (!node) {
      // No locator, or locator didn't resolve — likely a payload-less handler
      // (redirect, 204, streaming). Infer the containing function's return type.
      this.log(
        `No payload node found for request at ${request.file_path}:${request.line_number}; falling back to function return`
      );
      return this.inferFunctionReturn(sourceFile, request, wrappers, extractionConfig);
    }

    // The resolved node IS the payload subexpression in the MVP schema.
    // Transitional fallback: if a caller still supplies a bare call expression
    // (e.g., `res.json(users)`), drill to its first argument. No method-name list.
    let payloadNode: Node = node;
    if (Node.isCallExpression(node)) {
      const args = node.getArguments();
      if (args.length > 0) {
        payloadNode = args[0];
      }
    }

    const payloadType = payloadNode.getType();
    let typeString = payloadType.getText(payloadNode);

    const unwrapResult = this.unwrapTypeWithConfig(
      payloadType,
      payloadNode,
      extractionConfig,
      wrappers
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
    wrappers: WrapperRule[],
    extractionConfig?: ExtractionConfig
  ): InferredType | null {
    const callExpr = this.resolveTargetCallExpression(sourceFile, request);

    if (!callExpr) {
      return this.inferExpression(sourceFile, request, wrappers, extractionConfig);
    }

    // Walk up from the already-found call expression instead of re-searching
    const func = this.findContainingFunctionForNode(callExpr);
    const terminalNode = this.resolveCallResultTerminalNode(callExpr, func);
    const returnType = terminalNode.getType();
    let typeString = returnType.getText(terminalNode);
    let isExplicit = false;

    // Try extraction config first, then legacy wrappers
    const unwrapResult = this.unwrapTypeWithConfig(
      returnType,
      terminalNode,
      extractionConfig,
      wrappers
    );

    if (unwrapResult.wasUnwrapped) {
      typeString = unwrapResult.typeString;
      isExplicit = unwrapResult.isExplicit;
    } else {
      // Legacy wrapper resolution
      const wrapperResolution = this.resolveWrapperType(
        terminalNode,
        returnType,
        wrappers
      );
      if (wrapperResolution) {
        typeString = wrapperResolution.typeString;
        isExplicit = wrapperResolution.isExplicit;
      }
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
    wrappers: WrapperRule[],
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
      return this.inferExpression(sourceFile, request, wrappers, extractionConfig);
    }

    const typeNode = varDecl.getTypeNode();
    const isExplicit = typeNode !== undefined;
    let varType = varDecl.getType();
    let typeString = varType.getText(varDecl);

    // Apply extraction config
    const unwrapResult = this.unwrapTypeWithConfig(
      varType,
      varDecl,
      extractionConfig,
      wrappers
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
    wrappers: WrapperRule[],
    extractionConfig?: ExtractionConfig
  ): InferredType | null {
    const node = this.resolveTargetNode(sourceFile, request);

    if (!node) {
      return null;
    }

    const type = node.getType();
    let typeString = type.getText(node);

    // Apply extraction config
    const unwrapResult = this.unwrapTypeWithConfig(type, node, extractionConfig, wrappers);
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
    wrappers: WrapperRule[],
    extractionConfig?: ExtractionConfig
  ): InferredType | null {
    const node = this.resolveTargetNode(sourceFile, request);

    if (!node) {
      return null;
    }

    const payloadType = node.getType();
    let typeString = payloadType.getText(node);

    // Apply extraction config
    const unwrapResult = this.unwrapTypeWithConfig(
      payloadType,
      node,
      extractionConfig,
      wrappers
    );
    if (unwrapResult.wasUnwrapped) {
      typeString = unwrapResult.typeString;
    }

    return this.createInferredType(
      request,
      typeString,
      false,
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

  /**
   * Unwrap a type using the new ExtractionConfig system.
   * Falls back to legacy wrappers if extractionConfig is not provided.
   */
  private unwrapTypeWithConfig(
    type: Type,
    node: Node,
    extractionConfig?: ExtractionConfig,
    legacyWrappers?: WrapperRule[]
  ): UnwrapResult {
    // If no extraction config, try legacy wrappers
    if (!extractionConfig || extractionConfig.rules.length === 0) {
      if (legacyWrappers && legacyWrappers.length > 0) {
        const result = this.resolveWrapperType(node, type, legacyWrappers);
        if (result) {
          return {
            typeString: result.typeString,
            isExplicit: result.isExplicit,
            wasUnwrapped: true,
          };
        }
      }
      return {
        typeString: type.getText(node),
        isExplicit: false,
        wasUnwrapped: false,
      };
    }

    return this.unwrapType(type, node, extractionConfig, 0);
  }

  /**
   * Core unwrapping implementation with ExtractionConfig rules.
   *
   * Requirements:
   * 1. Exact wrapperSymbols match wins immediately
   * 2. machineryIndicators only trigger unwrap if originModuleGlobs also match
   * 3. Handle unions and intersections
   * 4. Support recursive unwrapping with depth limits
   */
  private unwrapType(
    type: Type,
    node: Node,
    config: ExtractionConfig,
    depth: number
  ): UnwrapResult {
    const maxGlobalDepth = 10; // Safety limit
    if (depth >= maxGlobalDepth) {
      return {
        typeString: type.getText(node),
        isExplicit: false,
        wasUnwrapped: false,
      };
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
        // Dedupe and join
        const unique = [...new Set(unwrappedParts)];
        return {
          typeString: unique.length === 1 ? unique[0] : unique.join(' | '),
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

    // Try each rule
    for (const rule of config.rules) {
      const result = this.tryUnwrapWithRule(type, node, rule, config, depth);
      if (result.wasUnwrapped) {
        return result;
      }
    }

    return {
      typeString: type.getText(node),
      isExplicit: false,
      wasUnwrapped: false,
    };
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
  ): UnwrapResult {
    const maxDepth = rule.maxDepth ?? 4;
    if (depth >= maxDepth) {
      return {
        typeString: type.getText(node),
        isExplicit: false,
        wasUnwrapped: false,
      };
    }

    const symbol = type.getSymbol() || type.getAliasSymbol();
    const symbolName = symbol?.getName();

    // 1. Check exact wrapperSymbols match
    if (rule.wrapperSymbols && symbolName && rule.wrapperSymbols.includes(symbolName)) {
      return this.extractPayloadFromWrapper(type, node, rule, config, depth);
    }

    // 2. Check machineryIndicators + originModuleGlobs
    if (rule.machineryIndicators && rule.machineryIndicators.length > 0) {
      // Only proceed if we also have originModuleGlobs
      if (!rule.originModuleGlobs || rule.originModuleGlobs.length === 0) {
        // Skip: machineryIndicators alone are too many false positives
        return {
          typeString: type.getText(node),
          isExplicit: false,
          wasUnwrapped: false,
        };
      }

      // Check if the type has machinery indicators (properties/methods)
      const hasMachineryIndicators = this.typeHasMachineryIndicators(type, rule.machineryIndicators);
      if (!hasMachineryIndicators) {
        return {
          typeString: type.getText(node),
          isExplicit: false,
          wasUnwrapped: false,
        };
      }

      // Check if symbol originates from allowed modules
      const originatesFromAllowed = this.symbolOriginatesFromModules(symbol, rule.originModuleGlobs);
      if (!originatesFromAllowed) {
        return {
          typeString: type.getText(node),
          isExplicit: false,
          wasUnwrapped: false,
        };
      }

      return this.extractPayloadFromWrapper(type, node, rule, config, depth);
    }

    return {
      typeString: type.getText(node),
      isExplicit: false,
      wasUnwrapped: false,
    };
  }

  /**
   * Extract the payload type from a matched wrapper.
   */
  private extractPayloadFromWrapper(
    type: Type,
    node: Node,
    rule: ExtractionRule,
    config: ExtractionConfig,
    depth: number
  ): UnwrapResult {
    // 1. Try generic type argument at payloadGenericIndex
    const genericIndex = rule.payloadGenericIndex ?? 0;
    const typeArgs = type.getTypeArguments();

    if (typeArgs.length > genericIndex) {
      const payloadArg = typeArgs[genericIndex];

      // Check if it's a useful type (not any/unknown/never)
      const argText = payloadArg.getText(node);
      if (!this.isUselessType(argText)) {
        // Recursive unwrap if configured
        if (rule.unwrapRecursively) {
          return this.unwrapType(payloadArg, node, config, depth + 1);
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
        const text = argType.getText(node);
        if (!this.isUselessType(text)) {
          if (rule.unwrapRecursively) {
            return this.unwrapType(argType, node, config, depth + 1);
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
        const propText = currentType.getText(node);
        if (!this.isUselessType(propText)) {
          if (rule.unwrapRecursively) {
            return this.unwrapType(currentType, node, config, depth + 1);
          }
          return {
            typeString: propText,
            isExplicit: false,
            wasUnwrapped: true,
          };
        }
      }
    }

    // Fallback: return type unchanged
    return {
      typeString: type.getText(node),
      isExplicit: false,
      wasUnwrapped: false,
    };
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
   * Supports: exact match, "*" suffix, and "package/*" patterns.
   */
  private filePathMatchesModuleGlob(filePath: string, glob: string): boolean {
    const normalizedPath = filePath.replace(/\\/g, '/');

    // Check if path contains node_modules/<glob>
    const nodeModulesPattern = `node_modules/${glob.replace(/\*/g, '')}`;
    if (normalizedPath.includes(nodeModulesPattern)) {
      return true;
    }

    // Also check for @types/<glob>
    if (glob.startsWith('@types/')) {
      if (normalizedPath.includes(`node_modules/${glob.replace(/\*/g, '')}`)) {
        return true;
      }
    } else {
      // Auto-check @types version
      const typesGlob = `@types/${glob.replace('@', '').replace(/\*/g, '')}`;
      if (normalizedPath.includes(`node_modules/${typesGlob}`)) {
        return true;
      }
    }

    return false;
  }

  /**
   * Check if a type string is "useless" for payload purposes.
   */
  private isUselessType(typeString: string): boolean {
    const useless = ['any', 'unknown', 'never', 'void', 'undefined', 'null', 'object', '{}'];
    const trimmed = typeString.trim();
    return useless.includes(trimmed) || trimmed === '';
  }

  // ===========================================================================
  // Legacy Wrapper Unwrapping (Preserved for backwards compatibility)
  // ===========================================================================

  private resolveWrapperType(
    node: Node,
    type: Type,
    wrappers: WrapperRule[]
  ): { typeString: string; isExplicit: boolean } | null {
    if (wrappers.length === 0) {
      return null;
    }

    for (const wrapper of wrappers) {
      if (wrapper.unwrap.kind === 'property') {
        const propertyAccess = this.getPropertyAccessNode(node);
        if (propertyAccess) {
          const baseExpr = propertyAccess.getExpression();
          const baseType = baseExpr.getType();
          if (this.matchesWrapperType(baseType, baseExpr, wrapper)) {
            if (propertyAccess.getName() === wrapper.unwrap.property) {
              const propertyType = propertyAccess.getType();
              let typeString = propertyType.getText(propertyAccess);
              typeString = this.unwrapPromise(typeString, propertyType);
              return { typeString, isExplicit: false };
            }
            return { typeString: 'unknown', isExplicit: false };
          }
        }

        if (this.matchesWrapperType(type, node, wrapper)) {
          return { typeString: 'unknown', isExplicit: false };
        }

        continue;
      }

      if (wrapper.unwrap.kind === 'generic_param') {
        if (!this.matchesWrapperType(type, node, wrapper)) {
          continue;
        }

        const explicitArg = this.findExplicitWrapperTypeArgument(node, wrapper);
        if (!explicitArg) {
          return { typeString: 'unknown', isExplicit: false };
        }

        return { typeString: explicitArg.typeString, isExplicit: true };
      }
    }

    return null;
  }

  private findExplicitWrapperTypeArgument(
    node: Node,
    wrapper: WrapperRule
  ): { typeString: string } | null {
    const index = wrapper.unwrap.index;
    if (index === undefined) {
      return null;
    }

    const callExpr = this.getCallExpressionFromNode(node);
    if (callExpr) {
      const typeArgs = callExpr.getTypeArguments();
      if (typeArgs.length > index) {
        return { typeString: typeArgs[index].getText() };
      }
    }

    const typeRef = this.findWrapperTypeReference(node, wrapper.type_name);
    if (typeRef) {
      const typeArgs = typeRef.getTypeArguments();
      if (typeArgs.length > index) {
        return { typeString: typeArgs[index].getText() };
      }
    }

    return null;
  }

  private getPropertyAccessNode(
    node: Node
  ): PropertyAccessExpression | null {
    const unwrapped = this.unwrapExpressionNode(node);
    if (Node.isPropertyAccessExpression(unwrapped)) {
      return unwrapped;
    }
    return null;
  }

  private getCallExpressionFromNode(node: Node): CallExpression | null {
    const unwrapped = this.unwrapExpressionNode(node);
    if (Node.isCallExpression(unwrapped)) {
      return unwrapped;
    }
    return null;
  }

  private findWrapperTypeReference(
    node: Node,
    typeName: string
  ): TypeReferenceNode | null {
    const typeRefs = node.getDescendantsOfKind(SyntaxKind.TypeReference);
    for (const ref of typeRefs) {
      const nameText = ref.getTypeName().getText();
      if (nameText === typeName || nameText.endsWith(`.${typeName}`)) {
        return ref;
      }
    }
    return null;
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

  private matchesWrapperType(
    type: Type,
    node: Node,
    wrapper: WrapperRule
  ): boolean {
    const symbol = type.getSymbol();
    if (!symbol) {
      return false;
    }

    if (symbol.getName() !== wrapper.type_name) {
      return false;
    }

    if (!this.declarationFromPackage(symbol, wrapper.package)) {
      if (!this.sourceFileImportsWrapper(node.getSourceFile(), wrapper)) {
        return false;
      }

      const aliasedSymbol = symbol.getAliasedSymbol?.();
      if (!aliasedSymbol) {
        return false;
      }
      if (!this.declarationFromPackage(aliasedSymbol, wrapper.package)) {
        return false;
      }
    }

    return true;
  }

  private sourceFileImportsWrapper(
    sourceFile: SourceFile,
    wrapper: WrapperRule
  ): boolean {
    for (const importDecl of sourceFile.getImportDeclarations()) {
      const moduleSpecifier = importDecl.getModuleSpecifierValue();
      if (
        moduleSpecifier === wrapper.package ||
        moduleSpecifier.startsWith(`${wrapper.package}/`)
      ) {
        return true;
      }
    }
    return false;
  }

  private declarationFromPackage(symbol: TsSymbol, packageName: string): boolean {
    const filePath = symbol.getDeclarations()?.[0]?.getSourceFile()?.getFilePath();
    const normalized = filePath?.replace(/\\/g, '/');
    return !!normalized && normalized.includes(`node_modules/${packageName}/`);
  }

  private extractExplicitTypeFromAncestor(node: Node): string | null {
    const varDecl = node.getFirstAncestorByKind(SyntaxKind.VariableDeclaration);
    if (varDecl) {
      const typeNode = varDecl.getTypeNode();
      if (typeNode) {
        return typeNode.getText();
      }
    }

    const asExpr = node.getFirstAncestorByKind(SyntaxKind.AsExpression);
    if (asExpr) {
      const typeNode = asExpr.getTypeNode();
      if (typeNode) {
        return typeNode.getText();
      }
    }

    const typeAssertion = node.getFirstAncestorByKind(
      SyntaxKind.TypeAssertionExpression
    );
    if (typeAssertion) {
      const typeNode = typeAssertion.getTypeNode();
      if (typeNode) {
        return typeNode.getText();
      }
    }

    return null;
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
    return undefined;
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
    const promiseMatch = typeString.match(/^Promise<(.+)>$/);
    if (promiseMatch) {
      return promiseMatch[1];
    }

    // Handle nested Promise via type arguments
    const typeArguments = type.getTypeArguments();
    if (typeArguments.length > 0 && typeString.startsWith('Promise<')) {
      return typeArguments[0].getText();
    }

    return typeString;
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
