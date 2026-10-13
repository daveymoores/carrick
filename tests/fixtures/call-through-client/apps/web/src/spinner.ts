/** Runs the work it is handed while a spinner shows. Makes no request. */
export async function withSpinner<T>(work: () => Promise<T>): Promise<T> {
  document.body.classList.add("busy");
  try {
    return await work();
  } finally {
    document.body.classList.remove("busy");
  }
}

/** Runs the work again when it fails. Makes no request. */
export async function retry<T>(
  work: () => Promise<T>,
  options: { onRetry: () => unknown },
): Promise<T> {
  try {
    return await work();
  } catch {
    options.onRetry();
    return work();
  }
}
