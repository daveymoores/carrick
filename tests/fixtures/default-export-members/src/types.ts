// The shape every surface below is checked against. A type-only module: it
// declares no function of its own.
export interface Handler {
  fetch(request: Request): Promise<Response>;
}

export interface Jobs {
  run(name: string): string;
}
