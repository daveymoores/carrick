import { listBooks } from "./books-api";

export function useBooks(): Promise<Response> {
  return listBooks();
}
