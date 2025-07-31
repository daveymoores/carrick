import { Node, ts } from "ts-morph";
import { getModuleSpecifierFromNodeModulesPath } from "./type-resolver";
import {
  EXCLUDED_MODULE_SPECIFIERS,
  extractPackageName,
  getTypesPackageName,
  getMainPackageFromTypes,
} from "./constants";

export class ImportHandler {
  private externalTypeImports = new Map<string, Set<string>>();

  addImportForExternalType(node: Node): void {
    const sourceFilePath = node.getSourceFile().getFilePath();

    // Try to get the module specifier (e.g. "express")
    const importDecl = node.getFirstAncestorByKind?.(
      ts.SyntaxKind.ImportDeclaration,
    );
    let moduleSpecifier: string | undefined;
    if (importDecl) {
      moduleSpecifier = importDecl.getModuleSpecifierValue();
    } else {
      moduleSpecifier = getModuleSpecifierFromNodeModulesPath(sourceFilePath);
    }

    // Get the type name - try multiple approaches
    let typeName: string | undefined;

    if (
      Node.isInterfaceDeclaration(node) ||
      Node.isClassDeclaration(node) ||
      Node.isTypeAliasDeclaration(node) ||
      Node.isEnumDeclaration(node)
    ) {
      typeName = node.getName?.();
    } else if (Node.isImportSpecifier(node)) {
      typeName = node.getName();
    } else {
      // Try to extract type name from the node text or symbol
      const symbol = node.getSymbol();
      if (symbol) {
        typeName = symbol.getName();
      } else {
        // Last resort: try to parse the node text
        const nodeText = node.getText();
        const match = nodeText.match(/\b([A-Z][a-zA-Z0-9]*)\b/);
        if (match) {
          typeName = match[1];
        }
      }
    }

    if (typeName && moduleSpecifier) {
      // Prevent imports from excluded module specifiers
      if (EXCLUDED_MODULE_SPECIFIERS.has(moduleSpecifier)) {
        return; // Do not add to externalTypeImports
      }

      if (!this.externalTypeImports.has(moduleSpecifier)) {
        this.externalTypeImports.set(moduleSpecifier, new Set());
      }
      this.externalTypeImports.get(moduleSpecifier)!.add(typeName);
    }
  }

  getExternalTypeImports(): Map<string, Set<string>> {
    return this.externalTypeImports;
  }

  generateImportStatements(usedDependencies: Record<string, string>): string {
    let importStatements = "";

    for (const [
      moduleSpecifier,
      typeNames,
    ] of this.externalTypeImports.entries()) {
      // Determine the correct import source
      let importFrom = moduleSpecifier;

      // If this is a @types package, try to import from the main package instead
      if (moduleSpecifier.startsWith("@types/")) {
        const mainPackage = getMainPackageFromTypes(moduleSpecifier);
        // Check if the main package exists in our dependencies
        if (usedDependencies[mainPackage]) {
          importFrom = mainPackage;
        }
      }

      importStatements += `import { ${Array.from(typeNames).join(", ")} } from "${importFrom}";\n`;
    }

    return importStatements;
  }

  getExternalPackages(): Set<string> {
    const externalPackages = new Set<string>();

    for (const [moduleSpecifier] of this.externalTypeImports.entries()) {
      let importFrom = moduleSpecifier;

      // If this is a @types package, try to use the main package instead
      if (moduleSpecifier.startsWith("@types/")) {
        const mainPackage = getMainPackageFromTypes(moduleSpecifier);
        importFrom = mainPackage;
      }

      // Extract package name from module specifier
      const packageName = extractPackageName(importFrom);
      externalPackages.add(packageName);
    }

    return externalPackages;
  }

  clear(): void {
    this.externalTypeImports.clear();
  }
}
