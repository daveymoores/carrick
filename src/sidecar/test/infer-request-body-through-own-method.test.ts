/**
 * carrick#2115: a request locator on an argument of a call into a function
 * the repository declares (`client.setTags(id, tags)`), or on that call. What
 * the call sends is what the function does with its parameters: the
 * argument unchanged (`forwarded`), an object literal it builds from them
 * (`built`), or nothing that can be decided (a decided abstain,
 * `argument_not_the_body`, so the capture's raw re-read does not publish the
 * argument again).
 */

import { describe, it, before, after } from 'node:test';
import * as assert from 'node:assert';
import * as fs from 'node:fs';
import * as path from 'node:path';
import { SidecarClient, FIXTURES_PATH } from './helpers.js';

const COMMAND = path.join(FIXTURES_PATH, 'src/argument-body-command.ts');

function lineOf(anchor: string): number {
  const source = fs.readFileSync(COMMAND, 'utf-8');
  const idx = source.indexOf(anchor);
  assert.ok(idx >= 0, `fixture must contain: ${anchor}`);
  assert.strictEqual(source.indexOf(anchor, idx + 1), -1, `ambiguous anchor: ${anchor}`);
  return source.slice(0, idx).split('\n').length;
}

interface Row {
  alias: string;
  type_string: string;
  any_provenance?: Array<{ path: string; reason: string; detail?: string }>;
}

