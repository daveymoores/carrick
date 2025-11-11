# WARP.md

This file provides guidance to WARP (warp.dev) when working with code in this repository.

## Project Overview

Carrick is a GitHub Action that analyzes API producers and consumers across repositories to catch mismatches in CI. It uses SWC (Speedy Web Compiler) to parse JavaScript/TypeScript files, extracts Express routes and API calls, and uses LLM analysis to detect cross-service incompatibilities.

## Development Commands

### Building and Running
```bash
# Build the project (development)
cargo build

# Build for release
cargo build --release

# Run the CLI tool locally with current directory
cargo run

# Run with specific repository path
cargo run -- /path/to/repo

# Run with mock storage (for testing without AWS)
CARRICK_MOCK_ALL=1 CARRICK_ORG=test-org cargo run -- test-repo
```

### Testing
```bash
# Run all tests
cargo test

# Run integration tests only
cargo test --test integration_test

# Run unit tests only
cargo test --lib

# Run a specific test
cargo test test_imported_router_endpoint_resolution

# Run tests with output
cargo test -- --nocapture
```

### Other Common Commands
```bash
# Check code without building
cargo check

# Format code
cargo fmt

# Run clippy for linting
cargo clippy

# Clean build artifacts
cargo clean
```

## Architecture Overview

Carrick follows a modular architecture with clear separation of concerns:

### Core Components

**Engine** (`src/engine/mod.rs`): The main orchestration layer that:
- Determines if running in CI or local mode
- Coordinates analysis across multiple repositories
- Handles cloud storage upload/download for cross-repo data sharing
- Manages the overall analysis workflow

**Parser** (`src/parser.rs`): Uses SWC to parse JavaScript/TypeScript files into ASTs for analysis.

**Visitor** (`src/visitor.rs`): Implements the visitor pattern to traverse ASTs and extract:
- Express route definitions
- Function calls (fetch, axios, etc.)
- Import/export relationships
- Type information

**Analyzer** (`src/analyzer/`): Core analysis engine that:
- Processes extracted data into structured endpoints and calls
- Performs cross-repository compatibility checking
- Runs TypeScript type checking on extracted types
- Detects mismatches, missing endpoints, and dependency conflicts

**Type Checker (`ts_check/`)**: A collection of TypeScript scripts that provide the core type analysis capabilities.
- **`extract-type-definitions.ts`**: Called by the Rust engine, it uses `ts-morph` to find a given type in the codebase and recursively collect all its dependent type definitions into a single, self-contained `.ts` file.
- **`run-type-checking.ts`**: Called at the end of a cross-repo analysis. It runs `npm install` on a staged project containing all type signatures from all repos, then uses the TypeScript compiler to verify that producer and consumer types for each API endpoint are compatible.

**Extractor** (`src/extractor.rs`): Contains traits and utilities for extracting structured data from AST nodes.

**Cloud Storage** (`src/cloud_storage.rs`): Handles AWS S3/DynamoDB integration for sharing data between repositories in the same organization.

**Gemini Service** (`src/gemini_service.rs`): Integrates with Google's Gemini 2.5 Flash for intelligent extraction of complex API calls that pattern matching can't handle.

### Data Flow

1. **File Discovery**: Find JavaScript/TypeScript files in the repository
2. **Parsing**: Use SWC to parse files into ASTs
3. **Visitor Pattern**: Extract routes, calls, and type information
4. **Local Analysis**: Build endpoints and API calls for current repository
5. **Cloud Sync**: Upload/download data to/from cloud storage for cross-repo analysis
6. **Type Extraction**: Generate TypeScript type files for compatibility checking
7. **Analysis**: Compare producers vs consumers across all repositories
8. **Results**: Generate formatted reports with mismatches and issues

### Key Data Structures

- `ApiEndpointDetails`: Represents both API endpoints (producers) and API calls (consumers)
- `CloudRepoData`: Serializable repository data for cross-repo sharing
- `DependencyConflict`: Version conflicts between packages across repositories
- `ApiIssues`: Collection of all detected issues (mismatches, missing endpoints, etc.)

## Environment Variables

- `CARRICK_ORG`: Organization name for grouping repositories (required in CI)
- `CARRICK_API_KEY`: API key for cloud storage access (required in CI)
- `CARRICK_MOCK_ALL`: Use mock storage instead of AWS (for local testing)
- `GITHUB_EVENT_NAME`: Determines upload behavior (no upload on pull_request)
- `GITHUB_REF`: Branch information for determining when to upload data

## Test Structure

The project includes integration tests that verify end-to-end functionality:

- `tests/integration_test.rs`: Tests full analysis pipeline with fixture projects
- `tests/dependency_analysis_test.rs`: Tests dependency conflict detection
- Test fixtures in `tests/fixtures/` directory simulate real project structures

Tests use `CARRICK_MOCK_ALL=1` to avoid requiring AWS credentials during development.

## Configuration Files

Projects can include a `carrick.json` configuration file to help classify API calls:
```json
{
  "internalEnvVars": ["API_URL", "SERVICE_URL"],
  "externalEnvVars": ["STRIPE_API", "GITHUB_API"],
  "internalDomains": ["api.yourcompany.com"],
  "externalDomains": ["api.stripe.com", "api.github.com"]
}
```

## Key Dependencies

- **SWC**: Fast JavaScript/TypeScript parser and AST manipulation
- **Tokio**: Async runtime for concurrent processing
- **serde/serde_json**: Serialization for data exchange
- **reqwest**: HTTP client for Gemini API integration
- **matchit**: Fast HTTP route matching for endpoint resolution

## Safety Rules

- **NEVER run Terraform deployment commands** such as `terraform apply`, `terraform destroy`, or any command that modifies infrastructure. Only read-only commands like `terraform plan`, `terraform validate`, or `terraform show` are allowed.
