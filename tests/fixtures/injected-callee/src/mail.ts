// 3. A local set to a parameter's field or the platform's `fetch`.
export interface MailDeps {
  send?: typeof fetch;
}

export async function deliver(to: string, deps: MailDeps = {}): Promise<Response> {
  const doFetch = deps.send || fetch;
  return doFetch("/api/mail", { method: "POST", body: to });
}

export function notifyOps(): Promise<Response> {
  return deliver("ops");
}
