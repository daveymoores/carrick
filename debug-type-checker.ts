#!/usr/bin/env ts-node

import {
  Project,
  SourceFile,
  InterfaceDeclaration,
  TypeAliasDeclaration,
} from "ts-morph";
import * as fs from "fs";
import * as path from "path";

interface TypeMismatch {
  endpoint: string;
  producerType: string;
  consumerCall: string;
  consumerType: string;
  isAssignable: boolean;
  errorDetails: string;
  producerLocation?: string;
  consumerLocation?: string;
}

interface TypeCheckResult {
  totalProducers: number;
  totalConsumers: number;
  compatiblePairs: number;
  incompatiblePairs: number;
  mismatches: TypeMismatch[];
  orphanedProducers: string[];
  orphanedConsumers: string[];
}

class DebugTypeChecker {
  private project: Project;
  private outputDir: string;

  constructor(outputDir: string = "ts_check/output") {
    this.outputDir = outputDir;
    this.project = new Project({
      compilerOptions: {
        target: 99, // Latest
        lib: ["es2022"],
        strict: true,
        skipLibCheck: true,
        allowSyntheticDefaultImports: true,
        esModuleInterop: true,
      },
    });
  }

  private parseTypeName(typeName: string): {
    endpoint: string;
    type: "producer" | "consumer";
    callId?: string;
  } | null {
    // Handle producer pattern
    if (typeName.endsWith("Producer")) {
      const withoutProducer = typeName.slice(0, -8);
      const endpoint = this.convertToEndpoint(withoutProducer);
      console.log(`DEBUG: Producer ${typeName} -> endpoint: ${endpoint}`);
      return { endpoint, type: "producer" };
    }

    // Handle consumer pattern
    const consumerMatch = typeName.match(/(.+)Consumer(Call\d+)$/);
    if (consumerMatch) {
      const [, baseType, callId] = consumerMatch;
      const endpoint = this.convertToEndpoint(baseType);
      console.log(`DEBUG: Consumer ${typeName} -> endpoint: ${endpoint}`);
      return { endpoint, type: "consumer", callId };
    }

    return null;
  }

