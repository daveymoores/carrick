#!/usr/bin/env ts-node

/**
 * Type Checking Runner
 *
 * Runs manifest-based type checking between producer and consumer APIs.
 *
 * Usage:
 *   npx ts-node run-type-checking.ts <tsconfig-path> --producer <path> --consumer <path> [options]
 */

import { TypeCompatibilityChecker } from "./lib/type-checker";
import { Project } from "ts-morph";
import * as path from "path";
import * as fs from "fs";

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

interface CliArgs {
  tsconfigPath: string;
  outputDir: string;
  producerManifest: string;
  consumerManifest: string;
  typesDir?: string;
}

function parseArgs(): CliArgs {
  const args = process.argv.slice(2);

  if (args.length === 0 || args.includes("--help") || args.includes("-h")) {
    printUsage();
    process.exit(args.length === 0 ? 1 : 0);
  }

  const result: Partial<CliArgs> = {
    tsconfigPath: args[0],
    outputDir: "ts_check/output",
  };

  for (let i = 1; i < args.length; i++) {
    const arg = args[i];

    switch (arg) {
      case "--producer":
        result.producerManifest = args[++i];
        break;
      case "--consumer":
        result.consumerManifest = args[++i];
        break;
      case "--output":
      case "-o":
        result.outputDir = args[++i];
        break;
      case "--types-dir":
        result.typesDir = args[++i];
        break;
    }
  }

  // Validate required arguments
  if (!result.producerManifest || !result.consumerManifest) {
    console.error(
      "Error: Both --producer and --consumer manifest paths are required"
    );
    printUsage();
    process.exit(1);
  }

  return result as CliArgs;
}

function printUsage(): void {
  console.log(`
Type Checking Runner

Usage:
  npx ts-node run-type-checking.ts <tsconfig-path> --producer <path> --consumer <path> [options]

Required Arguments:
  <tsconfig-path>       Path to tsconfig.json for the types project
  --producer <path>     Path to producer manifest JSON
  --consumer <path>     Path to consumer manifest JSON

Options:
  --output, -o <dir>    Output directory for results (default: ts_check/output)
  --types-dir <dir>     Directory containing bundled .d.ts files
  --help, -h            Show this help message

Example:
  npx ts-node run-type-checking.ts ts_check/output/tsconfig.json \\
    --producer ./producer-manifest.json \\
    --consumer ./consumer-manifest.json \\
    --types-dir ./bundled-types
`);
}

function logResults(typeCheckResult: {
  mismatches: Array<{ endpoint: string; errorDetails: string }>;
  compatiblePairs: number;
  incompatiblePairs: number;
  orphanedProducers: string[];
  orphanedConsumers: string[];
}): void {
  // Log summary
  if (typeCheckResult.mismatches.length === 0) {
    console.log("\n✅ All types are compatible!");
  } else {
    console.log(
      `\n❌ Found ${typeCheckResult.mismatches.length} type compatibility issues:`
    );
    typeCheckResult.mismatches.forEach((mismatch) => {
      console.log(
        `  - ${mismatch.endpoint}: ${cleanupPaths(mismatch.errorDetails)}`
      );
    });
  }

  console.log(`\nType checking summary:`);
  console.log(`  Compatible pairs: ${typeCheckResult.compatiblePairs}`);
  console.log(`  Incompatible pairs: ${typeCheckResult.incompatiblePairs}`);
  console.log(
    `  Orphaned producers: ${typeCheckResult.orphanedProducers.length}`
  );
  console.log(
    `  Orphaned consumers: ${typeCheckResult.orphanedConsumers.length}`
  );

  if (typeCheckResult.orphanedProducers.length > 0) {
    console.log(
      `  Orphaned producers: ${typeCheckResult.orphanedProducers.join(", ")}`
    );
  }
  if (typeCheckResult.orphanedConsumers.length > 0) {
    console.log(
      `  Orphaned consumers: ${typeCheckResult.orphanedConsumers.join(", ")}`
    );
  }
}

async function main() {
  try {
    const args = parseArgs();

    console.log(`📋 Type Checking Configuration:`);
    console.log(`   TSConfig: ${args.tsconfigPath}`);
    console.log(`   Producer manifest: ${args.producerManifest}`);
    console.log(`   Consumer manifest: ${args.consumerManifest}`);
    console.log(`   Output: ${args.outputDir}`);

    // Create ts-morph project
    const project = new Project({
      tsConfigFilePath: args.tsconfigPath,
    });

    // Create type checker instance
    const typeChecker = new TypeCompatibilityChecker(project);

    console.log("\n🔍 Starting manifest-based type compatibility checking...");
    console.log(`[manifest] Producer: ${args.producerManifest}`);
    console.log(`[manifest] Consumer: ${args.consumerManifest}`);

    // Load manifests
    const producerManifest = typeChecker.loadManifest(args.producerManifest);
    const consumerManifest = typeChecker.loadManifest(args.consumerManifest);

    // Create a types project if types directory is specified
    let typesProject: Project | undefined;
    if (args.typesDir && fs.existsSync(args.typesDir)) {
      console.log(`[types] Loading bundled types from: ${args.typesDir}`);
      typesProject = new Project({
        compilerOptions: {
          strict: true,
          skipLibCheck: true,
        },
      });

      // Add all .d.ts files from the types directory
      const dtsFiles = fs
        .readdirSync(args.typesDir)
        .filter((f) => f.endsWith(".d.ts"))
        .map((f) => path.join(args.typesDir!, f));

      for (const dtsFile of dtsFiles) {
        typesProject.addSourceFileAtPath(dtsFile);
      }
      console.log(`[types] Loaded ${dtsFiles.length} type definition files`);
    }

    // Run manifest-based type checking
    const typeCheckResult = await typeChecker.checkCompatibility(
      producerManifest,
      consumerManifest,
      typesProject
    );

    // Create result output
    const simplifiedResult = {
      producerRepo: producerManifest.repo_name,
      consumerRepo: consumerManifest.repo_name,
      mismatches: typeCheckResult.mismatches.map((mismatch) => ({
        endpoint: mismatch.endpoint,
        producerType: cleanupPaths(mismatch.producerType),
        consumerType: cleanupPaths(mismatch.consumerType),
        error: cleanupPaths(mismatch.errorDetails),
        isCompatible: mismatch.isAssignable,
        producerLocation: mismatch.producerLocation,
        consumerLocation: mismatch.consumerLocation,
      })),
      compatibleCount: typeCheckResult.compatiblePairs,
      totalChecked:
        typeCheckResult.compatiblePairs + typeCheckResult.incompatiblePairs,
      matchDetails: typeCheckResult.matchDetails?.length || 0,
    };

    // Ensure output directory exists
    if (!fs.existsSync(args.outputDir)) {
      fs.mkdirSync(args.outputDir, { recursive: true });
    }

    // Write type check results
    const typeCheckOutputPath = path.join(
      args.outputDir,
      "type-check-results.json"
    );
    fs.writeFileSync(
      typeCheckOutputPath,
      JSON.stringify(simplifiedResult, null, 2)
    );

    logResults(typeCheckResult);

    process.exit(0);
  } catch (error) {
    console.error("Type checking failed:", error);

    // Write error result
    const typeCheckOutputPath = "ts_check/output/type-check-results.json";
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
