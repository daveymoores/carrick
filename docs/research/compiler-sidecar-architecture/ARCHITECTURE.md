# Compiler Sidecar Architecture for Type Extraction

**Status:** Proposed  
**Author:** Carrick Team  
**Created:** 2025-01  
**Replaces:** Position-based type extraction via SWC + ts_check TypeExtractor

## Executive Summary

This document outlines a new architecture for type extraction in Carrick that replaces the current error-prone position-based approach with a robust "Compiler Sidecar" pattern. The sidecar leverages the actual TypeScript compiler (via `ts-morph` and `dts-bundle-generator`) to produce accurate, flattened type definitions.

### Core Design Principles

1. **REST-Based, Framework/Library Agnostic** - Works with any TypeScript HTTP server or client (Express, Fastify, Hono, tRPC, custom implementations)
2. **Parallel Startup** - Sidecar spawns immediately at CLI start; SWC scanning and LLM analysis proceed in parallel while TypeScript compiler initializes (~500ms)
3. **Implicit Type Inference** - Leverages TypeScript's type inference engine to extract types even when no explicit annotations exist
4. **CI-First** - Designed for fast, deterministic execution in CI pipelines

### Why This Change?

The current type extraction pipeline suffers from several fundamental issues:

1. **Fragile Position-Based Lookup**: The LLM provides line numbers, but type annotations often span multiple lines or are on different lines than the endpoint definition
2. **SWC Limitations**: SWC's visitor pattern for finding type positions is complex and error-prone
3. **Alias Naming Convention Dependencies**: The type checker relies on parsing alias names to match producers/consumers
4. **Incomplete Type Resolution**: The current system doesn't reliably follow type dependencies (imports, generics, intersections)
5. **No Implicit Type Support**: When developers don't write explicit type annotations, the current system returns nothing

The Compiler Sidecar approach solves these by:
- Using the TypeScript compiler for accurate type resolution **and inference**
- Generating flattened `.d.ts` files with all dependencies included
- Eliminating the need for position-based lookups
- Decoupling type extraction from alias naming conventions
- Extracting inferred types that only the TypeScript compiler can determine

## Architecture Overview

### Parallel Startup Timeline

```
Time ──────────────────────────────────────────────────────────────────────►

CLI Start
    │
    ├──► Spawn Sidecar (async) ──────────────────────────────────────────┐
    │         │                                                          │
    │         ├──► npm/node startup (~100ms)                            │
    │         ├──► ts-morph Project init (~300-500ms)                   │
    │         └──► Ready signal ─────────────────────────────────────────┤
    │                                                                    │
    ├──► SWC Scanning (parallel) ──────────────────────────────────────┐ │
    │         │                                                        │ │
    │         └──► Find candidate files (~50-100ms for 1000 files)     │ │
    │                                                                  │ │
    ├──► LLM Analysis (parallel, after SWC) ───────────────────────────┤ │
    │         │                                                        │ │
    │         └──► Gemini extracts endpoints/calls (~2-5s)             │ │
    │                                                                  │ │
    └──► Type Resolution (after LLM + sidecar ready) ──────────────────┴─┤
              │                                                          │
              └──► Sidecar queries (~50ms each) ─────────────────────────┘
                                                                         │
                                                                         ▼
                                                                    S3 Upload
```

### Component Architecture

```
┌─────────────────────────────────────────────────────────────────────────────┐
│                              RUST CLI                                        │
│                                                                              │
│  ┌─────────────────┐                           ┌─────────────────────┐      │
│  │ main.rs         │──spawn immediately───────►│ TypeSidecar Client  │      │
│  │ (CLI entry)     │                           │ (Rust struct)       │      │
│  └────────┬────────┘                           └──────────┬──────────┘      │
│           │                                               │                  │
│           ▼                                               │ JSON/stdio       │
│  ┌─────────────────┐    ┌───────────────────┐            │                  │
│  │ SWC Scanner     │───►│ FileOrchestrator  │◄───────────┘                  │
│  │ (AST Gatekeeper)│    │ (Gemini 3.0)      │                               │
│  └─────────────────┘    └───────────────────┘                               │
│                                                                              │
└─────────────────────────────────────────────────────────────────────────────┘
                                      │
                                      ▼
┌─────────────────────────────────────────────────────────────────────────────┐
│                     NODE.JS SIDECAR PROCESS                                  │
│                                                                              │
│  ┌──────────────────┐   ┌──────────────────┐   ┌─────────────────────────┐ │
│  │ ts-morph Project │   │ Type Inference   │   │ dts-bundle-generator    │ │
│  │ (loaded once)    │──►│ Engine           │──►│ (flattens + bundles)    │ │
│  │                  │   │ (explicit +      │   │                         │ │
│  │                  │   │  implicit types) │   │                         │ │
│  └──────────────────┘   └──────────────────┘   └───────────┬─────────────┘ │
│                                                             │               │
│                                                 ┌───────────▼───────────┐   │
│                                                 │ Flattened .d.ts      │   │
│                                                 │ (all types bundled)  │   │
│                                                 └───────────────────────┘   │
└─────────────────────────────────────────────────────────────────────────────┘
                                      │
                                      ▼
                              ┌───────────────┐
                              │ S3 Upload     │
                              │ (types.d.ts)  │
                              └───────────────┘
```

## Current vs. Proposed Flow Comparison

### Current Flow (Problematic)

```
1. Gemini analyzes file → returns line_number + response_type_string
2. SWC scans file → searches ±10 lines for type annotation → returns position
3. TypeExtractor (ts_check) → uses position to find type in AST
4. DeclarationCollector → traverses type dependencies recursively
5. OutputGenerator → writes collected declarations + composite aliases
6. TypeChecker → parses alias names to match producers/consumers
```

**Problems:**
- Step 2 frequently fails (multi-line signatures, generic types, arrow functions)
- Step 4 can miss dependencies or over-collect
- Step 6 depends on fragile alias naming conventions
- **No implicit types** - If the developer didn't write `: Response<User>`, we get nothing

### Proposed Flow (Compiler Sidecar)

```
0. CLI starts → spawns sidecar IMMEDIATELY (non-blocking)
   Sidecar begins TypeScript project initialization in background

1. SWC scans files (parallel with sidecar init)
   → finds candidate files with potential HTTP patterns

2. Gemini analyzes files (parallel with sidecar init)
   → returns for each endpoint/call:
      - path, method
      - handler_name or call expression
      - file_path + line_number (for location-based inference)

3. Sidecar ready signal received
   → Rust sends batch of type queries:
      - Option A: Explicit type at location (file:line)
      - Option B: Infer return type of function at location
      - Option C: Infer type of expression at location

4. Sidecar resolves types (explicit OR inferred)
   → Uses TypeScript compiler's type inference engine
   → Bundles all dependencies into flat .d.ts

5. Rust uploads bundled types + manifest to S3

6. TypeChecker uses manifest-based matching (not alias name parsing)
```

