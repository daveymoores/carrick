import type { Port } from './port';

function traced(target: unknown, context: ClassMethodDecoratorContext): void {
  void [target, context];
}

export class QueuePort implements Port {
  @traced
  async send(event: string, body?: unknown): Promise<void> {
    void [event, body];
  }
}
