import type { GraphResponse, SnapshotCreateResponse } from "@/types/graph";

const API_BASE = process.env.NEXT_PUBLIC_API_URL || "";

export async function fetchGraph(org: string): Promise<GraphResponse> {
  const res = await fetch(`${API_BASE}/graph/${encodeURIComponent(org)}`);
  if (!res.ok) throw new Error(`Failed to fetch graph: ${res.status}`);
  return res.json();
}

export async function fetchSnapshot(
  org: string,
  snapshotId: string,
): Promise<GraphResponse> {
  const res = await fetch(
    `${API_BASE}/graph/${encodeURIComponent(org)}/snapshot/${snapshotId}`,
  );
  if (!res.ok) throw new Error(`Failed to fetch snapshot: ${res.status}`);
  return res.json();
}

export async function createSnapshot(
  org: string,
): Promise<SnapshotCreateResponse> {
  const res = await fetch(
    `${API_BASE}/graph/${encodeURIComponent(org)}/snapshot`,
    { method: "POST" },
  );
  if (!res.ok) throw new Error(`Failed to create snapshot: ${res.status}`);
  return res.json();
}
