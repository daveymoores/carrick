#!/usr/bin/env ts-node

/**
 * Type Checking Runner
 *
 * Supports two modes:
 * 1. Legacy mode: Uses generated *_types.ts files and alias-based matching
 * 2. Manifest mode: Uses manifest JSON files for explicit endpoint matching
 *
 * Usage:
 *   Legacy mode:  npx ts-node run-type-checking.ts <tsconfig-path> [--legacy]
 *   Manifest mode: npx ts-node run-type-checking.ts <tsconfig-path> --manifest --producer <path> --consumer <path>
 */

import { TypeCompatibilityChecker, TypeCheckMode } from "./lib/type-checker";
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
    },
  );
}

interface CliArgs {
  tsconfigPath: string;
  mode: TypeCheckMode;
  outputDir: string;
  producerManifest?: string;
  consumerManifest?: string;
  typesDir?: string;
}

function parseArgs(): CliArgs {
  const args = process.argv.slice(2);

  if (args.length === 0) {
    printUsage();
    process.exit(1);
  }

  const result: CliArgs = {
    tsconfigPath: args[0],
    mode: 'legacy',
    outputDir: 'ts_check/output',
  };

  for (let i = 1; i < args.length; i++) {
    const arg = args[i];

    switch (arg) {
      case '--legacy':
        result.mode = 'legacy';
        break;
      case '--manifest':
        result.mode = 'manifest';
        break;
      case '--producer':
        result.producerManifest = args[++i];
        break;
      case '--consumer':
        result.consumerManifest = args[++i];
        break;
      case '--output':
      case '-o':
        result.outputDir = args[++i];
        break;
      case '--types-dir':
        result.typesDir = args[++i];
        break;
      case '--help':
      case '-h':
        printUsage();
        process.exit(0);
      default:
        if (!arg.startsWith('-')) {
          // Assume it's the tsconfig path if not already set
          if (i === 0) {
            result.tsconfigPath = arg;
          }
        }
    }
  }

  // Validate manifest mode arguments
  if (result.mode === 'manifest') {
    if (!result.producerManifest || !result.consumerManifest) {
      console.error('Error: Manifest mode requires both --producer and --consumer paths');
      printUsage();
      process.exit(1);
    }
  }

  return result;
}

function printUsage(): void {
  console.log(`
Type Checking Runner

Usage:
  Legacy mode (default):
    npx ts-node run-type-checking.ts <tsconfig-path> [options]

  Manifest mode:
    npx ts-node run-type-checking.ts <tsconfig-path> --manifest --producer <path> --consumer <path> [options]

Options:
  --legacy              Use legacy alias-based type matching (default)
  --manifest            Use manifest-based type matching
  --producer <path>     Path to producer manifest JSON (required for manifest mode)
  --consumer <path>     Path to consumer manifest JSON (required for manifest mode)
  --output, -o <dir>    Output directory for results (default: ts_check/output)
  --types-dir <dir>     Directory containing bundled .d.ts files (manifest mode)
  --help, -h            Show this help message

Examples:
  # Legacy mode
  npx ts-node run-type-checking.ts ts_check/output/tsconfig.json

  # Manifest mode
  npx ts-node run-type-checking.ts ts_check/output/tsconfig.json \\
    --manifest \\
    --producer ./producer-manifest.json \\
    --consumer ./consumer-manifest.json
`);
}

async function installDependencies(outputDir: string): Promise<void> {
  console.log("Installing dependencies for type checking...");

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
}

async function runLegacyMode(
  typeChecker: TypeCompatibilityChecker,
  outputDir: string
): Promise<void> {
  console.log("\n🔍 Starting type compatibility checking (LEGACY MODE)...");
  console.log(`[mode] Using alias-based type matching from generated files`);

  const typeCheckResult = await typeChecker.checkGeneratedTypes(outputDir);

  // Create a simplified result format for the Rust analyzer
  const simplifiedResult = {
    mode: 'legacy',
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

  // Write type check results
  const typeCheckOutputPath = path.join(outputDir, "type-check-results.json");
  fs.writeFileSync(
    typeCheckOutputPath,
    JSON.stringify(simplifiedResult, null, 2),
  );

  logResults(typeCheckResult);
}

async function runManifestMode(
  typeChecker: TypeCompatibilityChecker,
  args: CliArgs
): Promise<void> {
  console.log("\n🔍 Starting type compatibility checking (MANIFEST MODE)...");
  console.log(`[mode] Using manifest-based type matching`);
  console.log(`[manifest] Producer: ${args.producerManifest}`);
  console.log(`[manifest] Consumer: ${args.consumerManifest}`);

  // Load manifests
  const producerManifest = typeChecker.loadManifest(args.producerManifest!);
  const consumerManifest = typeChecker.loadManifest(args.consumerManifest!);

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
    const dtsFiles = fs.readdirSync(args.typesDir)
      .filter(f => f.endsWith('.d.ts'))
      .map(f => path.join(args.typesDir!, f));

    for (const dtsFile of dtsFiles) {
      typesProject.addSourceFileAtPath(dtsFile);
    }
    console.log(`[types] Loaded ${dtsFiles.length} type definition files`);
  }

  // Run manifest-based type checking
  const typeCheckResult = await typeChecker.checkCompatibilityWithManifests(
    producerManifest,
    consumerManifest,
    typesProject
  );

  // Create a simplified result format
  const simplifiedResult = {
    mode: 'manifest',
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

  // Write type check results
  const typeCheckOutputPath = path.join(args.outputDir, "type-check-results.json");
  fs.writeFileSync(
    typeCheckOutputPath,
    JSON.stringify(simplifiedResult, null, 2),
  );

  logResults(typeCheckResult);
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
      `\n❌ Found ${typeCheckResult.mismatches.length} type compatibility issues:`,
    );
    typeCheckResult.mismatches.forEach((mismatch) => {
      console.log(
        `  - ${mismatch.endpoint}: ${cleanupPaths(mismatch.errorDetails)}`,
      );
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
}

async function main() {
  try {
    const args = parseArgs();

    console.log(`📋 Type Checking Configuration:`);
    console.log(`   TSConfig: ${args.tsconfigPath}`);
    console.log(`   Mode: ${args.mode.toUpperCase()}`);
    console.log(`   Output: ${args.outputDir}`);

    // Install dependencies in legacy mode
    if (args.mode === 'legacy') {
      await installDependencies(args.outputDir);
    }

    // Create ts-morph project
    const project = new Project({
      tsConfigFilePath: args.tsconfigPath,
    });

    // Create type checker instance
    const typeChecker = new TypeCompatibilityChecker(project);
    typeChecker.setMode(args.mode);

    // Run appropriate mode
    if (args.mode === 'manifest') {
      await runManifestMode(typeChecker, args);
    } else {
      await runLegacyMode(typeChecker, args.outputDir);
    }

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
