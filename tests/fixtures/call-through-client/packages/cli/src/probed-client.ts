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

/**
 * The same client, whose probe is a `fetch` only the model reads, made once
 * per client from the origin it was built with. The probe's value is
 * dropped, so the request each method hands back is the one its call
 * reaches.
 */
export class ModelProbedClient {
  private probed?: Promise<void>;

  constructor(private readonly origin: string) {}

  getRun(id: string) {
    return this.perform(() => getRun({ client: transport, path: { id } }));
  }

  private async perform<T>(call: () => Promise<{ data: unknown }>): Promise<T> {
    await this.probe();
    return (await call()).data as T;
  }

  private probe(): Promise<void> {
    this.probed ??= (async () => {
      await fetch(`${new URL(this.origin).origin}/health`);
    })();
    return this.probed;
  }
}

export function createModelProbedClient(origin: string): ModelProbedClient {
  return new ModelProbedClient(origin);
}
