import request from "supertest";
import { app } from "../src/server";

// Test-client traffic: exercises routes in-process. This is NOT a runtime
// consumer and must not be extracted as a data call.
describe("catalog-api smoke", () => {
  it("responds on the health probe", async () => {
    await request(app).get("/api/v2/health").expect(200);
  });
});

declare function describe(name: string, fn: () => void): void;
declare function it(name: string, fn: () => Promise<void>): void;
