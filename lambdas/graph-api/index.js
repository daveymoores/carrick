const { DynamoDBClient } = require("@aws-sdk/client-dynamodb");
const {
  DynamoDBDocumentClient,
  ScanCommand,
  GetCommand,
  PutCommand,
} = require("@aws-sdk/lib-dynamodb");

const dynamoClient = new DynamoDBClient();
const docClient = DynamoDBDocumentClient.from(dynamoClient);

const TABLE_NAME = process.env.DYNAMODB_TABLE;
const SNAPSHOT_TABLE_NAME = process.env.SNAPSHOT_TABLE || TABLE_NAME;

// ─── URL Normalization ───────────────────────────────────────────────────────
// Simplified JS version of the Rust UrlNormalizer logic.
// Converts path params to a canonical form for matching:
//   /api/users/:id  →  /api/users/:param
//   /api/users/${userId}  →  /api/users/:param

function normalizePath(path) {
  if (!path) return "";
  return path
    .split("/")
    .map((seg) => {
      if (seg.startsWith(":")) return ":param";
      if (seg.startsWith("${") || seg.startsWith("{")) return ":param";
      if (seg === "*") return ":param";
      return seg.toLowerCase();
    })
    .join("/");
}

function makeEndpointId(repoName, method, path) {
  return `${repoName}::${method}::${path}`;
}

// ─── Graph Transform ─────────────────────────────────────────────────────────
// Takes an array of { repo, metadata: CloudRepoData } and produces the
// graph-ready JSON payload consumed by the webapp.

function buildGraph(org, repoItems) {
  const services = [];
  const connections = [];

  // Build a lookup of normalized producer endpoints for matching
  // key: "METHOD::/normalized/path" → { endpointId, repoName, typeManifest entries }
  const producerIndex = new Map();

  for (const item of repoItems) {
    const meta = item.metadata;
    if (!meta) continue;

    const repoName = meta.repo_name || item.repo;
    const serviceName = meta.service_name || repoName;

    // Prefer mount_graph.endpoints (resolved full paths) over flat endpoints
    const endpoints = meta.mount_graph?.endpoints || [];
    const dataCalls = meta.mount_graph?.data_calls || [];
    const flatEndpoints = meta.endpoints || [];
    const flatCalls = meta.calls || [];

    // Build service endpoints list
    const serviceEndpoints = [];

    if (endpoints.length > 0) {
      // Use mount_graph resolved endpoints
      for (const ep of endpoints) {
        const path = ep.full_path || ep.path;
        const id = makeEndpointId(repoName, ep.method, path);
        const manifest = findManifestEntry(
          meta.type_manifest,
          ep.method,
          path,
          "producer",
        );

        serviceEndpoints.push({
          id,
          method: ep.method,
          path,
          handler: ep.handler || null,
          fileLocation: ep.file_location || null,
          hasTypes: !!manifest,
          middlewareChain: ep.middleware_chain || [],
        });

        const normalizedKey = `${ep.method}::${normalizePath(path)}`;
        producerIndex.set(normalizedKey, {
          endpointId: id,
          repoName,
          manifest,
          bundledTypes: meta.bundled_types || null,
        });
      }
    } else {
      // Fallback to flat endpoints
      for (const ep of flatEndpoints) {
        const path = ep.route;
        const id = makeEndpointId(repoName, ep.method, path);
        const manifest = findManifestEntry(
          meta.type_manifest,
          ep.method,
          path,
          "producer",
        );

        serviceEndpoints.push({
          id,
          method: ep.method,
          path,
          handler: ep.handler_name || null,
          fileLocation: ep.file_path || null,
          hasTypes: !!manifest,
          middlewareChain: [],
        });

        const normalizedKey = `${ep.method}::${normalizePath(path)}`;
        producerIndex.set(normalizedKey, {
          endpointId: id,
          repoName,
          manifest,
          bundledTypes: meta.bundled_types || null,
        });
      }
    }

    // Build service calls list
    const serviceCalls = [];

    if (dataCalls.length > 0) {
      for (const call of dataCalls) {
        const id = makeEndpointId(repoName, call.method, call.target_url);
        serviceCalls.push({
          id,
          method: call.method,
          targetUrl: call.target_url,
          client: call.client,
          fileLocation: call.file_location || null,
        });
      }
    } else {
      for (const call of flatCalls) {
        const id = makeEndpointId(repoName, call.method, call.route);
        serviceCalls.push({
          id,
          method: call.method,
          targetUrl: call.route,
          client: call.handler_name || "unknown",
          fileLocation: call.file_path || null,
        });
      }
    }

    services.push({
      id: repoName,
      repoName,
      serviceName,
      lastUpdated: meta.last_updated || item.lastUpdated || null,
      commitHash: meta.commit_hash || item.hash || null,
      endpoints: serviceEndpoints,
      calls: serviceCalls,
      mountGraph: meta.mount_graph
        ? {
            nodeCount: Object.keys(meta.mount_graph.nodes || {}).length,
            mountCount: (meta.mount_graph.mounts || []).length,
          }
        : null,
    });
  }

  // Build connections: match each call against the producer index
  for (const service of services) {
    for (const call of service.calls) {
      const normalizedKey = `${call.method}::${normalizePath(call.targetUrl)}`;
      const producer = producerIndex.get(normalizedKey);

      if (producer && producer.repoName !== service.repoName) {
        // Find consumer manifest entry for type comparison
        const consumerItem = repoItems.find(
          (r) => (r.metadata?.repo_name || r.repo) === service.repoName,
        );
        const consumerManifest = consumerItem?.metadata
          ? findManifestEntry(
              consumerItem.metadata.type_manifest,
              call.method,
              call.targetUrl,
              "consumer",
            )
          : null;

        const typeDetail = computeTypeStatus(
          producer.manifest,
          consumerManifest,
        );

        connections.push({
          from: call.id,
          fromService: service.repoName,
          to: producer.endpointId,
          toService: producer.repoName,
          typeStatus: typeDetail.status,
          typeDetail,
        });
      }
    }
  }

  return {
    org,
    generatedAt: new Date().toISOString(),
    services,
    connections,
    stats: {
      totalServices: services.length,
      totalEndpoints: services.reduce(
        (n, s) => n + s.endpoints.length,
        0,
      ),
      totalCalls: services.reduce((n, s) => n + s.calls.length, 0),
      totalConnections: connections.length,
      typeMismatches: connections.filter((c) => c.typeStatus === "mismatch")
        .length,
    },
  };
}

