class Api {
  constructor(private base: string) {}
  users() {
    return fetch(`${this.base}/users`);
  }
}
export const api = new Api("/api/v1");

class Items {
  private base: string;
  constructor(base: string) {
    this.base = base;
  }
  list() {
    return fetch(this.base + "/items");
  }
}
export const items = new Items(process.env.ITEMS_URL!);

class Pinger {
  constructor(private root: string) {}
  ping() {
    return fetch(`${this.root}/ping`);
  }
}
export const east = new Pinger("/east");
export const west = new Pinger("/west");
