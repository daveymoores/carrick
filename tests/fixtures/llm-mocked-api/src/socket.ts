// A file whose only network call sites are raw WebSocket/EventSource
// constructors. There is no analyze-file prompt registered for the socket
// protocol yet, so the orchestrator must skip this file entirely rather
// than sending it to the HTTP prompt.
export function connect(): WebSocket {
  const socket = new WebSocket("wss://events.internal/stream");
  socket.onmessage = (event) => {
    console.log("message", event.data);
  };
  return socket;
}

export function subscribe(): EventSource {
  return new EventSource("https://events.internal/sse");
}