### Key Improvement: Implicit Type Inference

The TypeScript compiler can infer types even when not explicitly annotated:

```typescript
// No explicit type annotation - current system fails
app.get('/users', async (req, res) => {
  const users = await db.query('SELECT * FROM users');
  res.json(users);  // TypeScript KNOWS this returns User[]
});

// Sidecar can ask: "What is the inferred return type of the handler at line 5?"
// TypeScript answers: Promise<void> with res.json() receiving User[]
```

## Detailed Component Design

### Phase 1: Node.js Sidecar (`src/sidecar/`)

#### Directory Structure

```
carrick/
├── src/
│   └── sidecar/
│       ├── package.json
│       ├── tsconfig.json
│       ├── src/
│       │   ├── index.ts           # Entry point, stdio message loop
│       │   ├── project-loader.ts  # ts-morph project initialization
│       │   ├── type-inferrer.ts   # NEW: Implicit type inference engine
│       │   ├── bundler.ts         # dts-bundle-generator wrapper
│       │   ├── types.ts           # Shared type definitions
│       │   └── validators.ts      # Zod schemas for message validation
│       └── dist/                  # Compiled JavaScript
```

#### Message Protocol

**Request Schema:**

```typescript
interface SidecarRequest {
  action: 'init' | 'bundle' | 'infer' | 'health' | 'shutdown';
  request_id: string;
  
  // For 'init' action
  repo_root?: string;
  tsconfig_path?: string;
  
  // For 'bundle' action - explicit type extraction
  symbols?: SymbolRequest[];
  
  // For 'infer' action - implicit type inference
  infer_requests?: InferRequest[];
}

interface SymbolRequest {
  symbol_name: string;      // e.g., "User", "Order[]", "Response<User>"
  source_file: string;      // e.g., "./types/user.ts" (relative to repo root)
  alias?: string;           // Optional alias for the output
}

// NOTE: source_file paths are relative to repo_root. The sidecar resolves them
// correctly because the virtual entry file is written to the repo root directory.

// NEW: Request to infer types at specific code locations
interface InferRequest {
  file_path: string;        // e.g., "./routes/users.ts"
  line_number: number;      // 1-based line number
  infer_kind: InferKind;    // What to infer
  alias?: string;           // Alias for the inferred type
}

type InferKind = 
  | 'function_return'       // Infer return type of function at line
  | 'expression'            // Infer type of expression at line
  | 'call_result'           // Infer return type of call expression
  | 'variable'              // Infer type of variable declaration
  | 'response_body';        // Special: find res.json()/res.send() and infer argument type
```

**Response Schema:**

```typescript
interface SidecarResponse {
  request_id: string;
  status: 'success' | 'error' | 'partial' | 'ready' | 'not_ready';
  
  // For 'health' action - reports initialization status
  initialized?: boolean;
  init_time_ms?: number;
  
  // For successful bundle/infer
  dts_content?: string;           // The flattened .d.ts file content
  symbols_resolved?: string[];    // Which symbols were successfully resolved
  inferred_types?: InferredType[]; // Results of type inference requests
  
  // For errors
  error?: string;
  symbols_failed?: Array<{
    symbol: string;
    reason: string;
  }>;
}

// NEW: Result of type inference
interface InferredType {
  alias: string;                  // The requested alias
  type_string: string;            // e.g., "User[]", "{ id: string; name: string }"
  is_explicit: boolean;           // Was this an explicit annotation or inferred?
  source_location: string;        // Where we found it (for debugging)
}
```

#### Core Implementation

