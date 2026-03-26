# Duplicate Function Detection — Implementation Plan

Detect functions with duplicate business logic across repos in an org. An agent queries Carrick's MCP server to find functions that may be duplicates of one they're writing or reviewing.

## Current State

**Already have:**
- `FunctionDefinition` extracted per file via SWC visitor (`src/visitor.rs:85-93`): name, file_path, node_type, arguments
- `ImportedSymbol` extraction: local name, imported name, source path, kind
- `CloudRepoData.function_definitions: HashMap<String, FunctionDefinition>` uploaded per repo
- Cross-repo data download via Lambda (`get-cross-repo-data` action)
- MCP server with 5 tools + 2 resources

**Missing:**
- Function bodies / source text not captured (AST nodes are stored but skipped during serialization)
- No export visibility tracking
- No function-level indexing in DynamoDB (functions are buried in the S3 blob)
- No similarity comparison logic
- No MCP tool for querying functions

---

## Implementation

### Step 1: Extend `FunctionDefinition` to capture source text

**File:** `src/visitor.rs`

Add a `body_source` field to `FunctionDefinition`:

```rust
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct FunctionDefinition {
    pub name: String,
    pub file_path: PathBuf,
    pub node_type: FunctionNodeType,
    pub arguments: Vec<FunctionArgument>,
    // New fields:
    pub body_source: Option<String>,    // Raw source text of function body
    pub is_exported: bool,              // Whether the function is exported
    pub line_number: usize,             // Start line for navigation
}
```

The SWC visitor already has access to the full AST nodes (`ArrowExpr`, `FnDecl`, `FnExpr`). Use the source map spans to extract the original source text. The `FunctionDefinitionExtractor` needs access to the `SourceMap` to call `span_to_snippet()`.

**Considerations:**
- Cap `body_source` at ~2000 chars to avoid bloating the payload (5MB Lambda limit)
- Only capture exported functions to reduce noise — most duplicate logic lives in exported utilities
- Bump `CACHE_VERSION` to trigger re-analysis

### Step 2: Add `function_definitions` to the MCP types

**File:** `mcp-server/src/types.ts`

```typescript
export interface FunctionDefinition {
  name: string;
  file_path: string;
  node_type: string;
  arguments: FunctionArgument[];
  body_source?: string;
  is_exported: boolean;
  line_number: number;
}

export interface FunctionArgument {
  name: string;
}
```

Update `CloudRepoData.function_definitions` from `Record<string, unknown>` to `Record<string, FunctionDefinition>`.

### Step 3: New MCP tool — `find_similar_functions`

**File:** `mcp-server/src/tools/find-similar.ts`

```typescript
// Tool registration in server.ts:
server.tool(
  "find_similar_functions",
  "Find functions across all org services that may contain duplicate or similar business logic to a given function. Useful for identifying shared logic that could be extracted to a common package.",
  {
    function_source: z.string().describe("The source code of the function to find duplicates of"),
    function_name: z.string().optional().describe("Name of the function (helps narrow candidates)"),
    exclude_service: z.string().optional().describe("Service to exclude from results (typically the caller's own service)"),
    min_similarity: z.number().optional().describe("Minimum similarity threshold 0-1 (default 0.7)"),
  },
  async (params) => findSimilarFunctions(client, params),
);
```

**Implementation approach (demo-viable, no new infra):**

1. Download all repos' `CloudRepoData` via existing `get-cross-repo-data`
2. Collect all exported `FunctionDefinition` entries with `body_source`
3. **Pre-filter** candidates:
   - Skip tiny functions (< 3 lines)
   - Skip functions from `exclude_service`
   - Optional: fuzzy match on function name
4. **LLM similarity check** (Gemini Flash via existing integration or direct API call):
   - Batch candidates into groups
   - Prompt: "Rate similarity 0-1 between the query function and each candidate. Focus on business logic, not variable names or formatting."
   - Return matches above threshold
5. Return results with: service name, file path, line number, function name, similarity score, and a brief explanation

**Why LLM over embeddings for now:**
- No new infra (no vector DB)
- Gemini Flash is cheap and fast enough for demo-scale (< 100 repos)
- Better at semantic similarity ("these both validate email addresses") vs syntactic similarity
- Can explain *why* functions are similar, which is more compelling in a demo

### Step 4: Wire up and test

- Register the new tool in `mcp-server/src/server.ts`
- Test against `optaxe-ts-monorepo` (the target demo repo)
- Ensure the payload stays under 5MB with `body_source` included — if it doesn't, strip `body_source` from the S3 upload and only keep it in the local analysis cache, fetching source on-demand

---

## Demo Script Sketch

1. Agent is writing a new utility function (e.g., a date formatter, a validation helper)
2. Agent calls `find_similar_functions` with the function source
3. Carrick returns: "Found 2 similar functions across your org"
   - `formatISODate` in `billing-service/src/utils/dates.ts:42` (similarity: 0.89)
   - `toISOString` in `user-service/src/helpers/format.ts:17` (similarity: 0.74)
4. Agent suggests: "Consider importing from `billing-service` or extracting to a shared package"

---

## Estimated Effort

| Step | Effort | Notes |
|------|--------|-------|
| 1. Extend FunctionDefinition | ~0.5 day | SWC visitor changes, source text extraction |
| 2. MCP types update | ~1 hour | TypeScript interface changes |
| 3. find_similar_functions tool | ~1-1.5 days | Main work — API client, LLM prompt, result formatting |
| 4. Testing & demo prep | ~0.5 day | End-to-end with optaxe-ts-monorepo |
| **Total** | **~2-3 days** | |

## Future (post-demo)

- **Embeddings:** Generate function embeddings at analysis time, store in DynamoDB or a vector index for sub-second queries without LLM calls
- **DynamoDB GSI:** Add a `function_name` GSI for fast cross-repo function lookups
- **Structural hashing:** Normalize AST (strip names, whitespace) and hash for exact/near-exact duplicate detection without LLM
- **Shared package suggestions:** If duplicates found, auto-generate a shared package scaffold
