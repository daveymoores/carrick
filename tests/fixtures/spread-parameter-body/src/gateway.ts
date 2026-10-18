const GATEWAY_URL = `${process.env.GATEWAY_API_URL}/v1/gateway`;

export class Gateway {
  async run(op: string, input: object): Promise<unknown> {
    const response = await fetch(GATEWAY_URL, {
      method: "POST",
      body: JSON.stringify({ op, ...input }),
    });
    return response.json();
  }

  // The caller's object reaches `run`'s spread through this parameter.
  async purgeAll(scope: object): Promise<unknown> {
    return this.run("purge-all", scope);
  }

  async refreshBoth(): Promise<void> {
    await this.run("refresh-a", {});
    await this.run("refresh-b", {});
  }

  // Sends whatever object it is handed.
  async rpc(message: object): Promise<unknown> {
    const response = await fetch(GATEWAY_URL, {
      method: "POST",
      body: JSON.stringify(message),
    });
    return response.json();
  }

  async search(query: object): Promise<unknown> {
    return this.rpc({ op: "search", ...query });
  }

  // The key written after the spread is the one that goes out.
  async lock(opts: object): Promise<unknown> {
    const response = await fetch(GATEWAY_URL, {
      method: "POST",
      body: JSON.stringify({ ...opts, op: "lock" }),
    });
    return response.json();
  }
}