```typescript
// src/sidecar/src/index.ts
import { Project, SourceFile, Node, Type } from 'ts-morph';
import { generateDtsBundle } from 'dts-bundle-generator';
import * as readline from 'readline';
import * as fs from 'fs';
import * as path from 'path';
import { z } from 'zod';
import { RequestSchema, type SidecarRequest, type SidecarResponse, type InferRequest, type InferredType } from './types';

let project: Project | null = null;
let repoRoot: string | null = null;  // Store repo root for physical file placement
let initStartTime: number = 0;
let initEndTime: number = 0;

async function handleMessage(request: SidecarRequest): Promise<SidecarResponse> {
  switch (request.action) {
    case 'init':
      return initProject(request);
    case 'health':
      return healthCheck(request);
    case 'bundle':
      return bundleTypes(request);
    case 'infer':
      return inferTypes(request);
    case 'shutdown':
      process.exit(0);
    default:
      return { request_id: request.request_id, status: 'error', error: 'Unknown action' };
  }
}

function initProject(request: SidecarRequest): SidecarResponse {
  initStartTime = Date.now();
  try {
    // Store repo root for later use (physical file placement)
    repoRoot = request.repo_root || process.cwd();
    
    project = new Project({
      tsConfigFilePath: request.tsconfig_path,
      skipAddingFilesFromTsConfig: false,
    });
    initEndTime = Date.now();
    
    // Send ready signal immediately after init
    return { 
      request_id: request.request_id, 
      status: 'ready',
      initialized: true,
      init_time_ms: initEndTime - initStartTime,
    };
  } catch (e) {
    return { request_id: request.request_id, status: 'error', error: String(e) };
  }
}

function healthCheck(request: SidecarRequest): SidecarResponse {
  return {
    request_id: request.request_id,
    status: project ? 'ready' : 'not_ready',
    initialized: project !== null,
    init_time_ms: initEndTime > 0 ? initEndTime - initStartTime : undefined,
  };
}

// NEW: Infer types at specific code locations
async function inferTypes(request: SidecarRequest): Promise<SidecarResponse> {
  if (!project) {
    return { request_id: request.request_id, status: 'error', error: 'Project not initialized' };
  }

  const inferred: InferredType[] = [];
  const failed: Array<{ symbol: string; reason: string }> = [];

  for (const req of request.infer_requests || []) {
    try {
      const result = inferTypeAtLocation(project, req);
      if (result) {
        inferred.push(result);
      } else {
        failed.push({ symbol: req.alias || `${req.file_path}:${req.line_number}`, reason: 'Could not infer type' });
      }
    } catch (e) {
      failed.push({ symbol: req.alias || `${req.file_path}:${req.line_number}`, reason: String(e) });
    }
  }

  // Bundle all inferred types into a .d.ts
  const dtsContent = generateDtsFromInferred(inferred);

  return {
    request_id: request.request_id,
    status: failed.length === 0 ? 'success' : (inferred.length > 0 ? 'partial' : 'error'),
    dts_content: dtsContent,
    inferred_types: inferred,
    symbols_failed: failed.length > 0 ? failed : undefined,
  };
}

function inferTypeAtLocation(project: Project, req: InferRequest): InferredType | null {
  const sourceFile = project.getSourceFile(req.file_path);
  if (!sourceFile) return null;

  // Find the node at the specified line
  const line = req.line_number;
  
  switch (req.infer_kind) {
    case 'function_return':
      return inferFunctionReturn(sourceFile, line, req.alias);
    case 'expression':
      return inferExpression(sourceFile, line, req.alias);
    case 'call_result':
      return inferCallResult(sourceFile, line, req.alias);
    case 'variable':
      return inferVariable(sourceFile, line, req.alias);
    case 'response_body':
      return inferResponseBody(sourceFile, line, req.alias);
    default:
      return null;
  }
}

function inferFunctionReturn(sf: SourceFile, line: number, alias?: string): InferredType | null {
  // Find function/arrow function at or near this line
  const func = sf.getDescendants().find(node => {
    const nodeLine = node.getStartLineNumber();
    return (nodeLine === line || nodeLine === line - 1 || nodeLine === line + 1) &&
           (Node.isFunctionDeclaration(node) || Node.isArrowFunction(node) || Node.isMethodDeclaration(node));
  });

  if (!func) return null;

  // Get the return type - TypeScript will infer if not explicit
  const signature = func.getType().getCallSignatures()[0];
  if (!signature) return null;

  const returnType = signature.getReturnType();
  const typeString = returnType.getText();
  
  // Check if it was explicitly annotated
  let isExplicit = false;
  if (Node.isFunctionDeclaration(func) || Node.isMethodDeclaration(func)) {
    isExplicit = func.getReturnTypeNode() !== undefined;
  } else if (Node.isArrowFunction(func)) {
    isExplicit = func.getReturnTypeNode() !== undefined;
  }

  return {
    alias: alias || `InferredReturn_L${line}`,
    type_string: typeString,
    is_explicit: isExplicit,
    source_location: `${sf.getFilePath()}:${line}`,
  };
}

function inferResponseBody(sf: SourceFile, line: number, alias?: string): InferredType | null {
  // Find res.json() or res.send() calls near this line and infer their argument type
  // This is framework-agnostic - we look for .json() or .send() method calls
  const calls = sf.getDescendants().filter(node => {
    if (!Node.isCallExpression(node)) return false;
    const expr = node.getExpression();
    if (!Node.isPropertyAccessExpression(expr)) return false;
    const methodName = expr.getName();
    return (methodName === 'json' || methodName === 'send') &&
           Math.abs(node.getStartLineNumber() - line) <= 10;
  });

  if (calls.length === 0) return null;

  // Get the first argument's type
  const call = calls[0] as any;
  const args = call.getArguments();
  if (args.length === 0) return null;

  const argType = args[0].getType();
  const typeString = argType.getText();

  return {
    alias: alias || `ResponseBody_L${line}`,
    type_string: typeString,
    is_explicit: false, // Response body types are always inferred from usage
    source_location: `${sf.getFilePath()}:${call.getStartLineNumber()}`,
  };
}

// ... similar implementations for inferExpression, inferCallResult, inferVariable ...

async function bundleTypes(request: SidecarRequest): Promise<SidecarResponse> {
  if (!project || !repoRoot) {
    return { request_id: request.request_id, status: 'error', error: 'Project not initialized' };
  }

  // Generate virtual entrypoint content
  const entryContent = generateVirtualEntry(request.symbols || []);
  
  // IMPORTANT: Write virtual entry to a PHYSICAL FILE in the repo root
  // This is required because dts-bundle-generator needs a real file path
  // to resolve relative imports correctly (e.g., './types/user' must resolve
  // relative to the repo root, not some temp directory)
  const virtualEntryPath = path.join(repoRoot, '.carrick_virtual_entry.ts');
  
  try {
    // Write the virtual entry file to disk
    fs.writeFileSync(virtualEntryPath, entryContent, 'utf-8');
    
    // Also add to ts-morph project so it's aware of the file
    const virtualEntry = project.addSourceFileAtPath(virtualEntryPath);

    // Use dts-bundle-generator for flattening
    // The physical file ensures relative imports resolve correctly
    const dtsContent = generateDtsBundle([{
      filePath: virtualEntryPath,
    }], {
      preferredConfigPath: project.getCompilerOptions().configFilePath as string,
    })[0];

    // Clean up: remove from project and delete physical file
    project.removeSourceFile(virtualEntry);
    fs.unlinkSync(virtualEntryPath);

    return {
      request_id: request.request_id,
      status: 'success',
      dts_content: dtsContent,
      symbols_resolved: (request.symbols || []).map(s => s.symbol_name),
    };
  } catch (e) {
    // Clean up on error
    try { fs.unlinkSync(virtualEntryPath); } catch {}
    return { request_id: request.request_id, status: 'error', error: String(e) };
  }
}

function generateVirtualEntry(symbols: SymbolRequest[]): string {
  const exports: string[] = [];
  
  for (const sym of symbols) {
    if (sym.alias) {
      exports.push(`export type { ${sym.symbol_name} as ${sym.alias} } from '${sym.source_file}';`);
    } else {
      exports.push(`export type { ${sym.symbol_name} } from '${sym.source_file}';`);
    }
  }
  
  return exports.join('\n');
}

function generateDtsFromInferred(inferred: InferredType[]): string {
  const lines: string[] = ['// Auto-generated type definitions (includes inferred types)'];
  
  for (const t of inferred) {
    lines.push(`export type ${t.alias} = ${t.type_string};`);
  }
  
  return lines.join('\n');
}

// Main message loop
const rl = readline.createInterface({
  input: process.stdin,
  output: process.stdout,
  terminal: false,
});

rl.on('line', async (line) => {
  try {
    const request = RequestSchema.parse(JSON.parse(line));
    const response = await handleMessage(request);
    console.log(JSON.stringify(response));
  } catch (e) {
    console.log(JSON.stringify({ status: 'error', error: String(e) }));
  }
});

// Signal ready on stderr for Rust to know we're alive
console.error('[sidecar] Process started, awaiting init command');
```

### Phase 2: Rust Integration (`src/services/type_sidecar.rs`)

