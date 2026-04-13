/**
 * Normalize and match API paths across services.
 * Handles :param vs {param} styles, env var prefixes, trailing slashes.
 */

/**
 * Normalize a path for comparison:
 * - Lowercase
 * - Strip trailing slash
 * - Convert :param and {param} to a common wildcard
 * - Normalize path parameter styles to a common wildcard
 */
export function normalizePath(path: string): string {
  return (
    path
      .toLowerCase()
      .replace(/\/+$/, "") // strip trailing slashes
      .replace(/:[a-zA-Z_][a-zA-Z0-9_]*/g, ":param") // :id -> :param
      .replace(/\{[a-zA-Z_][a-zA-Z0-9_]*\}/g, ":param") // {id} -> :param
      || "/"
  );
}

/**
 * Check if two paths match after normalization.
 */
export function pathsMatch(a: string, b: string): boolean {
  return normalizePath(a) === normalizePath(b);
}

/**
 * Check if a path contains a substring (case-insensitive).
 */
export function pathContains(path: string, substring: string): boolean {
  return path.toLowerCase().includes(substring.toLowerCase());
}

/**
 * Extract named parameters from a path. Handles both `:id` and `{id}` styles.
 * Returns a record mapping parameter names to "string" (their inferred type).
 */
export function extractPathParams(path: string): Record<string, string> {
  const params: Record<string, string> = {};
  for (const match of path.matchAll(/:([a-zA-Z_][a-zA-Z0-9_]*)/g)) {
    params[match[1]] = "string";
  }
  for (const match of path.matchAll(/\{([a-zA-Z_][a-zA-Z0-9_]*)\}/g)) {
    params[match[1]] = "string";
  }
  return params;
}
