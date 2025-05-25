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
    } catch (error) {
      console.error("Error creating package.json:", error);
    }
  }

  clear(): void {
    this.usedDependencies = {};
  }
}
