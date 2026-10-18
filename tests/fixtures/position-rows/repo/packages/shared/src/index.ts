export function greet(name: string): string {
  return `hello ${name}`;
}

export class Counter {
  protected count = 0;

  increment(by: number): number {
    this.count += by;
    return this.count;
  }
}

export const LIMIT = 3;
