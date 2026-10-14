// carrick#2115: callers of argument-body-client.ts. One request_body item per call.
import {
  ItemsClient,
  Item,
  Rename,
  configure,
  sendJson,
  draftPayload,
  DraftService,
  DraftStore,
  notesApi,
  sendRename,
} from './argument-body-client';

declare const client: ItemsClient;
declare const axiosLike: { post<T = unknown, D = unknown>(url: string, data?: D): Promise<T> };

export async function run(owner: string, id: string, picked: string[], chosen: string[], next: Rename) {
  await client.removeItems(owner, picked);
  await client.setTags(id, chosen.slice(1));
  await client.rename(id, next);
  await client.removeOwner(owner);
  await client.removeVia(owner, picked);
  await client.archive(id, { reason: 'stale' });
}

export async function byId(item: Item, kept: Item, id: string, tagged: string[], owner: string, many: string[], labelled: string[]) {
  await client.update(item);
  await client.save(kept);
  await client.replaceTags(id, tagged);
  await client.removeMany(owner, many);
  await client.retitle(id, 'renamed');
  await client.label(id, labelled);
}

export async function arrows(note: Rename, renamed: Rename, project: string, rev: number | null, store: DraftStore, noteId: string) {
  await notesApi.create(note);
  await notesApi.relabel(noteId, 'urgent');
  await sendRename(renamed);
  await store.put(project, rev);
}

export async function each(owner: string, lists: string[][]) {
  await Promise.all(lists.map((ids) => client.removeItems(owner, ids)));
}

export async function settings(host: string, port: string) {
  await configure(host, parseInt(port, 10) || 9222);
}

export async function generic(item: Rename) {
  await sendJson('/items', item);
}

export async function drafts(project: string, revision: number | null, service: DraftService) {
  await service.createDraft(project, revision);
}

export async function controls(payload: Rename, url: string) {
  await axiosLike.post(url, payload);
  await fetch(url, { method: 'POST', body: JSON.stringify(payload) });
  await fetch(url, { method: 'POST', body: JSON.stringify(draftPayload(payload.title, [])) });
}

// A producer read: the handler's received body handed to the repository's own service.
type ParamDecorator2115 = (target: object, key: string | symbol, index: number) => void;
function Received(): ParamDecorator2115 {
  return () => undefined;
}
class NoteStore {
  private readonly notes: Rename[] = [];
  save(note: Rename): Rename {
    this.notes.push(note);
    return note;
  }
}
export class NotesHandler {
  constructor(private readonly store: NoteStore) {}
  create(@Received() note: Rename) {
    return this.store.save(note);
  }
}
