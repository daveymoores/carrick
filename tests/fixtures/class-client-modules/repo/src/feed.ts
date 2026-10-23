// 4. Constructed once, and extended: on a subclass's instance the field may
// hold anything the subclass passes.
export class Feed {
  constructor(private base: string) {}

  recent(): Promise<Response> {
    return fetch(`${this.base}/recent`);
  }
}

export const feed = new Feed("/feed");
