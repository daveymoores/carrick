import { BASE_PATH, Client } from "./config";
import { helper } from "./helper.mjs";

export const route = `${BASE_PATH}/${BUILD_ID}`;
export const size = helper(route) + new Client().list().length;
