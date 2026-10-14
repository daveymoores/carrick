/**
 * carrick#2282: a call inside a function that sits inside a returned
 * expression was typed as that expression.
 *
 * The call-result walk asks first whether the call is returned, and it looked
 * for the nearest `return` above the call. That search does not stop at a
 * function boundary. A request made in a callback, where the callback is part
 * of what an OUTER function returns, was answered with the outer function's
 * returned expression: a command table, a client object, a UI element.
 *
 * A return statement answers only for a call of the function it returns from.
 *
 * Nothing here matches a name or a library: the rule is the language's own
 * scoping of `return`.
 */

import { describe, it, before, after } from 'node:test';
import * as assert from 'node:assert';
import * as fs from 'node:fs';
import * as os from 'node:os';
import * as path from 'node:path';
import { SidecarClient } from './helpers.js';

const SOURCE_TS = `interface Item { id: string }
interface Saved { saved: boolean }
interface UiElement { tag: string; props: unknown }
declare const api: {
  sync(id: string): Promise<Saved>;
  save(value: string): Promise<Saved>;
  one(id: string): Promise<Item>;
};
declare function element(tag: string, props: { onClick: () => Promise<void> }): UiElement;
declare const url: string;

export function commands() {
  return [{ name: 'sync', run: async (id: string) => { await api.sync(id); } }];
}

export function Panel(value: string) {
  return element('button', { onClick: async () => { await api.save(value); } });
}

export function makeClient() {
  return {
    list: async () => (await fetch(url + '/items')).json() as Promise<Item[]>,
    one: async (id: string) => {
      const res = await fetch(url + '/items/' + id);
      return (await res.json()) as Item;
    },
  };
}

export async function returnsTheCall(id: string) {
  return api.one(id);
}

export const expressionBody = (id: string) => api.one(id);

export async function returnsAWrapper(id: string) {
  return { item: await api.one(id), at: 1 };
}

interface ClientRequest {
  on(event: string, listener: (error: Error) => void): ClientRequest;
  end(): void;
}
interface IncomingMessage {
  on(event: 'data', listener: (chunk: string) => void): void;
  on(event: 'end', listener: () => void): void;
}
declare function sendRequest(target: string, onResponse: (res: IncomingMessage) => void): ClientRequest;

export function fetchText(target: string): Promise<string> {
  return new Promise<string>((resolve, reject) => {
    const req = sendRequest(target, (res) => {
      let text = '';
      res.on('data', (chunk) => {
        text += chunk;
      });
      res.on('end', () => resolve(text));
    });
    req.on('error', reject);
    req.end();
  });
}
`;

interface InferShape {
  inferred_types?: Array<{ alias: string; type_string: string }>;
}

const collapse = (text: string): string => text.replace(/\s+/g, ' ').trim();

function lineOf(marker: string): number {
  const index = SOURCE_TS.split('\n').findIndex((line) => line.includes(marker));
  assert.ok(index >= 0, `fixture has no line holding ${JSON.stringify(marker)}`);
  return index + 1;
}

describe('carrick#2282: a return statement answers only for a call of its own function', () => {
  let client: SidecarClient;
  let repoDir: string;
  let sourcePath: string;

  before(async () => {
    repoDir = fs.mkdtempSync(path.join(os.tmpdir(), 'carrick-2282-'));
    fs.writeFileSync(
      path.join(repoDir, 'tsconfig.json'),
      JSON.stringify({
        compilerOptions: {
          strict: true,
          module: 'esnext',
          moduleResolution: 'bundler',
          target: 'es2022',
          lib: ['es2022', 'dom'],
          skipLibCheck: true,
        },
        include: ['src'],
      })
    );
    fs.mkdirSync(path.join(repoDir, 'src'), { recursive: true });
    sourcePath = path.join(repoDir, 'src', 'source.ts');
    fs.writeFileSync(sourcePath, SOURCE_TS);

    client = new SidecarClient();
    await client.start();
    await client.send({ action: 'init', request_id: 'init', repo_root: repoDir });
  });

  after(async () => {
    await client.stop();
    fs.rmSync(repoDir, { recursive: true, force: true });
  });

  const callResult = async (alias: string, expression: string, at: string): Promise<string | undefined> => {
    const response = await client.send<InferShape>({
      action: 'infer',
      request_id: `infer-${alias}`,
      requests: [
        {
          file_path: sourcePath,
          line_number: lineOf(at),
          expression_text: expression,
          expression_line: lineOf(at),
          infer_kind: 'call_result',
          alias,
        },
      ],
    });
    const inferred = response.inferred_types?.find((t) => t.alias === alias);
    return inferred ? collapse(inferred.type_string) : undefined;
  };

  it('reads a call in a callback of a returned table as its own result, not the table', async () => {
    assert.strictEqual(await callResult('InTable', 'api.sync(id)', 'api.sync(id)'), 'Saved');
  });

  it('reads a call in a handler of a returned element as its own result, not the element', async () => {
    assert.strictEqual(await callResult('InElement', 'api.save(value)', 'api.save(value)'), 'Saved');
  });

  it('reads a call in a method of a returned client as the body it reads, not the client', async () => {
    assert.strictEqual(await callResult('InClient', "fetch(url + '/items')", "fetch(url + '/items')"), '{ id: string; }[]');
  });

  it('follows the body read of the function the call is in, inside a returned client', async () => {
    assert.strictEqual(
      await callResult('OwnReturn', "fetch(url + '/items/' + id)", "fetch(url + '/items/' + id)"),
      '{ id: string; }'
    );
  });

  // Controls: a return still answers for a call of the function it returns from.

  it('still reads a function that returns the call itself', async () => {
    assert.strictEqual(await callResult('Returned', 'api.one(id)', 'return api.one(id)'), 'Item');
  });

  it('still reads an arrow with an expression body', async () => {
    assert.strictEqual(await callResult('Arrow', 'api.one(id)', 'expressionBody'), 'Item');
  });

  // The callback style: a request whose call returns the request object, made
  // inside the executor of the promise the outer function returns. What the
  // caller receives is what that promise settles with, not the request object.
  it('reads a request made in the executor of a returned promise as what the promise settles with', async () => {
    assert.strictEqual(
      await callResult('Executor', 'sendRequest(target, (res) => {', 'const req = sendRequest(target'),
      'string'
    );
  });

  it('still answers the returned expression where the function the call is in returns a wrapper', async () => {
    assert.strictEqual(
      await callResult('Wrapper', 'api.one(id)', 'item: await api.one(id)'),
      '{ item: Item; at: number; }'
    );
  });
});
