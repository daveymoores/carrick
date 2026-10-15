import { feed } from "./feed";

export function follow(): void {
  feed.events.on("item", (item: { id: string }) => {
    console.log(item.id);
  });
}
