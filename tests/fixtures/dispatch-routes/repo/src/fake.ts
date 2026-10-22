import { listen } from "./listen";

function parse(url: string | undefined): string {
  return (url ?? "/").split("?")[0];
}

export function createFake(): void {
  listen((req, res) => {
    const u = parse(req.url);
    if (u === "/v1/items") {
      res.end("items");
    }
  });
}
