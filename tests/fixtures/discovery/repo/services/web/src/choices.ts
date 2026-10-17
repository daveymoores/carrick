import { USERS_PATH } from "@fixture/shared";
import { EVENT_NAME } from "undeclared-events";
import { api } from "./api";
import { emit } from "./emit";

const PATH_V1 = "/v1/report";
const PATH_V2 = "/v2/report";

export function listUsers(): Promise<Response> {
  return api(USERS_PATH);
}

export function loadReport(useV2: boolean): Promise<Response> {
  return api(useV2 ? PATH_V2 : PATH_V1);
}

export function fire(): void {
  emit(EVENT_NAME, {});
}
