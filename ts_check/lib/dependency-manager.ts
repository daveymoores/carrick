import { Dependencies, PackageJsonContent } from "./types";
import {
  extractPackageName,
  getTypesPackageName,
  getMainPackageFromTypes,
  DEFAULT_PACKAGE_NAME,
  DEFAULT_PACKAGE_VERSION,
  DEFAULT_PACKAGE_TYPE,
} from "./constants";

export class DependencyManager {
  private usedDependencies: Record<string, string> = {};

  constructor(private allDependencies: Dependencies) {}

  extractUsedDependencies(externalTypeImports: Map<string, Set<string>>): void {
    for (const [moduleSpecifier] of externalTypeImports.entries()) {
      const packageName = extractPackageName(moduleSpecifier);

      // Add the main package if it exists in allDependencies
      if (this.allDependencies[packageName]) {
        this.usedDependencies[packageName] =
          this.allDependencies[packageName].version;
        console.log(`Added main package: ${packageName}`);
      } else if (this.allDependencies[moduleSpecifier]) {
        // Try exact match for the module specifier
        this.usedDependencies[moduleSpecifier] =
          this.allDependencies[moduleSpecifier].version;
        console.log(`Added exact match: ${moduleSpecifier}`);
      }

      // Check if a corresponding @types package exists and add it ONLY if it exists
      const typesPackageName = getTypesPackageName(packageName);

      if (this.allDependencies[typesPackageName]) {
        this.usedDependencies[typesPackageName] =
          this.allDependencies[typesPackageName].version;
        console.log(`Added types package: ${typesPackageName}`);
      }

      // If we started with a @types package, check if the main package exists
      if (packageName.startsWith("@types/")) {
        const mainPackageName = getMainPackageFromTypes(packageName);
        if (this.allDependencies[mainPackageName]) {
          this.usedDependencies[mainPackageName] =
            this.allDependencies[mainPackageName].version;
          console.log(`Added main package for types: ${mainPackageName}`);
        }
      }
    }
  }

  getUsedDependencies(): Record<string, string> {
    return this.usedDependencies;
  }

  createPackageJsonContent(): PackageJsonContent {
    return {
      name: DEFAULT_PACKAGE_NAME,
      version: DEFAULT_PACKAGE_VERSION,
      type: DEFAULT_PACKAGE_TYPE,
      dependencies: this.usedDependencies,
    };
  }

  writePackageJson(outputFile: string): void {
    const outputDir = outputFile.substring(0, outputFile.lastIndexOf("/"));
    const packageJsonPath = `${outputDir}/package.json`;
    const packageJsonContent = this.createPackageJsonContent();

    try {
      const packageJsonString = JSON.stringify(packageJsonContent, null, 2);
      require("fs").writeFileSync(packageJsonPath, packageJsonString);
      console.log(
        `Package.json created at ${packageJsonPath} with ${Object.keys(this.usedDependencies).length} dependencies`,
      );

      // Also copy tsconfig.json to output directory with dynamic path mappings
      const tsconfigPath = `${outputDir}/tsconfig.json`;
      const tsconfigContent = this.createDynamicTsconfig(outputDir);
      require("fs").writeFileSync(tsconfigPath, JSON.stringify(tsconfigContent, null, 2));
      console.log(`tsconfig.json created at ${tsconfigPath}`);
    } catch (error) {
      console.error("Error creating package.json or tsconfig.json:", error);
    }
  }

  private createDynamicTsconfig(outputDir: string): any {
    const paths: Record<string, string[]> = {
      "*-types": ["./*_types"]
    };

    // Scan for actual type files and create specific mappings
    try {
      const fs = require("fs");
      const entries = fs.readdirSync(outputDir);
      
      for (const fileName of entries) {
        if (fileName.endsWith("_types.ts")) {
          const baseName = fileName.replace(".ts", "");
          const moduleName = baseName.replace("_", "-");
          paths[moduleName] = [`./${baseName}`];
        }
      }
    } catch (error) {
      console.warn("Could not scan directory for type files:", error);
    }

    return {
      "compilerOptions": {
        "target": "ES2020",
        "module": "commonjs",
        "strict": true,
        "esModuleInterop": true,
        "skipLibCheck": true,
        "forceConsistentCasingInFileNames": true,
        "resolveJsonModule": true,
        "declaration": true,
        "outDir": "./dist",
        "baseUrl": ".",
        "paths": paths
      },
      "include": [
        "*.ts",
        "**/*.ts"
      ],
      "exclude": [
        "node_modules",
        "dist"
      ]
    };
  }

  clear(): void {
    this.usedDependencies = {};
  }
}
