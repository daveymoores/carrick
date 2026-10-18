export function hop1(): Promise<Response> {
  return hop2();
}

export function hop2(): Promise<Response> {
  return hop3();
}

export function hop3(): Promise<Response> {
  return hop4();
}

export function hop4(): Promise<Response> {
  return hop5();
}

export function hop5(): Promise<Response> {
  return hop6();
}

export function hop6(): Promise<Response> {
  return hop7();
}

export function hop7(): Promise<Response> {
  return fetch("/deep");
}

export function start(): Promise<Response> {
  return hop1();
}
