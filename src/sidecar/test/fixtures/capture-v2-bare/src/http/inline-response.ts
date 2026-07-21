// Issue #433 shape: producer response types declared as INLINE type literals
// inside a framework generic that cannot resolve on a bare checkout. fakelib
// is pinned in the lockfile but absent (no node_modules), so FakeRequest /
// FakeResponse are error-typed — but the literal type ARGUMENTS are
// dependency-free source syntax referencing only local types.
import type { FakeRequest, FakeResponse, FakeThing } from 'fakelib';
import { makeThing } from 'fakelib';
import type { Item } from '@app/models/item';

// Mirrors `express.Router()` on a bare checkout: the app value is error-typed
// through the missing library, so registration and send calls all type `any`.
const app = makeThing('app');

// Send-call shape: the anchor's expression text is the `res.json(...)` call,
// whole-typed `any` on bare (error-typed receiver). The declared response
// literal references a local named type plus primitive fields.
app.get('/items/:id', async (
  req: FakeRequest<{ id: string }>,
  res: FakeResponse<{ item: Item; message: string }>,
) => {
  const item = await makeThing(req);
  res.json({ item, message: 'found' });
});

// Argument-position shape: the payload is an identifier whose inferred type
// baked whole-`any` through the missing library.
app.get('/items', async (
  _req: FakeRequest,
  res: FakeResponse<{ items: Item[]; total: number }>,
) => {
  const items = await makeThing('items');
  res.json(items);
});

// Registration-call shape (span locator, no expression text): only ONE
// callback parameter annotation carries an inline literal argument.
app.get('/health', async (
  _req: FakeRequest,
  res: FakeResponse<{ service: string; ok: boolean }>,
) => {
  res.json({ service: 'bare-svc', ok: true });
});

// Ambiguous registration: BOTH parameter annotations carry inline literal
// arguments and there is no payload locator to disambiguate — recovery must
// refuse and the alias decays honestly.
app.get('/ambig/:id', async (
  req: FakeRequest<{ id: string }>,
  res: FakeResponse<{ n: number }>,
) => {
  res.json({ n: Number(req) });
});

// Literal leaning on a THIRD-PARTY type: not the tractable class — the
// recovery refuses (it would bake `any` at `thing`) and the alias keeps
// decaying honestly.
app.get('/external', async (
  _req: FakeRequest,
  res: FakeResponse<{ thing: FakeThing; ok: boolean }>,
) => {
  res.json({ thing: makeThing('t'), ok: true });
});
