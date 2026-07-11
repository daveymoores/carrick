import type { ApiResponse, OpaqueHandle } from 'wrapper-lib';

// Recursive unwrap whose extracted payload is ITSELF verified machinery with
// no recoverable payload: the inner pass collapses to `unknown`, and no
// wrapper symbol may leak into the anchor. Lives in its own fixture file —
// sidecar.test.ts hardcodes byte offsets into wrapper-usage.ts.
declare function apiGetOpaque(url: string): Promise<ApiResponse<OpaqueHandle>>;

export async function loadOpaqueHandle() {
  const handle = await apiGetOpaque('/api/opaque');
  return handle;
}
