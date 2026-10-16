// Café menu 🍰: the text before the first call holds an accented letter and
// a character outside the basic plane, so byte and UTF-16 offsets differ.
import { Counter, LIMIT, greet } from '@fixture/shared';
import { wrap, type Envelope } from '@fixture/kit';
import { clock } from '@fixture/outside';

interface Order {
  id: string;
  items?: { name: string }[];
  note?: { text: string } | null;
}

const label = 'naïve 🍰';

function sql(strings: TemplateStringsArray, ...values: unknown[]): string {
  return strings.join('?') + values.length;
}

interface Countable {
  increment(by: number): number;
}

class LimitedCounter extends Counter implements Countable {
  override increment(by: number): number {
    const next = super.increment(by);
    return next > LIMIT ? LIMIT : next;
  }
}

export function handle(order: Order, raw: unknown, lookup: Record<string, Order>): Envelope {
  const first = order.items?.[0]?.name;
  const note = order.note?.text.toUpperCase();
  const counter = new LimitedCounter();
  counter.increment(1);
  const byKey = lookup['a'];
  const query = sql`select ${order.id}`;
  const say = (greet as (name: string) => string)(label);
  const maybe: ((n: number) => number) | undefined = Math.abs;
  const size = maybe!(-2);
  const started = clock.now();
  const checks = [
    order.id === 'a',
    order.id !== 'b',
    size == 2,
    size != 3,
    size < 4,
    size > 5,
    size <= 6,
    size >= 7,
    counter instanceof Counter,
    'id' in order,
  ];
  const id = order.id;
  const summary = { id, first, note, byKey, query, say, started, checks, raw };
  return wrap(JSON.stringify(summary));
}
