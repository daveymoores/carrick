import {
  Project,
  SourceFile,
  InterfaceDeclaration,
  TypeAliasDeclaration,
  Type,
  SyntaxKind,
} from "ts-morph";
import {
  ManifestMatcher,
  TypeManifest,
  ManifestEntry,
  MatchResult,
  normalizePath,
  normalizeMethod,
} from "./manifest-matcher";

/**
 * Mode for type checking operations
 */
export type TypeCheckMode = 'legacy' | 'manifest';

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

/**
 * Result from manifest-based type checking
 */
export interface ManifestTypeCheckResult extends TypeCheckResult {
  /** The mode used for type checking */
  mode: TypeCheckMode;
  /** Match details from manifest matching */
  matchDetails?: MatchResult[];
}

export class TypeCompatibilityChecker {
  private manifestMatcher: ManifestMatcher;
  private mode: TypeCheckMode = 'legacy';

  constructor(private project: Project) {
    this.manifestMatcher = new ManifestMatcher();
  }

  /**
   * Set the type checking mode
   * @param mode - 'legacy' for alias-based matching, 'manifest' for manifest-based matching
   */
  setMode(mode: TypeCheckMode): void {
    this.mode = mode;
    console.log(`[type-checker] Mode set to: ${mode}`);
  }

  /**
   * Get the current type checking mode
   */
  getMode(): TypeCheckMode {
    return this.mode;
  }

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

