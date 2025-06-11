#!/usr/bin/env ts-node

import { TypeCompatibilityChecker } from "./lib/type-checker";
import { Project } from "ts-morph";
import * as path from "path";

function cleanupPaths(text: string): string {
  // Remove absolute paths from TypeScript error messages
  // Convert: import("/full/path/to/repo-c_types").Comment[]
  // To: import("repo-c-types").Comment[]
  return text.replace(
    /import\("([^"]*[/\\])([^"]*_types)"\)/g,
    (match, fullPath, fileName) => {
      const cleanName = fileName.replace("_", "-");
      return `import("${cleanName}")`;
    }
  );
}

async function main() {
  try {
    console.log("Installing dependencies for type checking...");

    // Install dependencies in the output directory
    const outputDir = "ts_check/output";
    const { execSync } = await import("child_process");

    try {
      execSync("npm install", {
        cwd: outputDir,
        stdio: "inherit",
        timeout: 60000, // 60 second timeout
      });
      console.log("Dependencies installed successfully");
    } catch (installError) {
      console.warn(
        "⚠️  Warning: Failed to install dependencies:",
        (installError as Error).message,
      );
      console.warn("Type checking may not work correctly without dependencies");
    }

    console.log("\n🔍 Starting type compatibility checking...");

    console.log(`outputDir -> ${outputDir}`);

    // Create ts-morph project
    const project = new Project({
      tsConfigFilePath: process.argv[2], // The tsconfig path is passed as the first argument
    });

    // Create type checker instance
    const typeChecker = new TypeCompatibilityChecker(project);

    // Perform type checking
    const typeCheckResult = await typeChecker.checkGeneratedTypes(outputDir);

    // Create a simplified result format for the Rust analyzer
    const simplifiedResult = {
      mismatches: typeCheckResult.mismatches.map((mismatch) => ({
        endpoint: mismatch.endpoint,
        producerType: cleanupPaths(mismatch.producerType),
        consumerType: cleanupPaths(mismatch.consumerType),
        error: cleanupPaths(mismatch.errorDetails),
        isCompatible: mismatch.isAssignable,
      })),
      compatibleCount: typeCheckResult.compatiblePairs,
      totalChecked:
        typeCheckResult.compatiblePairs + typeCheckResult.incompatiblePairs,
    };

    // Write type check results to a file that the Rust analyzer can read
    const typeCheckOutputPath = path.join(outputDir, "type-check-results.json");
    const fs = await import("fs");
    fs.writeFileSync(
      typeCheckOutputPath,
      JSON.stringify(simplifiedResult, null, 2),
    );

    // Log summary
    if (typeCheckResult.mismatches.length === 0) {
      console.log("\n✅ All types are compatible!");
    } else {
      console.log(
        `\n❌ Found ${typeCheckResult.mismatches.length} type compatibility issues:`,
      );
      typeCheckResult.mismatches.forEach((mismatch) => {
        console.log(`  - ${mismatch.endpoint}: ${cleanupPaths(mismatch.errorDetails)}`);
      });
    }

    console.log(`\nType checking summary:`);
    console.log(`  Compatible pairs: ${typeCheckResult.compatiblePairs}`);
    console.log(`  Incompatible pairs: ${typeCheckResult.incompatiblePairs}`);
    console.log(
      `  Orphaned producers: ${typeCheckResult.orphanedProducers.length}`,
    );
    console.log(
      `  Orphaned consumers: ${typeCheckResult.orphanedConsumers.length}`,
    );

    if (typeCheckResult.orphanedProducers.length > 0) {
      console.log(
        `  Orphaned producers: ${typeCheckResult.orphanedProducers.join(", ")}`,
      );
    }
    if (typeCheckResult.orphanedConsumers.length > 0) {
      console.log(
        `  Orphaned consumers: ${typeCheckResult.orphanedConsumers.join(", ")}`,
      );
    }

    process.exit(0);
  } catch (error) {
    console.error("Type checking failed:", error);

    // Write error result
    const typeCheckOutputPath = "ts_check/output/type-check-results.json";
    const fs = await import("fs");
    const errorResult = {
      mismatches: [],
      compatibleCount: 0,
      totalChecked: 0,
      error: (error as Error).message,
    };
    fs.writeFileSync(typeCheckOutputPath, JSON.stringify(errorResult, null, 2));

    process.exit(1);
  }
}

main();
