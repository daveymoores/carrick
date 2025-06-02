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
