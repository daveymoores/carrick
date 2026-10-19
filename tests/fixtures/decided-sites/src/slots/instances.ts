// carrick#2524: the base an instance's maker is handed rides onto the key
// of every request made through the instance.
import http from "@fixture/http";

// A base set on an instance a constructor builds into a field.
export class Invoices {
  private client;

  constructor() {
    this.client = http.create({ baseURL: "https://billing.example.com/v1" });
  }

  list(): Promise<unknown> {
    return this.client.get("/invoices");
  }
}

// A base set on a module instance.
const ledger = http.create({ baseURL: "https://ledger.example.com/api" });

export function entries(): Promise<unknown> {
  return ledger.get("/entries");
}

// A maker handed a base the code does not state: the base is unstated.
const audit = http.create({ baseURL: process.env.AUDIT_URL });

export function events(): Promise<unknown> {
  return audit.get("/events");
}

// A maker handed no base: the path is the key, as before.
const plain = http.create({ timeout: 5000 });

export function status(): Promise<unknown> {
  return plain.get("/status");
}
