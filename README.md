# Carrick 🪢

Carrick is a tool for finding API dependency issues in microservice architectures. It analyzes TypeScript and JavaScript code to detect problems in API endpoints and calls, helping developers identify issues during code changes.

## Features
- Analyzes API endpoints and calls to find mismatches, missing routes, and unused endpoints.
- Uses SWC for fast static analysis of TypeScript/JavaScript code.
- Checks TypeScript types with the TypeScript compiler to catch response and request shape issues.
- Integrates with GitHub Actions to report issues in CI pipelines.

## Example
Carrick can catch **drifting response types** between repositories. For example:

- **Repository A** defines an endpoint:
  ```typescript
  // server.ts
  app.get("/users", (req, res) => res.json([{ id: 1, name: "Alice" }]));
  ```

- **Repository B** calls the endpoint, expecting a `role` field:
  ```typescript
  // client.ts
  interface User {
    name: string;
    role: string;
  }
  async function fetchUsers(): Promise<User[]> {
    const response = await fetch("http://api.company.com/users");
    return response.json();
  }
  ```

Carrick detects the mismatch and reports:
```
Response mismatch: Type '{ id: number; name: string; }[]' is not assignable to type 'User[]'. Property 'role' is missing. (client.ts:7)
```
