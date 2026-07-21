/**
 * #439 part 1: builder-chain producer anchors must select the schema
 * argument, not the config descriptor.
 *
 * A producer request anchor whose line-based locator lands inside a fluent
 * builder chain's config-descriptor argument (`.meta({ openapi: {...} })`)
 * would otherwise capture that all-literal metadata object as the request
 * type. The fix re-aims at the chain's lone schema argument, or demotes when
 * the payload cannot be picked unambiguously — never keeping the descriptor
 * (the artifact behind v1's false-incompatible verdicts).
 *
 * Detection is STRUCTURAL (no method names): the descriptor is a non-empty,
 * all-literal object literal argument of a chain of >=2 fluent calls; the
 * payload is the lone non-literal, non-inline-function chain argument. All
 * fixtures are synthetic and generically named.
 */

import { describe, it, before, after } from 'node:test';
import * as assert from 'node:assert';
import * as fs from 'node:fs';
import * as os from 'node:os';
import * as path from 'node:path';
import { captureStub } from '../src/capture/index.js';
import type { CaptureStubResult } from '../src/capture/api.js';

let repoDir: string;
let outRoot: string;

const ROUTER_TS = `
interface Builder {
  meta(config: object): Builder;
  input(schema: unknown): Builder;
  output(schema: unknown): Builder;
  tag(name: string): Builder;
  mutation(handler: () => unknown): Route;
}
interface Route { readonly __route: true; }
declare const procedure: Builder;

// A non-schema string constant passed to a chain method.
declare const routeTag: string;

// A plain-object schema whose inferred payload is an anonymous object type
// (the shape a \`.input(schema)\` argument carries after inference).
declare const updateSchema: { documentId: number; title: string };

// A fluent builder whose config descriptor is all-literal metadata and whose
// single schema argument carries the request payload. The anchor's locator
// lands on the descriptor object literal.
export const updateRoute = procedure
  .meta({ openapi: { method: "POST", path: "/documents/update", summary: "x" } })
  .input(updateSchema)
  .mutation(() => ({ ok: true }));

declare const inputSchema: { a: number };
declare const outputSchema: { b: string };

// Two schema arguments (input + output) cannot be disambiguated structurally.
export const twoSchemaRoute = procedure
  .meta({ tag: "documents" })
  .input(inputSchema)
  .output(outputSchema)
  .mutation(() => ({}));

// Control: a real body object literal returned from a function (not a fluent
// chain argument) must be captured as-is, never demoted.
export function makeResponse() {
  return { id: 1, name: "widget" };
}

// Control: an all-literal object literal argument of a SINGLE call (not a
// fluent chain) must be captured as-is (the express \`res.json({...})\` shape).
declare function send(body: unknown): void;
export function handler() {
  send({ status: "ok", code: 200 });
}

// Non-schema lone argument: the only non-descriptor argument is a string const,
// not a schema. Capturing "documents" as a request payload is a false
// incompatible, so this must abstain (demote).
export const tagRoute = procedure
  .meta({ openapi: { method: "POST", path: "/documents/tag" } })
  .tag(routeTag)
  .mutation(() => ({}));

// A chain whose true payload is an all-literal object arg (never a candidate)
// alongside a non-schema string identifier: must abstain, not mis-aim onto the
// identifier.
export const literalPayloadRoute = procedure
  .meta({ openapi: { method: "POST", path: "/documents/create" } })
  .input({ orderId: 1, label: "x" })
  .tag(routeTag)
  .mutation(() => ({}));
`;

function writeRepo(): void {
  fs.mkdirSync(path.join(repoDir, 'src'), { recursive: true });
  fs.writeFileSync(
    path.join(repoDir, 'tsconfig.json'),
    JSON.stringify({
      compilerOptions: {
        strict: true,
        rootDir: 'src',
        module: 'esnext',
        moduleResolution: 'bundler',
        target: 'es2022',
        skipLibCheck: true,
      },
      include: ['src'],
    })
  );
  fs.writeFileSync(path.join(repoDir, 'src', 'router.ts'), ROUTER_TS);
}

