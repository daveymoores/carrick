import { EventEmitter } from "node:events";

export class Feed {
  events = new EventEmitter();

  publish(item: { id: string }): void {
    this.events.emit("item", item);
  }
}

export const feed = new Feed();
