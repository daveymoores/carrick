export function formatCents(cents: number): string {
  const whole = Math.floor(cents / 100);
  const rest = String(cents % 100).padStart(2, "0");
  return `${whole}.${rest}`;
}