    // Handle consumer pattern - accept both "Consumer" and "ConsumerCallN" formats
    // The file-centric flow uses "Consumer" without CallN, while the old flow used "ConsumerCallN"
    const consumerMatch = typeName.match(/(.+)Consumer(Call\d+)?$/);
    if (consumerMatch) {
      const [, baseType, callId = "Call0"] = consumerMatch; // Default to "Call0" if no CallN suffix
      // For EnvVar types, extract the actual endpoint part after the env var
      // Pattern 1 (old): "GetEnvVarCommentServiceUrlApiCommentsResponse"
      // Pattern 2 (new file-centric): "GetByOrderServiceUrlOrdersResponse"
      let endpointBase = baseType;

      // Check for old EnvVar pattern
      if (baseType.startsWith("GetEnvVar") && baseType.includes("Url")) {
        // Extract everything after the last "Url" - this is the actual endpoint
        const urlIndex = baseType.lastIndexOf("Url");
        if (urlIndex !== -1) {
          endpointBase = "Get" + baseType.slice(urlIndex + 3);
        }
      }
      // Check for new By{EnvVar}Url pattern from file-centric flow
      // Pattern: "GetByOrderServiceUrlOrders" -> extract "Orders" after "Url"
      else if (this.hasEnvVarUrlPatternInBase(baseType)) {
        const urlIndex = baseType.lastIndexOf("Url");
        if (urlIndex !== -1) {
          // Extract the method prefix (Get, Post, etc.)
          const methodMatch = baseType.match(/^(Get|Post|Put|Delete|Patch|Head|Options)/);
          const method = methodMatch ? methodMatch[1] : "Get";
          endpointBase = method + baseType.slice(urlIndex + 3);
        }
      }

      const endpoint = this.convertToEndpoint(endpointBase);
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

              // For simple type aliases (arrays, primitives, simple objects), we get better type
              // resolution by creating a temporary file with the actual import and variable declaration.
              // This is because TypeScript's compiler API sometimes shows import references like
              // "import('path').TypeName" instead of the resolved structure. The temp file approach
              // forces proper type resolution but is only used for simple types to avoid performance
              // issues and circular dependency problems with complex types.
              if (this.isSimpleTypeAlias(nodeText)) {
                // Create a temporary variable with this type to get proper resolution
                try {
                  const importedTypes = this.extractTypeNamesFromText(nodeText);
                  const importStatement =
                    importedTypes.length > 0
                      ? `import { ${importedTypes.join(", ")} } from "${fullFilePath.replace(/\.ts$/, "")}";`
                      : "";

                  const tempFile = this.project.createSourceFile(
                    `__temp_resolve_${Date.now()}.ts`,
                    `${importStatement}
const tempVar: ${nodeText} = null as any;`,
                    { overwrite: true },
                  );

                  const tempVar = tempFile.getVariableDeclarations()[0];
                  const resolvedType = tempVar.getType();
                  tempFile.delete();
                  return resolvedType;
                } catch (error) {
                  // Fallback: return the type from the type node
                  return typeNode.getType();
                }
              }

              // For complex types, return the original type
              return typeNode.getType();
            }
          }
        }
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
    // Pattern 1: "GetEnvVarCommentServiceUrlApiComments" (old format)
    // Pattern 2: "GetByOrderServiceUrlOrders" (new file-centric format with By{EnvVar}Url{Path})
    if (withoutSuffix.includes("EnvVar") || this.hasEnvVarUrlPattern(withoutSuffix)) {
      // For "GetEnvVarCommentServiceUrlApiComments", we want "ApiComments"
      // For "GetByOrderServiceUrlOrders", we want "Orders"
      const urlIndex = withoutSuffix.lastIndexOf("Url");

      if (urlIndex !== -1) {
        const pathPart = withoutSuffix.slice(urlIndex + 3); // +3 for "Url"

        const path = this.camelCaseToPath(pathPart);

        // Extract the HTTP method from the beginning
        const methodMatch = withoutSuffix.match(
          /^(Get|Post|Put|Delete|Patch|Head|Options)/,
        );
        const method = methodMatch ? methodMatch[1].toUpperCase() : "GET";

        const result = `${method} ${path}`;

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
   * Check if a type name has the By{EnvVar}Url{Path} pattern from file-centric flow
   * Examples: "GetByOrderServiceUrlOrders", "PostByApiUrlUsers"
   */
  private hasEnvVarUrlPattern(typeName: string): boolean {
    // Look for pattern: Method + By + SomethingUrl + Path
    // The "By" followed by something ending in "Url" indicates an env var pattern
    const pattern = /^(Get|Post|Put|Delete|Patch|Head|Options)By[A-Z][a-zA-Z]*Url[A-Z]/;
    return pattern.test(typeName);
  }

  /**
   * Check if a base type (without Consumer/Producer suffix) has the By{EnvVar}Url pattern
   * This is used in parseTypeName to detect env var patterns in consumer types
   * Examples: "GetByOrderServiceUrlOrdersResponse" -> true
   */
  private hasEnvVarUrlPatternInBase(baseType: string): boolean {
    // Look for pattern: Method + By + SomethingUrl (where Url is followed by more path)
    const pattern = /^(Get|Post|Put|Delete|Patch|Head|Options)By[A-Z][a-zA-Z]*Url[A-Z]/;
    return pattern.test(baseType);
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

      if (!parsed) {
        continue;
      }

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
      let producerType;
      if (producer.node.getKindName() === "TypeAliasDeclaration") {
        // For type aliases, get the type from the type node, not the declaration
        const typeNode = producer.node.getTypeNode();
        if (typeNode) {
          producerType = typeNode.getType();
        } else {
          producerType = producer.node.getType();
        }
      } else {
        producerType = producer.node.getType();
      }
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
          errorDetails:
            diagnosticMessage ||
            `Type '${producerType.getText()}' is not assignable to type '${consumerType.getText()}'.`,
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
   * Check type compatibility using manifest files
   *
   * This is the new manifest-based approach that:
   * 1. Uses ManifestMatcher to find endpoint matches
   * 2. Loads corresponding type aliases from bundled .d.ts files
   * 3. Uses ts-morph's type assignability checking
   *
   * @param producerManifest - Manifest containing producer types
   * @param consumerManifest - Manifest containing consumer types
   * @param typesProject - Optional ts-morph Project for the bundled types
   * @returns TypeCheckResult with compatibility information
   */
  async checkCompatibilityWithManifests(
    producerManifest: TypeManifest,
    consumerManifest: TypeManifest,
    typesProject?: Project
  ): Promise<ManifestTypeCheckResult> {
    console.log(`[type-checker:manifest] Starting manifest-based type checking`);
    console.log(`[type-checker:manifest] Producer repo: ${producerManifest.repo_name} (${producerManifest.entries.length} entries)`);
    console.log(`[type-checker:manifest] Consumer repo: ${consumerManifest.repo_name} (${consumerManifest.entries.length} entries)`);

    // Use provided project or fall back to instance project
    const project = typesProject || this.project;

    // Match endpoints using the manifest matcher
    const { matches, orphanedProducers, orphanedConsumers } =
      this.manifestMatcher.matchEndpoints(producerManifest, consumerManifest);

    console.log(`[type-checker:manifest] Found ${matches.length} endpoint matches`);
    console.log(`[type-checker:manifest] Orphaned producers: ${orphanedProducers.length}`);
    console.log(`[type-checker:manifest] Orphaned consumers: ${orphanedConsumers.length}`);

    const result: ManifestTypeCheckResult = {
      mode: 'manifest',
      totalProducers: producerManifest.entries.filter(e => e.role === 'producer').length,
      totalConsumers: consumerManifest.entries.filter(e => e.role === 'consumer').length,
      compatiblePairs: 0,
      incompatiblePairs: 0,
      mismatches: [],
      orphanedProducers: orphanedProducers.map(o => `${o.entry.method} ${o.entry.path} (${o.entry.type_alias})`),
      orphanedConsumers: orphanedConsumers.map(o => `${o.entry.method} ${o.entry.path} (${o.entry.type_alias})`),
      matchDetails: matches,
    };

    // For each match, compare the types
    for (const match of matches) {
      const endpoint = `${match.method} ${match.path}`;

      try {
        const mismatch = await this.compareManifestTypes(
          project,
          endpoint,
          match.producer,
          match.consumer
        );

        if (mismatch) {
          result.mismatches.push(mismatch);
          result.incompatiblePairs++;
        } else {
          result.compatiblePairs++;
        }
      } catch (error) {
        console.error(`[type-checker:manifest] Error comparing types for ${endpoint}:`, error);
        result.mismatches.push({
          endpoint,
          producerType: match.producer.type_alias,
          consumerCall: match.consumer.type_alias,
          consumerType: 'UNKNOWN',
          isAssignable: false,
          errorDetails: `Failed to compare types: ${error instanceof Error ? error.message : String(error)}`,
          producerLocation: `${match.producer.file_path}:${match.producer.line_number}`,
          consumerLocation: `${match.consumer.file_path}:${match.consumer.line_number}`,
        });
        result.incompatiblePairs++;
      }
    }

    console.log(`[type-checker:manifest] Type checking complete`);
    console.log(`[type-checker:manifest] Compatible: ${result.compatiblePairs}, Incompatible: ${result.incompatiblePairs}`);

    return result;
  }

  /**
   * Compare types from manifest entries
   *
   * @param project - The ts-morph project containing bundled types
   * @param endpoint - The endpoint being checked
   * @param producer - The producer manifest entry
   * @param consumer - The consumer manifest entry
   * @returns TypeMismatch if incompatible, null if compatible
   */
  private async compareManifestTypes(
    project: Project,
    endpoint: string,
    producer: ManifestEntry,
    consumer: ManifestEntry
  ): Promise<TypeMismatch | null> {
    // Try to find the type aliases in the project
    const producerType = this.findTypeInProject(project, producer.type_alias);
    const consumerType = this.findTypeInProject(project, consumer.type_alias);

    if (!producerType) {
      return {
        endpoint,
        producerType: producer.type_alias,
        consumerCall: consumer.type_alias,
        consumerType: consumer.type_alias,
        isAssignable: false,
        errorDetails: `Producer type '${producer.type_alias}' not found in project`,
        producerLocation: `${producer.file_path}:${producer.line_number}`,
        consumerLocation: `${consumer.file_path}:${consumer.line_number}`,
      };
    }

    if (!consumerType) {
      return {
        endpoint,
        producerType: producer.type_alias,
        consumerCall: consumer.type_alias,
        consumerType: consumer.type_alias,
        isAssignable: false,
        errorDetails: `Consumer type '${consumer.type_alias}' not found in project`,
        producerLocation: `${producer.file_path}:${producer.line_number}`,
        consumerLocation: `${consumer.file_path}:${consumer.line_number}`,
      };
    }

    // Check assignability: consumer type should be assignable from producer type
    // This means the producer should provide at least what the consumer expects
    const diagnosticMessage = this.getTypeCompatibilityError(
      producerType,
      consumerType
    );

    const isAssignable = !diagnosticMessage;

    if (!isAssignable) {
      return {
        endpoint,
        producerType: producerType.getText(),
        consumerCall: consumer.type_alias,
        consumerType: consumerType.getText(),
        isAssignable: false,
        errorDetails: diagnosticMessage || 'Types are not compatible',
        producerLocation: `${producer.file_path}:${producer.line_number}`,
        consumerLocation: `${consumer.file_path}:${consumer.line_number}`,
      };
    }

    return null;
  }

  /**
   * Find a type alias or interface in the project by name
   */
  private findTypeInProject(project: Project, typeName: string): Type | null {
    for (const sourceFile of project.getSourceFiles()) {
      // Check type aliases
      const typeAlias = sourceFile.getTypeAlias(typeName);
      if (typeAlias) {
        return typeAlias.getType();
      }

      // Check interfaces
      const iface = sourceFile.getInterface(typeName);
      if (iface) {
        return iface.getType();
      }
    }

    return null;
  }

  /**
   * Load manifest from file path
   */
  loadManifest(filePath: string): TypeManifest {
    return this.manifestMatcher.loadManifest(filePath);
  }

  /**
   * Parse manifest from JSON string
   */
  parseManifest(jsonContent: string): TypeManifest {
    return this.manifestMatcher.parseManifest(jsonContent);
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
      console.warn("⚠️  No TypeScript type files (*_types.ts) found in output directory");
      console.warn("   This typically means no type annotations were extracted from the source code.");
      console.warn("   Type checking requires explicit TypeScript type annotations (e.g., `res: Response<User[]>`).");

      // Return empty result instead of throwing
      return {
        totalProducers: 0,
        totalConsumers: 0,
        compatiblePairs: 0,
        incompatiblePairs: 0,
        mismatches: [],
        orphanedProducers: [],
        orphanedConsumers: [],
      };
    }

    return await this.checkCompatibility(sourceFiles);
  }

  /**
   * Check if a type alias is simple enough to warrant creating a temporary file for better resolution
   */
  private isSimpleTypeAlias(nodeText: string): boolean {
    // Array types
    if (nodeText.endsWith("[]") || nodeText.startsWith("Array<")) {
      return true;
    }

    // Primitive types
    if (
      ["string", "number", "boolean", "Date"].some((primitive) =>
        nodeText.includes(primitive),
      )
    ) {
      return true;
    }

    // Object literals
    if (nodeText.startsWith("{")) {
      return true;
    }

    // Simple generic types (avoid deeply nested generics)
    if (nodeText.includes("<") && nodeText.includes(">")) {
      const openCount = (nodeText.match(/</g) || []).length;
      const closeCount = (nodeText.match(/>/g) || []).length;
      // Only consider simple generics with one level of nesting
      return openCount === closeCount && openCount <= 2;
    }

    // Union types with simple components (avoid complex unions)
    if (
      nodeText.includes("|") &&
      !nodeText.includes("{") &&
      nodeText.length < 100
    ) {
      return true;
    }

    return false;
  }

  /**
   * Extract type names that need to be imported from a type string
   */
  private extractTypeNamesFromText(nodeText: string): string[] {
    const typeNames = new Set<string>();

    // Match capitalized identifiers that look like type names (not primitives)
    const typePattern = /\b[A-Z][a-zA-Z0-9]*\b/g;
    const matches = nodeText.match(typePattern) || [];

    for (const match of matches) {
      // Exclude known primitives and built-in types
      if (
        ![
          "Array",
          "Promise",
          "Date",
          "String",
          "Number",
          "Boolean",
          "Object",
        ].includes(match)
      ) {
        typeNames.add(match);
      }
    }

    return Array.from(typeNames);
  }

  /**
   * Auto-detect and run type checking in the appropriate mode
   *
   * @param options - Options for type checking
   * @returns TypeCheckResult or ManifestTypeCheckResult
   *
   * @deprecated Use checkCompatibility for legacy mode or checkCompatibilityWithManifests for manifest mode
   */
  async autoCheck(options: {
    outputDir?: string;
    producerManifestPath?: string;
    consumerManifestPath?: string;
    typesProject?: Project;
  }): Promise<TypeCheckResult | ManifestTypeCheckResult> {
    // If manifest paths provided, use manifest mode
    if (options.producerManifestPath && options.consumerManifestPath) {
      console.log('[type-checker] Auto-detected manifest mode');
      const producerManifest = this.loadManifest(options.producerManifestPath);
      const consumerManifest = this.loadManifest(options.consumerManifestPath);
      return this.checkCompatibilityWithManifests(
        producerManifest,
        consumerManifest,
        options.typesProject
      );
    }

    // Otherwise use legacy mode with output directory
    if (options.outputDir) {
      console.log('[type-checker] Auto-detected legacy mode');
      return this.checkGeneratedTypes(options.outputDir);
    }

    throw new Error('Either outputDir or both manifest paths must be provided');
  }
}