function findManifestEntry(manifest, method, path, role) {
  if (!manifest || !Array.isArray(manifest)) return null;
  const normalizedPath = normalizePath(path);
  return (
    manifest.find(
      (e) =>
        e.method === method &&
        normalizePath(e.path) === normalizedPath &&
        e.role === role,
    ) || null
  );
}

function computeTypeStatus(producerManifest, consumerManifest) {
  if (!producerManifest && !consumerManifest) {
    return { status: "unknown", reason: "No type information available" };
  }
  if (!producerManifest) {
    return { status: "unknown", reason: "Producer has no type manifest" };
  }
  if (!consumerManifest) {
    return { status: "unknown", reason: "Consumer has no type manifest" };
  }

  // Both have manifests — report what we know.
  // Full structural comparison requires the bundled .d.ts and a TS compiler,
  // which is beyond Lambda scope. We report the aliases so the webapp can show them.
  return {
    status: "typed",
    producerAlias: producerManifest.type_alias,
    consumerAlias: consumerManifest.type_alias,
    producerExplicit: producerManifest.is_explicit,
    consumerExplicit: consumerManifest.is_explicit,
    producerTypeState: producerManifest.type_state,
    consumerTypeState: consumerManifest.type_state,
  };
}

// ─── Snapshot ID ─────────────────────────────────────────────────────────────

function generateSnapshotId() {
  // Simple nanoid-like: 21 chars, URL-safe
  const chars =
    "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
  let id = "";
  const bytes = require("crypto").randomBytes(21);
  for (let i = 0; i < 21; i++) {
    id += chars[bytes[i] % 64];
  }
  return id;
}

