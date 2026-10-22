interface IdAsNumber {
  id: number;
  meta: { x: number };
}

interface MetaX {
  id: string;
  meta: { x: number };
}

export async function idAsNumber(): Promise<IdAsNumber> {
  const response = await fetch("/checkout", { method: "POST" });
  return response.json() as Promise<IdAsNumber>;
}

export async function metaX(): Promise<MetaX> {
  const response = await fetch("/checkout", { method: "POST" });
  return response.json() as Promise<MetaX>;
}