```rust
//! TypeSidecar - Manages the Node.js type resolution process
//!
//! This module provides a warm-standby Node.js process that handles
//! TypeScript type resolution using the actual compiler.
//!
//! Key design: The sidecar is spawned IMMEDIATELY when the CLI starts,
//! allowing TypeScript project initialization to happen in parallel with
//! SWC scanning and LLM analysis.

use serde::{Deserialize, Serialize};
use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};
use uuid::Uuid;

#[derive(Debug, Serialize)]
#[serde(tag = "action")]
pub enum SidecarRequest {
    #[serde(rename = "init")]
    Init {
        request_id: String,
        repo_root: String,
        tsconfig_path: Option<String>,
    },
    #[serde(rename = "health")]
    Health {
        request_id: String,
    },
    #[serde(rename = "bundle")]
    Bundle {
        request_id: String,
        symbols: Vec<SymbolRequest>,
    },
    #[serde(rename = "infer")]
    Infer {
        request_id: String,
        infer_requests: Vec<InferRequest>,
    },
    #[serde(rename = "shutdown")]
    Shutdown { request_id: String },
}

#[derive(Debug, Serialize)]
pub struct SymbolRequest {
    pub symbol_name: String,
    pub source_file: String,
    pub alias: Option<String>,
}

/// Request to infer type at a specific code location
#[derive(Debug, Serialize)]
pub struct InferRequest {
    pub file_path: String,
    pub line_number: u32,
    pub infer_kind: InferKind,
    pub alias: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum InferKind {
    FunctionReturn,
    Expression,
    CallResult,
    Variable,
    ResponseBody,
}

#[derive(Debug, Deserialize)]
pub struct SidecarResponse {
    pub request_id: String,
    pub status: String,
    pub initialized: Option<bool>,
    pub init_time_ms: Option<u64>,
    pub dts_content: Option<String>,
    pub symbols_resolved: Option<Vec<String>>,
    pub inferred_types: Option<Vec<InferredType>>,
    pub error: Option<String>,
    pub symbols_failed: Option<Vec<SymbolFailure>>,
}

#[derive(Debug, Deserialize)]
pub struct InferredType {
    pub alias: String,
    pub type_string: String,
    pub is_explicit: bool,
    pub source_location: String,
}

#[derive(Debug, Deserialize)]
pub struct SymbolFailure {
    pub symbol: String,
    pub reason: String,
}

/// State of sidecar initialization
#[derive(Debug, Clone, PartialEq)]
pub enum SidecarState {
    Spawning,
    Initializing,
    Ready,
    Failed(String),
}

pub struct TypeSidecar {
    child: Child,
    stdin: Mutex<ChildStdin>,
    stdout: Mutex<BufReader<ChildStdout>>,
    state: Arc<Mutex<SidecarState>>,
    spawn_time: Instant,
}

impl TypeSidecar {
    /// Spawn a new sidecar process IMMEDIATELY (non-blocking)
    /// 
    /// Call this at CLI startup, before SWC scanning begins.
    /// The sidecar will initialize in parallel with other work.
    pub fn spawn(sidecar_script_path: &PathBuf) -> Result<Self, String> {
        let spawn_time = Instant::now();
        
        let mut child = Command::new("node")
            .arg(sidecar_script_path.join("dist/index.js"))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit()) // Let errors flow to terminal
            .spawn()
            .map_err(|e| format!("Failed to spawn sidecar: {}", e))?;

        let stdin = child.stdin.take().ok_or("Failed to get stdin")?;
        let stdout = child.stdout.take().ok_or("Failed to get stdout")?;

        Ok(Self {
            child,
            stdin: Mutex::new(stdin),
            stdout: Mutex::new(BufReader::new(stdout)),
            state: Arc::new(Mutex::new(SidecarState::Spawning)),
            spawn_time,
        })
    }

    /// Start TypeScript project initialization (non-blocking)
    /// 
    /// This sends the init command and returns immediately.
    /// Use `wait_ready()` or `is_ready()` to check completion.
    pub fn start_init(&self, repo_root: &PathBuf, tsconfig_path: Option<&str>) {
        let request = SidecarRequest::Init {
            request_id: Uuid::new_v4().to_string(),
            repo_root: repo_root.to_string_lossy().to_string(),
            tsconfig_path: tsconfig_path.map(|s| s.to_string()),
        };

        // Update state
        {
            let mut state = self.state.lock().unwrap();
            *state = SidecarState::Initializing;
        }

        // Send init request (this is still synchronous write, but fast)
        if let Err(e) = self.send_request_no_wait(&request) {
            let mut state = self.state.lock().unwrap();
            *state = SidecarState::Failed(e);
        }
    }

    /// Check if the sidecar is ready without blocking
    pub fn is_ready(&self) -> bool {
        let state = self.state.lock().unwrap();
        *state == SidecarState::Ready
    }

    /// Get current state
    pub fn state(&self) -> SidecarState {
        self.state.lock().unwrap().clone()
    }

    /// Wait for sidecar to be ready, with timeout
    /// 
    /// Call this AFTER SWC scanning and LLM analysis, before type resolution.
    pub fn wait_ready(&self, timeout: Duration) -> Result<Duration, String> {
        let start = Instant::now();
        
        loop {
            // Check for response from init
            if let Ok(response) = self.try_read_response() {
                if response.status == "ready" {
                    let mut state = self.state.lock().unwrap();
                    *state = SidecarState::Ready;
                    let init_time = response.init_time_ms.unwrap_or(0);
                    eprintln!("[sidecar] Ready in {}ms (total wait: {:?})", init_time, start.elapsed());
                    return Ok(start.elapsed());
                } else if response.status == "error" {
                    let error = response.error.unwrap_or_else(|| "Unknown error".into());
                    let mut state = self.state.lock().unwrap();
                    *state = SidecarState::Failed(error.clone());
                    return Err(error);
                }
            }
            
            if start.elapsed() > timeout {
                return Err(format!("Sidecar init timeout after {:?}", timeout));
            }
            
            thread::sleep(Duration::from_millis(10));
        }
    }

    /// Bundle explicit types for the given symbols
    pub fn resolve_types(&self, symbols: Vec<SymbolRequest>) -> Result<String, String> {
        self.ensure_ready()?;

        let request = SidecarRequest::Bundle {
            request_id: Uuid::new_v4().to_string(),
            symbols,
        };

        let response = self.send_request(&request)?;
        self.handle_type_response(response)
    }

    /// Infer types at specific code locations (for implicit types)
    pub fn infer_types(&self, requests: Vec<InferRequest>) -> Result<(String, Vec<InferredType>), String> {
        self.ensure_ready()?;

        let request = SidecarRequest::Infer {
            request_id: Uuid::new_v4().to_string(),
            infer_requests: requests,
        };

        let response = self.send_request(&request)?;
        
        let dts = self.handle_type_response(response.clone())?;
        let inferred = response.inferred_types.unwrap_or_default();
        
        Ok((dts, inferred))
    }

    /// Combined: resolve explicit types AND infer implicit ones
    pub fn resolve_all_types(
        &self,
        explicit: Vec<SymbolRequest>,
        infer: Vec<InferRequest>,
    ) -> Result<String, String> {
        self.ensure_ready()?;
        
        let mut all_dts = String::new();
        
        // First, bundle explicit types
        if !explicit.is_empty() {
            let explicit_dts = self.resolve_types(explicit)?;
            all_dts.push_str(&explicit_dts);
            all_dts.push_str("\n\n");
        }
        
        // Then, infer implicit types
        if !infer.is_empty() {
            let (inferred_dts, _) = self.infer_types(infer)?;
            all_dts.push_str("// Inferred types (no explicit annotation in source)\n");
            all_dts.push_str(&inferred_dts);
        }
        
        Ok(all_dts)
    }

    fn ensure_ready(&self) -> Result<(), String> {
        match self.state() {
            SidecarState::Ready => Ok(()),
            SidecarState::Failed(e) => Err(format!("Sidecar failed: {}", e)),
            state => Err(format!("Sidecar not ready (state: {:?})", state)),
        }
    }

    fn handle_type_response(&self, response: SidecarResponse) -> Result<String, String> {
        match response.status.as_str() {
            "success" => response
                .dts_content
                .ok_or_else(|| "No dts_content in success response".into()),
            "partial" => {
                eprintln!("[sidecar] Partial resolution - some symbols failed:");
                if let Some(failures) = &response.symbols_failed {
                    for f in failures {
                        eprintln!("  - {}: {}", f.symbol, f.reason);
                    }
                }
                response
                    .dts_content
                    .ok_or_else(|| "No dts_content in partial response".into())
            }
            _ => Err(response.error.unwrap_or_else(|| "Unknown error".into())),
        }
    }

    fn send_request_no_wait(&self, request: &SidecarRequest) -> Result<(), String> {
        let json = serde_json::to_string(request)
            .map_err(|e| format!("Failed to serialize request: {}", e))?;

        let mut stdin = self.stdin.lock().unwrap();
        writeln!(stdin, "{}", json)
            .map_err(|e| format!("Failed to write to sidecar: {}", e))?;
        stdin.flush().map_err(|e| format!("Failed to flush: {}", e))?;
        
        Ok(())
    }

    fn send_request(&self, request: &SidecarRequest) -> Result<SidecarResponse, String> {
        self.send_request_no_wait(request)?;

        // Read response
        let mut response_line = String::new();
        {
            let mut stdout = self.stdout.lock().unwrap();
            stdout
                .read_line(&mut response_line)
                .map_err(|e| format!("Failed to read from sidecar: {}", e))?;
        }

        serde_json::from_str(&response_line)
            .map_err(|e| format!("Failed to parse response: {} - {}", e, response_line))
    }

    fn try_read_response(&self) -> Result<SidecarResponse, String> {
        let mut response_line = String::new();
        {
            let mut stdout = self.stdout.lock().unwrap();
            // Non-blocking would be ideal here, but for simplicity we just read
            stdout
                .read_line(&mut response_line)
                .map_err(|e| format!("Failed to read: {}", e))?;
        }

        serde_json::from_str(&response_line)
            .map_err(|e| format!("Failed to parse: {} - {}", e, response_line))
    }

    /// How long since the sidecar was spawned
    pub fn elapsed(&self) -> Duration {
        self.spawn_time.elapsed()
    }
}

impl Drop for TypeSidecar {
    fn drop(&mut self) {
        // Try to gracefully shutdown
        let shutdown = SidecarRequest::Shutdown {
            request_id: Uuid::new_v4().to_string(),
        };
        let _ = self.send_request(&shutdown);
        
        // Kill if still running
        let _ = self.child.kill();
    }
}
```

