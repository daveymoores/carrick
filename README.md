![Carrick Social Image](https://cdn.prod.website-files.com/685162a038275750f4f698e3/686cee204d48f5406664086d_social-image_1.png)

# Carrick 🪢

A GitHub Action that checks API producers and consumers across repositories to catch mismatches in CI.

Rather than contract testing, Carrick uses SWC to extract routes from Express apps and mounted routers to find producers, then extracts async call code and sends it to an LLM to find consumers. It extracts request/response types from both sides and runs a minimal TypeScript compiler pass to surface mismatches between services.

**Looking for beta testers with Express microservices. API keys going out January 18th - sign up at [carrick.tools](https://www.carrick.tools/)**

## How it works

1. **Extract producers**: Uses SWC to parse Express apps and extract route definitions from mounted routers
2. **Extract consumers**: Finds async function calls and sends them to Gemini 2.5 Flash for intelligent extraction of HTTP calls
3. **Type analysis**: Extracts TypeScript types from both producers and consumers
4. **Cross-repo analysis**: Shares data across repositories via cloud storage (DynamoDB + S3)
5. **Mismatch detection**: Runs TypeScript compiler with extracted types to find incompatibilities

Catches issues like type mismatches, method conflicts, missing endpoints, and orphaned routes.

## Example

**Producer** (Express service):
```typescript
app.get("/users/:id", (req, res) => {
  res.json({ id: 1, name: "Alice" });
});
```

**Consumer** (Client service):
```typescript
interface User {
  id: number;
  name: string;
  role: string;
}

async function getUser(id: string): Promise<User> {
  const response = await fetch(`${API_URL}/users/${id}`);
  return response.json();
}
```

**Result:**
```
Type compatibility issue: GET /users/:id
Producer: { id: number; name: string; }
Consumer: User
Error: Property 'role' is missing in producer type
```

## GitHub Action Output

When Carrick runs in your CI, it produces detailed reports like this (click sections to expand):

---


<!-- CARRICK_ISSUE_COUNT:24 -->
### 🪢 CARRICK: API Analysis Results

Analyzed **20 endpoints** and **13 API calls** across all repositories.

Found **24 total issues**: **2 critical mismatches**, **17 connectivity issues**, **2 dependency conflicts**, and **3 configuration suggestions**.

<br>

<details>
<summary>
<strong style="font-size: 1.1em;">2 Critical: API Mismatches</strong>
</summary>

> These issues indicate a direct conflict between the API consumer and producer and should be addressed first.

#### Type Compatibility Issue: `GET /users/:id`

Type compatibility issue detected.

  - **Endpoint:** `GET /users/:id`
  - **Producer Type:** `{ commentsByUser: repo-a-types.Comment[]; }`
  - **Consumer Type:** `repo-b-types.User`
  - **Error:** { commentsByUser: Comment[]; } missing properties from User: id, name, role

#### Method Mismatch

Issue details: Method mismatch: GET ENV_VAR:ORDER_SERVICE_URL:/orders is called but endpoint only supports POST
</details>
<hr>

<details>
<summary>
<strong style="font-size: 1.1em;">2 Dependency Conflicts</strong>
</summary>

> These packages have different versions across repositories, which could cause compatibility issues.

### Critical Conflicts (1) - Major Version Differences

> These conflicts involve major version differences that could cause breaking changes.

#### express

| Repository | Version | Source |
| :--- | :--- | :--- |
| `user-service` | `4.18.0` | `package.json` |
| `comment-service` | `3.4.8` | `package.json` |

### Warning Conflicts (1) - Minor Version Differences

> These conflicts involve minor version differences that may cause compatibility issues.

#### @types/node

| Repository | Version | Source |
| :--- | :--- | :--- |
| `user-service` | `18.15.0` | `package.json` |
| `comment-service` | `18.11.9` | `package.json` |

</details>
<hr>

<details>
<summary>
<strong style="font-size: 1.1em;">17 Connectivity Issues</strong>
</summary>

> These endpoints are either defined but never used (orphaned) or called but never defined (missing). This could be dead code or a misconfigured route.

#### 2 Missing Endpoints

| Method | Path |
| :--- | :--- |
| `GET` | `ENV_VAR:ORDER_SERVICE_URL:/route-does-not-exist` |
| `GET` | `/not-found` |

<br>

#### 15 Orphaned Endpoints

| Method | Path |
| :--- | :--- |
| `GET` | `/api/orders` |
| `GET` | `/api/orders/:id/comments` |
| `GET` | `/users` |
| `GET` | `/api/comments` |
| `GET` | `/posts/:postId` |
| `GET` | `/events/:eventId/register` |
| `GET` | `/api/potatoes` |
| `GET` | `/admin/stats` |
| `GET` | `/dynamic` |
| `GET` | `/api/profiles` |
| `GET` | `/users/:id/profile` |
| `GET` | `/api/v1/stats` |
| `POST` | `/api/comments` |
| `GET` | `/api/comments/:id` |
| `POST` | `/api/v1/chat` |
</details>
<hr>

<details>
<summary>
<strong style="font-size: 1.1em;">3 Configuration Suggestions</strong>
</summary>

> These API calls use environment variables to construct the URL. To enable full analysis, consider adding them to your tool's external API configuration.

  - `GET` using **[COMMENT_SERVICE_URL]** in `/api/comments`
  - `GET` using **[COMMENT_SERVICE_URL]** in `/comments`
</details>
<!-- CARRICK_OUTPUT_END -->

---

## Setup

Add to your GitHub workflow (`.github/workflows/carrick.yml`) for each Express service in your microservice architecture:

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
  carrick-analysis:
    name: Carrick Analysis
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4

      - name: Run Carrick Analysis
        id: carrick-analysis
        uses: daveymoores/carrick@v1
        with:
          carrick-org: your-org-name
          carrick-api-key: ${{ secrets.CARRICK_API_KEY }}

      - name: Comment PR with Results
        if: github.event_name == 'pull_request'
        uses: actions/github-script@v7
        env:
          COMMENT_BODY: ${{ steps.carrick-analysis.outputs.pr-comment }}
        with:
          script: |
            const comment = process.env.COMMENT_BODY;

            if (comment && comment.trim() !== '') {
              github.rest.issues.createComment({
                issue_number: context.issue.number,
                owner: context.repo.owner,
                repo: context.repo.repo,
                body: comment
              });
            } else {
              console.log('No comment content to post');
            }
```

### Requirements
- API key from [carrick.tools](https://carrick.tools)
- Add `CARRICK_API_KEY` to your repository secrets
- Replace `your-org-name` with your GitHub organization name

### How it works
Runs analysis on both main branch deployments and pull requests. On main, shares your repository's API metadata with other repositories in your organization for cross-service analysis. On PRs, automatically posts detailed analysis results as comments showing type mismatches, missing endpoints, and orphaned routes across all repositories in your organization.

## Technical details

**Producer extraction**: Uses SWC parser to walk ASTs and extract Express route definitions, including mounted routers and imported handlers.

**Consumer extraction**: Pattern matching finds basic fetch/axios calls. For complex cases (template literals, dynamic URLs), sends function source to Gemini 2.5 Flash for intelligent extraction.

**Type analysis**: Extracts TypeScript interface definitions and runs targeted compiler passes using only the relevant types to check compatibility.

**Cross-repository**: Stores extracted data in DynamoDB with type files in S3. Each repository downloads data from others in the same organization for analysis.

## Configuration

Create a `carrick.json` to help classify your API calls:

```json
{
  "internalEnvVars": ["API_URL", "SERVICE_URL"],
  "externalEnvVars": ["STRIPE_API", "GITHUB_API"],
  "internalDomains": ["api.yourcompany.com"],
  "externalDomains": ["api.stripe.com", "api.github.com"]
}
```

## What Carrick Catches

- Mismatched types
- Incorrect package versions
- Wrong HTTP verbs
- Missing or deprecated endpoints

## Join the Private Beta

We're now inviting developers to join our private beta. Here's how to get started:

1. Go to [carrick.tools](https://carrick.tools) and sign up for the beta
2. We'll send you your personal API key as we onboard new users
3. Once you have your key, follow the setup guide above to add the Carrick GitHub Action to your workflows

As an early user, your feedback will be invaluable in shaping the future of the product. We're excited to build it with you.
