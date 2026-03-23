import { TypeManifestEntry } from "../types.js";

/**
 * Extract type definitions from bundled .d.ts content using the type manifest.
 *
 * Strategy:
 * 1. Look up type_manifest entries by (method, path, role, type_kind)
 * 2. Get the type_alias from the manifest entry
 * 3. Regex-extract that alias's declaration from the bundled_types string
 */

export interface ExtractedType {
  type_alias: string;
  type_kind: "request" | "response";
  definition: string;
  is_explicit: boolean;
  file_path: string;
  line_number: number;
}

/**
 * Find type manifest entries matching the given method and path.
 */
export function findManifestEntries(
  manifest: TypeManifestEntry[],
  method: string,
  path: string,
  role: "producer" | "consumer" = "producer",
): TypeManifestEntry[] {
  const normalizedMethod = method.toUpperCase();
  const normalizedPath = normalizePath(path);

  return manifest.filter((entry) => {
    return (
      entry.method.toUpperCase() === normalizedMethod &&
      normalizePath(entry.path) === normalizedPath &&
      entry.role === role
    );
  });
}

/**
 * Extract a type declaration from bundled .d.ts content by alias name.
 * Handles: export type Foo = { ... }; and export interface Foo { ... }
 */
export function extractTypeDefinition(
  bundledTypes: string,
  typeAlias: string,
): string | null {
  // Try "export type Alias = ..." pattern
  const typePattern = new RegExp(
    `(?:export\\s+)?type\\s+${escapeRegex(typeAlias)}\\s*=\\s*([\\s\\S]*?);(?:\\s*(?:export|type|interface|$))`,
    "m",
  );
  const typeMatch = bundledTypes.match(typePattern);
  if (typeMatch) {
    return `type ${typeAlias} = ${typeMatch[1].trim()};`;
  }

  // Try "export interface Alias { ... }" pattern
  const interfacePattern = new RegExp(
    `(?:export\\s+)?interface\\s+${escapeRegex(typeAlias)}\\s*\\{([\\s\\S]*?)\\}`,
    "m",
  );
  const interfaceMatch = bundledTypes.match(interfacePattern);
  if (interfaceMatch) {
    return `interface ${typeAlias} {${interfaceMatch[1]}}`;
  }

  // Fallback: grab anything from "type/interface Alias" to the next top-level declaration
  const fallbackPattern = new RegExp(
    `(?:export\\s+)?(?:type|interface)\\s+${escapeRegex(typeAlias)}[\\s\\S]*?(?=(?:export\\s+)?(?:type|interface)\\s+\\w|$)`,
    "m",
  );
  const fallbackMatch = bundledTypes.match(fallbackPattern);
  if (fallbackMatch) {
    return fallbackMatch[0].trim();
  }

  return null;
}

/**
 * Extract all types for a given endpoint from bundled types using the manifest.
 */
export function extractEndpointTypes(
  manifest: TypeManifestEntry[],
  bundledTypes: string,
  method: string,
  path: string,
  role: "producer" | "consumer" = "producer",
): ExtractedType[] {
  const entries = findManifestEntries(manifest, method, path, role);
  const results: ExtractedType[] = [];

  for (const entry of entries) {
    const definition = extractTypeDefinition(bundledTypes, entry.type_alias);
    if (definition) {
      results.push({
        type_alias: entry.type_alias,
        type_kind: entry.type_kind,
        definition,
        is_explicit: entry.is_explicit,
        file_path: entry.file_path,
        line_number: entry.line_number,
      });
    }
  }

  return results;
}

function normalizePath(path: string): string {
  return (
    path
      .toLowerCase()
      .replace(/\/+$/, "")
      .replace(/:[a-zA-Z_][a-zA-Z0-9_]*/g, ":param")
      .replace(/\{[a-zA-Z_][a-zA-Z0-9_]*\}/g, ":param") || "/"
  );
}

function escapeRegex(str: string): string {
  return str.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
}
