#!/usr/bin/env ts-node

import { parseArguments } from "./lib/argument-parser";
import { TypeExtractor } from "./lib/type-extractor";
import { TypeCompatibilityChecker } from "./lib/type-checker";
import * as path from "path";

async function main() {
  try {
    // Parse command line arguments
    const args = parseArguments();
    
    // Get the output directory
    const outputDir = path.dirname(args.outputFile);
    // Create type extractor instance
    const typeExtractor = new TypeExtractor(
      args.tsconfigPath,
      args.allDependencies,
    );

    // Extract types and generate output
    const result = await typeExtractor.extractTypes(
      args.typeInfos,
      args.outputFile,
    );

    // Perform type checking if extraction was successful
    if (result.success) {
      console.log("\n📦 Installing dependencies for type checking...");

      try {
        // Install dependencies in the output directory
        const outputDir = path.dirname(args.outputFile);
        const { execSync } = await import("child_process");
        
        try {
          execSync("npm install", { 
            cwd: outputDir, 
            stdio: "inherit",
            timeout: 60000 // 60 second timeout
          });
          console.log("✅ Dependencies installed successfully");
        } catch (installError) {
          console.warn("⚠️  Warning: Failed to install dependencies:", (installError as Error).message);
          console.warn("Type checking may not work correctly without dependencies");
        }

        console.log("\n🔍 Starting type compatibility checking...");

        // Get the ts-morph project from the type extractor
        const project = typeExtractor.getProject();

        // Create type checker instance
        const typeChecker = new TypeCompatibilityChecker(project);

        // Use the same output directory

        // Perform type checking
        const typeCheckResult = await typeChecker.checkGeneratedTypes(outputDir);

        // Create a simplified result format for the Rust analyzer
        const simplifiedResult = {
          mismatches: typeCheckResult.mismatches.map(mismatch => ({
            endpoint: mismatch.endpoint,
            producerType: mismatch.producerType,
            consumerType: mismatch.consumerType,
            error: mismatch.errorDetails,
            isCompatible: mismatch.isAssignable
          })),
          compatibleCount: typeCheckResult.compatiblePairs,
          totalChecked: typeCheckResult.compatiblePairs + typeCheckResult.incompatiblePairs
        };

        // Write type check results to a file that the Rust analyzer can read
        const typeCheckOutputPath = path.join(outputDir, "type-check-results.json");
        const fs = await import("fs");
        fs.writeFileSync(typeCheckOutputPath, JSON.stringify(simplifiedResult, null, 2));

        // Log summary
        if (typeCheckResult.mismatches.length === 0) {
          console.log("\n✅ All types are compatible!");
        } else {
          console.log(`\n❌ Found ${typeCheckResult.mismatches.length} type compatibility issues:`);
          typeCheckResult.mismatches.forEach(mismatch => {
            console.log(`  - ${mismatch.endpoint}: ${mismatch.errorDetails}`);
          });
        }

        console.log(`\nType checking summary:`);
        console.log(`  Compatible pairs: ${typeCheckResult.compatiblePairs}`);
        console.log(`  Incompatible pairs: ${typeCheckResult.incompatiblePairs}`);
        console.log(`  Orphaned producers: ${typeCheckResult.orphanedProducers.length}`);
        console.log(`  Orphaned consumers: ${typeCheckResult.orphanedConsumers.length}`);

        if (typeCheckResult.orphanedProducers.length > 0) {
          console.log(`  Orphaned producers: ${typeCheckResult.orphanedProducers.join(", ")}`);
        }
        if (typeCheckResult.orphanedConsumers.length > 0) {
          console.log(`  Orphaned consumers: ${typeCheckResult.orphanedConsumers.join(", ")}`);
        }



      } catch (error) {
        console.error("Type checking failed:", error);
        // Write error result
        const typeCheckOutputPath = path.join(path.dirname(args.outputFile), "type-check-results.json");
        const fs = await import("fs");
        const errorResult = {
          mismatches: [],
          compatibleCount: 0,
          totalChecked: 0,
          error: (error as Error).message
        };
        fs.writeFileSync(typeCheckOutputPath, JSON.stringify(errorResult, null, 2));
        

      }
    }

    // Clean up
    typeExtractor.clear();

    // Exit with appropriate code
    process.exit(result.success ? 0 : 1);
  } catch (error) {
    console.error("Fatal error:", error);
    process.exit(1);
  }
}

main();
