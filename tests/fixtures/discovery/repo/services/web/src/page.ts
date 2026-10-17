import { useBooks } from "./use-books";

export async function renderPage(): Promise<void> {
  await useBooks();
}
