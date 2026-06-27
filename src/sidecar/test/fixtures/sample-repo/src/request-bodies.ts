/**
 * Request body fixture
 */

interface RequestBody {
  name: string;
  email: string;
}

interface Request {
  body: RequestBody;
}

export const createUser = async (req: Request) => {
  const payload = req.body;
  return payload;
};

export async function sendUser(user: RequestBody) {
  return fetch('/api/users', { method: 'POST', body: user });
}

interface RegisterRequest {
  username: string;
  password: string;
}

// A1: `await request.json()` typed via an `as T` cast. The raw type of the
// call node is `Promise<any>`; the declared cast must win.
export async function registerViaCast(request: globalThis.Request) {
  const body = (await request.json()) as RegisterRequest;
  return body;
}

// A1: `await request.json()` typed via a variable annotation. Same recovery,
// different syntax (typed binding instead of an `as` cast).
export async function registerViaBinding(request: globalThis.Request) {
  const body: RegisterRequest = await request.json();
  return body;
}

// Control: an untyped `request.formData()` has no annotation, so it must stay
// faithfully `FormData` / `any` and not be "recovered" into a declared type.
export async function uploadUntyped(request: globalThis.Request) {
  const form = await request.formData();
  return form;
}

// B: consumer-side `fetch` POST whose `body` is `JSON.stringify(payload)`. The
// call's own type is the useless `string`; the inferred request body must be
// the SERIALIZED ARGUMENT's type (`RegisterRequest`), not `string`.
export async function postViaStringify(payload: RegisterRequest) {
  return fetch('/api/register', {
    method: 'POST',
    body: JSON.stringify(payload),
  });
}
