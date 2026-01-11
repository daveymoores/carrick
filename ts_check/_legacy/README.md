# Legacy Extract Type Definitions Script (Archived)

This directory contains the archived entry point script for the legacy position-based type extraction system.

## File

- `extract-type-definitions.ts` - CLI script that was called from Rust's `analyzer/mod.rs`

## Usage (Deprecated)

```bash
ts-node extract-type-definitions.ts \
  '[{"filePath":"path/to/file.ts","startPosition":123,"compositeTypeString":"Response<User[]>","alias":"ResUsersGet"}]' \
  output.ts \
  tsconfig.json \
  dependencies.json
```

## Why Archived?

This script used position-based type extraction which has been replaced by the **TypeSidecar** architecture. The new approach:

1. Uses **symbol names** instead of positions
2. Leverages TypeScript's **type inference engine**
3. Produces flattened `.d.ts` bundles with all dependencies

## Migration

The Rust code in `src/analyzer/mod.rs` still calls this script via `extract_types_for_repo()`. This code path should be migrated to use the TypeSidecar (`src/sidecar/`) instead.

## Related Files

The library modules used by this script are archived in `ts_check/lib/_legacy/`:
- `type-extractor.ts`
- `type-processor.ts`
- `declaration-collector.ts`
- `import-handler.ts`
- `dependency-manager.ts`
- `output-generator.ts`
- `type-resolver.ts`

## See Also

- `src/sidecar/README.md` - Documentation for the new TypeSidecar
- `docs/research/compiler-sidecar-architecture/ARCHITECTURE.md` - Full architecture documentation