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
  expanded?: string;
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
  // Find the start of the declaration
  const declPattern = new RegExp(
    `(?:export\\s+)?(?:type|interface)\\s+${escapeRegex(typeAlias)}\\b`,
  );
  const declMatch = declPattern.exec(bundledTypes);
  if (!declMatch) return null;

  const startIdx = declMatch.index;
  const afterDecl = bundledTypes.slice(declMatch.index + declMatch[0].length);

  // Determine if this is a type alias or interface
  const isInterface = /interface/.test(declMatch[0]);

  if (isInterface) {
    // Find opening brace, then brace-count to find the matching close
    const braceStart = afterDecl.indexOf("{");
    if (braceStart === -1) return null;

    // Preserve generic parameters and heritage clauses between the
    // interface name and the opening brace (e.g. "<T> extends Base ").
    const header = afterDecl.slice(0, braceStart);

    const body = extractBraceBlock(afterDecl, braceStart);
    if (body === null) return null;
    return `interface ${typeAlias}${header}${body}`;
  }

  // Type alias: find the "=" then extract the value
  const eqIdx = afterDecl.indexOf("=");
  if (eqIdx === -1) return null;

  const afterEq = afterDecl.slice(eqIdx + 1).trimStart();

  // Always use top-level semicolon scanning — this handles simple types,
  // object types, and compound types like `{ a: string } & Other`.
  const end = findTopLevelSemicolon(afterEq);
  if (end === -1) {
    // Fallback: take up to next top-level declaration or end of string
    const fallbackEnd = bundledTypes.slice(startIdx).search(
      /\n(?:export\s+)?(?:type|interface)\s+\w/,
    );
    const chunk = fallbackEnd === -1
      ? bundledTypes.slice(startIdx)
      : bundledTypes.slice(startIdx, startIdx + fallbackEnd);
    return chunk.trim() || null;
  }
  return `type ${typeAlias} = ${afterEq.slice(0, end).trim()};`;
}

/**
 * Extract a brace-delimited block starting at the given index,
 * counting nested braces to find the matching close.
 */
function extractBraceBlock(source: string, openIndex: number): string | null {
  let depth = 0;
  for (let i = openIndex; i < source.length; i++) {
    if (source[i] === "{") depth++;
    else if (source[i] === "}") {
      depth--;
      if (depth === 0) {
        return source.slice(openIndex, i + 1);
      }
    }
  }
  return null;
}

/**
 * Find the index of the first semicolon that is not nested inside
 * braces, angle brackets, or parentheses.
 */
function findTopLevelSemicolon(source: string): number {
  let braces = 0;
  let angles = 0;
  let parens = 0;
  for (let i = 0; i < source.length; i++) {
    const ch = source[i];
    if (ch === "{") braces++;
    else if (ch === "}") braces--;
    else if (ch === "<") angles++;
    else if (ch === ">" && source[i - 1] !== "=") {
      // Only decrement for generic closes, not arrow functions (=>)
      angles = Math.max(0, angles - 1);
    } else if (ch === "(") parens++;
    else if (ch === ")") parens--;
    else if (ch === ";" && braces === 0 && angles === 0 && parens === 0) {
      return i;
    }
  }
  return -1;
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
    const definition = entry.resolved_definition
      ?? extractTypeDefinition(bundledTypes, entry.type_alias);
    if (definition) {
      results.push({
        type_alias: entry.type_alias,
        type_kind: entry.type_kind,
        definition,
        expanded: entry.expanded_definition,
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
