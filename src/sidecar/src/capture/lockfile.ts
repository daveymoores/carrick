/**
 * Exact-version dependency pins from the producer repo's lockfile (pinned
 * decision 2: stub packages pin FROM THE LOCKFILE; npm and pnpm lockfiles in
 * scope, yarn-berry is a tracked follow-up).
 *
 * Pin selection prefers the DIRECT (top-level) dependency version: a package
 * present at multiple versions must pin to the version the emitted tree
 * resolves against — the hoisted/direct install — never to whichever nested
 * copy happens to appear first in the lockfile (npm sorts
 * `node_modules/a/node_modules/b` before `node_modules/b`, so first-match
 * picks a nested version the tree never sees).
 */

import * as fs from 'node:fs';
import * as path from 'node:path';

/** package-lock.json (v2/v3 `packages` map, v1 `dependencies` fallback). */
function npmLockfileVersions(lockPath: string): Map<string, string> {
  const versions = new Map<string, string>();
  const nested = new Map<string, string>();
  try {
    const lock = JSON.parse(fs.readFileSync(lockPath, 'utf8'));
    if (lock.packages && typeof lock.packages === 'object') {
      for (const [key, entry] of Object.entries<{ version?: string }>(lock.packages)) {
        const idx = key.lastIndexOf('node_modules/');
        if (idx === -1 || !entry?.version) continue;
        const name = key.slice(idx + 'node_modules/'.length);
        // Direct dependency: the top-level install path, exactly
        // `node_modules/<name>` (idx === 0 means no nesting prefix).
        if (idx === 0) {
          if (!versions.has(name)) versions.set(name, entry.version);
        } else if (!nested.has(name)) {
          nested.set(name, entry.version);
        }
      }
      // Nested copies only pin packages with no top-level install at all.
      for (const [name, version] of nested) {
        if (!versions.has(name)) versions.set(name, version);
      }
    } else if (lock.dependencies && typeof lock.dependencies === 'object') {
      for (const [name, entry] of Object.entries<{ version?: string }>(lock.dependencies)) {
        if (entry?.version) versions.set(name, entry.version);
      }
    }
  } catch {
    // Unparseable lockfile: every external ends up in unpinned_externals.
  }
  return versions;
}

/** Strip a pnpm peer suffix / quoting from a resolved version value. */
function cleanPnpmVersion(raw: string): string {
  return raw.trim().replace(/^['"]|['"]$/g, '').split('(')[0].trim();
}

/**
 * pnpm-lock.yaml. Two tiers, both via a targeted line parse (no YAML dep):
 *
 *  1. `importers:` — every importer's dependencies/devDependencies/
 *     optionalDependencies with their RESOLVED `version:` values. These are
 *     the direct (top-level) installs and win.
 *  2. `packages:` — the flat closure. Keys look like `/zod@3.23.0:` (v6),
 *     `zod@3.23.0:` (v9), `/@scope/pkg@1.2.3(peer@x)…:`. Fallback for
 *     packages that are not a direct dependency of any importer; the
 *     packages map is name-sorted, so first-match here would otherwise pin
 *     the LOWEST version of a multi-version package.
 */
function pnpmLockfileVersions(lockPath: string): Map<string, string> {
  const direct = new Map<string, string>();
  const fallback = new Map<string, string>();
  try {
    const text = fs.readFileSync(lockPath, 'utf8');
    const lines = text.split('\n');

    let inPackages = false;
    let inImporters = false;
    let inDepsSection = false;
    let currentDep: string | undefined;

    for (const line of lines) {
      if (/^\S/.test(line)) {
        // New top-level key.
        inPackages = /^packages:\s*$/.test(line);
        inImporters = /^importers:\s*$/.test(line);
        inDepsSection = false;
        currentDep = undefined;
        continue;
      }

      if (inImporters) {
        // `  <importer>:` (2-space) resets; `    dependencies:` (4-space)
        // opens a deps section; `      <name>:` (6-space) names a dep;
        // `        version: <v>` (8-space) resolves it.
        if (/^ {2}\S/.test(line)) {
          inDepsSection = false;
          currentDep = undefined;
        } else if (/^ {4}\S/.test(line)) {
          inDepsSection = /^ {4}(dependencies|devDependencies|optionalDependencies):\s*$/.test(
            line
          );
          currentDep = undefined;
        } else if (inDepsSection && /^ {6}\S/.test(line)) {
          const m = /^ {6}['"]?([^'":]+)['"]?:\s*(\S.*)?$/.exec(line);
          currentDep = m?.[1];
          // Old inline form: `      zod: 3.23.8`.
          if (m?.[1] && m[2] && !direct.has(m[1])) {
            const v = cleanPnpmVersion(m[2]);
            if (/^\d/.test(v)) direct.set(m[1], v);
          }
        } else if (inDepsSection && currentDep && /^ {8}version:/.test(line)) {
          const v = cleanPnpmVersion(line.slice(line.indexOf(':') + 1));
          if (v && !direct.has(currentDep) && /^\d/.test(v)) direct.set(currentDep, v);
          currentDep = undefined;
        }
        continue;
      }

      if (inPackages) {
        // Two-space-indented keys only (package entries), e.g.
        //   /zod@3.23.0: | 'zod@3.23.0': | /@scope/pkg@1.2.3(peer@1.0.0):
        const m = /^ {2}['"]?\/?((?:@[^/@'"]+\/)?[^/@'"]+)@([^('":]+)/.exec(line);
        if (!m) continue;
        const [, name, version] = m;
        if (!fallback.has(name)) fallback.set(name, version.trim());
      }
    }
  } catch {
    // Unparseable lockfile: every external ends up in unpinned_externals.
  }

  const versions = new Map<string, string>(direct);
  for (const [name, version] of fallback) {
    if (!versions.has(name)) versions.set(name, version);
  }
  return versions;
}

/**
 * Resolve exact versions for the repo. Walks up from repoRoot so monorepo
 * members with a hoisted root lockfile still pin (nearest lockfile wins;
 * package-lock.json preferred over pnpm-lock.yaml at the same level).
 */
export function lockfileVersions(repoRoot: string): Map<string, string> {
  let dir = path.resolve(repoRoot);
  for (;;) {
    const npmLock = path.join(dir, 'package-lock.json');
    if (fs.existsSync(npmLock)) return npmLockfileVersions(npmLock);
    const pnpmLock = path.join(dir, 'pnpm-lock.yaml');
    if (fs.existsSync(pnpmLock)) return pnpmLockfileVersions(pnpmLock);
    const parent = path.dirname(dir);
    if (parent === dir) return new Map();
    dir = parent;
  }
}
