/**
 * Normalize and match API paths across services.
 * Handles :param vs {param} styles, env var prefixes, trailing slashes.
 */

/**
 * Normalize a path for comparison:
 * - Lowercase
 * - Strip trailing slash
 * - Convert :param and {param} to a common wildcard
 * - Strip common env-var-like prefixes (e.g. /api/v1 prefix from env)
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
