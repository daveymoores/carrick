// carrick#2115: a client class whose methods send a body built from their
// parameters. Synthetic: `transport` stands for any HTTP transport.

declare const transport: {
  put(url: string, opts: { body?: unknown }): Promise<unknown>;
  post(url: string, opts: { body?: unknown }): Promise<unknown>;
  delete(url: string, opts: { body?: unknown }): Promise<unknown>;
};
declare function putItemBody(opts: { path: { id: string }; body?: { tags: string[] } }): Promise<unknown>;

export interface Rename {
  title: string;
}

export interface Item {
  id: string;
  title: string;
}

export class ItemsClient {
  removeItems(owner: string, ids: string[]) {
    return transport.delete(`/owners/${owner}/items`, { body: { ids } });
  }
  setTags(id: string, tags: string[]) {
    return putItemBody({ path: { id }, body: { tags } });
  }
  rename(id: string, body: Rename) {
    return transport.put(`/items/${id}`, { body });
  }
  removeOwner(owner: string) {
    return transport.delete(`/owners/${owner}`, {});
  }
  removeVia(owner: string, ids: string[]) {
    return this.removeItems(owner, ids);
  }
  archive(id: string, note?: { reason?: string }) {
    return transport.post(`/items/${id}/archive`, { body: note ?? {} });
  }
  update(item: Item) {
    return transport.put(`/items/${item.id}`, { body: item });
  }
  save(item: Item) {
    if (!item) throw new Error('nothing to save');
    console.info('saving', item.id);
    return transport.put(`/items/${item.id}`, { body: item });
  }
  replaceTags(id: string, tags: string[]) {
    const opts = { body: tags };
    return transport.put(`/items/${id}/tags`, opts);
  }
  removeMany(owner: string, ids: string[]) {
    const body = { ids };
    return transport.delete(`/owners/${owner}/items`, { body });
  }
  label(id: string, tags: string[]) {
    return transport.put(`/items/${id}/labels`, { body: { labels: tags, count: tags.length } });
  }
  retitle = (id: string, title: string) => transport.put(`/items/${id}`, { body: { title } });
}

/** Methods written as arrow-valued properties of an object literal. */
export const notesApi = {
  create: (note: Rename) => transport.post('/notes', { body: note }),
  relabel: (id: string, label: string) => transport.post(`/notes/${id}`, { body: { label } }),
};

/** A sender a factory returns: no body the reading can reach at the call. */
export function makeSender() {
  return (payload: Rename) => transport.post('/renames', { body: payload });
}
export const sendRename = makeSender();

/** An abstract method: no body anywhere to read. */
export abstract class DraftStore {
  abstract put(project: string, expectedRevision: number | null): Promise<unknown>;
}

export function configure(host: string, port: number): Promise<{ endpoint: string }> {
  return transport.put('/settings/endpoint', { body: { host, port } }) as Promise<{ endpoint: string }>;
}

export function sendJson<T>(url: string, payload: T): Promise<unknown> {
  return transport.post(url, { body: JSON.stringify(payload) });
}

export function draftPayload(title: string, tags: string[]): { title: string; tags: string[] } {
  return { title, tags };
}

/** A service reached only through its interface: no body to read at the call. */
export interface DraftService {
  createDraft(project: string, expectedRevision: number | null): Promise<{ id: string }>;
}
