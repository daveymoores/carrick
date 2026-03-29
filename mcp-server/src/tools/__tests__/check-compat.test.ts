import { describe, it, expect, vi, beforeEach } from "vitest";
import { checkCompatibility } from "../check-compat.js";
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
    commit_hash: "abc123",
    ...overrides,
  };
}

function makeProducer(
  endpoints: Array<{ method: string; path: string }>,
): CloudRepoData {
  return makeRepoData({
    repo_name: "org/producer-api",
    service_name: "producer-api",
    mount_graph: {
      nodes: {},
      mounts: [],
      data_calls: [],
      endpoints: endpoints.map((e) => ({
        method: e.method,
        full_path: e.path,
        path: e.path,
        owner: "app",
        file_location: "src/routes.ts",
        middleware_chain: [],
      })),
    },
  });
}

function makeConsumer(
  calls: Array<{ method: string; url: string }>,
): CloudRepoData {
  return makeRepoData({
    repo_name: "org/consumer-app",
    service_name: "consumer-app",
    mount_graph: {
      nodes: {},
      mounts: [],
      endpoints: [],
      data_calls: calls.map((c) => ({
        method: c.method,
        target_url: c.url,
        client: "axios",
        file_location: "src/api.ts",
      })),
    },
  });
}

function mockClient(repos: CloudRepoData[]): ApiClient {
  return {
    getAllRepoData: vi.fn().mockResolvedValue(repos),
    findService: vi.fn(),
    invalidateCache: vi.fn(),
  } as unknown as ApiClient;
}

function parseResult(result: { content: Array<{ text: string }> }) {
  return JSON.parse(result.content[0].text);
}

