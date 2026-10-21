const API_BASE = process.env.NEXT_PUBLIC_API_URL;

async function getCsrfToken(): Promise<string> {
  const res = await fetch(`${API_BASE}/auth/csrf-token`, { credentials: "include" });
  const data = await res.json();
  return data.token;
}

export async function apiGet<T>(path: string): Promise<T> {
  const res = await fetch(`${API_BASE}${path}`, { credentials: "include" });
  return res.json();
}

export async function apiPost<T>(path: string, body: unknown): Promise<T> {
  const token = await getCsrfToken();
  const res = await fetch(`${API_BASE}${path}`, {
    method: "POST",
    headers: { "Content-Type": "application/json", "X-CSRF-Token": token },
    body: JSON.stringify(body),
  });
  return res.json();
}

export async function apiPut<T>(path: string, body: unknown): Promise<T> {
  const token = await getCsrfToken();
  const res = await fetch(`${API_BASE}${path}`, {
    method: "PUT",
    headers: { "Content-Type": "application/json", "X-CSRF-Token": token },
    body: JSON.stringify(body),
  });
  return res.json();
}

export async function apiPatch<T>(path: string, body: unknown): Promise<T> {
  const res = await fetch(`${API_BASE}${path}`, {
    method: "PATCH",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify(body),
  });
  return res.json();
}

export async function apiDelete(path: string): Promise<void> {
  const token = await getCsrfToken();
  await fetch(`${API_BASE}${path}`, {
    method: "DELETE",
    headers: { "X-CSRF-Token": token },
  });
}
