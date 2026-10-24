// carrick#2674: a wrapper that writes a key onto the options it hands on, by
// assignment or `Object.assign`, holds what the caller wrote there no longer:
// the request's body and method are not read at the caller.

function traced<T>(run: () => T): T {
  return run();
}

// The rest is written through before it is spread.
function sendMut(config: { path: string; data?: object; method?: string; body?: string }) {
  const { path, data, ...mutRest } = config;
  if (data) mutRest.body = JSON.stringify(data);
  return fetch(`${process.env.API}${path}/mut`, { ...mutRest, credentials: "include" });
}

export function mutCaller(note: object): Promise<Response> {
  return sendMut({ path: "/mut", method: "POST", data: note });
}

// The rest is the target of Object.assign before it is spread.
function sendAssign(config: { path: string; data?: object; method?: string }) {
  const { path, data, ...assignRest } = config;
  Object.assign(assignRest, { body: JSON.stringify(data) });
  return fetch(`${process.env.API}${path}/assign`, { ...assignRest, credentials: "include" });
}

export function assignCaller(note: object): Promise<Response> {
  return sendAssign({ path: "/assign", method: "POST", data: note });
}

// The rest is written through inside the closure that sends the request.
function sendClosure(config: { path: string; data?: object; method?: string }) {
  const { path, data, ...closureRest } = config;
  return traced(() => {
    closureRest.body = JSON.stringify(data);
    return fetch(`${process.env.API}${path}/closure`, { ...closureRest, credentials: "include" });
  });
}

export function closureCaller(note: object): Promise<Response> {
  return sendClosure({ path: "/closure", method: "POST", data: note });
}

// The function's own parameter is written through before it is spread.
function postMut(path: string, init: { data?: object; body?: string }) {
  init.body = JSON.stringify(init.data);
  return fetch(`${process.env.API}${path}/pm`, { method: "POST", ...init });
}

export function postMutCaller(note: object): Promise<Response> {
  return postMut("/pm", { data: note });
}

// The function's own parameter is the target of Object.assign.
function postAssign(path: string, assignInit: { data?: object; body?: string }) {
  Object.assign(assignInit, { body: JSON.stringify(assignInit.data) });
  return fetch(`${process.env.API}${path}/pa`, { method: "POST", ...assignInit });
}

export function postAssignCaller(note: object): Promise<Response> {
  return postAssign("/pa", { data: note });
}

// A hop spreads the rest into a new object with the body beside it.
function sendInner(config: { path: string; method?: string; body?: string }) {
  const { path, ...innerRest } = config;
  return fetch(`${process.env.API}${path}/inner`, { ...innerRest, credentials: "include" });
}

function sendOuter(config: { path: string; data?: object; method?: string }) {
  const { data, ...outerRest } = config;
  return sendInner({ ...outerRest, body: JSON.stringify(data) });
}

export function outerCaller(note: object): Promise<Response> {
  return sendOuter({ path: "/outer", method: "POST", data: note });
}

// The method is written onto the rest, or onto the parameter.
function sendMethodMut(config: { path: string; method?: string }) {
  const { path, ...methodRest } = config;
  methodRest.method = "DELETE";
  return fetch(`${process.env.API}${path}/mm`, { ...methodRest, credentials: "include" });
}

export function methodMutCaller(): Promise<Response> {
  return sendMethodMut({ path: "/mm" });
}

function sendMethodAssign(path: string, methodInit: { method?: string }) {
  Object.assign(methodInit, { method: "PUT" });
  return fetch(`${process.env.API}${path}/ma`, { credentials: "include", ...methodInit });
}

export function methodAssignCaller(): Promise<Response> {
  return sendMethodAssign("/ma", {});
}

// Control: Object.assign onto a new object reads the rest, never writes it.
function sendCopy(config: { path: string; method?: string; body?: string }) {
  const { path, ...copyRest } = config;
  const copy = Object.assign({}, copyRest);
  void copy;
  return fetch(`${process.env.API}${path}/copy`, { ...copyRest, credentials: "include" });
}

export function copyCaller(): Promise<Response> {
  return sendCopy({ path: "/copy", method: "POST" });
}
