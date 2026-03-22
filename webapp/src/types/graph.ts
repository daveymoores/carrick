// Graph API response types — mirrors the Lambda output shape

export interface GraphResponse {
  org: string;
  generatedAt: string;
  services: Service[];
  connections: Connection[];
  stats: GraphStats;
}

export interface Service {
  id: string;
  repoName: string;
  serviceName: string;
  lastUpdated: string | null;
  commitHash: string | null;
  endpoints: Endpoint[];
  calls: Call[];
  mountGraph: { nodeCount: number; mountCount: number } | null;
}

export interface Endpoint {
  id: string;
  method: string;
  path: string;
  handler: string | null;
  fileLocation: string | null;
  hasTypes: boolean;
  middlewareChain: string[];
}

export interface Call {
  id: string;
  method: string;
  targetUrl: string;
  client: string;
  fileLocation: string | null;
}

export interface Connection {
  from: string;
  fromService: string;
  to: string;
  toService: string;
  typeStatus: "typed" | "mismatch" | "unknown";
  typeDetail: TypeDetail;
}

export interface TypeDetail {
  status: string;
  reason?: string;
  producerAlias?: string;
  consumerAlias?: string;
  producerExplicit?: boolean;
  consumerExplicit?: boolean;
  producerTypeState?: string;
  consumerTypeState?: string;
}

export interface GraphStats {
  totalServices: number;
  totalEndpoints: number;
  totalCalls: number;
  totalConnections: number;
  typeMismatches: number;
}

export interface SnapshotCreateResponse {
  snapshotId: string;
  url: string;
}
