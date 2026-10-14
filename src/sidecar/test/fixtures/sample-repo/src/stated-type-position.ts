// carrick#2113: a type is stated for a node only where the node reaches the
// annotation or cast through wrappers that leave its value as it is. An
// annotation further out types something else.
export interface ItemDto {
  id: string;
  title: string;
  createdAt: string;
}
export interface ItemUpsert {
  title: string;
  tags?: string[];
}
export interface CommandSpec {
  name: string;
  run: () => Promise<void>;
}
declare const client: {
  putItem(id: string, body: ItemUpsert): Promise<ItemDto>;
  list(): Promise<ItemDto[]>;
};
declare function print(x: unknown): void;

export async function rename(id: string, title: string) {
  const result: ItemDto = await client.putItem(id, { title, tags: [] });
  print(result.id);
}

export async function renameTyped(id: string, title: string) {
  const body: ItemUpsert = { title };
  await client.putItem(id, body);
  await client.putItem(id, { title } as ItemUpsert);
}

export const COMMANDS: readonly CommandSpec[] = [
  {
    name: 'list',
    run: async () => {
      print(await client.list());
    },
  },
];

export async function read() {
  const items: ItemDto[] = await client.list();
  const one = (await client.list()) as ItemDto[];
  const pair: ItemDto[] = [await client.putItem('a', { title: 'a' })];
  return [items, one, pair];
}

export const api: { load: (id: string) => Promise<ItemDto> } = {
  load: (id) => client.putItem(id, { title: id }),
};

export async function tolerant(res: Response) {
  return (await res.json().catch(() => ({}))) as Partial<ItemDto>;
}

export async function tolerantRead(id: string) {
  const res = await fetch(`/items/${id}`);
  const body = (await res.json().catch(() => ({}))) as Partial<ItemDto>;
  return body.title;
}
