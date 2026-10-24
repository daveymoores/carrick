// carrick#2674: a wrapper hands its caller's options on through the rest of a
// destructured parameter. One caller writes a body; the other writes none.

function send(config: { path: string; method?: string; body?: string }) {
  const { path, ...rest } = config;
  return fetch(path, { ...rest });
}

export interface Note {
  text: string;
}

export function addNote(note: Note): Promise<Response> {
  return send({ path: "/notes", method: "POST", body: JSON.stringify(note) });
}

export function ping(): Promise<Response> {
  return send({ path: "/ping", method: "POST" });
}