// ─── DynamoDB helpers ────────────────────────────────────────────────────────

async function fetchAllReposForOrg(org) {
  let allItems = [];
  let lastKey;

  do {
    const params = {
      TableName: TABLE_NAME,
      FilterExpression: "begins_with(pk, :orgPrefix)",
      ExpressionAttributeValues: { ":orgPrefix": `repo#${org}/` },
    };
    if (lastKey) params.ExclusiveStartKey = lastKey;

    const result = await docClient.send(new ScanCommand(params));
    if (result.Items) allItems = allItems.concat(result.Items);
    lastKey = result.LastEvaluatedKey;
  } while (lastKey);

  return allItems.map((item) => ({
    repo: item.pk.split("/")[1],
    hash: item.hash,
    metadata: item.cloudRepoData || null,
    lastUpdated: item.updatedAt || item.createdAt,
  }));
}

async function saveSnapshot(org, snapshotId, graphData) {
  const item = {
    pk: `snapshot#${org}`,
    sk: `snap#${snapshotId}`,
    org,
    snapshotId,
    graphData,
    createdAt: new Date().toISOString(),
    ttl: Math.floor(Date.now() / 1000) + 90 * 24 * 60 * 60, // 90 days
  };

  await docClient.send(
    new PutCommand({ TableName: SNAPSHOT_TABLE_NAME, Item: item }),
  );
  return item;
}

async function getSnapshot(org, snapshotId) {
  const result = await docClient.send(
    new GetCommand({
      TableName: SNAPSHOT_TABLE_NAME,
      Key: { pk: `snapshot#${org}`, sk: `snap#${snapshotId}` },
    }),
  );
  return result.Item || null;
}

// ─── Handler ─────────────────────────────────────────────────────────────────

exports.handler = async (event) => {
  const method = event.requestContext?.http?.method || event.httpMethod;
  const path = event.requestContext?.http?.path || event.path || "";

  // CORS preflight
  if (method === "OPTIONS") {
    return response(204, "");
  }

  try {
    // GET /graph/{org}
    const liveMatch = path.match(/^\/graph\/([^/]+)$/);
    if (liveMatch && method === "GET") {
      const org = decodeURIComponent(liveMatch[1]);
      const repoItems = await fetchAllReposForOrg(org);
      const graph = buildGraph(org, repoItems);
      return response(200, graph);
    }

    // POST /graph/{org}/snapshot
    const createSnapshotMatch = path.match(/^\/graph\/([^/]+)\/snapshot$/);
    if (createSnapshotMatch && method === "POST") {
      const org = decodeURIComponent(createSnapshotMatch[1]);
      const repoItems = await fetchAllReposForOrg(org);
      const graph = buildGraph(org, repoItems);
      const snapshotId = generateSnapshotId();
      await saveSnapshot(org, snapshotId, graph);
      return response(201, { snapshotId, url: `/snapshot/${snapshotId}` });
    }

    // GET /graph/{org}/snapshot/{id}
    const getSnapshotMatch = path.match(
      /^\/graph\/([^/]+)\/snapshot\/([^/]+)$/,
    );
    if (getSnapshotMatch && method === "GET") {
      const org = decodeURIComponent(getSnapshotMatch[1]);
      const snapshotId = getSnapshotMatch[2];
      const snapshot = await getSnapshot(org, snapshotId);
      if (!snapshot) return response(404, { error: "Snapshot not found" });
      return response(200, snapshot.graphData);
    }

    return response(404, { error: "Not found" });
  } catch (err) {
    console.error("Error:", err);
    return response(500, { error: "Internal server error", debug: err.message });
  }
};

function response(statusCode, body) {
  return {
    statusCode,
    headers: {
      "Content-Type": "application/json",
      "Access-Control-Allow-Origin": "*",
      "Access-Control-Allow-Methods": "GET, POST, OPTIONS",
      "Access-Control-Allow-Headers": "Content-Type",
      "Cache-Control":
        statusCode === 200 ? "public, max-age=30, s-maxage=60" : "no-cache",
    },
    body: typeof body === "string" ? body : JSON.stringify(body),
  };
}
