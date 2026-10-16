export class ParamClient {
  constructor(private readonly base: string) {}

  items(): Promise<Response> {
    return fetch(`${this.base}/param/items`);
  }
}

export class MovingClient {
  private base = "http://localhost:4100";

  point(origin: string): void {
    this.base = origin;
  }

  items(): Promise<Response> {
    return fetch(`${this.base}/moving/items`);
  }
}

export const paramClient = new ParamClient(String(globalThis.location));
export const movingClient = new MovingClient();
