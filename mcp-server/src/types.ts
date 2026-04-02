/**
 * TypeScript mirrors of Carrick's Rust data structures.
 * These match the JSON serialization of CloudRepoData and related types.
 */

export interface CloudRepoData {
  repo_name: string;
  service_name?: string;
  endpoints: ApiEndpointDetails[];
  calls: ApiEndpointDetails[];
  mounts: Mount[];
  apps: Record<string, unknown>;
  imported_handlers: [string, string, string, string][];
  function_definitions: Record<string, unknown>;
  config_json?: string;
  package_json?: string;
  packages?: Packages;
  last_updated: string;
  commit_hash: string;
  mount_graph?: MountGraph;
  bundled_types?: string;
  type_manifest?: TypeManifestEntry[];
}

export interface ApiEndpointDetails {
  owner?: OwnerType;
  route: string;
  method: string;
  params: string[];
  request_body?: unknown;
  response_body?: unknown;
  handler_name?: string;
  request_type?: unknown;
  response_type?: unknown;
  file_path: string;
}

export type OwnerType =
  | { App: string }
  | { Router: string }
  | { Middleware: string };

export interface Mount {
  parent: OwnerType;
  child: OwnerType;
  prefix: string;
}

export interface MountGraph {
  nodes: Record<string, GraphNode>;
  mounts: MountEdge[];
  endpoints: ResolvedEndpoint[];
  data_calls: DataFetchingCall[];
}

export interface GraphNode {
  name: string;
  node_type: "Root" | "Mountable" | "Unknown";
  creation_site?: string;
  file_location: string;
}

export interface MountEdge {
  parent: string;
  child: string;
  path_prefix: string;
  middleware_stack: string[];
}

export interface ResolvedEndpoint {
  method: string;
  path: string;
  full_path: string;
  handler?: string;
  owner: string;
  file_location: string;
  middleware_chain: string[];
  repo_name?: string;
}

export interface DataFetchingCall {
  method: string;
  target_url: string;
  client: string;
  file_location: string;
}

export type ManifestRole = "producer" | "consumer";
export type ManifestTypeKind = "request" | "response";
export type ManifestTypeState = "explicit" | "implicit" | "unknown";

export interface TypeManifestEntry {
  method: string;
  path: string;
  role: ManifestRole;
  type_kind: ManifestTypeKind;
  type_alias: string;
  file_path: string;
  line_number: number;
  is_explicit: boolean;
  type_state: ManifestTypeState;
  evidence: TypeEvidence;
  /** Original declaration text as written (preserves named types) */
  resolved_definition?: string;
  /** Compiler-expanded form with all types fully inlined */
  expanded_definition?: string;
}

export interface TypeEvidence {
  file_path: string;
  span_start?: number;
  span_end?: number;
  line_number: number;
  infer_kind: string;
  is_explicit: boolean;
  type_state: ManifestTypeState;
}

export interface Packages {
  package_jsons: PackageJson[];
  source_paths: string[];
  merged_dependencies: Record<string, PackageInfo>;
}

export interface PackageJson {
  name?: string;
  version?: string;
  dependencies: Record<string, string>;
  devDependencies: Record<string, string>;
  peerDependencies: Record<string, string>;
}

export interface PackageInfo {
  name: string;
  version: string;
  source_path: string;
}

/** Shape returned by the Lambda get-cross-repo-data action */
export interface CrossRepoResponse {
  repos: AdjacentRepo[];
  processing_errors?: Array<{
    repo: string;
    error: string;
    pk: string;
  }>;
}

export interface AdjacentRepo {
  repo: string;
  hash: string;
  s3Url: string;
  filename: string;
  metadata?: CloudRepoData;
  lastUpdated?: string;
}
