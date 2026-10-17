import { Orders, Users } from "./models";

export const registry: { users: Users; orders: Orders } = {
  users: new Users(),
  orders: new Orders(),
};
