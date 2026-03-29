import { describe, it, expect } from "vitest";
import {
  findManifestEntries,
  extractTypeDefinition,
  extractEndpointTypes,
} from "../type-extractor.js";
import { TypeManifestEntry } from "../../types.js";

function makeManifestEntry(
  overrides: Partial<TypeManifestEntry> = {},
): TypeManifestEntry {
  return {
    method: "GET",
    path: "/api/users/:id",
    role: "producer",
    type_kind: "response",
    type_alias: "GetUserResponse",
    file_path: "src/routes/users.ts",
    line_number: 10,
    is_explicit: true,
    type_state: "explicit",
    evidence: {
      file_path: "src/routes/users.ts",
      line_number: 10,
      infer_kind: "annotation",
      is_explicit: true,
      type_state: "explicit",
    },
    ...overrides,
  };
}

describe("findManifestEntries", () => {
  const manifest: TypeManifestEntry[] = [
    makeManifestEntry({
      method: "GET",
      path: "/api/users/:id",
      role: "producer",
      type_kind: "response",
      type_alias: "GetUserResponse",
    }),
    makeManifestEntry({
      method: "GET",
      path: "/api/users/:id",
      role: "producer",
      type_kind: "request",
      type_alias: "GetUserRequest",
    }),
    makeManifestEntry({
      method: "POST",
      path: "/api/users",
      role: "producer",
      type_kind: "request",
      type_alias: "CreateUserRequest",
    }),
    makeManifestEntry({
      method: "GET",
      path: "/api/users/:id",
      role: "consumer",
      type_kind: "response",
      type_alias: "ConsumerGetUserResponse",
    }),
  ];

  it("finds entries matching method and path", () => {
    const results = findManifestEntries(manifest, "GET", "/api/users/:id");
    expect(results).toHaveLength(2);
    expect(results.map((r) => r.type_alias)).toEqual([
      "GetUserResponse",
      "GetUserRequest",
    ]);
  });

  it("matches case-insensitively on method", () => {
    const results = findManifestEntries(manifest, "get", "/api/users/:id");
    expect(results).toHaveLength(2);
  });

  it("normalizes path parameters for matching", () => {
    const results = findManifestEntries(manifest, "GET", "/api/users/{userId}");
    expect(results).toHaveLength(2);
  });

  it("filters by role", () => {
    const results = findManifestEntries(
      manifest,
      "GET",
      "/api/users/:id",
      "consumer",
    );
    expect(results).toHaveLength(1);
    expect(results[0].type_alias).toBe("ConsumerGetUserResponse");
  });

  it("returns empty when no match", () => {
    const results = findManifestEntries(manifest, "DELETE", "/api/users/:id");
    expect(results).toHaveLength(0);
  });
});

