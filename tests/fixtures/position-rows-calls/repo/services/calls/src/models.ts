export class Users {
  list(): Promise<Response> {
    return fetch("/users");
  }
}

export class Orders {
  list(): Promise<Response> {
    return fetch("/orders");
  }
}

export class Base {
  load(): Promise<Response> {
    return fetch("/base");
  }
}

export class Child extends Base {
  load(): Promise<Response> {
    return fetch("/child");
  }
}
