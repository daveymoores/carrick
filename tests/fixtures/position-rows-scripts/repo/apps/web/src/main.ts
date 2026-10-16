import { Client } from "./config";

export const names = new Client().list().join(",");
