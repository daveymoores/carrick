# Per-Endpoint Resolved Types

## Problem

The MCP `get_endpoint_types` tool uses regex to extract type definitions from a monolithic `bundled_types` string. This is lossy:
- Misses transitive dependencies (e.g. `UserProfile` references `User`, `Order` — those aren't included)
- Can't preserve unions, enums, or complex types
- Agents get incomplete type information and can't write correct cross-service calls

## Solution

Pre-resolve types at CI time using the TypeScript compiler (ts-morph), not at query time in the MCP Lambda.

Each `TypeManifestEntry` gains two new fields:
- `definition` — the original declaration text as written (preserves named types for readability)
- `expanded` — the compiler-expanded form with all types fully inlined (guaranteed complete)

### Example

```json
{
  "type_alias": "UserProfile",
  "definition": "export interface UserProfile { user: User; orders: Order[]; comments: Comment[]; }",
  "expanded": "{ user: { id: number; name: string; email: string }; orders: { id: string; status: \"pending\" | \"shipped\" }[]; comments: { id: string; author: string; content: string }[] }"
}
```

## How it works

The sidecar already has ts-morph with the full TypeScript project loaded. After type bundling, for each manifest entry:

```typescript
const decl = sourceFile.getTypeAlias(alias) ?? sourceFile.getInterface(alias);
const definition = decl.getText();
const expanded = decl.getType().getText(decl, ts.TypeFormatFlags.NoTruncation);
```

The compiler does all the work. No graph walking, no regex, no manual dependency resolution.

### Deeply nested types

`getText()` with `NoTruncation` expands everything recursively. TypeScript handles recursive types by printing the alias name after a certain depth (e.g. `type Tree = { children: Tree[] }` doesn't expand infinitely). Worst case is a large string, not an infinite one.

## Data flow

```
BEFORE:
  Sidecar CI  -> bundled_types (one big .d.ts)
  Sidecar CI  -> type_manifest (alias lookup table)
  MCP query   -> regex extract alias from bundled_types -> return to agent (lossy)

AFTER:
  Sidecar CI  -> bundled_types (kept for backward compat)
  Sidecar CI  -> type_manifest with definition + expanded per entry
  MCP query   -> return definition + expanded directly (lossless)
```

## MCP tool changes

- `get_endpoint_types` returns `definition` + `expanded` per type, with a hint pointing to `get_type_definition`
- New `get_type_definition` tool — lookup a specific type alias by name, returns definition + expanded

### Agent workflow

1. `get_api_endpoints` — discover what endpoints exist
2. `get_endpoint_types` — get types for an endpoint (named form + expanded)
3. `get_type_definition` — drill into any specific type alias if needed

## Backward compatibility

- `resolved_definition` is `Option<String>` in Rust / `string | undefined` in TypeScript
- Old data without the field falls back to existing regex extraction
- `bundled_types` is kept for the `service-types` resource and as a fallback

## Design decisions

**Why not graph walking?** Earlier iterations attempted to walk AST nodes or type references to collect transitive dependencies. This is fragile — it's regex with extra steps. The TypeScript compiler already resolves the full type graph; we just ask it for the answer.

**Why both definition and expanded?** The named form (`definition`) is readable and compact — agents can understand the domain model. The expanded form gives the concrete types when needed. Returning both lets the agent choose what it needs without extra round trips.

**Why at CI time?** The sidecar has ts-morph with the full project loaded. The MCP Lambda doesn't have the compiler — it can only do string manipulation. Doing the work at CI time means the MCP is a pure serving layer.
