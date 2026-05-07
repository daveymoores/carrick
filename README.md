# Carrick

Carrick is a live, type-aware, intent-aware index of every TypeScript service in your GitHub org, exposed to AI coding agents over the Model Context Protocol.

> Carrick is TypeScript only. Cross-repo features need at least two services indexed in the same GitHub org. A single-service install still gets same-repo validation.

## What an agent can ask

Connect Claude Code, Cursor, Windsurf, or Codex to the Carrick MCP endpoint. Carrick answers semantic questions about your org that an agent normally has to grep across repos to answer, badly:

- "Which functions handle webhook signing across our services?"
- "Where do we deduplicate users by email?"
- "What calls `/api/users` and what response shape do they expect?"
- "Show me every function that retries on rate-limit errors."

These work because the index combines structural facts, resolved types, and a per-function description of what the code actually does.

## What's in the index

For every scanned function in every repo in your org, Carrick stores three layers:

- **Structural.** Endpoints declared, outbound calls made, mounts, normalised paths.
- **Type-aware.** Request and response types resolved through the TypeScript compiler, so cross-repo type compatibility is checkable.
- **Intent-aware.** A one or two sentence description of what each function does, generated at scan time and stored alongside the structural and type data.

The intent layer is the difference. It is what lets an agent answer "where do we deduplicate users by email" rather than "which functions are named `dedupeUser`."

## Connect your agent

The MCP endpoint lives at `https://api.carrick.tools/mcp`.

```bash
claude mcp add --transport http carrick https://api.carrick.tools/mcp
```

The recommended authentication is sign-in-with-Carrick: your agent opens a browser, you click Approve once, and no API key changes hands. A manual key paste is available as a fallback. Carrick is currently invite-only while the new authentication flow is being shipped; once your org is provisioned both paths work.

## Populate the index

The index is populated by running the Carrick GitHub Action on each TypeScript repo you want indexed. On the main branch the action refreshes that repo's contribution. On pull requests it can optionally post the drift comment described under [Pull request signal](#pull-request-signal).

```yaml
name: Carrick

on:
  push:
    branches: [main]
  pull_request:
    branches: [main]

permissions:
  contents: read
  issues: write
  pull-requests: write

jobs:
  carrick:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4

      - id: carrick
        uses: daveymoores/carrick@v1
        with:
          carrick-api-key: ${{ secrets.CARRICK_API_KEY }}

      - if: github.event_name == 'pull_request'
        uses: actions/github-script@v7
        env:
          COMMENT_BODY: ${{ steps.carrick.outputs.pr-comment }}
        with:
          script: |
            const body = process.env.COMMENT_BODY;
            if (body && body.trim()) {
              github.rest.issues.createComment({
                issue_number: context.issue.number,
                owner: context.repo.owner,
                repo: context.repo.repo,
                body,
              });
            }
```

Add `CARRICK_API_KEY` to the repository's Actions secrets. The dashboard issues the key after you sign in with GitHub.

## MCP tools

The MCP endpoint exposes the index as structured tools your agent can call directly.

| Tool | Purpose |
| :--- | :--- |
| `list_services` | Every service Carrick has indexed in your org |
| `list_function_intents` | One or two sentence descriptions of exported functions, searchable by service |
| `get_api_endpoints` | Endpoints declared by a given service |
| `get_endpoint_types` | Resolved request and response types for a specific endpoint |
| `get_type_definition` | Fully resolved TypeScript type by name, across the org |
| `get_service_dependencies` | Services that call a given producer |
| `check_compatibility` | Whether service A's call to service B matches the producer's contract |

## Pull request signal

The same Carrick action that populates the index can comment on pull requests. The comment summarises drift detected against the rest of your org: producer/consumer type mismatches, mismatched HTTP verbs, missing endpoints, orphaned routes, and npm-dependency-version conflicts. The comment is the proof point that the type analysis is rigorous; the MCP index is the durable product surface.

## Configuration

Add a `carrick.json` to each indexed service to help classify outbound calls.

```json
{
  "serviceName": "order-service",
  "internalEnvVars": ["USER_SERVICE_URL", "INVENTORY_API"],
  "externalEnvVars": ["STRIPE_API", "GITHUB_API"],
  "internalDomains": ["https://api.yourcompany.com"],
  "externalDomains": ["https://api.stripe.com", "https://api.github.com"]
}
```

| Field | Description |
| :--- | :--- |
| `serviceName` | Friendly name for this service |
| `internalEnvVars` | Env vars pointing at other services in your org. Calls are validated against the index. |
| `externalEnvVars` | Env vars pointing at third-party APIs. Calls are ignored. |
| `internalDomains` | Full URL prefixes for internal services |
| `externalDomains` | Full URL prefixes for third-party APIs to ignore |

When Carrick sees a call like `fetch(process.env.ORDER_SERVICE_URL + '/orders')`, it needs to know whether `ORDER_SERVICE_URL` points internally or externally. Unclassified env vars surface as a configuration suggestion in the PR comment.

## How it works

1. SWC parses each TypeScript file into an AST.
2. A static-analysis pass extracts function exports, mounted routers, and pattern-matched HTTP calls.
3. An LLM agent handles the cases pattern matching can't reach: dynamic URLs, factory functions, framework-specific routing.
4. A TypeScript sidecar resolves request and response types against the actual TypeScript compiler.
5. A second LLM pass writes the per-function intent description.
6. The org index lives in DynamoDB and S3 and refreshes each time a service's main branch runs.

## License

[Elastic License 2.0](LICENSE.md). Copyright (c) 2026 Far Harbour B.V.

## Development

See [AGENTS.md](AGENTS.md) for build, test, and contribution conventions.

```bash
cargo test
cargo fmt
cargo clippy
```

Install the optional pre-commit hook to run formatting and tests before each commit:

```bash
./scripts/install-hooks.sh
```
