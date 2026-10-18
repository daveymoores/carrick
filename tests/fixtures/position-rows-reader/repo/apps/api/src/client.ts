export const BASE_PATH = "/v1/items";

export class ItemsClient {
  constructor(private readonly base: string) {}

  list(): Promise<Response> {
    return fetch(this.base);
  }
}

export const items = new ItemsClient(BASE_PATH);
