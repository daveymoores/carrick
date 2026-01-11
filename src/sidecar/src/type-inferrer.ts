/**
 * Type Inferrer - Scope-based type inference for implicit types
 *
 * This module extracts types even when developers don't write explicit
 * annotations. It uses scope-based search (not line windows) to handle
 * large handlers with middleware, validation, logging before response.
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
  type BinaryExpression,
  type ReturnStatement,
  type Type,
} from 'ts-morph';
import type {
  InferRequestItem,
  InferResult,
  InferredType,
  InferKind,
  SourceLocation,
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
  infer(requests: InferRequestItem[]): InferResult {
    const inferredTypes: InferredType[] = [];
    const errors: string[] = [];

    for (const request of requests) {
      try {
        const result = this.inferSingle(request);
        if (result) {
          inferredTypes.push(result);
        } else {
          errors.push(
            `Could not infer type at ${request.file_path}:${request.line_number} (${request.infer_kind})`
          );
        }
      } catch (err) {
        const error = err instanceof Error ? err.message : String(err);
        errors.push(
          `Error inferring type at ${request.file_path}:${request.line_number}: ${error}`
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
  private inferSingle(request: InferRequestItem): InferredType | null {
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
        return this.inferCallResult(sourceFile, request);
      case 'variable':
        return this.inferVariable(sourceFile, request);
      case 'expression':
        return this.inferExpression(sourceFile, request);
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
   * Infer the return type of a function at the given line
   */
  private inferFunctionReturn(
    sourceFile: SourceFile,
    request: InferRequestItem
  ): InferredType | null {
    const func = this.findContainingFunction(sourceFile, request.line_number);
    if (!func) {
      this.log(`No function found at line ${request.line_number}`);
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
    const func = this.findContainingFunction(sourceFile, request.line_number);
    if (!func) {
      this.log(`No function found at line ${request.line_number}`);
      return null;
    }

    // Collect all response types from the function body
    const responseTypes: string[] = [];

    // Search for response patterns in the entire function scope
    const callExpressions = func.getDescendantsOfKind(SyntaxKind.CallExpression);
    const binaryExpressions = func.getDescendantsOfKind(SyntaxKind.BinaryExpression);
    const returnStatements = func.getDescendantsOfKind(SyntaxKind.ReturnStatement);

    // Check res.json(), res.send(), Response.json() patterns
    for (const call of callExpressions) {
      const responseType = this.extractResponseFromCall(call);
      if (responseType && !responseTypes.includes(responseType)) {
        responseTypes.push(responseType);
      }
    }

    // Check ctx.body = data patterns (Koa style)
    for (const binary of binaryExpressions) {
      const responseType = this.extractResponseFromAssignment(binary);
      if (responseType && !responseTypes.includes(responseType)) {
        responseTypes.push(responseType);
      }
    }

    // Check return statements for Hono/Web API style
    for (const ret of returnStatements) {
      const responseType = this.extractResponseFromReturn(ret);
      if (responseType && !responseTypes.includes(responseType)) {
        responseTypes.push(responseType);
      }
    }

    if (responseTypes.length === 0) {
      return null;
    }

    // Create union type if multiple response types
    const typeString =
      responseTypes.length === 1
        ? responseTypes[0]
        : responseTypes.join(' | ');

    return this.createInferredType(
      request,
      typeString,
      false, // Response body inference is always implicit
      this.getNodeLocation(func, sourceFile)
    );
  }

  /**
   * Infer the return type of a call expression at the given line
   */
  private inferCallResult(
    sourceFile: SourceFile,
    request: InferRequestItem
  ): InferredType | null {
    const node = this.findNodeAtLine(sourceFile, request.line_number);
    if (!node) return null;

    // Find the nearest call expression
    const callExpr =
      node.getKind() === SyntaxKind.CallExpression
        ? (node as CallExpression)
        : node.getFirstAncestorByKind(SyntaxKind.CallExpression);

    if (!callExpr) {
      this.log(`No call expression found at line ${request.line_number}`);
      return null;
    }

    let returnType = callExpr.getReturnType();
    let typeString = returnType.getText(callExpr);

    // Unwrap Promise<T>
    typeString = this.unwrapPromise(typeString, returnType);

    return this.createInferredType(
      request,
      typeString,
      false,
      this.getNodeLocation(callExpr, sourceFile)
    );
  }

  /**
   * Infer the type of a variable at the given line
   */
  private inferVariable(
    sourceFile: SourceFile,
    request: InferRequestItem
  ): InferredType | null {
    const node = this.findNodeAtLine(sourceFile, request.line_number);
    if (!node) return null;

    // Find variable declaration
    const varDecl =
      node.getKind() === SyntaxKind.VariableDeclaration
        ? node
        : node.getFirstAncestorByKind(SyntaxKind.VariableDeclaration);

    if (!varDecl || !Node.isVariableDeclaration(varDecl)) {
      this.log(`No variable declaration found at line ${request.line_number}`);
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
   * Infer the type of an expression at the given line
   */
  private inferExpression(
    sourceFile: SourceFile,
    request: InferRequestItem
  ): InferredType | null {
    const node = this.findNodeAtLine(sourceFile, request.line_number);
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

  // ===========================================================================
  // Response Pattern Extractors
  // ===========================================================================

  /**
   * Extract response type from call expressions like res.json(data), res.send(data), Response.json(data)
   */
  private extractResponseFromCall(call: CallExpression): string | null {
    const callText = call.getExpression().getText();

    // Match patterns: res.json, res.send, Response.json, c.json (Hono)
    const responsePatterns = [
      /\bres\.json\b/,
      /\bres\.send\b/,
      /\bresponse\.json\b/i,
      /\bResponse\.json\b/,
      /\bc\.json\b/, // Hono context
      /\bctx\.json\b/, // Some frameworks use ctx
    ];

    const isResponseCall = responsePatterns.some((p) => p.test(callText));
    if (!isResponseCall) return null;

    // Get the first argument's type
    const args = call.getArguments();
    if (args.length === 0) return null;

    const argType = args[0].getType();
    return argType.getText(args[0]);
  }

  /**
   * Extract response type from assignments like ctx.body = data
   */
  private extractResponseFromAssignment(binary: BinaryExpression): string | null {
    // Only handle assignment expressions
    if (binary.getOperatorToken().getKind() !== SyntaxKind.EqualsToken) {
      return null;
    }

    const left = binary.getLeft().getText();

    // Match ctx.body pattern (Koa style)
    if (!/\b(?:ctx|context)\.body\b/.test(left)) {
      return null;
    }

    const right = binary.getRight();
    const rightType = right.getType();
    return rightType.getText(right);
  }

  /**
   * Extract response type from return statements
   */
  private extractResponseFromReturn(ret: ReturnStatement): string | null {
    const expr = ret.getExpression();
    if (!expr) return null;

    // Check for Response.json() or new Response() patterns
    const exprText = expr.getText();
    if (/Response\.json\(/.test(exprText) || /new Response\(/.test(exprText)) {
      // Try to get the argument type
      const callExpr = expr.getKind() === SyntaxKind.CallExpression
        ? (expr as CallExpression)
        : expr.getFirstDescendantByKind(SyntaxKind.CallExpression);

      if (callExpr) {
        const args = callExpr.getArguments();
        if (args.length > 0) {
          return args[0].getType().getText(args[0]);
        }
      }
    }

    // For direct return of data (common in Hono/modern frameworks)
    const exprType = expr.getType();
    const typeText = exprType.getText(expr);

    // Skip void/undefined returns
    if (typeText === 'void' || typeText === 'undefined') {
      return null;
    }

    return typeText;
  }

  // ===========================================================================
  // Scope-Based Search Utilities
  // ===========================================================================

  /**
   * Find the innermost function containing the target line
   *
   * CRITICAL: This ensures we search the right scope even for nested functions
   */
  private findContainingFunction(
    sourceFile: SourceFile,
    targetLine: number
  ): FunctionLike | null {
    const functions: FunctionLike[] = [];

    // Collect all function-like nodes
    functions.push(...sourceFile.getDescendantsOfKind(SyntaxKind.FunctionDeclaration));
    functions.push(...sourceFile.getDescendantsOfKind(SyntaxKind.ArrowFunction));
    functions.push(...sourceFile.getDescendantsOfKind(SyntaxKind.FunctionExpression));
    functions.push(...sourceFile.getDescendantsOfKind(SyntaxKind.MethodDeclaration));

    // Find all functions that contain the target line
    const containing = functions.filter((func) => {
      const startLine = func.getStartLineNumber();
      const endLine = func.getEndLineNumber();
      return targetLine >= startLine && targetLine <= endLine;
    });

    if (containing.length === 0) {
      return null;
    }

    // Return the innermost (smallest range) function
    return containing.reduce((innermost, current) => {
      const innermostRange =
        innermost.getEndLineNumber() - innermost.getStartLineNumber();
      const currentRange =
        current.getEndLineNumber() - current.getStartLineNumber();
      return currentRange < innermostRange ? current : innermost;
    });
  }

  /**
   * Find the most relevant node at the given line
   */
  private findNodeAtLine(sourceFile: SourceFile, line: number): Node | null {
    // Find all nodes on this line
    const allNodes = sourceFile.getDescendants();
    const nodesOnLine = allNodes.filter(
      (n) => n.getStartLineNumber() === line
    );

    if (nodesOnLine.length === 0) {
      return null;
    }

    // Return the first meaningful node on this line
    return nodesOnLine[0];
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
