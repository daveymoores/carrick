#!/usr/bin/env ts-node

import { Project } from 'ts-morph';
import { TypeCompatibilityChecker } from './lib/type-checker';

async function main() {
  try {
    const outputDir = process.argv[2] || 'ts_check/output';
    
    const project = new Project({
      compilerOptions: {
        target: 99, // Latest
        lib: ["es2022"],
        strict: true,
        skipLibCheck: true,
        allowSyntheticDefaultImports: true,
        esModuleInterop: true,
      }
    });

    const checker = new TypeCompatibilityChecker(project);
    const result = await checker.checkGeneratedTypes(outputDir);
    
    console.log(JSON.stringify(result));
    process.exit(result.incompatiblePairs > 0 ? 1 : 0);
  } catch (error) {
    console.log(JSON.stringify({
      error: true,
      message: (error as Error).message || String(error),
      totalProducers: 0,
      totalConsumers: 0,
      compatiblePairs: 0,
      incompatiblePairs: 0,
      mismatches: [],
      orphanedProducers: [],
      orphanedConsumers: []
    }));
    process.exit(1);
  }
}

main();