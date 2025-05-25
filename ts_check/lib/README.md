# Type Extractor Library

This library provides a modular TypeScript type extraction system that recursively collects type definitions and their dependencies from TypeScript projects.

## Architecture

The library is organized into several focused modules:

### Core Modules

- **`types.ts`** - Type definitions and interfaces used throughout the library
- **`constants.ts`** - Shared constants and utility functions
- **`config.ts`** - Configuration management with default settings
- **`logger.ts`** - Logging utilities with different log levels
- **`error-handler.ts`** - Centralized error handling and reporting

### Processing Modules

- **`argument-parser.ts`** - Command line argument parsing and validation
- **`project-utils.ts`** - TypeScript project management and source file handling
- **`type-resolver.ts`** - Type reference resolution and symbol lookup
- **`import-handler.ts`** - External type import management
- **`type-processor.ts`** - TypeNode processing and type analysis
- **`declaration-collector.ts`** - Declaration collection and recursive traversal
- **`dependency-manager.ts`** - Package dependency extraction and management
- **`output-generator.ts`** - Output file generation and formatting

### Main Orchestrator

- **`type-extractor.ts`** - Main class that coordinates all modules

## Usage

### Basic Usage

```typescript
import { TypeExtractor } from './lib/type-extractor.js';

const extractor = new TypeExtractor(tsconfigPath, dependencies);
const result = await extractor.extractTypes(typeInfos, outputFile);
```

### With Configuration

```typescript
import { TypeExtractor } from './lib/type-extractor.js';
import { ConfigManager } from './lib/config.js';

const config = new ConfigManager({
  maxPropertyDepth: 10,
  enableDebugMode: true
});

const extractor = new TypeExtractor(tsconfigPath, dependencies);
// Configure individual modules as needed
```

### Command Line Interface

The refactored version maintains compatibility with the original CLI:

```bash
./extract-type-definitions-refactored.ts '[typeInfos]' outputFile tsconfigPath dependencies
```

## Module Responsibilities

### TypeExtractor (Main Orchestrator)
- Coordinates all processing modules
- Manages the overall extraction workflow
- Handles cleanup and resource management

### ProjectManager
- Manages TypeScript project instances
- Handles source file caching
- Provides utilities for file operations

### ImportHandler
- Tracks external type imports
- Generates import statements
- Manages package name resolution

### DeclarationCollector
- Recursively collects type declarations
- Processes type references
- Manages composite type aliases

### TypeProcessor
- Processes TypeNode structures
- Handles complex type constructs (unions, intersections, etc.)
- Manages recursion depth and limits

### DependencyManager
- Extracts used dependencies from imports
- Generates package.json files
- Manages version resolution

### OutputGenerator
- Creates formatted output files
- Sorts declarations for readability
- Handles file writing and error reporting

## Configuration Options

The library supports extensive configuration through the `ConfigManager`:

```typescript
interface ExtractorConfig {
  maxPropertyDepth: number;           // Maximum depth for property traversal
  maxPropertiesLimit: number;         // Maximum properties to process per type
  maxRecursionDepth: number;          // Maximum recursion depth
  excludedModuleSpecifiers: Set<string>; // Modules to exclude from imports
  defaultOutputFile: string;          // Default output file path
  defaultPackageName: string;         // Default package name
  defaultPackageVersion: string;      // Default package version
  defaultPackageType: string;         // Default package type
  enableLogging: boolean;             // Enable/disable logging
  enableDebugMode: boolean;           // Enable/disable debug output
}
```

## Error Handling

The library includes comprehensive error handling:

- **ExtractorError** - Base error class for all extraction errors
- **FileNotFoundError** - File system related errors
- **TypeNotFoundError** - Type resolution errors
- **ParseError** - Parsing and validation errors
- **OutputError** - Output generation errors

## Logging

Structured logging with different levels:

- **ERROR** - Critical errors that prevent execution
- **WARN** - Non-critical issues that may affect output
- **INFO** - General information about processing
- **DEBUG** - Detailed debugging information
