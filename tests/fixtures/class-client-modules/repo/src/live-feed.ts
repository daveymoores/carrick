import { Feed } from "./feed";

export class LiveFeed extends Feed {
  constructor() {
    super("/live");
  }
}
