import {
  Project,
  SourceFile,
  InterfaceDeclaration,
  TypeAliasDeclaration,
  Type,
  SyntaxKind,
} from "ts-morph";

export interface TypeMismatch {
  endpoint: string;
  producerType: string;
  consumerCall: string;
  consumerType: string;
  isAssignable: boolean;
  errorDetails: string;
  producerLocation?: string;
  consumerLocation?: string;
}

export interface TypeCheckResult {
  totalProducers: number;
  totalConsumers: number;
  compatiblePairs: number;
  incompatiblePairs: number;
  mismatches: TypeMismatch[];
  orphanedProducers: string[];
  orphanedConsumers: string[];
}

export interface ParsedTypeName {
  endpoint: string;
  type: "producer" | "consumer";
  callId?: string;
}

export class TypeCompatibilityChecker {
  constructor(private project: Project) {}

  /**
   * Parse type name to extract endpoint and type info
   * Examples:
   * - "GetApiCommentsResponseProducer" -> { endpoint: "GET /api/comments", type: "producer" }
   * - "GetEnvVarCommentServiceUrlApiCommentsResponseConsumerCall1" -> { endpoint: "GET /api/comments", type: "consumer", callId: "Call1" }
   */
  parseTypeName(typeName: string): ParsedTypeName | null {
    // Handle producer pattern
    if (typeName.endsWith("Producer")) {
      const withoutProducer = typeName.slice(0, -8); // Remove "Producer"
      const endpoint = this.convertToEndpoint(withoutProducer);
      return { endpoint, type: "producer" };
    }

    // Handle consumer pattern
    const consumerMatch = typeName.match(/(.+)Consumer(Call\d+)$/);
    if (consumerMatch) {
      const [, baseType, callId] = consumerMatch;
      const endpoint = this.convertToEndpoint(baseType);
      return { endpoint, type: "consumer", callId };
    }

    return null;
  }

  /**
   * Unwrap Response<T> wrapper types to get the inner type
   */
  private unwrapResponseType(type: Type): Type {
    const typeText = type.getText();

    // For import references like "import(...).TypeName", extract the TypeName
    if (typeText.startsWith("import(")) {
      const match = typeText.match(/import\("([^"]+)"\)\.(\w+)/);
      if (match) {
        const [, filePath, typeName] = match;

        // Get the source file for the import (add .ts extension if not present)
        const fullFilePath = filePath.endsWith(".ts")
          ? filePath
          : `${filePath}.ts`;
        const sourceFile = this.project.getSourceFile(fullFilePath);

        if (sourceFile) {
          // Find the type alias in the source file
          const typeAlias = sourceFile.getTypeAlias(typeName);

          if (typeAlias) {
            const typeNode = typeAlias.getTypeNode();
            if (typeNode) {
              const nodeText = typeNode.getText();

              // Check if it's Response<T>
              if (nodeText.startsWith("Response<")) {
                // Parse the type arguments from the node
                const typeRef = typeNode as any;
                const typeArgs = typeRef.getTypeArguments?.();

                if (typeArgs && typeArgs.length > 0) {
                  const firstArgType = typeArgs[0].getType();
                  return firstArgType;
                }
              }
            }
          }
        }
      }
    }

    // Direct check for Response pattern
    if (typeText.includes("Response<")) {
      const typeArgs = type.getTypeArguments();

      if (typeArgs.length > 0) {
        const unwrapped = typeArgs[0];
        return unwrapped;
      }
    }
    return type;
  }

  /**
   * Resolve type if it's showing as an import reference
   */
  private resolveTypeReference(type: Type, node: any): Type {
    const typeText = type.getText();

    if (typeText.includes("import(")) {
      console.error(`🔗 Consumer type is an import reference, resolving...`);

      // For type alias declarations, get the actual type from the type node
      if (node.getKind && node.getKind() === 255) {
        // TypeAliasDeclaration
        const typeNode = node.getTypeNode();
        if (typeNode) {
          const resolvedType = typeNode.getType();
          return resolvedType;
        }
      }

      // Alternative: try to get the aliased type
      const aliasedType = type.getAliasTypeArguments();
      if (aliasedType.length > 0) {
        console.error(`🎯 Found aliased type: ${aliasedType[0].getText()}`);
        return aliasedType[0];
      }
    }

    return type;
  }

