function logged(_target: unknown, _key: string, descriptor: PropertyDescriptor): PropertyDescriptor {
  return descriptor;
}

export class Reports {
  @logged
  fetchAll(): Promise<Response> {
    return fetch("/reports");
  }
}
