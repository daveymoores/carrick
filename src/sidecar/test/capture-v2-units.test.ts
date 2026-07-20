/**
 * Unit tests for the capture bundle's deterministic helpers: lockfile pin
 * parsing (npm v3, npm v1, pnpm v6/v9, walk-up) and specifier
 * matching/rewriting (the paths-rewrite building blocks).
 */

import { describe, it } from 'node:test';
import * as assert from 'node:assert';
import * as fs from 'node:fs';
import * as os from 'node:os';
import * as path from 'node:path';
import { lockfileVersions } from '../src/capture/lockfile.js';
import {
  collectSpecifiers,
  matchPathsPattern,
  packageNameOf,
  rewriteSpecifiers,
} from '../src/capture/specifiers.js';

function tempDir(): string {
  return fs.mkdtempSync(path.join(os.tmpdir(), 'carrick-capture-units-'));
}

describe('lockfileVersions', () => {
  it('parses npm lockfile v3 packages map, direct entry wins', () => {
    const dir = tempDir();
    fs.writeFileSync(
      path.join(dir, 'package-lock.json'),
      JSON.stringify({
        lockfileVersion: 3,
        packages: {
          '': {},
          'node_modules/zod': { version: '3.23.0' },
          'node_modules/@scope/pkg': { version: '2.0.1' },
          'node_modules/other/node_modules/zod': { version: '4.0.0' },
        },
      })
    );
    const versions = lockfileVersions(dir);
    assert.strictEqual(versions.get('zod'), '3.23.0');
    assert.strictEqual(versions.get('@scope/pkg'), '2.0.1');
  });

  it('npm: prefers the direct dependency when a nested copy sorts first', () => {
    // npm sorts `node_modules/a/node_modules/b` BEFORE `node_modules/b`, so
    // first-match-wins pinned the nested version the emitted tree never
    // resolves against (adversarial-review finding 4).
    const dir = tempDir();
    fs.writeFileSync(
      path.join(dir, 'package-lock.json'),
      JSON.stringify({
        lockfileVersion: 3,
        packages: {
          '': {},
          'node_modules/aaa': { version: '1.0.0' },
          'node_modules/aaa/node_modules/zod': { version: '2.9.9' },
          'node_modules/zod': { version: '3.23.0' },
          // Nested-only package: the nested copy is the only install.
          'node_modules/aaa/node_modules/nested-only': { version: '5.5.5' },
        },
      })
    );
    const versions = lockfileVersions(dir);
    assert.strictEqual(versions.get('zod'), '3.23.0');
    assert.strictEqual(versions.get('nested-only'), '5.5.5');
  });

  it('pnpm: prefers the importer-resolved (direct) version over the packages map', () => {
    // The pnpm packages map is name-sorted, so a multi-version package's
    // first entry is its LOWEST version; the importers section carries the
    // resolved direct-dependency version (adversarial-review finding 4).
    const dir = tempDir();
    fs.writeFileSync(
      path.join(dir, 'pnpm-lock.yaml'),
      [
        'lockfileVersion: 9.0',
        '',
        'importers:',
        '  .:',
        '    dependencies:',
        '      zod:',
        "        specifier: ^3.24.0",
        "        version: 3.24.1",
        '  packages/member:',
        '    dependencies:',
        '      redis:',
        "        specifier: ^4.6.0",
        "        version: 4.6.7(peer@1.0.0)",
        '',
        'packages:',
        '',
        // Lower version of a transitive copy sorts first.
        "  zod@3.22.0:",
        "    resolution: {integrity: sha512-a}",
        '',
        "  zod@3.24.1:",
        "    resolution: {integrity: sha512-b}",
        '',
        "  redis@4.6.7:",
        "    resolution: {integrity: sha512-c}",
        '',
        "  transitive-only@1.2.3:",
        "    resolution: {integrity: sha512-d}",
        '',
      ].join('\n')
    );
    const versions = lockfileVersions(dir);
    assert.strictEqual(versions.get('zod'), '3.24.1');
    // Peer suffix stripped from the importer-resolved version.
    assert.strictEqual(versions.get('redis'), '4.6.7');
    // Non-direct packages still pin from the packages map.
    assert.strictEqual(versions.get('transitive-only'), '1.2.3');
  });

  it('parses npm lockfile v1 dependencies fallback', () => {
    const dir = tempDir();
    fs.writeFileSync(
      path.join(dir, 'package-lock.json'),
      JSON.stringify({
        lockfileVersion: 1,
        dependencies: { express: { version: '4.18.2' } },
      })
    );
    assert.strictEqual(lockfileVersions(dir).get('express'), '4.18.2');
  });

  it('parses pnpm-lock.yaml v6 and v9 style package keys', () => {
    const dir = tempDir();
    fs.writeFileSync(
      path.join(dir, 'pnpm-lock.yaml'),
      [
        'lockfileVersion: 9.0',
        '',
        'importers:',
        '  .:',
        '    dependencies:',
        '      zod:',
        "        specifier: ^3.23.0",
        "        version: 3.23.0",
        '',
        'packages:',
        '',
        "  zod@3.23.0:",
        "    resolution: {integrity: sha512-x}",
        '',
        "  '@scope/pkg@2.4.0(peerdep@1.0.0)':",
        "    resolution: {integrity: sha512-y}",
        '',
        "  /legacy-style@1.1.1:",
        "    resolution: {integrity: sha512-z}",
        '',
      ].join('\n')
    );
    const versions = lockfileVersions(dir);
    assert.strictEqual(versions.get('zod'), '3.23.0');
    assert.strictEqual(versions.get('@scope/pkg'), '2.4.0');
    assert.strictEqual(versions.get('legacy-style'), '1.1.1');
  });

  it('walks up to a hoisted monorepo lockfile', () => {
    const root = tempDir();
    const member = path.join(root, 'packages', 'svc');
    fs.mkdirSync(member, { recursive: true });
    fs.writeFileSync(
      path.join(root, 'package-lock.json'),
      JSON.stringify({
        lockfileVersion: 3,
        packages: { 'node_modules/redis': { version: '4.6.0' } },
      })
    );
    assert.strictEqual(lockfileVersions(member).get('redis'), '4.6.0');
  });

  it('returns empty for an unparseable lockfile (everything unpinned)', () => {
    const dir = tempDir();
    fs.writeFileSync(path.join(dir, 'package-lock.json'), '{nope');
    assert.strictEqual(lockfileVersions(dir).size, 0);
  });
});

