export async function loadWith({ send = fetch }: { send?: typeof fetch }): Promise<unknown> {
  return send("/defaulted");
}

export interface Deps {
  fetch: typeof fetch;
}

export function loadFrom(deps: Deps, url: string): Promise<Response> {
  return deps.fetch(url);
}

export function makeLoader(): (path: string) => Promise<Response> {
  return (path) => Promise.resolve(new Response(path));
}

const load = makeLoader();

export function useLoader(): Promise<Response> {
  return load("/made");
}

export class Poller {
  private readonly pull = makeLoader();

  run(): Promise<Response> {
    return this.pull("/polled");
  }
}
