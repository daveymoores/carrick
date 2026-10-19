// carrick#2524: one request, one row. Where the request summaries state a
// request's row, a site is not added anywhere else.

export class Streams {
  private base: string;

  constructor(base: string) {
    this.base = base;
  }

  read(stream: string): Promise<Response> {
    return fetch(`${this.base}/streams/${stream}/records`);
  }
}

export function readOrders(streams: Streams, id: string): Promise<Response> {
  return streams.read(`orders-${id}`);
}

function notices(query: string): Promise<Response> {
  return fetch(`/api/notices?${query}`);
}

export function unreadNotices(): Promise<Response> {
  return notices("unread=1");
}

function account(id: string): Promise<Response> {
  return fetch(`/accounts/${id}`);
}

export function accountOf(user: { id: string }): Promise<Response> {
  return account(user.id);
}
