import {
  archiveRun,
  getRun,
  getStatus,
  getToken,
  listRuns,
  recordAudit,
  saveRun,
  transport,
  type Transport,
} from "@fixture/client";

/** One method per request; each hands the request to `send`. */
export class Client {
  constructor(private readonly transport: Transport) {}

  getRun(id: string) {
    return this.send(() => getRun({ client: this.transport, path: { id } }));
  }

  listRuns() {
    return this.send(() => listRuns({ client: this.transport }));
  }

  saveRun(id: string, state: string) {
    return this.send(() =>
      saveRun({ client: this.transport, path: { id }, body: { state } }),
    );
  }

  getStatus() {
    return this.send(() => getStatus({ client: this.transport }));
  }

  getToken() {
    return this.send(() => getToken({ client: this.transport }));
  }

  recordAudit(entry: unknown) {
    return this.send(() => recordAudit({ client: this.transport, body: entry }));
  }

  archiveRun(id: string) {
    return this.send(() => archiveRun({ client: this.transport, path: { id } }));
  }

  private async send<T>(call: () => Promise<{ data: unknown }>): Promise<T> {
    return (await call()).data as T;
  }
}

export function createClient(): Client {
  return new Client(transport);
}
