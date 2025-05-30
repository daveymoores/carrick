import { Project, SourceFile, InterfaceDeclaration, Type } from 'ts-morph';

/**
 * Example TypeScript workflow for comparing producer vs consumer types
 * This demonstrates how the naming strategy enables type parity checking
 */

interface TypeComparisonResult {
  routeMethod: string;
  producerType: string;
  consumerTypes: string[];
  isCompatible: boolean;
  errors: string[];
}

class TypeParityChecker {
  private project: Project;

  constructor() {
    this.project = new Project({
      useInMemoryFileSystem: true,
      compilerOptions: {
        target: 99, // Latest
        lib: ["es2022"],
        strict: true,
      }
    });
  }

  /**
   * Compare producer and consumer types for a specific endpoint
   */
  async compareTypes(
    producerDefinition: string,
    consumerDefinitions: string[],
    commonTypeName: string
  ): Promise<TypeComparisonResult> {
    
    // Create source files for producer and consumers
    const producerFile = this.project.createSourceFile(
      "producer.ts", 
      producerDefinition
    );
    
    const consumerFiles = consumerDefinitions.map((def, index) => 
      this.project.createSourceFile(`consumer${index + 1}.ts`, def)
    );

    // Get the producer type
    const producerInterface = producerFile.getInterface(commonTypeName);
    if (!producerInterface) {
      return {
        routeMethod: commonTypeName,
        producerType: 'NOT_FOUND',
        consumerTypes: [],
        isCompatible: false,
        errors: [`Producer interface ${commonTypeName} not found`]
      };
    }

    const producerType = producerInterface.getType();
    const errors: string[] = [];
    const consumerTypes: string[] = [];
    let allCompatible = true;

    // Check each consumer against the producer
    for (let i = 0; i < consumerFiles.length; i++) {
      const consumerFile = consumerFiles[i];
      const consumerInterface = consumerFile.getInterface(commonTypeName);
      
      if (!consumerInterface) {
        errors.push(`Consumer ${i + 1}: Interface ${commonTypeName} not found`);
        consumerTypes.push('NOT_FOUND');
        allCompatible = false;
        continue;
      }

      const consumerType = consumerInterface.getType();
      consumerTypes.push(consumerType.getText());

      // Check if producer type is assignable to consumer type
      const isAssignable = producerType.isAssignableTo(consumerType);
      
      if (!isAssignable) {
        allCompatible = false;
        errors.push(
          `Consumer ${i + 1}: Type mismatch - Producer type '${producerType.getText()}' ` +
          `is not assignable to expected type '${consumerType.getText()}'`
        );
      }
    }

    return {
      routeMethod: commonTypeName,
      producerType: producerType.getText(),
      consumerTypes,
      isCompatible: allCompatible,
      errors
    };
  }

  /**
   * Example workflow matching the Carrick naming strategy
   */
  async exampleWorkflow(): Promise<void> {
    console.log("=== TypeScript Type Comparison Workflow ===\n");

    // Example 1: Compatible types (should pass)
    const producerDef1 = `
      export interface GetApiCommentsResponse {
        id: string;
        authorId: number;
        content: string;
      }[];
    `;

    const consumerDefs1 = [
      `export interface GetApiCommentsResponse {
         id: string;
         authorId: number;
         content: string;
       }[];`,
      `export interface GetApiCommentsResponse {
         id: string;
         authorId: number;
         content: string;
       }[];`,
      `export interface GetApiCommentsResponse {
         id: string;
         authorId: number;
         content: string;
       }[];`
    ];

    const result1 = await this.compareTypes(
      producerDef1, 
      consumerDefs1, 
      "GetApiCommentsResponse"
    );

    console.log("Example 1: Compatible Types");
    console.log(`Route: ${result1.routeMethod}`);
    console.log(`Producer: ${result1.producerType}`);
    console.log(`Consumers: ${result1.consumerTypes.length} fetch calls`);
    console.log(`Compatible: ${result1.isCompatible ? '✅' : '❌'}`);
    if (result1.errors.length > 0) {
      console.log("Errors:", result1.errors);
    }
    console.log();

    // Example 2: Incompatible types (should fail)
    const producerDef2 = `
      export interface GetApiUsersResponse {
        id: number;
        name: string;
      }[];
    `;

    const consumerDefs2 = [
      `export interface GetApiUsersResponse {
         id: number;
         name: string;
         role: string; // Extra field - should cause mismatch
       }[];`,
      `export interface GetApiUsersResponse {
         id: string; // Wrong type - should cause mismatch
         name: string;
       }[];`
    ];

    const result2 = await this.compareTypes(
      producerDef2, 
      consumerDefs2, 
      "GetApiUsersResponse"
    );

    console.log("Example 2: Incompatible Types");
    console.log(`Route: ${result2.routeMethod}`);
    console.log(`Producer: ${result2.producerType}`);
    console.log(`Consumers: ${result2.consumerTypes.length} fetch calls`);
    console.log(`Compatible: ${result2.isCompatible ? '✅' : '❌'}`);
    if (result2.errors.length > 0) {
      console.log("Errors:");
      result2.errors.forEach(error => console.log(`  - ${error}`));
    }
  }

