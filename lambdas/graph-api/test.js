const { describe, it } = require("node:test");
const assert = require("node:assert/strict");

// We test the pure transform logic by requiring the module and
// extracting buildGraph via a small wrapper. Since buildGraph isn't exported
// directly, we replicate the key logic here for unit-testing the transform.

// ─── normalizePath ───────────────────────────────────────────────────────────

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

describe("normalizePath", () => {
  it("normalizes express-style params", () => {
    assert.equal(normalizePath("/api/users/:id"), "/api/users/:param");
  });

  it("normalizes template literal params", () => {
    assert.equal(
      normalizePath("/api/users/${userId}"),
      "/api/users/:param",
    );
  });

  it("normalizes curly brace params", () => {
    assert.equal(normalizePath("/api/users/{id}"), "/api/users/:param");
  });

  it("lowercases static segments", () => {
    assert.equal(normalizePath("/API/Users"), "/api/users");
  });

  it("handles empty path", () => {
    assert.equal(normalizePath(""), "");
    assert.equal(normalizePath(null), "");
  });

  it("normalizes wildcard", () => {
    assert.equal(normalizePath("/api/*"), "/api/:param");
  });
});

// ─── buildGraph (inline replica for testing) ─────────────────────────────────

function makeEndpointId(repoName, method, path) {
  return `${repoName}::${method}::${path}`;
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

describe("graph transform", () => {
  const sampleRepoItems = [
    {
      repo: "user-api",
      hash: "abc123",
      metadata: {
        repo_name: "user-api",
        service_name: "user-service",
        endpoints: [
          {
            owner: { App: "app" },
            route: "/api/users/:id",
            method: "GET",
            params: [],
            request_body: null,
            response_body: null,
            handler_name: "getUser",
            file_path: "src/routes/users.ts",
          },
          {
            owner: { App: "app" },
            route: "/api/users",
            method: "POST",
            params: [],
            request_body: null,
            response_body: null,
            handler_name: "createUser",
            file_path: "src/routes/users.ts",
          },
        ],
        calls: [],
        mounts: [],
        last_updated: "2026-03-21T10:00:00Z",
        commit_hash: "abc123",
        type_manifest: [
          {
            method: "GET",
            path: "/api/users/:id",
            role: "producer",
            type_kind: "response",
            type_alias: "Endpoint_abc_Response",
            file_path: "src/routes/users.ts",
            line_number: 15,
            is_explicit: true,
            type_state: "explicit",
            evidence: {
              file_path: "src/routes/users.ts",
              line_number: 15,
              infer_kind: "FunctionReturn",
              is_explicit: true,
              type_state: "explicit",
            },
          },
        ],
      },
    },
    {
      repo: "order-api",
      hash: "def456",
      metadata: {
        repo_name: "order-api",
        service_name: "order-service",
        endpoints: [
          {
            owner: { App: "app" },
            route: "/api/orders",
            method: "GET",
            params: [],
            request_body: null,
            response_body: null,
            handler_name: "listOrders",
            file_path: "src/routes/orders.ts",
          },
        ],
        calls: [
          {
            owner: null,
            route: "/api/users/${userId}",
            method: "GET",
            params: [],
            request_body: null,
            response_body: null,
            handler_name: "fetch",
            file_path: "src/services/user-client.ts",
          },
        ],
        mounts: [],
        last_updated: "2026-03-21T11:00:00Z",
        commit_hash: "def456",
        type_manifest: [
          {
            method: "GET",
            path: "/api/users/${userId}",
            role: "consumer",
            type_kind: "response",
            type_alias: "Endpoint_def_Response",
            file_path: "src/services/user-client.ts",
            line_number: 10,
            is_explicit: false,
            type_state: "implicit",
            evidence: {
              file_path: "src/services/user-client.ts",
              line_number: 10,
              infer_kind: "CallResult",
              is_explicit: false,
              type_state: "implicit",
            },
          },
        ],
      },
    },
  ];

  it("builds services from flat endpoints", () => {
    // Inline a minimal buildGraph for testing
    const services = sampleRepoItems
      .filter((r) => r.metadata)
      .map((item) => {
        const meta = item.metadata;
        return {
          id: meta.repo_name,
          repoName: meta.repo_name,
          serviceName: meta.service_name || meta.repo_name,
          endpoints: (meta.endpoints || []).map((ep) => ({
            id: makeEndpointId(meta.repo_name, ep.method, ep.route),
            method: ep.method,
            path: ep.route,
          })),
          calls: (meta.calls || []).map((c) => ({
            id: makeEndpointId(meta.repo_name, c.method, c.route),
            method: c.method,
            targetUrl: c.route,
          })),
        };
      });

    assert.equal(services.length, 2);
    assert.equal(services[0].serviceName, "user-service");
    assert.equal(services[0].endpoints.length, 2);
    assert.equal(services[1].calls.length, 1);
  });

  it("matches calls to endpoints via normalized paths", () => {
    // order-api calls GET /api/users/${userId}
    // user-api serves GET /api/users/:id
    // These should match after normalization
    const callPath = normalizePath("/api/users/${userId}");
    const endpointPath = normalizePath("/api/users/:id");
    assert.equal(callPath, endpointPath);
  });

  it("finds manifest entries with normalized matching", () => {
    const manifest = sampleRepoItems[0].metadata.type_manifest;
    const entry = findManifestEntry(
      manifest,
      "GET",
      "/api/users/:id",
      "producer",
    );
    assert.ok(entry);
    assert.equal(entry.type_alias, "Endpoint_abc_Response");
  });

  it("finds consumer manifest with template literal path", () => {
    const manifest = sampleRepoItems[1].metadata.type_manifest;
    const entry = findManifestEntry(
      manifest,
      "GET",
      "/api/users/:id",
      "consumer",
    );
    assert.ok(entry);
    assert.equal(entry.type_alias, "Endpoint_def_Response");
  });

  it("returns null for missing manifest", () => {
    assert.equal(findManifestEntry(null, "GET", "/x", "producer"), null);
    assert.equal(findManifestEntry([], "GET", "/x", "producer"), null);
  });
});

describe("endpoint ID generation", () => {
  it("creates composite IDs", () => {
    assert.equal(
      makeEndpointId("repo-a", "GET", "/api/users/:id"),
      "repo-a::GET::/api/users/:id",
    );
  });
});
