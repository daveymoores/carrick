import { Project, SourceFile, InterfaceDeclaration, TypeAliasDeclaration, Type } from 'ts-morph';

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
  type: 'producer' | 'consumer';
  callId?: string;
}

export class TypeCompatibilityChecker {
  constructor(private project: Project) {}

  /**
   * Parse type name to extract endpoint and type info
   * Examples:
   * - "GetApiCommentsResponseProducer" -> { endpoint: "GET /api/comments", type: "producer" }
   * - "GetApiCommentsResponseConsumerCall1" -> { endpoint: "GET /api/comments", type: "consumer", callId: "Call1" }
   */
  parseTypeName(typeName: string): ParsedTypeName | null {
    // Handle producer pattern
    if (typeName.endsWith('Producer')) {
      const withoutProducer = typeName.slice(0, -8); // Remove "Producer"
      const endpoint = this.convertToEndpoint(withoutProducer);
      return { endpoint, type: 'producer' };
    }

    // Handle consumer pattern
    const consumerMatch = typeName.match(/(.+)Consumer(Call\d+)$/);
    if (consumerMatch) {
      const [, baseType, callId] = consumerMatch;
      const endpoint = this.convertToEndpoint(baseType);
      return { endpoint, type: 'consumer', callId };
    }

    return null;
  }

  /**
   * Convert camelCase type name to endpoint format
   * "GetApiCommentsResponse" -> "GET /api/comments"
   * "GetEnvVarCommentServiceUrlApiCommentsResponse" -> "GET /api/comments"
   */
  private convertToEndpoint(typeName: string): string {
    // Remove "Response" or "Request" suffix if present
    let withoutSuffix = typeName;
    if (withoutSuffix.endsWith('Response')) {
      withoutSuffix = withoutSuffix.slice(0, -8);
    } else if (withoutSuffix.endsWith('Request')) {
      withoutSuffix = withoutSuffix.slice(0, -7);
    }
    
    // Check for env var pattern and extract the actual endpoint path
    const envVarMatch = withoutSuffix.match(/^GetEnvVar(.+)Url(.+)$/);
    if (envVarMatch) {
      const [, , pathPart] = envVarMatch;
      const path = this.camelCaseToPath(pathPart);
      return `GET ${path}`;
    }
    
    // Extract HTTP method (Get, Post, Put, Delete, etc.)
    const methodMatch = withoutSuffix.match(/^(Get|Post|Put|Delete|Patch|Head|Options)/);
    if (!methodMatch) return withoutSuffix;
    
    const method = methodMatch[1].toUpperCase();
    const pathPart = withoutSuffix.slice(methodMatch[1].length);
    
    const path = this.camelCaseToPath(pathPart);
    return `${method} ${path}`;
  }

