export abstract class Store {
  abstract save(key: string, value: string): void;
}

export class MemoryStore extends Store {
  private readonly entries = new Map<string, string>();

  save(key: string, value: string): void {
    this.entries.set(key, value);
  }
}

export class LogStore extends Store {
  save(key: string, value: string): void {
    console.log(key, value);
  }
}
