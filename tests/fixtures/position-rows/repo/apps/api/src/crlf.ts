import { greet } from '@fixture/shared';

export function welcome(names: string[]): string[] {
  return names.map((name) => greet(name));
}
