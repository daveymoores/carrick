import { connect } from "@fixture/wire";

const wire = connect("ws://x");

export function join(): void {
  const roomId = Math.random().toString();
  wire.emit("room:" + roomId, { at: 1 });
}

const init = { method: "DELETE" };

export function remove(): Promise<Response> {
  return fetch("/items/1", init);
}

export function listen(): void {
  wire.on("user.joined", (data) => {
    console.log(data);
  });
}

export function settle(shipped: boolean): void {
  const topic = shipped ? "order.shipped" : "order.cancelled";
  wire.emit(topic, { shipped });
}

export function create(): Promise<Response> {
  return fetch("/items", { method: "POST" });
}

function onLeft(data: unknown): void {
  console.log(data);
}

export function listenLeft(): void {
  wire.on("user.left", onLeft);
}

export function forward(options: RequestInit): Promise<Response> {
  return fetch("/items/3", options);
}

export function ping(worker: { send(message: unknown): void }): void {
  worker.send("ping");
}
