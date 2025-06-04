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

## Installation & Usage

```bash
# Build from source
cargo build --release

# Analyze a project
./target/release/carrick /path/to/your/project

# Or with cargo run
cargo run -- /path/to/your/project

# Force local mode (useful in CI environments)
FORCE_LOCAL_MODE=1 ./target/release/carrick /path/to/your/project
```

## Testing

Carrick includes comprehensive integration tests to ensure endpoint detection works correctly:

```bash
# Run all tests
cargo test

# Run integration tests specifically
cargo test --test integration_test

# Install test dependencies first
cd test-repo && npm install
cd ../test-multiple && npm install  
cd ../tests/fixtures/imported-routers && npm install
```

### Test Coverage

- **Basic endpoint detection**: Tests using `test-repo/` fixture
- **Imported router resolution**: Tests complex routing with imported Express routers
- **Regression tests**: Specifically catches the imported router endpoint resolution bug

The tests verify that:
1. Endpoints are correctly detected and resolved with their full paths
2. Imported routers are processed correctly with their imported names
3. No duplicate processing occurs that could break endpoint resolution
4. The number of detected endpoints matches expectations

## CI/CD Integration

Use the provided GitHub Actions workflow to run tests on every PR and push:

```yaml
# .github/workflows/ci.yml is included for:
- Unit and integration tests
- Code formatting and linting  
- Security audit
- Endpoint detection regression tests
```

### Environment Variables

- `FORCE_LOCAL_MODE=1` - Forces local analysis mode even in CI environments
- `MOCK_STORAGE=1` - Uses mock storage instead of MongoDB in CI mode
- `MONGODB_URI` - MongoDB connection string (required for CI mode)
