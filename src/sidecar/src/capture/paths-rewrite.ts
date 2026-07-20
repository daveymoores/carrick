/**
 * Post-emit specifier rewrite pass (design doc, Capture step 3 -- REQUIRED
 * component). Declaration emit ships tsconfig-`paths`-mapped specifiers
 * verbatim (`@app/models/item` dangles after relocation into the stub), and
 * node-builder-printed import types can carry absolute source paths. Both
 * are mapped onto tree-relative specifiers here, at capture time, in the
 * producer's own resolution context -- `paths` cannot be fixed at check time
 * (program-global, and namespaces collide across stubs).
 *
 * Specifiers that map outside the emitted tree are left untouched: the
 * per-alias self-check classifies them as dangling internals with a recorded
 * reason, which is the honest outcome.
 */

import ts from 'typescript';
import * as fs from 'node:fs';
import * as path from 'node:path';
import {
  isRelative,
  matchPathsPattern,
  rewriteSpecifiers,
  type PathsPattern,
} from './specifiers.js';

export interface RewriteArgs {
  /** Absolute path of the stub's types/ directory. */
  typesDir: string;
  /** Tree-relative emitted file paths (as recorded in emitted_files, without
   * the leading "types/"). */
  files: string[];
  /** The producer repo's parsed compiler options. */
  options: ts.CompilerOptions;
  /** Absolute path of the tsconfig the options came from. */
  configPath: string;
  /** Effective rootDir the emit ran with (tree layout mirrors it). */
  entryDir: string;
}

export function parsePathsPatterns(
  options: ts.CompilerOptions,
  configPath: string
): PathsPattern[] {
  const paths = options.paths;
  if (!paths) return [];
  // Since TS 4.1 `paths` without `baseUrl` resolves relative to the config
  // file; the parser records that base internally as pathsBasePath.
  const base = path.resolve(
    options.baseUrl ??
      ((options as { pathsBasePath?: string }).pathsBasePath || path.dirname(configPath))
  );
  const patterns: PathsPattern[] = [];
  for (const [pattern, targets] of Object.entries(paths)) {
    const starIdx = pattern.indexOf('*');
    patterns.push({
      pattern,
      prefix: starIdx === -1 ? pattern : pattern.slice(0, starIdx),
      suffix: starIdx === -1 ? undefined : pattern.slice(starIdx + 1),
      targets: targets.map((t) => path.resolve(base, t)),
    });
  }
  return patterns;
}

/** Map an absolute source path to its tree-relative emitted file, if any. */
function treeFileFor(
  absSource: string,
  entryDir: string,
  emitted: Set<string>
): string | undefined {
  const noExt = absSource.replace(/\.(d\.ts|ts|tsx|mts|cts)$/, '');
  const rel = path.relative(entryDir, noExt).split(path.sep).join('/');
  if (rel.startsWith('..')) return undefined;
  for (const candidate of [`${rel}.d.ts`, `${rel}/index.d.ts`]) {
    if (emitted.has(candidate)) return candidate;
  }
  return undefined;
}

/** Tree-relative .d.ts path -> extensionless specifier from `fromFile`. */
function relativeSpecifier(fromFile: string, toFile: string): string {
  const fromDir = path.posix.dirname(fromFile);
  let rel = path.posix.relative(fromDir, toFile.replace(/\.d\.ts$/, ''));
  if (!rel.startsWith('.')) rel = `./${rel}`;
  return rel;
}

/**
 * Rewrite paths-mapped and absolute-internal specifiers in every emitted
 * file. Returns the number of specifiers rewritten.
 */
export function rewriteEmittedSpecifiers(args: RewriteArgs): number {
  const patterns = parsePathsPatterns(args.options, args.configPath);
  const emitted = new Set(args.files);
  let total = 0;

  for (const file of args.files) {
    const absFile = path.join(args.typesDir, file);
    const text = fs.readFileSync(absFile, 'utf8');
    const { text: rewritten, rewrites } = rewriteSpecifiers(text, (spec) => {
      // Absolute internal paths (node-builder import types).
      if (spec.startsWith('/')) {
        const target = treeFileFor(spec, args.entryDir, emitted);
        return target ? relativeSpecifier(file, target) : undefined;
      }
      if (isRelative(spec)) return undefined;
      // tsconfig-paths patterns, first matching target that exists in-tree
      // (mirrors the resolver's declaration-order semantics).
      for (const pattern of patterns) {
        const star = matchPathsPattern(spec, pattern);
        if (star === undefined) continue;
        for (const targetTemplate of pattern.targets) {
          const absTarget = targetTemplate.replace('*', star);
          const target = treeFileFor(absTarget, args.entryDir, emitted);
          if (target) return relativeSpecifier(file, target);
        }
        // Matched a pattern but no in-tree target: leave it for the
        // self-check to classify (never guess).
        return undefined;
      }
      return undefined;
    });
    if (rewrites > 0) {
      fs.writeFileSync(absFile, rewritten);
      total += rewrites;
    }
  }
  return total;
}
