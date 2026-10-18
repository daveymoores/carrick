import { missing } from './does-not-exist';

export function run(): number {
  const value = missing.compute(1);
  return value.length;
}