  /**
   * Simulate the full workflow from Carrick output
   */
  async simulateCarrickWorkflow(): Promise<void> {
    console.log("\n=== Simulated Carrick Integration ===\n");

    // This simulates what would come from Redis/storage after Carrick analysis
    const carrickOutput = {
      producers: {
        "GetApiCommentsResponse": "Comment[]",
        "GetApiUsersResponse": "User[]"
      },
      consumers: {
        "GetApiCommentsResponse": [
          { callId: "GetApiCommentsResponseCall1", type: "Comment[]", file: "user-service/handlers.ts:25" },
          { callId: "GetApiCommentsResponseCall2", type: "Comment[]", file: "user-service/handlers.ts:45" },
          { callId: "GetApiCommentsResponseCall3", type: "Comment[]", file: "user-service/handlers.ts:67" }
        ],
        "GetApiUsersResponse": [
          { callId: "GetApiUsersResponseCall1", type: "UserWithRole[]", file: "comment-service/api.ts:12" }
        ]
      }
    };

    console.log("Carrick Analysis Results:");
    console.log("Producers:", Object.keys(carrickOutput.producers).length);
    console.log("Consumer Groups:", Object.keys(carrickOutput.consumers).length);
    
    let totalConsumers = 0;
    Object.values(carrickOutput.consumers).forEach(consumers => {
      totalConsumers += consumers.length;
    });
    console.log("Total Consumer Calls:", totalConsumers);

    // Check each producer-consumer group
    for (const [typeName, producerType] of Object.entries(carrickOutput.producers)) {
      const consumers = carrickOutput.consumers[typeName] || [];
      
      console.log(`\nChecking ${typeName}:`);
      console.log(`  Producer: ${producerType}`);
      console.log(`  Consumers: ${consumers.length} calls`);
      
      // In a real implementation, you would:
      // 1. Load actual type definitions from generated files
      // 2. Use ts-morph to compare them
      // 3. Report specific mismatches with file locations
      
      consumers.forEach(consumer => {
        const compatible = consumer.type === producerType;
        console.log(`    ${consumer.callId}: ${consumer.type} ${compatible ? '✅' : '❌'}`);
        if (!compatible) {
          console.log(`      Location: ${consumer.file}`);
          console.log(`      Expected: ${producerType}, Got: ${consumer.type}`);
        }
      });
    }
  }

  cleanup(): void {
    this.project.getSourceFiles().forEach(file => file.delete());
  }
}

// Example usage
async function main() {
  const checker = new TypeParityChecker();
  
  try {
    await checker.exampleWorkflow();
    await checker.simulateCarrickWorkflow();
  } finally {
    checker.cleanup();
  }
}

// Export for use in other modules
export { TypeParityChecker, TypeComparisonResult };

// Run example if this file is executed directly
if (require.main === module) {
  main().catch(console.error);
}