### Phase 3: CLI Integration - Parallel Startup

The sidecar must be spawned at the very beginning of CLI execution:

```rust
// In src/main.rs

fn main() -> Result<(), Box<dyn Error>> {
    let args = parse_cli_args();
    
    // STEP 1: Spawn sidecar IMMEDIATELY (non-blocking)
    // This starts the Node.js process while we do other work
    let sidecar = if args.enable_type_extraction {
        let sidecar_path = get_sidecar_path()?;
        let sidecar = TypeSidecar::spawn(&sidecar_path)?;
        
        // Start initialization (non-blocking)
        let tsconfig = find_tsconfig(&args.repo_path);
        sidecar.start_init(&args.repo_path, tsconfig.as_deref());
        
        eprintln!("[main] Sidecar spawned, initializing in background...");
        Some(sidecar)
    } else {
        None
    };
    
    // STEP 2: SWC Scanning (parallel with sidecar init)
    let scanner = SwcScanner::new();
    let candidate_files = scanner.find_candidate_files(&args.repo_path)?;
    eprintln!("[main] Found {} candidate files", candidate_files.len());
    
    // STEP 3: LLM Analysis (parallel with sidecar init)
    let orchestrator = FileOrchestrator::new(gemini_service);
    let analysis_results = orchestrator.analyze_files(&candidate_files, &guidance).await?;
    eprintln!("[main] Analyzed {} files with LLM", analysis_results.len());
    
    // STEP 4: Wait for sidecar (should be ready by now, or nearly so)
    let bundled_types = if let Some(ref sidecar) = sidecar {
        // Wait with timeout - sidecar should already be ready after LLM work
        sidecar.wait_ready(Duration::from_secs(30))?;
        
        // Collect type requests from analysis results
        let (explicit, infer) = collect_type_requests(&analysis_results);
        
        // Resolve all types
        Some(sidecar.resolve_all_types(explicit, infer)?)
    } else {
        None
    };
    
    // STEP 5: Upload to S3
    // ...
}
```

### Phase 4: FileOrchestrator Integration

The key change is in how we extract type information after Gemini analysis:

