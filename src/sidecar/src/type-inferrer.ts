/**
 * Type Inferrer - Scope-based type inference for implicit types
 *
 * This module extracts types even when developers don't write explicit
 * annotations. It uses span-based node lookup (no line windows) to target
 * precise expressions provided by the Rust/LLM pipeline.
 *
 * Framework-agnostic patterns detected:
 * - res.json(data) / res.send(data) - Express/Fastify style
 * - ctx.body = data - Koa style
 * - return data / return Response.json(data) - Hono/Web API style
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
} from 'ts-morph';
import type {
  InferRequestItem,
  InferResult,
  InferredType,
  InferKind,
  SourceLocation,
  WrapperRule,
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
 * TypeInferrer - Extracts types from source code, both explicit and inferred
 *
 * Usage:
 *   const inferrer = new TypeInferrer({ project });
 *   const result = inferrer.infer(requests);
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
   * @returns InferResult with inferred types or errors
   */
  infer(requests: InferRequestItem[], wrappers: WrapperRule[] = []): InferResult {
    const inferredTypes: InferredType[] = [];
    const errors: string[] = [];

    for (const request of requests) {
      try {
        const result = this.inferSingle(request, wrappers);
        if (result) {
          inferredTypes.push(result);
        } else {
          errors.push(
            `Could not infer type at ${request.file_path}:${request.span_start}-${request.span_end} (${request.infer_kind})`
          );
        }
      } catch (err) {
        const error = err instanceof Error ? err.message : String(err);
        errors.push(
          `Error inferring type at ${request.file_path}:${request.span_start}-${request.span_end}: ${error}`
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
   * Infer a single type based on the request
   */
  private inferSingle(
    request: InferRequestItem,
    wrappers: WrapperRule[]
  ): InferredType | null {
    const sourceFile = this.getSourceFile(request.file_path);
    if (!sourceFile) {
      this.logError(`Source file not found: ${request.file_path}`);
      return null;
    }

    switch (request.infer_kind) {
      case 'function_return':
        return this.inferFunctionReturn(sourceFile, request);
      case 'response_body':
        return this.inferResponseBody(sourceFile, request);
      case 'call_result':
        return this.inferCallResult(sourceFile, request, wrappers);
      case 'variable':
        return this.inferVariable(sourceFile, request);
      case 'expression':
        return this.inferExpression(sourceFile, request);
      case 'request_body':
        return this.inferRequestBody(sourceFile, request);
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
      } catch {
        // File might not exist
        return undefined;
      }
    }

    return sourceFile;
  }

  // ===========================================================================
  // Inference Strategies
  // ===========================================================================

  /**
   * Infer the return type of a function containing the target span
   */
  private inferFunctionReturn(
    sourceFile: SourceFile,
    request: InferRequestItem
  ): InferredType | null {
    const func = this.findContainingFunctionBySpan(
      sourceFile,
      request.span_start,
      request.span_end
    );
    if (!func) {
      this.log(
        `No function found for span ${request.span_start}-${request.span_end}`
      );
      return null;
    }

    // Check for explicit return type annotation
    const returnTypeNode = func.getReturnTypeNode();
    const isExplicit = returnTypeNode !== undefined;

    // Get the return type (explicit or inferred)
    let returnType = func.getReturnType();
    let typeString = returnType.getText(func);

    // Unwrap Promise<T> to T for response types
    typeString = this.unwrapPromise(typeString, returnType);

    return this.createInferredType(
      request,
      typeString,
      isExplicit,
      this.getNodeLocation(func, sourceFile)
    );
  }

  /**
   * Infer the response body type from framework-agnostic patterns
   *
   * This searches the ENTIRE containing function body for terminal statements,
   * handling large handlers with middleware/validation/logging before response.
   */
  private inferResponseBody(
    sourceFile: SourceFile,
    request: InferRequestItem
  ): InferredType | null {
    const node = this.findNodeAtSpan(
      sourceFile,
      request.span_start,
      request.span_end
    );
    if (!node) {
      this.log(
        `No response expression found for span ${request.span_start}-${request.span_end}`
      );
      return null;
    }

    const payloadNode = this.resolveResponsePayloadNode(node);
    if (!payloadNode) {
      return null;
    }

    const payloadType = payloadNode.getType();
    let typeString = payloadType.getText(payloadNode);

    typeString = this.unwrapPromise(typeString, payloadType);

    return this.createInferredType(
      request,
      typeString,
      false,
      this.getNodeLocation(payloadNode, sourceFile)
    );
  }

  /**
   * Infer the return type of a call expression containing the target span
   */
  private inferCallResult(
    sourceFile: SourceFile,
    request: InferRequestItem,
    wrappers: WrapperRule[]
  ): InferredType | null {
    const callExpr = this.findCallExpressionAtSpan(
      sourceFile,
      request.span_start,
      request.span_end
    );
    if (!callExpr) {
      this.log(
        `No call expression found for span ${request.span_start}-${request.span_end}`
      );
      return null;
    }

    const func = this.findContainingFunctionBySpan(
      sourceFile,
      callExpr.getStart(),
      callExpr.getEnd()
    );
    const terminalNode = this.resolveCallResultTerminalNode(callExpr, func);

    const returnType = terminalNode.getType();
    let typeString = returnType.getText(terminalNode);
    let isExplicit = false;

    typeString = this.unwrapPromise(typeString, returnType);

    const wrapperResolution = this.resolveWrapperType(
      terminalNode,
      returnType,
      wrappers
    );
    if (wrapperResolution) {
      return this.createInferredType(
        request,
        wrapperResolution.typeString,
        wrapperResolution.isExplicit,
        this.getNodeLocation(terminalNode, sourceFile)
      );
    }

    const explicitType = this.extractExplicitTypeFromAncestor(terminalNode);
    if (explicitType) {
      typeString = explicitType;
      isExplicit = true;
    }

    return this.createInferredType(
      request,
      typeString,
      isExplicit,
      this.getNodeLocation(terminalNode, sourceFile)
    );
  }

  /**
   * Infer the type of a variable containing the target span
   */
  private inferVariable(
    sourceFile: SourceFile,
    request: InferRequestItem
  ): InferredType | null {
    const node = this.findNodeAtSpan(
      sourceFile,
      request.span_start,
      request.span_end
    );
    if (!node) return null;

    // Find variable declaration
    const varDecl =
      node.getKind() === SyntaxKind.VariableDeclaration
        ? node
        : node.getFirstAncestorByKind(SyntaxKind.VariableDeclaration);

    if (!varDecl || !Node.isVariableDeclaration(varDecl)) {
      this.log(
        `No variable declaration found for span ${request.span_start}-${request.span_end}`
      );
      return null;
    }

    // Check for explicit type annotation
    const typeNode = varDecl.getTypeNode();
    const isExplicit = typeNode !== undefined;

    let varType = varDecl.getType();
    let typeString = varType.getText(varDecl);

    // Unwrap Promise<T>
    typeString = this.unwrapPromise(typeString, varType);

    return this.createInferredType(
      request,
      typeString,
      isExplicit,
      this.getNodeLocation(varDecl, sourceFile)
    );
  }

  /**
   * Infer the type of an expression containing the target span
   */
  private inferExpression(
    sourceFile: SourceFile,
    request: InferRequestItem
  ): InferredType | null {
    const node = this.findNodeAtSpan(
      sourceFile,
      request.span_start,
      request.span_end
    );
    if (!node) return null;

    // Get the type of whatever node we found
    const type = node.getType();
    let typeString = type.getText(node);

    // Unwrap Promise<T>
    typeString = this.unwrapPromise(typeString, type);

    return this.createInferredType(
      request,
      typeString,
      false,
      this.getNodeLocation(node, sourceFile)
    );
  }

  /**
   * Infer request body type from handlers or call payloads
   */
  private inferRequestBody(
    sourceFile: SourceFile,
    request: InferRequestItem
  ): InferredType | null {
    const node = this.findNodeAtSpan(
      sourceFile,
      request.span_start,
      request.span_end
    );
    if (!node) {
      this.log(
        `No request payload found for span ${request.span_start}-${request.span_end}`
      );
      return null;
    }

    const payloadType = node.getType();
    let typeString = payloadType.getText(node);

    typeString = this.unwrapPromise(typeString, payloadType);

    return this.createInferredType(
      request,
      typeString,
      false,
      this.getNodeLocation(node, sourceFile)
    );
  }

  // ===========================================================================
  // Response Helpers
  // ===========================================================================

  private resolveResponsePayloadNode(node: Node): Node | null {
    if (Node.isExpressionStatement(node)) {
      return this.resolveResponsePayloadNode(node.getExpression());
    }

    if (Node.isReturnStatement(node)) {
      const expr = node.getExpression();
      return expr ? this.resolveResponsePayloadNode(expr) : null;
    }

    if (Node.isBinaryExpression(node)) {
      if (node.getOperatorToken().getKind() === SyntaxKind.EqualsToken) {
        return this.resolveResponsePayloadNode(node.getRight());
      }
    }

    if (Node.isCallExpression(node)) {
      const args = node.getArguments();
      if (args.length === 0) {
        return null;
      }
      return args[0];
    }

    return node;
  }

  // ===========================================================================
  // Call Result Def-Use
  // ===========================================================================

  private resolveCallResultTerminalNode(
    callExpr: CallExpression,
    func: FunctionLike | null
  ): Node {
    const returnStmt = callExpr.getFirstAncestorByKind(SyntaxKind.ReturnStatement);
    if (returnStmt) {
      return returnStmt.getExpression() ?? callExpr;
    }

    if (!func) {
      return callExpr;
    }

    const binding = this.extractBindingFromCall(callExpr);
    if (!binding || binding.names.length === 0) {
      return callExpr;
    }

    let currentNames = new Set(binding.names);
    let lastNode: Node = binding.node ?? callExpr;
    const startPos = callExpr.getStart();

    const candidates = this.collectDefUseNodes(func, startPos);
    for (const node of candidates) {
      if (Node.isReturnStatement(node)) {
        const expr = node.getExpression();
        if (expr && this.expressionUsesNames(expr, currentNames)) {
          return expr;
        }
        continue;
      }

      if (Node.isVariableDeclaration(node)) {
        const initializer = node.getInitializer();
        if (!initializer) {
          continue;
        }
        if (!this.expressionUsesNames(initializer, currentNames)) {
          continue;
        }
        const names = this.extractBindingNames(node.getNameNode());
        if (names.length > 0) {
          currentNames = new Set(names);
          lastNode = this.getPrimaryBindingNode(node.getNameNode()) ?? node;
        }
        continue;
      }

      if (Node.isBinaryExpression(node)) {
        if (node.getOperatorToken().getKind() !== SyntaxKind.EqualsToken) {
          continue;
        }
        const right = node.getRight();
        if (!this.expressionUsesNames(right, currentNames)) {
          continue;
        }
        const names = this.extractBindingNames(node.getLeft());
        if (names.length > 0) {
          currentNames = new Set(names);
          lastNode = this.getPrimaryBindingNode(node.getLeft()) ?? node;
        }
      }
    }

    return lastNode;
  }

  private extractBindingFromCall(
    callExpr: CallExpression
  ): { names: string[]; node?: Node } | null {
    const varDecl = callExpr.getFirstAncestorByKind(SyntaxKind.VariableDeclaration);
    if (varDecl) {
      const initializer = varDecl.getInitializer();
      if (
        initializer &&
        callExpr.getStart() >= initializer.getStart() &&
        callExpr.getEnd() <= initializer.getEnd()
      ) {
        const names = this.extractBindingNames(varDecl.getNameNode());
        const node = this.getPrimaryBindingNode(varDecl.getNameNode()) ?? varDecl;
        return { names, node };
      }
    }

    const assignment = callExpr.getFirstAncestorByKind(SyntaxKind.BinaryExpression);
    if (
      assignment &&
      assignment.getOperatorToken().getKind() === SyntaxKind.EqualsToken
    ) {
      const right = assignment.getRight();
      if (
        callExpr.getStart() >= right.getStart() &&
        callExpr.getEnd() <= right.getEnd()
      ) {
        const names = this.extractBindingNames(assignment.getLeft());
        const node =
          this.getPrimaryBindingNode(assignment.getLeft()) ?? assignment.getLeft();
        return { names, node };
      }
    }

    return null;
  }

  private extractBindingNames(node: Node): string[] {
    if (Node.isIdentifier(node)) {
      return [node.getText()];
    }

    if (Node.isObjectBindingPattern(node) || Node.isArrayBindingPattern(node)) {
      const names: string[] = [];
      for (const element of node.getElements()) {
        if (!Node.isBindingElement(element)) {
          continue;
        }
        const elementName = element.getNameNode();
        names.push(...this.extractBindingNames(elementName));
      }
      return names;
    }

    return [];
  }

  private getPrimaryBindingNode(node: Node): Node | null {
    if (Node.isIdentifier(node)) {
      return node;
    }

    if (Node.isObjectBindingPattern(node) || Node.isArrayBindingPattern(node)) {
      for (const element of node.getElements()) {
        if (!Node.isBindingElement(element)) {
          continue;
        }
        const elementName = element.getNameNode();
        const found = this.getPrimaryBindingNode(elementName);
        if (found) {
          return found;
        }
      }
    }

    return null;
  }

  private collectDefUseNodes(func: FunctionLike, startPos: number): Node[] {
    const candidates: Node[] = [];
    candidates.push(...func.getDescendantsOfKind(SyntaxKind.VariableDeclaration));
    candidates.push(...func.getDescendantsOfKind(SyntaxKind.BinaryExpression));
    candidates.push(...func.getDescendantsOfKind(SyntaxKind.ReturnStatement));

    return candidates
      .filter((node) => {
        if (!this.isInFunctionScope(node, func)) {
          return false;
        }
        return node.getStart() > startPos;
      })
      .sort((a, b) => a.getStart() - b.getStart());
  }

  private expressionUsesNames(node: Node, names: Set<string>): boolean {
    const identifiers = node.getDescendantsOfKind(SyntaxKind.Identifier);
    return identifiers.some((identifier) =>
      this.isIdentifierUsage(identifier, names)
    );
  }

  private isIdentifierUsage(node: Node, names: Set<string>): boolean {
    if (!Node.isIdentifier(node)) {
      return false;
    }

    const text = node.getText();
    if (!names.has(text)) {
      return false;
    }

    const parent = node.getParent();
    if (Node.isVariableDeclaration(parent) && parent.getNameNode() === node) {
      return false;
    }
    if (Node.isParameterDeclaration(parent)) {
      return false;
    }
    if (Node.isFunctionDeclaration(parent) && parent.getNameNode() === node) {
      return false;
    }
    if (Node.isPropertyAccessExpression(parent) && parent.getNameNode() === node) {
      return false;
    }
    if (Node.isPropertyAssignment(parent) && parent.getNameNode() === node) {
      return false;
    }
    if (Node.isBindingElement(parent) && parent.getNameNode() === node) {
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
  // Wrapper Unwrapping
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
    for (const ancestor of node.getAncestors()) {
      if (!Node.isTypeReference(ancestor)) {
        continue;
      }
      const nameText = ancestor.getTypeName().getText();
      if (nameText === typeName || nameText.endsWith(`.${typeName}`)) {
        return ancestor;
      }
    }

    return null;
  }

  private unwrapExpressionNode(node: Node): Node {
    let current = node;
    while (true) {
      if (Node.isAwaitExpression(current)) {
        current = current.getExpression();
        continue;
      }
      if (Node.isParenthesizedExpression(current)) {
        current = current.getExpression();
        continue;
      }
      if (Node.isAsExpression(current)) {
        current = current.getExpression();
        continue;
      }
      if (Node.isTypeAssertion(current)) {
        current = current.getExpression();
        continue;
      }
      break;
    }

    return current;
  }

  private matchesWrapperType(type: Type, node: Node, wrapper: WrapperRule): boolean {
    const symbol = type.getSymbol() ?? type.getAliasSymbol();
    if (!symbol || symbol.getName() !== wrapper.type_name) {
      return false;
    }

    if (
      symbol
        .getDeclarations()
        .some((decl) => this.declarationFromPackage(decl, wrapper.package))
    ) {
      return true;
    }

    const aliasedSymbol = symbol.getAliasedSymbol();
    if (
      aliasedSymbol &&
      aliasedSymbol
        .getDeclarations()
        .some((decl) => this.declarationFromPackage(decl, wrapper.package))
    ) {
      return true;
    }

    return this.sourceFileImportsWrapper(node.getSourceFile(), wrapper);
  }

  private sourceFileImportsWrapper(
    sourceFile: SourceFile,
    wrapper: WrapperRule
  ): boolean {
    for (const decl of sourceFile.getImportDeclarations()) {
      if (decl.getModuleSpecifierValue() !== wrapper.package) {
        continue;
      }
      if (decl.getDefaultImport()?.getText() === wrapper.type_name) {
        return true;
      }
      if (decl.getNamedImports().some((named) => named.getName() === wrapper.type_name)) {
        return true;
      }
    }

    return false;
  }

  private declarationFromPackage(node: Node, packageName: string): boolean {
    const filePath = node.getSourceFile().getFilePath();
    const normalized = filePath.replace(/\\/g, '/');
    return normalized.includes(`/node_modules/${packageName}/`);
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
    if (typeAssertion && Node.isTypeAssertion(typeAssertion)) {
      const typeNode = typeAssertion.getTypeNode();
      if (typeNode) {
        return typeNode.getText();
      }
    }

    return null;
  }

  // ===========================================================================
  // Span-Based Search Utilities
  // ===========================================================================

  /**
   * Find the innermost function containing the target span
   *
   * CRITICAL: This ensures we search the right scope even for nested functions
   */
  private findContainingFunctionBySpan(
    sourceFile: SourceFile,
    spanStart: number,
    spanEnd: number
  ): FunctionLike | null {
    const functions: FunctionLike[] = [];

    // Collect all function-like nodes
    functions.push(...sourceFile.getDescendantsOfKind(SyntaxKind.FunctionDeclaration));
    functions.push(...sourceFile.getDescendantsOfKind(SyntaxKind.ArrowFunction));
    functions.push(...sourceFile.getDescendantsOfKind(SyntaxKind.FunctionExpression));
    functions.push(...sourceFile.getDescendantsOfKind(SyntaxKind.MethodDeclaration));

    // Find all functions that contain the target span
    const containing = functions.filter((func) => {
      const start = func.getStart();
      const end = func.getEnd();
      return spanStart >= start && spanEnd <= end;
    });

    if (containing.length === 0) {
      return null;
    }

    // Return the innermost (smallest range) function
    return containing.reduce((innermost, current) => {
      const innermostRange = innermost.getEnd() - innermost.getStart();
      const currentRange = current.getEnd() - current.getStart();
      return currentRange < innermostRange ? current : innermost;
    });
  }

  /**
   * Find the most specific node that contains the target span
   */
  private findNodeAtSpan(
    sourceFile: SourceFile,
    spanStart: number,
    spanEnd: number
  ): Node | null {
    const allNodes = sourceFile.getDescendants();
    const containing = allNodes.filter((node) => {
      if (node.getKind() === SyntaxKind.SyntaxList) {
        return false;
      }
      const start = node.getStart();
      const end = node.getEnd();
      return spanStart >= start && spanEnd <= end;
    });

    if (containing.length === 0) {
      return null;
    }

    return containing.reduce((best, current) => {
      const bestRange = best.getEnd() - best.getStart();
      const currentRange = current.getEnd() - current.getStart();
      if (currentRange !== bestRange) {
        return currentRange < bestRange ? current : best;
      }
      const bestDelta = Math.abs(spanStart - best.getStart());
      const currentDelta = Math.abs(spanStart - current.getStart());
      if (currentDelta !== bestDelta) {
        return currentDelta < bestDelta ? current : best;
      }
      return best;
    });
  }

  /**
   * Find the most specific call expression that contains the target span.
   */
  private findCallExpressionAtSpan(
    sourceFile: SourceFile,
    spanStart: number,
    spanEnd: number
  ): CallExpression | null {
    const callExpressions = sourceFile.getDescendantsOfKind(
      SyntaxKind.CallExpression
    );
    const candidates = callExpressions.filter((call) => {
      const start = call.getStart();
      const end = call.getEnd();
      return spanStart >= start && spanEnd <= end;
    });

    if (candidates.length === 0) {
      return null;
    }

    return candidates.reduce((best, current) => {
      const bestRange = best.getEnd() - best.getStart();
      const currentRange = current.getEnd() - current.getStart();
      if (currentRange !== bestRange) {
        return currentRange < bestRange ? current : best;
      }
      const bestDelta = Math.abs(spanStart - best.getStart());
      const currentDelta = Math.abs(spanStart - current.getStart());
      if (currentDelta !== bestDelta) {
        return currentDelta < bestDelta ? current : best;
      }
      return best;
    });
  }

  // ===========================================================================
  // Type Utilities
  // ===========================================================================

  /**
   * Unwrap Promise<T> to T
   */
  private unwrapPromise(typeString: string, type: Type): string {
    // Check if it's a Promise type
    const promiseMatch = typeString.match(/^Promise<(.+)>$/);
    if (promiseMatch) {
      return promiseMatch[1];
    }

    // Also handle complex cases where TypeScript reports the full type
    const typeArguments = type.getTypeArguments();
    if (
      type.getSymbol()?.getName() === 'Promise' &&
      typeArguments.length > 0
    ) {
      return typeArguments[0].getText();
    }

    return typeString;
  }

  /**
   * Get source location information for a node
   */
  private getNodeLocation(node: Node, sourceFile: SourceFile): SourceLocation {
    // ts-morph doesn't expose getLineStarts directly, so we compute columns differently
    const startLinePos = sourceFile.getLineAndColumnAtPos(node.getStart());
    const endLinePos = sourceFile.getLineAndColumnAtPos(node.getEnd());

    return {
      file_path: sourceFile.getFilePath(),
      start_line: node.getStartLineNumber(),
      end_line: node.getEndLineNumber(),
      start_column: startLinePos.column - 1, // Convert to 0-based
      end_column: endLinePos.column - 1,
    };
  }

  /**
   * Create an InferredType result
   */
  private createInferredType(
    request: InferRequestItem,
    typeString: string,
    isExplicit: boolean,
    location: SourceLocation
  ): InferredType {
    // Generate alias if not provided
    const alias =
      request.alias ||
      this.generateAlias(request.file_path, request.line_number, request.infer_kind);

    return {
      alias,
      type_string: typeString,
      is_explicit: isExplicit,
      source_location: location,
      infer_kind: request.infer_kind,
    };
  }

  /**
   * Generate a default alias for an inferred type
   */
  private generateAlias(
    filePath: string,
    lineNumber: number,
    inferKind: InferKind
  ): string {
    // Extract filename without extension
    const fileName = filePath
      .split('/')
      .pop()
      ?.replace(/\.[^.]+$/, '') || 'unknown';

    // Convert to PascalCase
    const pascalName = fileName
      .split(/[-_]/)
      .map((part) => part.charAt(0).toUpperCase() + part.slice(1))
      .join('');

    return `${pascalName}Line${lineNumber}${this.inferKindSuffix(inferKind)}`;
  }

  /**
   * Get suffix for infer kind
   */
  private inferKindSuffix(kind: InferKind): string {
    switch (kind) {
      case 'function_return':
        return 'Return';
      case 'response_body':
        return 'Response';
      case 'request_body':
        return 'Request';
      case 'call_result':
        return 'Result';
      case 'variable':
        return 'Var';
      case 'expression':
        return 'Expr';
      default:
        return 'Type';
    }
  }

  /**
   * Log a message to stderr
   */
  private log(message: string): void {
    console.error(`[sidecar:type-inferrer] ${message}`);
  }

  /**
   * Log an error to stderr
   */
  private logError(message: string): void {
    console.error(`[sidecar:type-inferrer:error] ${message}`);
  }
}
