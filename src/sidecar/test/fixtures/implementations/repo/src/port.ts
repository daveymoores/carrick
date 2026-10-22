export interface Port {
  send(event: string): Promise<void>;
  send(event: string, body: unknown): Promise<void>;
}