  /**
   * Get TypeScript's actual diagnostic message for type incompatibility
   */
  private getTypeCompatibilityError(
    producerType: Type,
    consumerType: Type,
  ): string {
    try {
      // Create a test assignment to get TypeScript's diagnostic
      const testCode = `
        declare const producer: ${producerType.getText()};
        declare const consumer: ${consumerType.getText()};

        // This assignment should fail and give us the diagnostic
        const test: ${consumerType.getText()} = producer;
      `;

      // Create a temporary source file to get diagnostics
      const tempFile = this.project.createSourceFile(
        `__temp_${Date.now()}.ts`,
        testCode,
        { overwrite: true },
      );

      // Get the diagnostics
      const diagnostics = tempFile.getPreEmitDiagnostics();

      // Clean up
      tempFile.delete();

      // Find the assignment error
      const assignmentError = diagnostics.find((d) => {
        const message = d.getMessageText();
        const messageStr =
          typeof message === "string" ? message : message.getMessageText();
        return (
          messageStr.includes("not assignable") || messageStr.includes("Type ")
        );
      });

      if (assignmentError) {
        const message = assignmentError.getMessageText();
        return typeof message === "string" ? message : message.getMessageText();
      }

      return `Types are incompatible but no specific diagnostic available`;
    } catch (error) {
      return `Type compatibility check failed: ${error}`;
    }
  }

  /**
   * Convert camelCase type name to endpoint format
   * "GetApiCommentsResponse" -> "GET /api/comments"
   * "GetEnvVarCommentServiceUrlApiCommentsResponse" -> "GET /api/comments"
   */
  private convertToEndpoint(typeName: string): string {
    // Remove "Response" or "Request" suffix if present
    let withoutSuffix = typeName;
    if (withoutSuffix.endsWith("Response")) {
      withoutSuffix = withoutSuffix.slice(0, -8);
    } else if (withoutSuffix.endsWith("Request")) {
      withoutSuffix = withoutSuffix.slice(0, -7);
    }

    // Handle env var patterns more flexibly
    if (withoutSuffix.includes("EnvVar")) {
      // For "GetEnvVarCommentServiceUrlApiComments", we want "ApiComments"
      const urlIndex = withoutSuffix.lastIndexOf("Url");

      if (urlIndex !== -1) {
        const pathPart = withoutSuffix.slice(urlIndex + 3); // +3 for "Url"

        const path = this.camelCaseToPath(pathPart);

        const result = `GET ${path}`;

        return result;
      }
    }

    // Extract HTTP method (Get, Post, Put, Delete, etc.)
    const methodMatch = withoutSuffix.match(
      /^(Get|Post|Put|Delete|Patch|Head|Options)/,
    );
    if (!methodMatch) {
      return withoutSuffix;
    }

    const method = methodMatch[1].toUpperCase();
    const pathPart = withoutSuffix.slice(methodMatch[1].length);

    const path = this.camelCaseToPath(pathPart);
    const result = `${method} ${path}`;

    return result;
  }

