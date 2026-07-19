/**
 * Exact-version dependency pins from the producer repo's lockfile (pinned
 * decision 2: stub packages pin FROM THE LOCKFILE; npm and pnpm lockfiles in
 * scope, yarn-berry is a tracked follow-up).
 */

import * as fs from 'node:fs';
import * as path from 'node:path';

/** package-lock.json (v2/v3 `packages` map, v1 `dependencies` fallback). */
function npmLockfileVersions(lockPath: string): Map<string, string> {
  const versions = new Map<string, string>();
  try {
    const lock = JSON.parse(fs.readFileSync(lockPath, 'utf8'));
    if (lock.packages && typeof lock.packages === 'object') {
      for (const [key, entry] of Object.entries<{ version?: string }>(lock.packages)) {
        const idx = key.lastIndexOf('node_modules/');
        if (idx === -1) continue;
        const name = key.slice(idx + 'node_modules/'.length);
        if (entry?.version && !versions.has(name)) versions.set(name, entry.version);
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

/**
 * pnpm-lock.yaml `packages:` section. Keys look like `/zod@3.23.0:` (v6),
 * `zod@3.23.0:` (v9), `/@scope/pkg@1.2.3(peer@x)…:`. A targeted line parse
 * avoids a YAML dependency; peer-suffix parentheses are stripped.
 */
function pnpmLockfileVersions(lockPath: string): Map<string, string> {
  const versions = new Map<string, string>();
  try {
    const text = fs.readFileSync(lockPath, 'utf8');
    const lines = text.split('\n');
    let inPackages = false;
    for (const line of lines) {
      if (/^packages:\s*$/.test(line)) {
        inPackages = true;
        continue;
      }
      if (inPackages && /^\S/.test(line)) inPackages = false; // next top-level key
      if (!inPackages) continue;
      // Two-space-indented keys only (package entries), e.g.
      //   /zod@3.23.0: | 'zod@3.23.0': | /@scope/pkg@1.2.3(peer@1.0.0):
      const m = /^ {2}['"]?\/?((?:@[^/@'"]+\/)?[^/@'"]+)@([^('":]+)/.exec(line);
      if (!m) continue;
      const [, name, version] = m;
      if (!versions.has(name)) versions.set(name, version.trim());
    }
  } catch {
    // Unparseable lockfile: every external ends up in unpinned_externals.
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