```rust
// In src/agents/file_orchestrator.rs

impl FileOrchestrator {
    /// Collect type extraction requests from analysis results
    /// 
    /// Returns (explicit_types, infer_requests) tuple:
    /// - explicit_types: Symbols that have explicit type annotations
    /// - infer_requests: Locations where we need TypeScript to infer the type
    pub fn collect_type_requests(
        &self,
        file_results: &HashMap<String, FileAnalysisResult>,
    ) -> (Vec<SymbolRequest>, Vec<InferRequest>) {
        let mut explicit = Vec::new();
        let mut infer = Vec::new();

        for (file_path, result) in file_results {
            for endpoint in &result.endpoints {
                let alias = self.generate_alias(&endpoint.method, &endpoint.path, false);
                
                if let Some(ref type_str) = endpoint.response_type_string {
                    // Has explicit type annotation - use symbol extraction
                    if let Some(symbol) = self.extract_primary_symbol(type_str) {
                        explicit.push(SymbolRequest {
                            symbol_name: symbol.name,
                            source_file: symbol.source.unwrap_or_else(|| file_path.clone()),
                            alias: Some(alias),
                        });
                    }
                } else {
                    // No explicit type - ask TypeScript to infer it
                    // Try multiple strategies for framework-agnostic inference
                    infer.push(InferRequest {
                        file_path: file_path.clone(),
                        line_number: endpoint.line_number as u32,
                        infer_kind: InferKind::ResponseBody, // Look for res.json()/res.send()
                        alias: Some(alias.clone()),
                    });
                    
                    // Also try inferring function return type
                    infer.push(InferRequest {
                        file_path: file_path.clone(),
                        line_number: endpoint.line_number as u32,
                        infer_kind: InferKind::FunctionReturn,
                        alias: Some(format!("{}_Handler", alias)),
                    });
                }
            }

            for data_call in &result.data_calls {
                let alias = self.generate_alias(
                    data_call.method.as_deref().unwrap_or("GET"),
                    &data_call.target,
                    true,
                );
                
                if let Some(ref type_str) = data_call.response_type_string {
                    if let Some(symbol) = self.extract_primary_symbol(type_str) {
                        explicit.push(SymbolRequest {
                            symbol_name: symbol.name,
                            source_file: symbol.source.unwrap_or_else(|| file_path.clone()),
                            alias: Some(alias),
                        });
                    }
                } else {
                    // No explicit type - infer from call expression
                    infer.push(InferRequest {
                        file_path: file_path.clone(),
                        line_number: data_call.line_number as u32,
                        infer_kind: InferKind::CallResult,
                        alias: Some(alias),
                    });
                }
            }
        }

        (explicit, infer)
    }
}
```

## Implicit Type Inference - Deep Dive

This is one of the most important capabilities of the sidecar approach. The TypeScript compiler can infer types even when developers don't write explicit annotations.

### Why This Matters for CI

In real-world codebases, many endpoints lack explicit type annotations:

```typescript
// Common pattern - NO explicit types, but TypeScript knows the types!
app.get('/users', async (req, res) => {
  const users = await userService.findAll();  // TypeScript knows: User[]
  res.json(users);                            // TypeScript knows: void, arg is User[]
});

// Another common pattern - type comes from ORM/database
router.post('/orders', async (req, res) => {
  const order = await prisma.order.create({   // TypeScript infers: Order
    data: req.body
  });
  res.status(201).json(order);
});
```

The current position-based approach returns **nothing** for these cases. The sidecar can extract the actual types.

### Framework-Agnostic Inference Strategies

The sidecar uses multiple inference strategies that work regardless of framework:

| Strategy | What It Does | Works With |
|----------|--------------|------------|
| `function_return` | Get return type of handler function | All frameworks |
| `response_body` | Find `.json()`, `.send()` calls within function scope | Express, Fastify, Koa, etc. |
| `call_result` | Get return type of a call expression | fetch, axios, custom clients |
| `variable` | Get type of variable declaration | All |

### Critical Design Decision: Scope-Based Search (Not Line Windows)

**Problem:** Large controllers often have middleware, logging, validation, and error handling before the `res.json()` call. A fixed line window (e.g., ±15 lines) would miss response statements in longer handlers.

**Solution:** The sidecar finds the **function body** associated with the line number Gemini provided and scans the **entire scope** of that function for terminal statements (`return`, `res.json()`, `res.send()`, etc.), regardless of line count.

```typescript
// Example: Handler with 50+ lines of setup before response
app.get('/orders/:id', async (req, res) => {
  // Line 10: Gemini reports this line as the endpoint
  const { id } = req.params;
  
  // Lines 11-30: Validation, auth checks, logging...
  const user = await validateAuth(req);
  if (!user) return res.status(401).json({ error: 'Unauthorized' });
  
  const canAccess = await checkPermissions(user, id);
  if (!canAccess) return res.status(403).json({ error: 'Forbidden' });
  
  logger.info('Fetching order', { userId: user.id, orderId: id });
  
  // Lines 31-50: Database queries, transformations...
  const order = await prisma.order.findUnique({ where: { id } });
  if (!order) return res.status(404).json({ error: 'Not found' });
  
  const enrichedOrder = await enrichWithCustomerData(order);
  
  // Line 55: The actual response - sidecar MUST find this!
  res.json(enrichedOrder);  // TypeScript knows: EnrichedOrder
});
```

The sidecar algorithm:
1. Find the function/arrow function containing the target line
2. Get the function's full body (start to end)
3. Find ALL terminal statements within that scope
4. Infer types from each terminal statement
5. Return union type if multiple response types exist

### Implementation Details

