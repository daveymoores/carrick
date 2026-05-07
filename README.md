![Carrick Social Image](https://cdn.prod.website-files.com/685162a038275750f4f698e3/686cee204d48f5406664086d_social-image_1.png)

# Carrick

Carrick indexes every TypeScript service in your GitHub org. The index is exposed to AI coding agents (Claude Code, Cursor, Windsurf, Codex) over the Model Context Protocol, so an agent can answer cross-repo questions without grepping the codebase.

> Carrick is TypeScript only. Cross-repo features need at least two services onboarded in the same GitHub org. A single-repo install still gets same-repo validation.

## What it indexes

For each TypeScript service in your org, Carrick tracks three things:

- **Exported functions**, each with a one-line LLM-generated description of what it does.
- **npm dependencies**, per service and aggregated across the org so version conflicts surface immediately.
- **API endpoints**, with their real TypeScript request and response types extracted from the source.

## What it unblocks

Connect your AI agent to Carrick's MCP endpoint and ask the kind of questions an agent normally can't answer alone:

- "Is there already a helper for this somewhere?"
- "What is the response shape of `GET /users/:id`?"
- "What other services call this endpoint, and would adding a required field break them?"

## On pull requests

Carrick also runs as a GitHub Action. On the main branch it refreshes the org index. On pull requests it posts a comment summarising drift detected against the rest of the org: producer/consumer type mismatches, mismatched HTTP verbs, missing endpoints, orphaned routes, and dependency-version conflicts.

## Install

Carrick is currently invite-only while we ship a refreshed authentication flow. Once your org is provisioned, the install is three steps.

### 1. Sign in with GitHub on the Carrick dashboard.

The dashboard issues an API key for the GitHub Action and sets up the org-wide MCP authorisation.

### 2. Add the Carrick GitHub Action workflow to each TypeScript service in your org.

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

Add `CARRICK_API_KEY` to the repository's Actions secrets.

### 3. Connect your AI agent to the Carrick MCP server.

The recommended path is sign-in-with-Carrick. Your agent opens a browser, you click Approve once, and no API key changes hands. A manual key-paste path is available as a fallback.

```bash
claude mcp add --transport http carrick https://api.carrick.tools/mcp
```

## MCP tools

The MCP endpoint exposes Carrick's index as structured tools your agent can call directly.

| Tool | Purpose |
| :--- | :--- |
| `list_services` | Every service Carrick has indexed in your org |
| `list_function_intents` | One-line descriptions of exported functions, searchable by service |
| `get_api_endpoints` | Endpoints declared by a given service |
| `get_endpoint_types` | Resolved request and response types for a specific endpoint |
| `get_type_definition` | Fully resolved TypeScript type by name, across the org |
| `get_service_dependencies` | Services that call a given producer |
| `check_compatibility` | Whether service A's call to service B matches the producer's contract |

## Configuration

Add a `carrick.json` to each service's repository root to help classify outbound calls.

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
2. A static-analysis pass extracts function exports, mounted Express routers, and pattern-matched HTTP calls.
3. An LLM agent handles the cases pattern matching can't reach: dynamic URLs, factory functions, framework-specific routing.
4. A TypeScript sidecar resolves the request and response types against the actual TypeScript compiler.
5. The org index lives in DynamoDB and S3 and refreshes each time a service's main branch runs.

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
