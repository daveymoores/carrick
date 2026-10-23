// 3. Constructed in two modules with different literals.
export class Region {
  constructor(private base: string) {}

  ping(): Promise<Response> {
    return fetch(`${this.base}/ping`);
  }
}
