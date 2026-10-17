export type Verb = 'get' | 'post';

export type Method = (path: string) => Promise<unknown>;

/** The members come from a mapped type, so none has a declaration. */
export type Client = { [K in Verb]: Method };

async function request(verb: string, path: string): Promise<unknown> {
  const response = await fetch(`/api${path}`, { method: verb.toUpperCase() });
  return response.json();
}

function makeMethod(verb: string): Method {
  return (path) => request(verb, path);
}

export function createClient(): Client {
  return {
    get: makeMethod('get'),
    post: makeMethod('post'),
  };
}
