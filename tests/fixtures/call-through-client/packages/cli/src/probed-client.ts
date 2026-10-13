import { checkHealth, getRun, transport } from "@fixture/client";

/**
 * A client whose shared private transport makes a request of its own before
 * every call. Each method therefore reaches two operations, and a call
 * through one of them names no single request.
 */
export class ProbedClient {
  getRun(id: string) {
    return this.invoke(() => getRun({ client: transport, path: { id } }));
  }

  private async invoke<T>(call: () => Promise<{ data: unknown }>): Promise<T> {
    await checkHealth({ client: transport });
    return (await call()).data as T;
  }
}

export function createProbedClient(): ProbedClient {
  return new ProbedClient();
}
