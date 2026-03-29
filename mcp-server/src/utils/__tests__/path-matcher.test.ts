import { describe, it, expect } from "vitest";
import { normalizePath, pathsMatch, pathContains } from "../path-matcher.js";

describe("normalizePath", () => {
  it("lowercases the path", () => {
    expect(normalizePath("/API/Users")).toBe("/api/users");
  });

  it("strips trailing slashes", () => {
    expect(normalizePath("/api/users/")).toBe("/api/users");
    expect(normalizePath("/api/users///")).toBe("/api/users");
  });

  it("normalizes :param style parameters", () => {
    expect(normalizePath("/api/users/:id")).toBe("/api/users/:param");
    expect(normalizePath("/api/users/:userId/posts/:postId")).toBe(
      "/api/users/:param/posts/:param",
    );
  });

  it("normalizes {param} style parameters", () => {
    expect(normalizePath("/api/users/{id}")).toBe("/api/users/:param");
    expect(normalizePath("/api/users/{userId}/posts/{postId}")).toBe(
      "/api/users/:param/posts/:param",
    );
  });

  it("returns / for empty string", () => {
    expect(normalizePath("")).toBe("/");
  });

  it("handles root path", () => {
    expect(normalizePath("/")).toBe("/");
  });

  it("handles path with underscores in params", () => {
    expect(normalizePath("/api/:user_id")).toBe("/api/:param");
    expect(normalizePath("/api/{user_id}")).toBe("/api/:param");
  });
});

describe("pathsMatch", () => {
  it("matches identical paths", () => {
    expect(pathsMatch("/api/users", "/api/users")).toBe(true);
  });

  it("matches paths with different param styles", () => {
    expect(pathsMatch("/api/users/:id", "/api/users/{id}")).toBe(true);
  });

  it("matches paths with different casing", () => {
    expect(pathsMatch("/API/Users", "/api/users")).toBe(true);
  });

  it("matches when one has trailing slash", () => {
    expect(pathsMatch("/api/users/", "/api/users")).toBe(true);
  });

  it("matches paths with different param names", () => {
    expect(pathsMatch("/api/users/:userId", "/api/users/:id")).toBe(true);
  });

  it("does not match different paths", () => {
    expect(pathsMatch("/api/users", "/api/posts")).toBe(false);
  });

  it("does not match different depth paths", () => {
    expect(pathsMatch("/api/users", "/api/users/active")).toBe(false);
  });
});

describe("pathContains", () => {
  it("matches substring case-insensitively", () => {
    expect(pathContains("/api/users/active", "users")).toBe(true);
    expect(pathContains("/api/Users/active", "users")).toBe(true);
    expect(pathContains("/api/users/active", "USERS")).toBe(true);
  });

  it("returns false when substring not found", () => {
    expect(pathContains("/api/users", "posts")).toBe(false);
  });

  it("matches partial segments", () => {
    expect(pathContains("/api/user-profiles", "user")).toBe(true);
  });
});