describe("extractTypeDefinition", () => {
  it("extracts a type alias declaration", () => {
    const bundled = `export type GetUserResponse = {
  id: string;
  name: string;
};
export type OtherType = string;`;

    const result = extractTypeDefinition(bundled, "GetUserResponse");
    expect(result).toContain("type GetUserResponse =");
    expect(result).toContain("id: string");
    expect(result).toContain("name: string");
  });

  it("extracts an interface declaration", () => {
    const bundled = `export interface CreateUserRequest {
  name: string;
  email: string;
}`;

    const result = extractTypeDefinition(bundled, "CreateUserRequest");
    expect(result).toContain("interface CreateUserRequest");
    expect(result).toContain("name: string");
    expect(result).toContain("email: string");
  });

  it("returns null when type not found", () => {
    const bundled = `export type Foo = string;`;
    const result = extractTypeDefinition(bundled, "NonExistent");
    expect(result).toBeNull();
  });

  it("handles type without export keyword", () => {
    const bundled = `type InternalType = {
  value: number;
};
type NextType = string;`;

    const result = extractTypeDefinition(bundled, "InternalType");
    expect(result).not.toBeNull();
    expect(result).toContain("InternalType");
    expect(result).toContain("value: number");
  });

  it("handles simple type alias", () => {
    const bundled = `export type UserId = string;
export type UserName = string;`;

    const result = extractTypeDefinition(bundled, "UserId");
    expect(result).not.toBeNull();
    expect(result).toContain("type UserId =");
    expect(result).toContain("string");
  });

  it("extracts type with nested objects", () => {
    const bundled = `export type ComplexType = {
  nested: {
    deep: {
      value: string;
    };
  };
};
export type NextType = number;`;

    const result = extractTypeDefinition(bundled, "ComplexType");
    expect(result).not.toBeNull();
    expect(result).toContain("nested");
    expect(result).toContain("deep");
  });

  it("preserves generic parameters on interfaces", () => {
    const bundled = `export interface ApiResponse<T> {
  data: T;
  status: number;
}`;

    const result = extractTypeDefinition(bundled, "ApiResponse");
    expect(result).not.toBeNull();
    expect(result).toContain("interface ApiResponse<T>");
    expect(result).toContain("data: T");
  });

  it("preserves extends clauses on interfaces", () => {
    const bundled = `export interface AdminUser extends BaseUser {
  permissions: string[];
}`;

    const result = extractTypeDefinition(bundled, "AdminUser");
    expect(result).not.toBeNull();
    expect(result).toContain("interface AdminUser");
    expect(result).toContain("extends BaseUser");
    expect(result).toContain("permissions: string[]");
  });

  it("preserves generic params and extends together", () => {
    const bundled = `export interface PaginatedResponse<T> extends BaseResponse {
  items: T[];
  total: number;
}`;

    const result = extractTypeDefinition(bundled, "PaginatedResponse");
    expect(result).not.toBeNull();
    expect(result).toContain("<T>");
    expect(result).toContain("extends BaseResponse");
    expect(result).toContain("items: T[]");
  });

  it("handles intersection types with object literals", () => {
    const bundled = `export type UserWithMeta = { name: string } & { createdAt: Date };
export type Other = string;`;

    const result = extractTypeDefinition(bundled, "UserWithMeta");
    expect(result).not.toBeNull();
    expect(result).toContain("name: string");
    expect(result).toContain("&");
    expect(result).toContain("createdAt: Date");
  });

  it("handles union types with object literals", () => {
    const bundled = `export type Result = { ok: true; data: string } | { ok: false; error: string };
export type Other = number;`;

    const result = extractTypeDefinition(bundled, "Result");
    expect(result).not.toBeNull();
    expect(result).toContain("ok: true");
    expect(result).toContain("|");
    expect(result).toContain("error: string");
  });

  it("handles arrow function types", () => {
    const bundled = `export type Handler = (req: Request) => Response;
export type Other = string;`;

    const result = extractTypeDefinition(bundled, "Handler");
    expect(result).not.toBeNull();
    expect(result).toContain("(req: Request) => Response");
  });

  it("does not match partial type names", () => {
    const bundled = `export type UserResponse = string;
export type GetUserResponseData = number;`;

    const result = extractTypeDefinition(bundled, "UserResponse");
    expect(result).not.toBeNull();
    expect(result).toContain("type UserResponse =");
    // Should not contain the other type's definition
    expect(result).not.toContain("number");
  });
});

describe("extractEndpointTypes", () => {
  const bundledTypes = `export type GetUserResponse = {
  id: string;
  name: string;
};
export type GetUserRequest = {
  id: string;
};
export type UnrelatedType = boolean;`;

  const manifest: TypeManifestEntry[] = [
    makeManifestEntry({
      method: "GET",
      path: "/api/users/:id",
      type_kind: "response",
      type_alias: "GetUserResponse",
    }),
    makeManifestEntry({
      method: "GET",
      path: "/api/users/:id",
      type_kind: "request",
      type_alias: "GetUserRequest",
    }),
  ];

  it("extracts all types for an endpoint", () => {
    const results = extractEndpointTypes(
      manifest,
      bundledTypes,
      "GET",
      "/api/users/:id",
    );
    expect(results).toHaveLength(2);
    expect(results[0].type_alias).toBe("GetUserResponse");
    expect(results[0].type_kind).toBe("response");
    expect(results[0].definition).toContain("id: string");
    expect(results[1].type_alias).toBe("GetUserRequest");
    expect(results[1].type_kind).toBe("request");
  });

  it("returns empty array when no manifest entries match", () => {
    const results = extractEndpointTypes(
      manifest,
      bundledTypes,
      "DELETE",
      "/api/users/:id",
    );
    expect(results).toHaveLength(0);
  });

  it("skips entries where type definition is not found in bundled types", () => {
    const sparseManifest: TypeManifestEntry[] = [
      makeManifestEntry({
        method: "GET",
        path: "/api/users/:id",
        type_alias: "MissingType",
      }),
    ];
    const results = extractEndpointTypes(
      sparseManifest,
      bundledTypes,
      "GET",
      "/api/users/:id",
    );
    expect(results).toHaveLength(0);
  });

  it("preserves manifest metadata in results", () => {
    const results = extractEndpointTypes(
      manifest,
      bundledTypes,
      "GET",
      "/api/users/:id",
    );
    expect(results[0].is_explicit).toBe(true);
    expect(results[0].file_path).toBe("src/routes/users.ts");
    expect(results[0].line_number).toBe(10);
  });
});