describe("checkCompatibility", () => {
  it("reports compatible when all consumer calls match producer endpoints", async () => {
    const producer = makeProducer([
      { method: "GET", path: "/api/users" },
      { method: "GET", path: "/api/users/:id" },
    ]);
    const consumer = makeConsumer([
      { method: "GET", url: "/api/users" },
      { method: "GET", url: "/api/users/:id" },
    ]);
    const client = mockClient([producer, consumer]);

    const result = await checkCompatibility(client, {
      consumer_service: "consumer-app",
      producer_service: "producer-api",
    });

    const data = parseResult(result);
    expect(data.compatible).toBe(true);
    expect(data.issues.filter((i: { severity: string }) => i.severity === "error")).toHaveLength(0);
  });

  it("reports missing endpoint when consumer calls something producer doesn't expose", async () => {
    const producer = makeProducer([
      { method: "GET", path: "/api/users" },
    ]);
    const consumer = makeConsumer([
      { method: "GET", url: "/api/users" },
      { method: "DELETE", url: "/api/users/:id" },
    ]);
    const client = mockClient([producer, consumer]);

    const result = await checkCompatibility(client, {
      consumer_service: "consumer-app",
      producer_service: "producer-api",
    });

    const data = parseResult(result);
    expect(data.compatible).toBe(false);
    const errors = data.issues.filter((i: { severity: string }) => i.severity === "error");
    expect(errors).toHaveLength(1);
    expect(errors[0].category).toBe("missing_endpoint");
    expect(errors[0].message).toContain("DELETE");
  });

  it("reports unused endpoints as info", async () => {
    const producer = makeProducer([
      { method: "GET", path: "/api/users" },
      { method: "POST", path: "/api/users" },
      { method: "DELETE", path: "/api/users/:id" },
    ]);
    const consumer = makeConsumer([
      { method: "GET", url: "/api/users" },
    ]);
    const client = mockClient([producer, consumer]);

    const result = await checkCompatibility(client, {
      consumer_service: "consumer-app",
      producer_service: "producer-api",
    });

    const data = parseResult(result);
    expect(data.compatible).toBe(true);
    const infos = data.issues.filter((i: { severity: string }) => i.severity === "info");
    expect(infos).toHaveLength(2);
    expect(infos.every((i: { category: string }) => i.category === "unused_endpoint")).toBe(true);
  });

  it("matches endpoints with different param styles", async () => {
    const producer = makeProducer([
      { method: "GET", path: "/api/users/:id" },
    ]);
    const consumer = makeConsumer([
      { method: "GET", url: "/api/users/{id}" },
    ]);
    const client = mockClient([producer, consumer]);

    const result = await checkCompatibility(client, {
      consumer_service: "consumer-app",
      producer_service: "producer-api",
    });

    const data = parseResult(result);
    expect(data.compatible).toBe(true);
  });

  it("filters by method when provided", async () => {
    const producer = makeProducer([
      { method: "GET", path: "/api/users" },
      { method: "POST", path: "/api/users" },
    ]);
    const consumer = makeConsumer([
      { method: "GET", url: "/api/users" },
      { method: "POST", url: "/api/users" },
      { method: "DELETE", url: "/api/users/:id" },
    ]);
    const client = mockClient([producer, consumer]);

    const result = await checkCompatibility(client, {
      consumer_service: "consumer-app",
      producer_service: "producer-api",
      method: "GET",
    });

    const data = parseResult(result);
    expect(data.compatible).toBe(true);
    expect(data.consumer_calls).toBe(1);
  });

  it("filters by path when provided", async () => {
    const producer = makeProducer([
      { method: "GET", path: "/api/users" },
    ]);
    const consumer = makeConsumer([
      { method: "GET", url: "/api/users" },
      { method: "DELETE", url: "/api/posts/:id" },
    ]);
    const client = mockClient([producer, consumer]);

    const result = await checkCompatibility(client, {
      consumer_service: "consumer-app",
      producer_service: "producer-api",
      path: "/api/users",
    });

    const data = parseResult(result);
    expect(data.compatible).toBe(true);
    expect(data.consumer_calls).toBe(1);
  });

  it("returns error when consumer service not found", async () => {
    const producer = makeProducer([]);
    const client = mockClient([producer]);

    const result = await checkCompatibility(client, {
      consumer_service: "nonexistent",
      producer_service: "producer-api",
    });

    expect(result.content[0].text).toContain("Consumer service");
    expect(result.content[0].text).toContain("not found");
  });

  it("returns error when producer service not found", async () => {
    const consumer = makeConsumer([]);
    const client = mockClient([consumer]);

    const result = await checkCompatibility(client, {
      consumer_service: "consumer-app",
      producer_service: "nonexistent",
    });

    expect(result.content[0].text).toContain("Producer service");
    expect(result.content[0].text).toContain("not found");
  });

  it("falls back to top-level endpoints/calls when mount_graph is missing", async () => {
    const producer = makeRepoData({
      repo_name: "org/producer-api",
      service_name: "producer-api",
      endpoints: [
        {
          method: "GET",
          route: "/api/users",
          params: [],
          file_path: "src/routes.ts",
        },
      ],
    });
    const consumer = makeRepoData({
      repo_name: "org/consumer-app",
      service_name: "consumer-app",
      calls: [
        {
          method: "GET",
          route: "/api/users",
          params: [],
          file_path: "src/api.ts",
        },
      ],
    });
    const client = mockClient([producer, consumer]);

    const result = await checkCompatibility(client, {
      consumer_service: "consumer-app",
      producer_service: "producer-api",
    });

    const data = parseResult(result);
    expect(data.compatible).toBe(true);
  });

  it("does not report unused endpoints when filtering by method", async () => {
    const producer = makeProducer([
      { method: "GET", path: "/api/users" },
      { method: "POST", path: "/api/users" },
    ]);
    const consumer = makeConsumer([
      { method: "GET", url: "/api/users" },
    ]);
    const client = mockClient([producer, consumer]);

    const result = await checkCompatibility(client, {
      consumer_service: "consumer-app",
      producer_service: "producer-api",
      method: "GET",
    });

    const data = parseResult(result);
    const infos = data.issues.filter((i: { severity: string }) => i.severity === "info");
    expect(infos).toHaveLength(0);
  });
});
