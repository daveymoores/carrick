# Legacy Type Extraction Code (Archived)

This directory contains archived type extraction code that has been superseded by the **TypeSidecar** architecture (`src/sidecar/`).

## Why Archived?

The legacy type extraction system used a **position-based approach**:
1. The LLM would identify type positions (line numbers + character offsets)
2. `ts-morph` would then attempt to find types at those positions
3. Dependencies would be recursively collected

This approach was error-prone because:
- Character positions from LLMs are often inaccurate
- Multi-line signatures caused position mismatches
- The system couldn't handle implicit types (no annotations)

## New Architecture: TypeSidecar

The new **compiler sidecar** architecture (`src/sidecar/`) uses a fundamentally different approach:
1. The LLM identifies **type symbols** (e.g., `User`, `Response<Order[]>`)
2. The sidecar uses TypeScript's compiler to resolve and bundle types
3. **Type inference** extracts types even when no annotations exist

This provides:
- ✅ More accurate type resolution
- ✅ Implicit type extraction via TypeScript's inference engine
- ✅ Flattened `.d.ts` output with all dependencies bundled
- ✅ Framework-agnostic approach (works with Express, Fastify, Hono, etc.)

## Files in This Directory

| File | Description |
|------|-------------|
| `type-extractor.ts` | Main orchestrator for position-based extraction |
| `type-processor.ts` | Processes TypeScript type nodes |
| `declaration-collector.ts` | Recursively collects type declarations |
| `import-handler.ts` | Manages external type imports |
| `dependency-manager.ts` | Tracks package dependencies |
| `output-generator.ts` | Generates the output `.ts` file |
| `type-resolver.ts` | Finds type references at specific positions |

## Entry Point (Also Archived)

The script `ts_check/_legacy/extract-type-definitions.ts` was the CLI entry point for this system. It's called from Rust's `analyzer/mod.rs` but is being phased out.

## Backward Compatibility

These modules are still exported from `ts_check/lib/index.ts` for backward compatibility, but they are **deprecated**. New code should use the TypeSidecar instead.

## Migration Timeline

- **Phase 1-3**: TypeSidecar implemented and integrated ✅
- **Phase 4**: Legacy code archived (this step) ✅
- **Future**: Remove legacy code entirely once all callers migrated

## See Also

- `src/sidecar/README.md` - Documentation for the new TypeSidecar
- `docs/research/compiler-sidecar-architecture/ARCHITECTURE.md` - Full architecture documentation
- `docs/research/compiler-sidecar-architecture/IMPLEMENTATION_PLAN.md` - Migration plan