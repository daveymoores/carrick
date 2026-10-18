const BASE = `${process.env.BOOKS_API_URL}/v2`;
type Query = Record<string, string | undefined>;
export interface CallOptions { method?: 'GET' | 'POST' | 'PUT' | 'DELETE'; body?: unknown; query?: Query; signal?: AbortSignal }

function withQuery(path: string, query?: Query): string {
  const params = new URLSearchParams();
  for (const [key, value] of Object.entries(query ?? {})) { if (value) params.set(key, value); }
  const qs = params.toString();
  return `${BASE}${path}${qs ? `?${qs}` : ''}`;
}

export async function call<T>(path: string, opts: CallOptions = {}): Promise<T> {
  const { method = 'GET', body, query, signal } = opts;
  const res = await fetch(withQuery(path, query), { method, body: body === undefined ? undefined : JSON.stringify(body), signal });
  if (!res.ok) throw new Error(`request failed: ${res.status}`);
  return (await res.json()) as T;
}

function track(page: string) { return fetch('/events', { method: 'POST', body: JSON.stringify({ page }) }); }
function label(path: string) { return path.toUpperCase(); }

type Sig = { signal?: AbortSignal };
export const books = {
  async create(input: unknown) { return await call<unknown>('/books', { method: 'POST', body: input }); },
  list: (o: Sig = {}) => call<unknown[]>('/books', o),
  get: (id: string, o: Sig = {}) => call<unknown>(`/books/${encodeURIComponent(id)}`, o),
  search: (q: string, o: Sig = {}) => call<unknown[]>('/books/search', { ...o, query: { q } }),
  remove: (id: string) => call<void>(`/books/${encodeURIComponent(id)}`, { method: 'DELETE' }),
  view: () => track('/books'),
  title: () => label('/books'),
};
