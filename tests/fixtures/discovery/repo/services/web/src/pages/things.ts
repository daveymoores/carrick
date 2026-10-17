import { v1 } from "../api";

export function loadThings(): Promise<Response> {
  return v1("/things");
}