```typescript
// In src/sidecar/src/type-inferrer.ts

export class TypeInferrer {
  constructor(private project: Project) {}

  /**
   * Framework-agnostic response body inference
   * 
   * IMPORTANT: Uses SCOPE-BASED search, not line windows!
   * Finds the containing function and searches its ENTIRE body.
   * 
   * Looks for common response patterns:
   * - res.json(data)
   * - res.send(data)
   * - ctx.body = data
   * - return data (for Hono, tRPC, etc.)
   */
  inferResponseBody(sourceFile: SourceFile, targetLine: number): InferredType | null {
    // Step 1: Find the function containing this line (SCOPE-BASED, not line window)
    const containingFunction = this.findContainingFunction(sourceFile, targetLine);
    if (!containingFunction) {
      // Fallback: search nearby if no function found
      return this.inferResponseBodyFallback(sourceFile, targetLine);
    }

    // Step 2: Search the ENTIRE function body for terminal statements
    const responseTypes: Array<{ type: Type; location: number }> = [];

    // Strategy 1: Find all res.json() / res.send() calls in function scope
    const jsonSendCalls = this.findMethodCallsInScope(containingFunction, ['json', 'send']);
    for (const call of jsonSendCalls) {
      const args = call.getArguments();
      if (args.length > 0) {
        responseTypes.push({
          type: args[0].getType(),
          location: call.getStartLineNumber(),
        });
      }
    }

    // Strategy 2: Find ctx.body assignments in function scope (Koa style)
    const bodyAssignments = this.findPropertyAssignmentsInScope(containingFunction, 'body');
    for (const assignment of bodyAssignments) {
      responseTypes.push({
        type: assignment.getRight().getType(),
        location: assignment.getStartLineNumber(),
      });
    }

    // Strategy 3: Find return statements in function scope (Hono, tRPC, plain functions)
    const returnStatements = this.findReturnStatementsInScope(containingFunction);
    for (const ret of returnStatements) {
      const expr = ret.getExpression();
      if (expr) {
        responseTypes.push({
          type: expr.getType(),
          location: ret.getStartLineNumber(),
        });
      }
    }

    if (responseTypes.length === 0) return null;

    // Step 3: Build result - union type if multiple responses
    const uniqueTypes = this.deduplicateTypes(responseTypes.map(r => r.type));
    const typeString = uniqueTypes.length === 1 
      ? uniqueTypes[0].getText()
      : uniqueTypes.map(t => t.getText()).join(' | ');

    return {
      alias: `ResponseBody_L${targetLine}`,
      type_string: typeString,
      is_explicit: false,
      source_location: `${sourceFile.getFilePath()}:${responseTypes[0].location}`,
    };
  }

  /**
   * Find the function (arrow, declaration, or method) containing the target line
   */
  private findContainingFunction(sourceFile: SourceFile, targetLine: number): Node | null {
    // Find all functions in the file
    const functions = [
      ...sourceFile.getDescendantsOfKind(SyntaxKind.FunctionDeclaration),
      ...sourceFile.getDescendantsOfKind(SyntaxKind.ArrowFunction),
      ...sourceFile.getDescendantsOfKind(SyntaxKind.MethodDeclaration),
      ...sourceFile.getDescendantsOfKind(SyntaxKind.FunctionExpression),
    ];

    // Find the innermost function containing our target line
    let bestMatch: Node | null = null;
    let bestSize = Infinity;

    for (const func of functions) {
      const startLine = func.getStartLineNumber();
      const endLine = func.getEndLineNumber();
      
      if (targetLine >= startLine && targetLine <= endLine) {
        const size = endLine - startLine;
        // Prefer the smallest (innermost) function
        if (size < bestSize) {
          bestSize = size;
          bestMatch = func;
        }
      }
    }

    return bestMatch;
  }

  /**
   * Find method calls within a function's scope (searches entire function body)
   */
  private findMethodCallsInScope(func: Node, methodNames: string[]): CallExpression[] {
    return func.getDescendantsOfKind(SyntaxKind.CallExpression)
      .filter(call => {
        const expr = call.getExpression();
        if (!Node.isPropertyAccessExpression(expr)) return false;
        return methodNames.includes(expr.getName());
      });
  }

  /**
   * Find property assignments within a function's scope
   */
  private findPropertyAssignmentsInScope(func: Node, propertyName: string): BinaryExpression[] {
    return func.getDescendantsOfKind(SyntaxKind.BinaryExpression)
      .filter(expr => {
        const left = expr.getLeft();
        if (!Node.isPropertyAccessExpression(left)) return false;
        return left.getName() === propertyName && 
               expr.getOperatorToken().getKind() === SyntaxKind.EqualsToken;
      });
  }

  /**
   * Find return statements within a function's scope
   */
  private findReturnStatementsInScope(func: Node): ReturnStatement[] {
    return func.getDescendantsOfKind(SyntaxKind.ReturnStatement);
  }

  /**
   * Infer type from fetch/axios/custom HTTP client calls
   */
  inferCallResult(sourceFile: SourceFile, line: number): InferredType | null {
    const callExpr = this.findCallExpressionAtLine(sourceFile, line);
    if (!callExpr) return null;

    // Get the return type of the call
    const callType = callExpr.getType();
    
    // If it's a Promise, unwrap it
    const typeText = this.unwrapPromise(callType);

    return {
      alias: `CallResult_L${line}`,
      type_string: typeText,
      is_explicit: false,
      source_location: `${sourceFile.getFilePath()}:${line}`,
    };
  }

  private unwrapPromise(type: Type): string {
    const typeText = type.getText();
    
    // Handle Promise<T> -> T
    if (typeText.startsWith('Promise<')) {
      const typeArgs = type.getTypeArguments();
      if (typeArgs.length > 0) {
        return typeArgs[0].getText();
      }
    }
    
    return typeText;
  }

  private deduplicateTypes(types: Type[]): Type[] {
    const seen = new Set<string>();
    return types.filter(t => {
      const text = t.getText();
      if (seen.has(text)) return false;
      seen.add(text);
      return true;
    });
  }
}
```

### Handling Ambiguous Cases

When multiple response types are possible (e.g., different branches return different types), the sidecar reports a union type:

```typescript
// Source code
app.get('/user/:id', async (req, res) => {
  const user = await findUser(req.params.id);
  if (!user) {
    return res.status(404).json({ error: 'Not found' });
  }
  res.json(user);
});

// Inferred type (union) - sidecar finds BOTH responses in function scope
type GetUserByIdResponse = User | { error: string };
```

This is actually **more accurate** than an explicit single type annotation would be!

### Critical: Physical File for Virtual Entry

**Problem:** `dts-bundle-generator` requires a physical file path to resolve relative imports. An in-memory virtual file via ts-morph doesn't have a "real" parent directory, so imports like `./types/user` fail to resolve.

**Solution:** The sidecar writes the virtual entry to a temporary physical file in the **repo root**:

```typescript
// Virtual entry written to: {repo_root}/.carrick_virtual_entry.ts
export type { User } from './types/user';      // Resolves correctly!
export type { Order } from './models/order';   // Resolves correctly!
```

This ensures relative paths resolve exactly as they do in the source code. The file is deleted immediately after bundling.

```typescript
// File lifecycle:
// 1. Write to {repo_root}/.carrick_virtual_entry.ts
// 2. Run dts-bundle-generator
// 3. Delete the file (in finally block, even on error)
```

**Gitignore:** The pattern `.carrick_*` should be added to `.gitignore` to prevent accidental commits.

### Performance Optimization

Type inference can be expensive. The sidecar optimizes this:

1. **Batch requests**: Collect all infer requests, send in one batch
2. **Cache per-file**: Once a file's AST is analyzed, cache type info
3. **Parallel inference**: Multiple files can be processed in parallel
4. **Early termination**: If we find a definitive type, stop looking

```typescript
// Batched inference request
{
  "action": "infer",
  "request_id": "batch-1",
  "infer_requests": [
    { "file_path": "./routes/users.ts", "line_number": 15, "infer_kind": "response_body", "alias": "GetUsersResponse" },
    { "file_path": "./routes/users.ts", "line_number": 28, "infer_kind": "response_body", "alias": "GetUserByIdResponse" },
    { "file_path": "./routes/orders.ts", "line_number": 12, "infer_kind": "response_body", "alias": "GetOrdersResponse" },
    // ... more requests
  ]
}
```

Update the file analysis schema to include import source information:

