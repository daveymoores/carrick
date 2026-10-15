import { register, registerNamed, registerAsync } from "./registry";
import type { Handler } from "./registry";
import { swap } from "./reassigned";
import { own } from "./own-map";
import { foreign } from "./foreign-map";
import { mount } from "rpc-mount";

const listOrders: Handler = () => [];
const readOrder: Handler = (input) => input;
const saveOrder: Handler = (input) => input;

function withAudit(handler: Handler): Handler {
  return handler;
}

register(listOrders, "orders.list");
registerNamed("orders.read", readOrder);
register(
  saveOrder,
  "orders.save",
);
register(withAudit(listOrders), "orders.audit");
registerAsync(listOrders, "orders.later").then(() => undefined);
swap("orders.swap", listOrders);
own("orders.own", listOrders);
foreign("orders.foreign", listOrders);
mount(listOrders, "orders.mount");
