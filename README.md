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

<!-- CARRICK_ISSUE_COUNT:22 -->
### 🪢 CARRICK: API Analysis Results

Analyzed **20 endpoints** and **13 API calls** across all repositories.

Found **22 total issues**: **2 critical mismatches**, **17 connectivity issues**, and **3 configuration suggestions**.

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

## Setup

Add to your GitHub workflow:

```yaml
- uses: davidjonathanmoores/carrick@v1
  with:
    carrick-org: your-org-name
    carrick-api-key: ${{ secrets.CARRICK_API_KEY }}
```

Run on `main` to analyze deployed code, and on PRs to catch divergence before merging.

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

## Beta testing

Looking for teams with Express microservices to test this. It's fast, low-effort to integrate, and should help catch bugs early across services.

API keys going out January 18th. Sign up at [carrick.tools](https://www.carrick.tools/) if interested.