  /**
   * Convert camelCase to path format
   * "ApiComments" -> "/api/comments"
   * "UsersByIdComments" -> "/users/:id/comments"
   * "UsersByParamComments" -> "/users/:param/comments"
   */
  private camelCaseToPath(camelCase: string): string {
    if (!camelCase) return "/";

    // Handle patterns like "UsersByIdComments" -> "/users/:id/comments"
    // Also handle "UsersByParamComments" -> "/users/:param/comments"
    let withParams = camelCase.replace(/By([A-Z][a-z]+)/g, (match, param) => {
      return `/:${param.toLowerCase()}`;
    });

    // Convert remaining camelCase to kebab-case with slashes
    const path = withParams
      .replace(/([A-Z])/g, "/$1")
      .toLowerCase()
      .replace(/^\//, "/");

    return path || "/";
  }

  /**
   * Extract type definitions from source files
   */
  extractTypeDefinitions(
    sourceFiles: SourceFile[],
  ): Map<
    string,
    { file: string; node: InterfaceDeclaration | TypeAliasDeclaration }
  > {
    const types = new Map();

    for (const sourceFile of sourceFiles) {
      const fileName = sourceFile.getBaseName();

      // Get interfaces
      const interfaces = sourceFile.getInterfaces();
      for (const iface of interfaces) {
        const name = iface.getName();
        types.set(name, { file: fileName, node: iface });
      }

      // Get type aliases
      const typeAliases = sourceFile.getTypeAliases();
      for (const typeAlias of typeAliases) {
        const name = typeAlias.getName();
        types.set(name, { file: fileName, node: typeAlias });
      }
    }

    return types;
  }

  /**
   * Group types by endpoint into producers and consumers
   */
  groupTypesByEndpoint(
    typeDefinitions: Map<string, { file: string; node: any }>,
  ) {
    const producers = new Map<
      string,
      { name: string; file: string; node: any }
    >();
    const consumers = new Map<
      string,
      { name: string; file: string; node: any; callId: string }[]
    >();

    for (const [typeName, typeInfo] of typeDefinitions) {
      const parsed = this.parseTypeName(typeName);

      if (!parsed) continue;

      if (parsed.type === "producer") {
        producers.set(parsed.endpoint, { name: typeName, ...typeInfo });
      } else if (parsed.type === "consumer") {
        if (!consumers.has(parsed.endpoint)) {
          consumers.set(parsed.endpoint, []);
        }
        consumers.get(parsed.endpoint)!.push({
          name: typeName,
          callId: parsed.callId!,
          ...typeInfo,
        });
      }
    }

    return { producers, consumers };
  }

  /**
   * Compare producer and consumer types for compatibility
   */
  async compareTypes(
    endpoint: string,
    producer: { name: string; file: string; node: any },
    consumer: { name: string; file: string; node: any; callId: string },
  ): Promise<TypeMismatch | null> {
    try {
      let producerType = producer.node.getType();
      let consumerType = consumer.node.getType();

      // Unwrap Response<T> wrapper from producer
      producerType = this.unwrapResponseType(producerType);

      // Resolve consumer type if it's an import reference
      consumerType = this.resolveTypeReference(consumerType, consumer.node);

      // Check compatibility
      const isAssignable = producerType.isAssignableTo(consumerType);

      if (!isAssignable) {
        // Get TypeScript's diagnostic message
        const diagnosticMessage = this.getTypeCompatibilityError(
          producerType,
          consumerType,
        );

        return {
          endpoint,
          producerType: producerType.getText(),
          consumerCall: consumer.callId,
          consumerType: consumerType.getText(),
          isAssignable: false,
          errorDetails: diagnosticMessage,
          producerLocation: producer.file,
          consumerLocation: consumer.file,
        };
      }

      return null; // Types are compatible
    } catch (error) {
      throw new Error(`Type comparison failed: ${error}`);
    }
  }

  /**
   * Enhanced compareTypes that tries path matching if exact endpoint match fails
   */
  async checkCompatibility(
    sourceFiles: SourceFile[],
  ): Promise<TypeCheckResult> {
    const typeDefinitions = this.extractTypeDefinitions(sourceFiles);
    const { producers, consumers } = this.groupTypesByEndpoint(typeDefinitions);

    const result: TypeCheckResult = {
      totalProducers: producers.size,
      totalConsumers: Array.from(consumers.values()).reduce(
        (sum, group) => sum + group.length,
        0,
      ),
      compatiblePairs: 0,
      incompatiblePairs: 0,
      mismatches: [],
      orphanedProducers: [],
      orphanedConsumers: [],
    };

    // Check each consumer against all producers using flexible path matching
    for (const [consumerEndpoint, consumerList] of consumers) {
      let foundMatch = false;

      for (const consumer of consumerList) {
        let matchedProducer = null;

        // First try exact match
        if (producers.has(consumerEndpoint)) {
          matchedProducer = producers.get(consumerEndpoint)!;
        } else {
          // Try flexible path matching
          for (const [producerEndpoint, producer] of producers) {
            if (this.pathsMatch(consumerEndpoint, producerEndpoint)) {
              matchedProducer = producer;
              break;
            }
          }
        }

        if (matchedProducer) {
          foundMatch = true;
          try {
            const mismatch = await this.compareTypes(
              consumerEndpoint,
              matchedProducer,
              consumer,
            );
            if (mismatch) {
              result.mismatches.push(mismatch);
              result.incompatiblePairs++;
            } else {
              result.compatiblePairs++;
            }
          } catch (error) {
            result.mismatches.push({
              endpoint: consumerEndpoint,
              producerType: matchedProducer.name,
              consumerCall: consumer.name,
              consumerType: "UNKNOWN",
              isAssignable: false,
              errorDetails: `Failed to compare types: ${error}`,
              producerLocation: matchedProducer.file,
              consumerLocation: consumer.file,
            });
            result.incompatiblePairs++;
          }
        }
      }

      if (!foundMatch) {
        result.orphanedConsumers.push(
          ...consumerList.map((c) => `${consumerEndpoint} (${c.name})`),
        );
      }
    }

    // Find orphaned producers
    for (const [producerEndpoint, producer] of producers) {
      let hasMatch = false;

      for (const consumerEndpoint of consumers.keys()) {
        if (
          producerEndpoint === consumerEndpoint ||
          this.pathsMatch(producerEndpoint, consumerEndpoint)
        ) {
          hasMatch = true;
          break;
        }
      }

      if (!hasMatch) {
        result.orphanedProducers.push(`${producerEndpoint} (${producer.name})`);
      }
    }

    return result;
  }

  /**
   * Normalize path parameters to a consistent format for matching
   * Examples:
   * "/users/:id" -> "/users/{param}"
   * "/users/:param" -> "/users/{param}"
   * "/users/:userId" -> "/users/{param}"
   * "/events/:eventid/register" -> "/events/{param}/register"
   */
  private normalizePathForMatching(path: string): string {
    // Replace any parameter (starting with :) with a generic {param} placeholder
    return path.replace(/:[\w]+/g, "{param}");
  }

  /**
   * Check if two paths match, accounting for parameter differences
   */
  private pathsMatch(path1: string, path2: string): boolean {
    const normalized1 = this.normalizePathForMatching(path1);
    const normalized2 = this.normalizePathForMatching(path2);
    return normalized1 === normalized2;
  }

  /**
   * Load generated TypeScript files from output directory and perform type checking
   */
  async checkGeneratedTypes(outputDir: string): Promise<TypeCheckResult> {
    const fs = await import("fs");
    const path = await import("path");

    if (!fs.existsSync(outputDir)) {
      throw new Error(`Output directory ${outputDir} does not exist`);
    }

    const tsFiles = fs
      .readdirSync(outputDir)
      .filter((file) => file.endsWith("_types.ts"))
      .map((file) => path.join(outputDir, file));

    const sourceFiles: SourceFile[] = [];

    for (const filePath of tsFiles) {
      try {
        const sourceFile = this.project.addSourceFileAtPath(filePath);
        sourceFiles.push(sourceFile);
      } catch (error) {
        console.error(`Failed to load ${filePath}:`, error);
      }
    }

    if (sourceFiles.length === 0) {
      throw new Error("No TypeScript files found in output directory");
    }

    return await this.checkCompatibility(sourceFiles);
  }
}
