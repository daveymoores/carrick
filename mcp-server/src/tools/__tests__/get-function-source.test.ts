import { describe, it, expect, vi } from "vitest";
import { getFunctionSource } from "../get-function-source.js";
import { ApiClient } from "../../api-client.js";
import { CloudRepoData } from "../../types.js";

function makeRepoData(
  overrides: Partial<CloudRepoData> = {},
): CloudRepoData {
  return {
    repo_name: "org/test-service",
    endpoints: [],
    calls: [],
    mounts: [],
    apps: {},
    imported_handlers: [],
    function_definitions: {},
    last_updated: "2026-01-01T00:00:00Z",
    commit_hash: "abc123def456",
    ...overrides,
  };
}

function makeServiceRepo(): CloudRepoData {
  return makeRepoData({
    repo_name: "org/user-service",
    service_name: "user-service",
    function_definitions: {
      get_users__id_profile_handler: {
        name: "get_users__id_profile_handler",
        file_path: "src/routes/users.ts",
        node_type: "ArrowFunction",
        arguments: [{ name: "req" }, { name: "res" }],
        is_exported: true,
        line_number: 46,
        end_line_number: 90,
        intent:
          "Builds a user profile by fetching and validating their orders and comments.",
        calls: [
          { name: "isOrderArray", file_path: "src/utils.ts", line_number: 10 },
          {
            name: "isCommentArray",
            file_path: "src/utils.ts",
            line_number: 20,
          },
        ],
      },
      isOrderArray: {
        name: "isOrderArray",
        file_path: "src/utils.ts",
        node_type: "ArrowFunction",
        arguments: [{ name: "arr" }],
        is_exported: true,
        line_number: 10,
        end_line_number: 15,
        intent: "Type guard for Order arrays.",
      },
    },
    mount_graph: {
      nodes: {},
      mounts: [],
      endpoints: [
        {
          method: "GET",
          path: "/users/:id/profile",
          full_path: "/users/:id/profile",
          handler: "get_users__id_profile_handler",
          owner: "app",
          file_location: "src/routes/users.ts",
          middleware_chain: [],
        },
      ],
      data_calls: [
        {
          method: "GET",
          target_url: "/orders?userId=",
          client: "axios",
          file_location: "src/routes/users.ts",
        },
        {
          method: "GET",
          target_url: "/api/comments?userId=",
          client: "axios",
          file_location: "src/routes/users.ts",
        },
        {
          method: "POST",
          target_url: "/audit/log",
          client: "axios",
          file_location: "src/middleware/audit.ts",
        },
      ],
    },
    type_manifest: [
      {
        method: "GET",
        path: "/users/:id/profile",
        role: "producer",
        type_kind: "response",
        type_alias: "UserProfile",
        file_path: "src/routes/users.ts",
        line_number: 48,
        is_explicit: true,
        type_state: "explicit",
        evidence: {
          file_path: "src/routes/users.ts",
          line_number: 48,
          infer_kind: "response_body",
          is_explicit: true,
          type_state: "explicit",
        },
        resolved_definition:
          "interface UserProfile { userId: number; name: string; orders: Order[]; }",
        expanded_definition:
          "{ userId: number; name: string; orders: { id: number; total: number; }[]; }",
      },
    ],
    bundled_types:
      "export interface UserProfile { userId: number; name: string; orders: Order[]; }",
  });
}

function makeOrderServiceRepo(): CloudRepoData {
  return makeRepoData({
    repo_name: "org/order-service",
    service_name: "order-service",
    mount_graph: {
      nodes: {},
      mounts: [],
      data_calls: [],
      endpoints: [
        {
          method: "GET",
          path: "/orders",
          full_path: "/orders",
          handler: "getOrders",
          owner: "app",
          file_location: "src/routes.ts",
          middleware_chain: [],
        },
      ],
    },
  });
}

