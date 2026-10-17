export function traced<T>(name: string, run: () => Promise<T>): Promise<T> {
  console.debug(name);
  return run();
}
