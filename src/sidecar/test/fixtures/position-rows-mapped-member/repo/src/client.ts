export type Verb = "get" | "post";
export type Fetcher = (url: string) => Promise<string>;
export type MappedClient = { [K in Verb]: Fetcher };
export type PlainClient = { get: Fetcher; post: Fetcher };

const send: Fetcher = async (url) => url;

export function makeMapped(): MappedClient {
  return { get: send, post: send };
}

export function makePlain(): PlainClient {
  return { get: send, post: send };
}