describe('capture v2: builder-chain payload selection over config descriptor', () => {
  let result: CaptureStubResult;
  let surface: string;

  before(() => {
    repoDir = fs.mkdtempSync(path.join(os.tmpdir(), 'carrick-chain-repo-'));
    outRoot = fs.mkdtempSync(path.join(os.tmpdir(), 'carrick-chain-stub-'));
    writeRepo();
    result = captureStub({
      repoRoot: repoDir,
      serviceName: 'chain-svc',
      outDir: path.join(outRoot, 'stub'),
      anchors: [
        // Locator lands on the descriptor object literal of the fluent chain.
        {
          kind: 'infer',
          alias: 'Payload_Request',
          source_file: 'src/router.ts',
          anchor_origin: 'deterministic-infer',
          expression_text:
            '{ openapi: { method: "POST", path: "/documents/update", summary: "x" } }',
          unwrap: 'none',
        },
        // Two schema args -> ambiguous -> demote.
        {
          kind: 'infer',
          alias: 'TwoSchema_Request',
          source_file: 'src/router.ts',
          anchor_origin: 'deterministic-infer',
          expression_text: '{ tag: "documents" }',
          unwrap: 'none',
        },
        // Control: return-body object literal, captured as-is.
        {
          kind: 'infer',
          alias: 'Body_Response',
          source_file: 'src/router.ts',
          anchor_origin: 'deterministic-infer',
          expression_text: '{ id: 1, name: "widget" }',
          unwrap: 'none',
        },
        // Control: single-call object-literal argument, captured as-is.
        {
          kind: 'infer',
          alias: 'Single_Request',
          source_file: 'src/router.ts',
          anchor_origin: 'deterministic-infer',
          expression_text: '{ status: "ok", code: 200 }',
          unwrap: 'none',
        },
        // Lone non-schema argument (a string const) -> must abstain, never
        // capture the string as a payload.
        {
          kind: 'infer',
          alias: 'Tag_Request',
          source_file: 'src/router.ts',
          anchor_origin: 'deterministic-infer',
          expression_text: '{ openapi: { method: "POST", path: "/documents/tag" } }',
          unwrap: 'none',
        },
        // Chain with a literal-object payload + a non-schema identifier -> must
        // abstain, never re-aim onto the identifier.
        {
          kind: 'infer',
          alias: 'LiteralPayload_Request',
          source_file: 'src/router.ts',
          anchor_origin: 'deterministic-infer',
          expression_text: '{ openapi: { method: "POST", path: "/documents/create" } }',
          unwrap: 'none',
        },
      ],
    });
    assert.strictEqual(result.success, true, JSON.stringify(result.errors));
    surface = fs.readFileSync(
      path.join(result.stub_dir, 'types', 'surface.d.ts'),
      'utf8'
    );
  });

  after(() => {
    fs.rmSync(repoDir, { recursive: true, force: true });
    fs.rmSync(outRoot, { recursive: true, force: true });
  });

  function record(alias: string) {
    const r = result.aliases.find((a) => a.alias === alias);
    assert.ok(r, `no alias record for ${alias}`);
    return r;
  }

  it('captures the schema payload, not the descriptor, for a fluent chain', () => {
    const r = record('Payload_Request');
    assert.strictEqual(r.self_check, 'ok', r.self_check_detail);
    // The payload shape wins.
    assert.match(surface, /Payload_Request = [\s\S]*documentId/);
    assert.match(surface, /Payload_Request = [\s\S]*title/);
    // The config descriptor is gone.
    assert.ok(
      !/Payload_Request = [\s\S]*openapi/.test(surface),
      `descriptor metadata must not be captured:\n${surface}`
    );
    assert.match(r.self_check_detail ?? '', /re-aimed at the chain schema argument/);
  });

  it('demotes a chain with two schema arguments (ambiguous request/response)', () => {
    const r = record('TwoSchema_Request');
    assert.strictEqual(r.serialization, 'structural_fallback');
    assert.ok(r.capture_failure_reason, 'ambiguous chain must demote with a reason');
    assert.match(r.capture_failure_reason!, /2 schema arguments/);
    assert.match(surface, /TwoSchema_Request = unknown;/);
    // Never the descriptor.
    assert.ok(!/TwoSchema_Request = [\s\S]*tag/.test(surface), surface);
  });

  it('control: a return-body object literal is captured as-is (not a chain)', () => {
    const r = record('Body_Response');
    assert.strictEqual(r.self_check, 'ok', r.self_check_detail);
    assert.match(surface, /Body_Response = [\s\S]*id/);
    assert.match(surface, /Body_Response = [\s\S]*name/);
    assert.ok(!/Body_Response = unknown;/.test(surface), surface);
  });

  it('control: a single-call object argument is captured as-is (not fluent)', () => {
    const r = record('Single_Request');
    assert.strictEqual(r.self_check, 'ok', r.self_check_detail);
    assert.match(surface, /Single_Request = [\s\S]*status/);
    assert.match(surface, /Single_Request = [\s\S]*code/);
    assert.ok(!/Single_Request = unknown;/.test(surface), surface);
  });

  it('abstains when the lone chain argument is a non-schema string const', () => {
    // Pre-fix this captured `string` (the tag const) as the request payload.
    const r = record('Tag_Request');
    assert.strictEqual(r.serialization, 'structural_fallback');
    assert.ok(r.capture_failure_reason, 'a non-schema lone arg must demote');
    assert.match(surface, /Tag_Request = unknown;/);
    // Never captures the string constant as a payload.
    assert.ok(!/Tag_Request = string;/.test(surface), surface);
  });

  it('abstains when a literal-object payload sits beside a non-schema identifier', () => {
    // Pre-fix this re-aimed onto the string identifier instead of abstaining.
    const r = record('LiteralPayload_Request');
    assert.strictEqual(r.serialization, 'structural_fallback');
    assert.ok(r.capture_failure_reason, 'an ambiguous non-object candidate must demote');
    assert.match(surface, /LiteralPayload_Request = unknown;/);
    assert.ok(!/LiteralPayload_Request = string;/.test(surface), surface);
  });
});
