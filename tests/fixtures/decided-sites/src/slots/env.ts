const API = process.env.API;

export function listItems(): Promise<Response> {
  return fetch(`${process.env.API}/items`);
}

export function listThings(): Promise<Response> {
  return fetch(`${API}/things`);
}

export function listBySuffix(): Promise<Response> {
  const suffix = Math.random().toString();
  return fetch(`${process.env.API}/items/${suffix}`);
}

export function listByPrefix(path: string): Promise<Response> {
  return fetch("/api" + path);
}