describe('specifier helpers', () => {
  it('collects from-imports, bare imports, and import() type references', () => {
    const text = [
      "import { a } from './rel';",
      "import 'side-effect';",
      'export declare const x: import("zod").ZodType<string>;',
    ].join('\n');
    const specs = collectSpecifiers(text);
    assert.ok(specs.has('./rel'));
    assert.ok(specs.has('side-effect'));
    assert.ok(specs.has('zod'));
  });

  it('extracts package names from deep and scoped specifiers', () => {
    assert.strictEqual(packageNameOf('zod'), 'zod');
    assert.strictEqual(packageNameOf('zod/lib/types'), 'zod');
    assert.strictEqual(packageNameOf('@scope/pkg/sub/deep'), '@scope/pkg');
  });

  it('matches star and exact paths patterns', () => {
    const star = { pattern: '@app/*', prefix: '@app/', suffix: '', targets: [] };
    assert.strictEqual(matchPathsPattern('@app/models/item', star), 'models/item');
    assert.strictEqual(matchPathsPattern('@other/x', star), undefined);
    const exact = { pattern: 'config', prefix: 'config', suffix: undefined, targets: [] };
    assert.strictEqual(matchPathsPattern('config', exact), '');
    assert.strictEqual(matchPathsPattern('config/sub', exact), undefined);
  });

  it('rewrites only mapped specifiers and counts them', () => {
    const text = 'import { Item } from "@app/models/item";\nimport { z } from "zod";';
    const { text: out, rewrites } = rewriteSpecifiers(text, (spec) =>
      spec.startsWith('@app/') ? './rewritten' : undefined
    );
    assert.strictEqual(rewrites, 1);
    assert.ok(out.includes('from "./rewritten"'));
    assert.ok(out.includes('from "zod"'));
  });
});
