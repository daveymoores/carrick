/**
 * Line-anchored expression resolution for pub/sub payload locators: the SAME
 * locator text (`payloadValue`) occurs at two publish sites with different
 * types (pubsub-handlers.ts lines 61 and 74, > the search window apart). The
 * pub/sub infer collector always supplies an `expression_line` — the model's
 * own line when reported, otherwise the operation's line — precisely so this
 * search stays anchored: each request must resolve its OWN site's type. This
 * pins the sidecar half of that contract.
 */

import { describe, it, before, after } from 'node:test';
import * as assert from 'node:assert';
import * as path from 'node:path';
import { SidecarClient, FIXTURES_PATH } from './helpers.js';

const FIXTURE = path.join(FIXTURES_PATH, 'src/pubsub-handlers.ts');
const noWs = (s: string) => s.replace(/\s/g, '');

interface InferResponseShape {
  request_id: string;
  status: string;
  inferred_types?: Array<{ type_string: string; alias: string }>;
  errors?: string[];
}

describe('expression infer resolves the line-anchored occurrence', () => {
  let client: SidecarClient;

  before(async () => {
    client = new SidecarClient();
    await client.start();
    await client.send({
      action: 'init',
      request_id: 'ela-init',
      repo_root: FIXTURES_PATH,
    });
  });

  after(async () => {
    await client.stop();
  });

  it('same text, two sites: each anchored request gets its own type', async () => {
    const response = await client.send<InferResponseShape>({
      action: 'infer',
      request_id: 'ela-1',
      requests: [
        {
          file_path: FIXTURE,
          line_number: 61,
          expression_text: 'payloadValue',
          expression_line: 61,
          infer_kind: 'expression',
          alias: 'ELA_First',
        },
        {
          file_path: FIXTURE,
          line_number: 74,
          expression_text: 'payloadValue',
          expression_line: 74,
          infer_kind: 'expression',
          alias: 'ELA_Second',
        },
      ],
    });

    const byAlias = new Map(
      (response.inferred_types ?? []).map((t) => [t.alias, noWs(t.type_string)])
    );
    const first = byAlias.get('ELA_First');
    const second = byAlias.get('ELA_Second');
    assert.ok(first, 'first site should infer');
    assert.ok(second, 'second site should infer');
    assert.ok(
      first!.includes('n:number') && !first!.includes('s:string'),
      `first site must carry its own shape, got ${first}`
    );
    assert.ok(
      second!.includes('s:string') && !second!.includes('n:number'),
      `second site must carry its own shape, got ${second}`
    );
  });
});
