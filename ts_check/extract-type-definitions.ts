#!/usr/bin/env ts-node

import { parseArguments } from "./lib/argument-parser";
import { TypeExtractor } from "./lib/type-extractor";

async function main() {
  try {
    // Parse command line arguments
    const args = parseArguments();

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