```rust
// In src/agents/schemas.rs - file_analysis_schema()

// Add to endpoint schema:
"primary_type_symbol": {
    "type": "STRING",
    "nullable": true,
    "description": "The primary type symbol name (e.g., 'User', 'Order') - just the identifier, not the full type annotation"
},
"type_import_source": {
    "type": "STRING", 
    "nullable": true,
    "description": "The import path where the type is defined (e.g., './types/user'). Null if type is inline or from current file."
}
```

Update the LLM prompt to extract this information:

```
When extracting response types:
- `response_type_string`: The full type annotation (e.g., "Response<User[]>")
- `primary_type_symbol`: Just the main type identifier (e.g., "User")  
- `type_import_source`: The import path if the type is imported (look for import statements at the top of the file)
```

## Migration Strategy

### Phase 1: Parallel Implementation (Week 1-2)
- Build sidecar in `src/sidecar/`
- Add TypeSidecar Rust wrapper
- Keep existing ts_check pipeline functional

### Phase 2: Feature Flag Integration (Week 2-3)
- Add `--use-sidecar` flag to CLI
- Run both pipelines, compare outputs
- Validate bundled types are correct

### Phase 3: Switch Default (Week 3-4)
- Make sidecar the default
- Keep old pipeline as `--legacy-type-extraction` fallback
- Monitor for issues

### Phase 4: Cleanup (Week 4+)
- Remove old position-based code from `swc_scanner.rs`
- Simplify `ts_check/` to only do type checking (not extraction)
- Remove DeclarationCollector, TypeProcessor, etc.

## Type Checking Refactor

With the sidecar producing bundled `.d.ts` files, type checking becomes simpler:

### Current Type Checker Problems
1. Depends on parsing alias names to match producers/consumers
2. Uses regex patterns that are fragile
3. `groupTypesByEndpoint()` logic is complex

### Proposed: Manifest-Based Matching

Instead of parsing alias names, use a manifest that explicitly links endpoints:

```typescript
interface TypeManifest {
  repo_name: string;
  commit_hash: string;
  endpoints: EndpointEntry[];
}

interface EndpointEntry {
  // Canonical endpoint identifier
  method: string;           // "GET", "POST", etc.
  path: string;             // "/api/users/:id"
  
  // Type information
  producer_alias?: string;  // Type alias in the .d.ts file
  consumer_alias?: string;  // For consumer repos
  
  // Source location
  file_path: string;
  line_number: number;
}
```

The type checker then:
1. Loads manifest for each repo
2. Matches endpoints by `(method, normalized_path)`
3. Compares types directly using TypeScript assignability

## Benefits Summary

| Aspect | Current Approach | Sidecar Approach |
|--------|-----------------|------------------|
| Type Position Finding | ±10 line window search | Not needed |
| Dependency Resolution | Manual AST traversal | Compiler handles it |
| Generic Type Support | Partial, error-prone | Full support |
| Cross-File Imports | Fragile | Built-in to compiler |
| External Type Support | Manual import handling | dts-bundle-generator |
| Type Alias Flattening | Manual collection | Automatic |
| Warm Startup Cost | N/A (per-file parsing) | ~500ms once |
| Per-File Cost | ~100-200ms | ~50ms (pre-loaded) |

## Files to Create/Modify

### New Files
- `carrick/src/sidecar/package.json`
- `carrick/src/sidecar/tsconfig.json`
- `carrick/src/sidecar/src/index.ts`
- `carrick/src/sidecar/src/project-loader.ts`
- `carrick/src/sidecar/src/bundler.ts`
- `carrick/src/sidecar/src/types.ts`
- `carrick/src/sidecar/src/validators.ts`
- `carrick/src/services/type_sidecar.rs`
- `carrick/src/services/mod.rs`

### Modified Files
- `carrick/src/lib.rs` - Add `services` module
- `carrick/src/main.rs` - Spawn sidecar on startup
- `carrick/src/agents/file_orchestrator.rs` - Use sidecar for type resolution
- `carrick/src/multi_agent_orchestrator.rs` - Update `extract_types_from_file_results()`
- `carrick/src/agents/schemas.rs` - Add `primary_type_symbol` and `type_import_source`

### Files to Eventually Remove (Phase 4)
- `carrick/ts_check/lib/type-extractor.ts` - Replaced by sidecar
- `carrick/ts_check/lib/type-processor.ts` - Replaced by sidecar
- `carrick/ts_check/lib/declaration-collector.ts` - Replaced by sidecar
- `carrick/ts_check/lib/output-generator.ts` - Simplified or removed
- `carrick/ts_check/lib/import-handler.ts` - Replaced by sidecar
- `carrick/ts_check/lib/type-resolver.ts` - Replaced by sidecar
- `carrick/src/swc_scanner.rs` - Remove TypePositionFinder (keep CandidateVisitor)

## Testing Strategy

### Unit Tests
- Sidecar message parsing/serialization
- Virtual entrypoint generation
- Rust TypeSidecar struct lifecycle

### Integration Tests
- Full round-trip: LLM result → sidecar → bundled types
- Type checking with bundled types
- Error handling (missing types, circular deps)

### End-to-End Tests
- express-demo-1 repo analysis with sidecar
- Multi-repo type compatibility checking
- S3 upload of bundled types

## Open Questions

1. **dts-bundle-generator vs ts-morph emit**: Which produces better output for our use case?
2. **Virtual file path**: Should the virtual entrypoint be in-memory only or written to disk temporarily?
3. **Incremental bundling**: Can we cache type resolutions between files in the same repo?
4. **Error recovery**: How should we handle partial failures (some types resolve, others don't)?

## Appendix: Current ts_check Module Structure

For reference, here's what the current ts_check module does:

```
ts_check/
├── lib/
│   ├── type-extractor.ts      # Main entry point for extraction
│   ├── type-checker.ts        # Checks producer/consumer compatibility
│   ├── type-resolver.ts       # Finds type declarations at positions
│   ├── type-processor.ts      # Recursively processes type nodes
│   ├── declaration-collector.ts # Collects all required declarations
│   ├── import-handler.ts      # Manages external type imports
│   ├── output-generator.ts    # Writes the final .ts file
│   ├── dependency-manager.ts  # Tracks npm dependencies
│   ├── project-utils.ts       # ts-morph project utilities
│   ├── argument-parser.ts     # CLI argument parsing
│   ├── config.ts              # Configuration
│   ├── constants.ts           # Shared constants
│   ├── types.ts               # TypeScript interfaces
│   └── logger.ts              # Logging utilities
├── extract-type-definitions.ts # CLI entry point for extraction
├── run-type-checking.ts        # CLI entry point for checking
└── package.json
```

The sidecar approach consolidates type extraction into a single, more reliable component while preserving the type checking functionality.