function mockClient(repos: CloudRepoData[]): ApiClient {
  return {
    getAllRepoData: vi.fn().mockResolvedValue(repos),
    findService: vi.fn((name: string) => {
      const lower = name.toLowerCase();
      return Promise.resolve(
        repos.find(
          (r) =>
            r.service_name?.toLowerCase() === lower ||
            r.repo_name.toLowerCase().includes(lower),
        ),
      );
    }),
    invalidateCache: vi.fn(),
  } as unknown as ApiClient;
}

function parseResult(result: { content: Array<{ text: string }> }) {
  return JSON.parse(result.content[0].text);
}

describe("getFunctionSource", () => {
  describe("service/function lookup", () => {
    it("returns error when service is not found", async () => {
      const client = mockClient([]);
      const result = await getFunctionSource(client, {
        service: "nonexistent",
        function_name: "foo",
      });
      expect(result.content[0].text).toContain("not found");
      expect(result.content[0].text).toContain("nonexistent");
    });

    it("returns error when function is not found in service", async () => {
      const client = mockClient([makeServiceRepo()]);
      const result = await getFunctionSource(client, {
        service: "user-service",
        function_name: "nonexistent_handler",
      });
      expect(result.content[0].text).toContain("not found");
      expect(result.content[0].text).toContain("nonexistent_handler");
    });
  });

  describe("signature detail", () => {
    it("returns metadata without source coordinates", async () => {
      const client = mockClient([makeServiceRepo(), makeOrderServiceRepo()]);
      const result = await getFunctionSource(client, {
        service: "user-service",
        function_name: "get_users__id_profile_handler",
        detail: "signature",
      });
      const data = parseResult(result);

      expect(data.function_name).toBe("get_users__id_profile_handler");
      expect(data.service).toBe("user-service");
      expect(data.route).toBe("GET /users/:id/profile");
      expect(data.intent).toContain("user profile");
      expect(data).not.toHaveProperty("source");
    });

    it("includes resolved route params", async () => {
      const client = mockClient([makeServiceRepo(), makeOrderServiceRepo()]);
      const result = await getFunctionSource(client, {
        service: "user-service",
        function_name: "get_users__id_profile_handler",
        detail: "signature",
      });
      const data = parseResult(result);
      expect(data.params).toEqual({ id: "string" });
    });

    it("includes internal calls", async () => {
      const client = mockClient([makeServiceRepo(), makeOrderServiceRepo()]);
      const result = await getFunctionSource(client, {
        service: "user-service",
        function_name: "get_users__id_profile_handler",
        detail: "signature",
      });
      const data = parseResult(result);
      expect(data.internal_calls).toEqual(["isOrderArray", "isCommentArray"]);
    });

    it("includes intent", async () => {
      const client = mockClient([makeServiceRepo(), makeOrderServiceRepo()]);
      const result = await getFunctionSource(client, {
        service: "user-service",
        function_name: "get_users__id_profile_handler",
        detail: "signature",
      });
      const data = parseResult(result);
      expect(data.intent).toBe(
        "Builds a user profile by fetching and validating their orders and comments.",
      );
    });
  });

  describe("type resolution", () => {
    it("resolves response type with alias and expanded definition", async () => {
      const client = mockClient([makeServiceRepo(), makeOrderServiceRepo()]);
      const result = await getFunctionSource(client, {
        service: "user-service",
        function_name: "get_users__id_profile_handler",
        detail: "signature",
      });
      const data = parseResult(result);

      expect(data.response_type).toBeTruthy();
      expect(data.response_type.alias).toBe("UserProfile");
      expect(data.response_type.definition).toContain("userId");
      expect(data.response_type.is_explicit).toBe(true);
    });

    it("returns null for request_body when none exists", async () => {
      const client = mockClient([makeServiceRepo(), makeOrderServiceRepo()]);
      const result = await getFunctionSource(client, {
        service: "user-service",
        function_name: "get_users__id_profile_handler",
        detail: "signature",
      });
      const data = parseResult(result);
      expect(data.request_body).toBeNull();
    });

    it("returns null types for functions with no endpoint", async () => {
      const client = mockClient([makeServiceRepo(), makeOrderServiceRepo()]);
      const result = await getFunctionSource(client, {
        service: "user-service",
        function_name: "isOrderArray",
        detail: "signature",
      });
      const data = parseResult(result);

      expect(data.route).toBeNull();
      expect(data.params).toBeNull();
      expect(data.response_type).toBeNull();
      expect(data.request_body).toBeNull();
    });
  });

  describe("external calls", () => {
    it("includes data calls from the function's file", async () => {
      const client = mockClient([makeServiceRepo(), makeOrderServiceRepo()]);
      const result = await getFunctionSource(client, {
        service: "user-service",
        function_name: "get_users__id_profile_handler",
        detail: "signature",
      });
      const data = parseResult(result);

      expect(data.external_calls).toHaveLength(2);
      expect(data.external_calls[0].method).toBe("GET");
      expect(data.external_calls[0].path).toBe("/orders");
    });

    it("excludes data calls from other files", async () => {
      const client = mockClient([makeServiceRepo(), makeOrderServiceRepo()]);
      const result = await getFunctionSource(client, {
        service: "user-service",
        function_name: "get_users__id_profile_handler",
        detail: "signature",
      });
      const data = parseResult(result);

      // The POST /audit/log call is in middleware/audit.ts, not routes/users.ts
      const auditCall = data.external_calls.find(
        (c: { path: string }) => c.path === "/audit/log",
      );
      expect(auditCall).toBeUndefined();
    });

    it("resolves call targets to known service names", async () => {
      const client = mockClient([makeServiceRepo(), makeOrderServiceRepo()]);
      const result = await getFunctionSource(client, {
        service: "user-service",
        function_name: "get_users__id_profile_handler",
        detail: "signature",
      });
      const data = parseResult(result);

      const orderCall = data.external_calls.find(
        (c: { path: string }) => c.path === "/orders",
      );
      expect(orderCall.service).toBe("order-service");
    });

    it("returns empty external_calls for utility functions", async () => {
      const client = mockClient([makeServiceRepo(), makeOrderServiceRepo()]);
      const result = await getFunctionSource(client, {
        service: "user-service",
        function_name: "isOrderArray",
        detail: "signature",
      });
      const data = parseResult(result);
      expect(data.external_calls).toEqual([]);
    });
  });

  describe("full detail (source coordinates)", () => {
    it("includes source coordinates by default", async () => {
      const client = mockClient([makeServiceRepo(), makeOrderServiceRepo()]);
      const result = await getFunctionSource(client, {
        service: "user-service",
        function_name: "get_users__id_profile_handler",
      });
      const data = parseResult(result);

      expect(data.source).toBeDefined();
      expect(data.source.repo).toBe("org/user-service");
      expect(data.source.path).toBe("src/routes/users.ts");
      expect(data.source.ref).toBe("abc123def456");
      expect(data.source.start_line).toBe(46);
      expect(data.source.end_line).toBe(90);
    });

    it("defaults to full detail when detail param is omitted", async () => {
      const client = mockClient([makeServiceRepo(), makeOrderServiceRepo()]);
      const result = await getFunctionSource(client, {
        service: "user-service",
        function_name: "get_users__id_profile_handler",
      });
      const data = parseResult(result);
      expect(data.source).toBeDefined();
    });

    it("omits source when detail is signature", async () => {
      const client = mockClient([makeServiceRepo(), makeOrderServiceRepo()]);
      const result = await getFunctionSource(client, {
        service: "user-service",
        function_name: "get_users__id_profile_handler",
        detail: "signature",
      });
      const data = parseResult(result);
      expect(data).not.toHaveProperty("source");
    });
  });
});