describe('request body through a function the repository declares (carrick#2115)', () => {
  let client: SidecarClient;

  async function infer(alias: string, expressionText: string, anchor: string): Promise<Row | undefined> {
    const line = lineOf(anchor);
    const response = await client.send<{ inferred_types?: Row[]; errors?: string[] }>({
      action: 'infer',
      request_id: `own-method-${alias}`,
      requests: [
        {
          file_path: COMMAND,
          line_number: line,
          expression_text: expressionText,
          expression_line: line,
          infer_kind: 'request_body',
          alias,
        },
      ],
    });
    return response.inferred_types?.find((t) => t.alias === alias);
  }

  function assertArgumentNotTheBody(row: Row | undefined): void {
    assert.ok(row, 'a decided abstain returns a row');
    assert.strictEqual(row.type_string, 'unknown');
    const root = (row.any_provenance ?? []).find((p) => p.path === '');
    assert.strictEqual(root?.reason, 'argument_not_the_body');
    assert.strictEqual(root?.detail, undefined, 'the detail sentence waits for the owner');
  }

  before(async () => {
    client = new SidecarClient();
    await client.start();
    await client.send({ action: 'init', request_id: 'own-method-init', repo_root: FIXTURES_PATH });
  });

  after(async () => {
    await client.stop();
  });

  describe('built: the method sends an object literal made from the argument', () => {
    it('a method that wraps the argument in a one-key body', async () => {
      const row = await infer('RemoveItems', 'picked', 'client.removeItems(owner, picked)');
      assert.strictEqual(row?.type_string, '{ ids: string[]; }');
    });

    it('the body member of an object literal handed to another declared function', async () => {
      const row = await infer('SetTags', 'chosen.slice(1)', 'client.setTags(');
      assert.strictEqual(row?.type_string, '{ tags: string[]; }');
    });

    it('a callback parameter named again as the argument: the locator means the use', async () => {
      const row = await infer('EachIds', 'ids', 'lists.map((ids) => client.removeItems(owner, ids))');
      assert.strictEqual(row?.type_string, '{ ids: string[]; }');
    });

    it('through a second method of the same class', async () => {
      const row = await infer('RemoveVia', 'picked', 'client.removeVia(owner, picked)');
      assert.strictEqual(row?.type_string, '{ ids: string[]; }');
    });
  });

  describe('forwarded: the method sends the argument unchanged', () => {
    it('keeps the argument as the body', async () => {
      const row = await infer('Rename', 'next', 'client.rename(id, next)');
      assert.strictEqual(row?.type_string, '{ title: string; }');
    });

    it('a parameter typed by a type parameter, sent unchanged, keeps the argument', async () => {
      const row = await infer('Generic', 'item', "sendJson('/items', item)");
      assert.strictEqual(row?.type_string, '{ title: string; }');
    });
  });

  describe('decided abstain: argument_not_the_body', () => {
    it('an argument the method puts in the URL', async () => {
      assertArgumentNotTheBody(await infer('RemoveOwner', 'owner', 'client.removeOwner(owner)'));
    });

    it('an argument the method sends only through an operand (`note ?? {}`)', async () => {
      assertArgumentNotTheBody(await infer('Archive', "{ reason: 'stale' }", 'client.archive('));
    });

    it('a callee the repository declares with no body to read (amendment B)', async () => {
      assertArgumentNotTheBody(
        await infer('CreateDraft', 'revision', 'service.createDraft(project, revision)')
      );
    });
  });

  describe('the located node is the call itself (amendment A)', () => {
    it('reads the literal the function builds from its arguments, not its result', async () => {
      const row = await infer(
        'Configure',
        'host, parseInt(port, 10) || 9222',
        'configure(host, parseInt(port, 10) || 9222)'
      );
      assert.strictEqual(row?.type_string, '{ host: string; port: number; }');
    });

    it('a call no argument of which reaches a body abstains', async () => {
      assertArgumentNotTheBody(
        await infer('RemoveOwnerCall', 'client.removeOwner(owner)', 'client.removeOwner(owner)')
      );
    });
  });

  describe('review: reads of the parameter that send nothing do not disagree', () => {
    it('update by id: the parameter read for the URL and sent as the body keeps its type', async () => {
      const row = await infer('UpdateById', 'item', 'client.update(item)');
      assert.strictEqual(row?.type_string, '{ id: string; title: string; }');
    });

    it('a guard and a log call beside the send keep the argument', async () => {
      const row = await infer('SaveGuarded', 'kept', 'client.save(kept)');
      assert.strictEqual(row?.type_string, '{ id: string; title: string; }');
    });

    it('a field read inside the same built literal agrees with the parameter it reads', async () => {
      const row = await infer('Label', 'labelled', 'client.label(id, labelled)');
      assert.strictEqual(row?.type_string, '{ labels: string[]; count: number; }');
    });
  });

  describe('review: a body literal bound to a const before it is handed on', () => {
    it('the body member of a const-bound request config forwards the argument', async () => {
      const row = await infer('ReplaceTags', 'tagged', 'client.replaceTags(id, tagged)');
      assert.strictEqual(row?.type_string, 'string[]');
    });

    it('a const-bound literal sent as the body is the body built', async () => {
      const row = await infer('RemoveMany', 'many', 'client.removeMany(owner, many)');
      assert.strictEqual(row?.type_string, '{ ids: string[]; }');
    });
  });

  describe('review: callees written as arrow functions', () => {
    it('a class field arrow builds the body', async () => {
      const row = await infer('Retitle', "'renamed'", "client.retitle(id, 'renamed')");
      assert.strictEqual(row?.type_string, '{ title: string; }');
    });

    it('an arrow-valued object property forwards the argument', async () => {
      const row = await infer('NotesCreate', 'note', 'notesApi.create(note)');
      assert.strictEqual(row?.type_string, '{ title: string; }');
    });

    it('a callee whose body cannot be found keeps the argument', async () => {
      const row = await infer('FactorySender', 'renamed', 'sendRename(renamed)');
      assert.strictEqual(row?.type_string, '{ title: string; }');
    });

    it('an abstract method is provably bodiless and abstains', async () => {
      assertArgumentNotTheBody(await infer('AbstractPut', 'rev', 'store.put(project, rev)'));
    });
  });

  describe('controls that must not move', () => {
    it('an external request call keeps the argument', async () => {
      const row = await infer('AxiosLike', 'payload', 'axiosLike.post(url, payload)');
      assert.strictEqual(row?.type_string, '{ title: string; }');
    });

    it('a serialised fetch body keeps the payload', async () => {
      const row = await infer(
        'FetchBody',
        'JSON.stringify(payload)',
        "await fetch(url, { method: 'POST', body: JSON.stringify(payload) })"
      );
      assert.strictEqual(row?.type_string, '{ title: string; }');
    });

    it("a payload builder's result handed to the request keeps its own type", async () => {
      const row = await infer(
        'Builder',
        "draftPayload(payload.title, [])",
        "JSON.stringify(draftPayload(payload.title, []))"
      );
      assert.strictEqual(row?.type_string, '{ title: string; tags: string[]; }');
    });

    it("a handler's received value handed to its own service keeps its type", async () => {
      const row = await infer('Received', 'note', 'return this.store.save(note);');
      assert.strictEqual(row?.type_string, '{ title: string; }');
    });
  });
});