  /**
   * Convert camelCase type name to endpoint format
   * "GetApiCommentsResponse" -> "GET /api/comments"
   * "GetEnvVarCommentServiceUrlApiCommentsResponse" -> "GET /api/comments"
   */
  private convertToEndpoint(typeName: string): string {
    console.log(`Converting type name: ${typeName}`);

    // Remove "Response" or "Request" suffix if present
    let withoutSuffix = typeName;
    if (withoutSuffix.endsWith("Response")) {
      withoutSuffix = withoutSuffix.slice(0, -8);
    } else if (withoutSuffix.endsWith("Request")) {
      withoutSuffix = withoutSuffix.slice(0, -7);
    }
    console.log(`After removing suffix: ${withoutSuffix}`);

    // Handle env var patterns more flexibly
    if (withoutSuffix.includes("EnvVar")) {
      console.log(`Has EnvVar pattern`);
      // For "GetEnvVarCommentServiceUrlApiComments", we want "ApiComments"
      const urlIndex = withoutSuffix.lastIndexOf("Url");
      console.log(`Last Url index: ${urlIndex}`);

      if (urlIndex !== -1) {
        const pathPart = withoutSuffix.slice(urlIndex + 3); // +3 for "Url"
        console.log(`Path part after Url: ${pathPart}`);
        const path = this.camelCaseToPath(pathPart);
        console.log(`Converted path: ${path}`);
        const result = `GET ${path}`;
        console.log(`Final result: ${result}`);
        return result;
      }
    }

    // Extract HTTP method (Get, Post, Put, Delete, etc.)
    const methodMatch = withoutSuffix.match(
      /^(Get|Post|Put|Delete|Patch|Head|Options)/,
    );
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
   * "UsersByParamComments" -> "/users/:id/comments" (normalize param names)
   */
  private camelCaseToPath(camelCase: string): string {
    if (!camelCase) return "/";

    // Handle patterns like "UsersByIdComments" -> "/users/:id/comments"
    // Normalize all parameter names to ":id" for matching
    let withParams = camelCase.replace(/By([A-Z][a-z]+)/g, (match, param) => {
      // Normalize common parameter names to :id
      const normalizedParam = this.normalizeParameterName(param.toLowerCase());
      return `/:${normalizedParam}`;
    });

    // Convert remaining camelCase to kebab-case with slashes
    const path = withParams
      .replace(/([A-Z])/g, "/$1")
      .toLowerCase()
      .replace(/^\//, "/");

    return path || "/";
  }

  /**
   * Normalize parameter names for consistent matching
   */
  private normalizeParameterName(param: string): string {
    // Map various parameter names to standard ones
    const paramMap: { [key: string]: string } = {
      param: "id",
      userid: "id",
      orderid: "id",
      commentid: "id",
      postid: "id",
      eventid: "id",
      // Add more mappings as needed
    };

    return paramMap[param] || param;
  }

  private loadGeneratedFiles(): SourceFile[] {
    const files: SourceFile[] = [];

    if (!fs.existsSync(this.outputDir)) {
      throw new Error(`Output directory ${this.outputDir} does not exist`);
    }

    const tsFiles = fs
      .readdirSync(this.outputDir)
      .filter((file) => file.endsWith("_types.ts"))
      .map((file) => path.join(this.outputDir, file));

    for (const filePath of tsFiles) {
      try {
        const sourceFile = this.project.addSourceFileAtPath(filePath);
        files.push(sourceFile);
        console.log(`DEBUG: Loaded: ${filePath}`);
      } catch (error) {
        console.error(`Failed to load ${filePath}:`, error);
      }
    }

    return files;
  }

  private extractTypeDefinitions(
    sourceFiles: SourceFile[],
  ): Map<
    string,
    { file: string; node: InterfaceDeclaration | TypeAliasDeclaration }
  > {
    const types = new Map();

    for (const sourceFile of sourceFiles) {
      const fileName = path.basename(sourceFile.getFilePath());

      const interfaces = sourceFile.getInterfaces();
      for (const iface of interfaces) {
        const name = iface.getName();
        types.set(name, { file: fileName, node: iface });
        console.log(`DEBUG: Found interface: ${name} in ${fileName}`);
      }

      const typeAliases = sourceFile.getTypeAliases();
      for (const typeAlias of typeAliases) {
        const name = typeAlias.getName();
        types.set(name, { file: fileName, node: typeAlias });
        console.log(
          `DEBUG: Found type alias: ${name} = ${typeAlias.getTypeNode()?.getText()} in ${fileName}`,
        );
      }
    }

    return types;
  }

  async checkTypes(): Promise<TypeCheckResult> {
    console.log("DEBUG: Starting cross-repo type checking...");

    const sourceFiles = this.loadGeneratedFiles();
    if (sourceFiles.length === 0) {
      throw new Error("No TypeScript files found in output directory");
    }

    const typeDefinitions = this.extractTypeDefinitions(sourceFiles);
    console.log(
      `DEBUG: Found ${typeDefinitions.size} type definitions across ${sourceFiles.length} files`,
    );

    // Group types by endpoint
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
        console.log(
          `DEBUG: Added producer ${typeName} for endpoint ${parsed.endpoint}`,
        );
      } else if (parsed.type === "consumer") {
        if (!consumers.has(parsed.endpoint)) {
          consumers.set(parsed.endpoint, []);
        }
        consumers.get(parsed.endpoint)!.push({
          name: typeName,
          callId: parsed.callId!,
          ...typeInfo,
        });
        console.log(
          `DEBUG: Added consumer ${typeName} for endpoint ${parsed.endpoint}`,
        );
      }
    }

    console.log(
      `DEBUG: Found ${producers.size} producer types and ${consumers.size} consumer groups`,
    );

    // Debug: Show all producer-consumer matches
    for (const [endpoint, producer] of producers) {
      const endpointConsumers = consumers.get(endpoint) || [];
      console.log(`DEBUG: Endpoint ${endpoint}:`);
      console.log(`  Producer: ${producer.name}`);
      console.log(
        `  Consumers: ${endpointConsumers.map((c) => c.name).join(", ")}`,
      );

      if (endpointConsumers.length > 0) {
        for (const consumer of endpointConsumers) {
          console.log(`DEBUG: Comparing types for ${endpoint}:`);
          console.log(`  Producer type: ${producer.node.getType().getText()}`);
          console.log(`  Consumer type: ${consumer.node.getType().getText()}`);

          const producerType = producer.node.getType();
          const consumerType = consumer.node.getType();
          const isAssignable = producerType.isAssignableTo(consumerType);
          const reverseAssignable = consumerType.isAssignableTo(producerType);

          console.log(`  Producer -> Consumer assignable: ${isAssignable}`);
          console.log(
            `  Consumer -> Producer assignable: ${reverseAssignable}`,
          );
        }
      }
    }

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

    return result;
  }

  cleanup(): void {
    this.project.getSourceFiles().forEach((file) => file.delete());
  }
}

// CLI interface
async function main() {
  const args = process.argv.slice(2);
  const outputDir = args[0] || "ts_check/output";

  const checker = new DebugTypeChecker(outputDir);

  try {
    const result = await checker.checkTypes();
    console.log("DEBUG: Type checking completed");
  } catch (error) {
    console.error("Type checking failed:", error);
    process.exit(1);
  } finally {
    checker.cleanup();
  }
}

if (require.main === module) {
  main();
}