  /**
   * Convert camelCase to path format
   * "ApiComments" -> "/api/comments"
   * "UsersByIdComments" -> "/users/:id/comments"
   * "UsersByParamComments" -> "/users/:param/comments"
   */
  private camelCaseToPath(camelCase: string): string {
    if (!camelCase) return '/';
    
    // Handle patterns like "UsersByIdComments" -> "/users/:id/comments"
    // Also handle "UsersByParamComments" -> "/users/:param/comments"
    let withParams = camelCase.replace(/By([A-Z][a-z]+)/g, (match, param) => {
      return `/:${param.toLowerCase()}`;
    });
    
    // Convert remaining camelCase to kebab-case with slashes
    const path = withParams
      .replace(/([A-Z])/g, '/$1')
      .toLowerCase()
      .replace(/^\//, '/');
    
    return path || '/';
  }

  /**
   * Extract type definitions from source files
   */
  extractTypeDefinitions(sourceFiles: SourceFile[]): Map<string, { file: string; node: InterfaceDeclaration | TypeAliasDeclaration }> {
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
  groupTypesByEndpoint(typeDefinitions: Map<string, { file: string; node: any }>) {
    const producers = new Map<string, { name: string; file: string; node: any }>();
    const consumers = new Map<string, { name: string; file: string; node: any; callId: string }[]>();

    for (const [typeName, typeInfo] of typeDefinitions) {
      const parsed = this.parseTypeName(typeName);
      if (!parsed) continue;

      if (parsed.type === 'producer') {
        producers.set(parsed.endpoint, { name: typeName, ...typeInfo });
      } else if (parsed.type === 'consumer') {
        if (!consumers.has(parsed.endpoint)) {
          consumers.set(parsed.endpoint, []);
        }
        consumers.get(parsed.endpoint)!.push({ 
          name: typeName, 
          callId: parsed.callId!, 
          ...typeInfo 
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
    consumer: { name: string; file: string; node: any; callId: string }
  ): Promise<TypeMismatch | null> {
    try {
      const producerType = producer.node.getType();
      const consumerType = consumer.node.getType();

      // Check if producer type is assignable to consumer type
      const isAssignable = producerType.isAssignableTo(consumerType);
      
      if (!isAssignable) {
        // Try the reverse to provide more detailed error information
        const reverseAssignable = consumerType.isAssignableTo(producerType);
        
        let errorDetails = `Producer type '${producerType.getText()}' is not assignable to consumer type '${consumerType.getText()}'`;
        
        if (reverseAssignable) {
          errorDetails += ' (consumer type is more restrictive than producer)';
        } else {
          errorDetails += ' (types are incompatible)';
        }

        return {
          endpoint,
          producerType: producerType.getText(),
          consumerCall: consumer.callId,
          consumerType: consumerType.getText(),
          isAssignable: false,
          errorDetails,
          producerLocation: producer.file,
          consumerLocation: consumer.file
        };
      }

      return null; // Types are compatible
    } catch (error) {
      throw new Error(`Type comparison failed: ${error}`);
    }
  }

  /**
   * Perform the actual type checking on source files
   */
  async checkCompatibility(sourceFiles: SourceFile[]): Promise<TypeCheckResult> {
    const typeDefinitions = this.extractTypeDefinitions(sourceFiles);
    const { producers, consumers } = this.groupTypesByEndpoint(typeDefinitions);

    const result: TypeCheckResult = {
      totalProducers: producers.size,
      totalConsumers: Array.from(consumers.values()).reduce((sum, group) => sum + group.length, 0),
      compatiblePairs: 0,
      incompatiblePairs: 0,
      mismatches: [],
      orphanedProducers: [],
      orphanedConsumers: []
    };

    // Check each producer against its consumers
    for (const [endpoint, producer] of producers) {
      const endpointConsumers = consumers.get(endpoint) || [];
      
      if (endpointConsumers.length === 0) {
        result.orphanedProducers.push(`${endpoint} (${producer.name})`);
        continue;
      }

      for (const consumer of endpointConsumers) {
        try {
          const mismatch = await this.compareTypes(endpoint, producer, consumer);
          if (mismatch) {
            result.mismatches.push(mismatch);
            result.incompatiblePairs++;
          } else {
            result.compatiblePairs++;
          }
        } catch (error) {
          result.mismatches.push({
            endpoint,
            producerType: producer.name,
            consumerCall: consumer.name,
            consumerType: 'UNKNOWN',
            isAssignable: false,
            errorDetails: `Failed to compare types: ${error}`,
            producerLocation: producer.file,
            consumerLocation: consumer.file
          });
          result.incompatiblePairs++;
        }
      }
    }

    // Find orphaned consumers
    for (const [endpoint, endpointConsumers] of consumers) {
      if (!producers.has(endpoint)) {
        result.orphanedConsumers.push(
          ...endpointConsumers.map(c => `${endpoint} (${c.name})`)
        );
      }
    }

    return result;
  }

  /**
   * Load generated TypeScript files from output directory and perform type checking
   */
  async checkGeneratedTypes(outputDir: string): Promise<TypeCheckResult> {
    const fs = await import('fs');
    const path = await import('path');

    if (!fs.existsSync(outputDir)) {
      throw new Error(`Output directory ${outputDir} does not exist`);
    }

    const tsFiles = fs.readdirSync(outputDir)
      .filter(file => file.endsWith('_types.ts'))
      .map(file => path.join(outputDir, file));

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
      throw new Error('No TypeScript files found in output directory');
    }

    return await this.checkCompatibility(sourceFiles);
  }
}