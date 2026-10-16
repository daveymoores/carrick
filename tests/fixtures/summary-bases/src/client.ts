class ServiceClient {
  private readonly base = `${process.env.SVC_URL ?? "http://localhost:4000"}/v1`;

  list(): Promise<Response> {
    return fetch(`${this.base}/items`);
  }
}

export const client = new ServiceClient();
