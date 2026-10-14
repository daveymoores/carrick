export class Client {
  constructor(private readonly origin: string) {}

  getRun(id: string) {
    return `${this.origin}/runs/${id}`;
  }

  listRuns() {
    return [`${this.origin}/runs`];
  }
}

/**
 * A second class that declares the same member names. The package's surface
 * alone cannot say which class a receiver is, so only the factory's own
 * declaration can.
 */
export class BatchClient {
  getRun(id: string) {
    return id;
  }

  listRuns() {
    return [] as string[];
  }
}

export interface RunReader {
  getRun(id: string): string;
}

/** The return annotation names the class. */
export function createClient(): Client {
  return new Client("local");
}

/** The annotation names a promise of the class. */
export async function openClient(): Promise<Client> {
  return new Client("remote");
}

/** The annotation names an interface, which declares no method body. */
export function createReader(): RunReader {
  return new Client("reader");
}

/** The annotation names two classes. */
export function createEither(batch: boolean): Client | BatchClient {
  return batch ? new BatchClient() : new Client("either");
}

/** No annotation and two `return` statements. */
export function createByMode(batch: boolean) {
  if (batch) {
    return new BatchClient();
  }
  return new Client("mode");
}
