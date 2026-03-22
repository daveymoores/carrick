import type { GraphResponse, Service, Connection } from "@/types/graph";

// Cytoscape element types
export interface CyNode {
  group: "nodes";
  data: {
    id: string;
    label: string;
    parent?: string;
    nodeType: "service" | "endpoint" | "call";
    method?: string;
    path?: string;
    handler?: string;
    client?: string;
    fileLocation?: string;
    hasTypes?: boolean;
    service?: Service;
    middlewareChain?: string[];
  };
}

export interface CyEdge {
  group: "edges";
  data: {
    id: string;
    source: string;
    target: string;
    typeStatus: string;
    connection: Connection;
  };
}

export type CyElement = CyNode | CyEdge;

const METHOD_COLORS: Record<string, string> = {
  GET: "#22c55e",
  POST: "#3b82f6",
  PUT: "#f59e0b",
  PATCH: "#f59e0b",
  DELETE: "#ef4444",
};

export function getMethodColor(method: string): string {
  return METHOD_COLORS[method.toUpperCase()] || "#6b7280";
}

export function getEdgeColor(typeStatus: string): string {
  switch (typeStatus) {
    case "typed":
      return "#22c55e";
    case "mismatch":
      return "#ef4444";
    default:
      return "#6b7280";
  }
}

export function transformToElements(graph: GraphResponse): CyElement[] {
  const elements: CyElement[] = [];

  for (const service of graph.services) {
    // Parent node for the service
    elements.push({
      group: "nodes",
      data: {
        id: service.id,
        label: service.serviceName,
        nodeType: "service",
        service,
      },
    });

    // Child nodes for endpoints
    for (const ep of service.endpoints) {
      elements.push({
        group: "nodes",
        data: {
          id: ep.id,
          label: `${ep.method} ${ep.path}`,
          parent: service.id,
          nodeType: "endpoint",
          method: ep.method,
          path: ep.path,
          handler: ep.handler ?? undefined,
          fileLocation: ep.fileLocation ?? undefined,
          hasTypes: ep.hasTypes,
          middlewareChain: ep.middlewareChain,
        },
      });
    }

    // Child nodes for calls
    for (const call of service.calls) {
      elements.push({
        group: "nodes",
        data: {
          id: call.id,
          label: `${call.method} ${call.targetUrl}`,
          parent: service.id,
          nodeType: "call",
          method: call.method,
          path: call.targetUrl,
          client: call.client,
          fileLocation: call.fileLocation ?? undefined,
        },
      });
    }
  }

  // Edges for connections
  for (const conn of graph.connections) {
    elements.push({
      group: "edges",
      data: {
        id: `edge::${conn.from}::${conn.to}`,
        source: conn.from,
        target: conn.to,
        typeStatus: conn.typeStatus,
        connection: conn,
      },
    });
  }

  return elements;
}
