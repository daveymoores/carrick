export const MAX_PROPERTY_DEPTH = 5;
export const MAX_PROPERTIES_LIMIT = 50;
export const MAX_RECURSION_DEPTH = 10;

export const EXCLUDED_MODULE_SPECIFIERS = new Set([
  "typescript",
  "@types/node"
]);

export const DEFAULT_OUTPUT_FILE = "out/all-types-recursive.ts";
export const DEFAULT_PACKAGE_NAME = "extracted-types";
export const DEFAULT_PACKAGE_VERSION = "1.0.0";
export const DEFAULT_PACKAGE_TYPE = "module";

export function isNodeModulesPath(filePath: string): boolean {
  return filePath.includes("node_modules");
}

export function extractPackageName(moduleSpecifier: string): string {
  if (moduleSpecifier.startsWith("@")) {
    const parts = moduleSpecifier.split("/");
    return parts.length >= 2 ? `${parts[0]}/${parts[1]}` : moduleSpecifier;
  } else {
    const parts = moduleSpecifier.split("/");
    return parts[0];
  }
}

export function getTypesPackageName(packageName: string): string {
  return packageName.startsWith("@types/") ? packageName : `@types/${packageName}`;
}

export function getMainPackageFromTypes(typesPackageName: string): string {
  return typesPackageName.replace("@types/", "");
}