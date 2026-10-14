/**
 * carrick#2113: a type the source states is read for a node only where the
 * node reaches the annotation or cast through wrappers that leave its value
 * as it is (parentheses, `await`, `!`, `satisfies`, a cast, a `.catch` /
 * `.finally` link). The nearest annotated declaration above the node used to
 * answer wherever the node sat in it, so:
 *
 *  - a body literal passed to a call whose result is bound to an annotated
 *    name was published as that annotation (the response DTO);
 *  - a call inside a function inside an annotated container (a table of
 *    commands, a service object) was published as the container's type;
 *  - a call that is one element of an annotated array read the array type.
 *
 * Fixture: `test/fixtures/sample-repo/src/stated-type-position.ts`.
 */

import { describe, it, before, after } from 'node:test';
import * as assert from 'node:assert';
import * as fs from 'node:fs';
import * as path from 'node:path';
import { SidecarClient, FIXTURES_PATH } from './helpers.js';

const FIXTURE = path.join(FIXTURES_PATH, 'src/stated-type-position.ts');
const SOURCE = fs.readFileSync(FIXTURE, 'utf-8');

const ITEM_UPSERT = '{ title: string; tags?: string[]; }';

function lineOf(anchor: string): number {
  const idx = SOURCE.indexOf(anchor);
  assert.ok(idx >= 0, `fixture must contain: ${anchor}`);
  assert.strictEqual(SOURCE.indexOf(anchor, idx + 1), -1, `ambiguous anchor: ${anchor}`);
  return SOURCE.slice(0, idx).split('\n').length;
}

interface InferResponseShape {
  inferred_types?: Array<{ alias: string; type_string: string; is_explicit: boolean }>;
  errors?: string[];
}

describe('a stated type is read only at the node it states (carrick#2113)', () => {
  let client: SidecarClient;

  async function infer(
    kind: 'request_body' | 'call_result',
    expressionText: string,
    anchor: string
  ): Promise<{ type_string: string; is_explicit: boolean }> {
    const line = lineOf(anchor);
    const alias = `Stated_${kind}_${line}_${expressionText.length}`;
    const response = await client.send<InferResponseShape>({
      action: 'infer',
      request_id: `stated-position-${alias}`,
      requests: [
        {
          file_path: FIXTURE,
          line_number: line,
          expression_text: expressionText,
          expression_line: line,
          infer_kind: kind,
          alias,
        },
      ],
    });
    const row = response.inferred_types?.find((t) => t.alias === alias);
    assert.ok(row, `no inferred type for ${alias}: ${JSON.stringify(response.errors)}`);
    return row;
  }

  before(async () => {
    client = new SidecarClient();
    await client.start();
    await client.send({ action: 'init', request_id: 'stated-position-init', repo_root: FIXTURES_PATH });
  });

  after(async () => {
    await client.stop();
  });

  describe('request body', () => {
    it('a literal passed to a call bound to an annotated name is the literal, not the annotation', async () => {
      const got = await infer('request_body', '{ title, tags: [] }', 'const result: ItemDto = await client.putItem');
      assert.strictEqual(got.type_string, '{ title: string; tags: never[]; }');
      assert.strictEqual(got.is_explicit, false);
    });

    it('a literal inside an annotated array element is the literal', async () => {
      const got = await infer('request_body', "{ title: 'a' }", 'const pair: ItemDto[]');
      assert.strictEqual(got.type_string, '{ title: string; }');
    });

    it('a literal inside a method of an annotated service object is the literal', async () => {
      const got = await infer('request_body', '{ title: id }', 'load: (id) => client.putItem');
      assert.strictEqual(got.type_string, '{ title: string; }');
    });

    it('a typed binding passed by name keeps its type (control)', async () => {
      const got = await infer('request_body', 'body', 'await client.putItem(id, body);');
      assert.strictEqual(got.type_string, ITEM_UPSERT);
    });

    it('a cast on the literal keeps the cast (control)', async () => {
      const got = await infer(
        'request_body',
        '{ title } as ItemUpsert',
        'await client.putItem(id, { title } as ItemUpsert)'
      );
      assert.strictEqual(got.type_string, ITEM_UPSERT);
      assert.strictEqual(got.is_explicit, true);
    });
  });

  describe('call result', () => {
    it('a call inside a function in an annotated table is its own result, not the table', async () => {
      const got = await infer('call_result', 'client.list()', 'print(await client.list());');
      assert.strictEqual(got.type_string, 'ItemDto[]');
    });

    it('a call that is one element of an annotated array is the element, not the array', async () => {
      const got = await infer('call_result', "client.putItem('a', { title: 'a' })", 'const pair: ItemDto[]');
      assert.strictEqual(got.type_string, 'ItemDto');
    });

    it('a call returned by a method of an annotated service object is its own result', async () => {
      const got = await infer('call_result', 'client.putItem(id, { title: id })', 'load: (id) => client.putItem');
      assert.strictEqual(got.type_string, 'ItemDto');
    });

    it('an annotated binding the awaited call initialises still answers (control)', async () => {
      const got = await infer('call_result', 'client.list()', 'const items: ItemDto[] = await client.list();');
      assert.strictEqual(got.type_string, 'ItemDto[]');
    });

    it('a cast on the awaited call still answers (control)', async () => {
      const got = await infer('call_result', 'client.list()', 'const one = (await client.list()) as ItemDto[];');
      assert.strictEqual(got.type_string, 'ItemDto[]');
    });

    it('a cast after a `.catch(cb)` link on the body read states the body the request call answers', async () => {
      // The request call is `fetch`; its result is read through `res.json()`,
      // whose value the `.catch` link hands on unchanged before the cast.
      const got = await infer('call_result', 'fetch(`/items/${id}`)', 'const res = await fetch(`/items/${id}`);');
      assert.strictEqual(
        got.type_string,
        '{ id?: string | undefined; title?: string | undefined; createdAt?: string | undefined; }'
      );
      assert.strictEqual(got.is_explicit, true);
    });

    it('a cast after a `.catch(cb)` link on the located call states its value (control)', async () => {
      const got = await infer('call_result', 'res.json()', 'return (await res.json().catch');
      assert.strictEqual(
        got.type_string,
        '{ id?: string | undefined; title?: string | undefined; createdAt?: string | undefined; }'
      );
      assert.strictEqual(got.is_explicit, true);
    });
  });
});
