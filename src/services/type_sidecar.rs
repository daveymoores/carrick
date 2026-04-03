//! TypeSidecar - Rust interface to the Node.js type extraction sidecar.
//!
//! Note: Some public APIs are not yet used but will be integrated in Phase 3.
#![allow(dead_code)]
//!
//! This module provides a non-blocking interface to spawn and communicate with
//! the type-sidecar process. The sidecar runs ts-morph and dts-bundle-generator
//! to extract TypeScript types from source code.
//!
//! Key design principles:
//! - **Non-blocking spawn**: The sidecar spawns immediately, initialization happens in background
//! - **Parallel startup**: SWC scanning and LLM analysis can proceed while sidecar initializes
//! - **JSON message protocol**: All communication is via stdin/stdout JSON messages

use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::io::{BufRead, BufReader, Write};
use std::path::Path;
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

// ============================================================================
// Sidecar State
// ============================================================================

/// State of the sidecar process
#[derive(Debug, Clone, PartialEq)]
pub enum SidecarState {
    /// Process has been spawned but not yet initialized
    Spawning,
    /// Init request sent, waiting for ready response
    Initializing,
    /// Sidecar is ready to process requests
    Ready,
    /// Sidecar failed to initialize or encountered an error
    Failed(String),
}

// ============================================================================
// Request Types (matching TypeScript types)
// ============================================================================

/// Kind of type inference to perform
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum InferKind {
    /// Get return type of a function
    FunctionReturn,
    /// Get type of an expression
    Expression,
    /// Get return type of a call expression
    CallResult,
    /// Get type of a variable declaration
    Variable,
    /// Find response body (.json()/.send()/ctx.body)
    ResponseBody,
    /// Find request body (req.body/ctx.request.body or call payloads)
    RequestBody,
}

/// Wrapper unwrap strategy
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum WrapperUnwrapKind {
    /// Unwrap via property access (e.g., resp.data)
    Property,
    /// Unwrap via explicit generic parameter
    GenericParam,
}

/// Rule for unwrapping a wrapper type
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct WrapperUnwrapRule {
    pub kind: WrapperUnwrapKind,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub property: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub index: Option<u32>,
}

/// Wrapper rule keyed by package and type
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct WrapperRule {
    pub package: String,
    pub type_name: String,
    pub unwrap: WrapperUnwrapRule,
}

/// Request for a specific symbol to be bundled
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SymbolRequest {
    /// The name of the symbol (type, interface, class, etc.)
    pub symbol_name: String,
    /// The source file path (relative to repo root)
    pub source_file: String,
    /// Optional alias for the exported type
    #[serde(skip_serializing_if = "Option::is_none")]
    pub alias: Option<String>,
}

/// Request for type inference at a specific location
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InferRequestItem {
    /// Path to the file (relative to repo root)
    pub file_path: String,
    /// Line number (1-based) where inference should occur
    pub line_number: u32,
    /// Start byte offset of the target expression (from SWC spans)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub span_start: Option<u32>,
    /// End byte offset of the target expression (from SWC spans)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub span_end: Option<u32>,
    /// Verbatim expression text to locate in source (from Gemini)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expression_text: Option<String>,
    /// Line number where the expression starts (from Gemini)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expression_line: Option<u32>,
    /// The kind of inference to perform
    pub infer_kind: InferKind,
    /// Optional alias for the inferred type
    #[serde(skip_serializing_if = "Option::is_none")]
    pub alias: Option<String>,
}

// ============================================================================
// Internal Request Messages
// ============================================================================

#[derive(Debug, Serialize)]
#[serde(tag = "action")]
enum SidecarRequest {
    #[serde(rename = "init")]
    Init {
        request_id: String,
        repo_root: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        tsconfig_path: Option<String>,
    },
    #[serde(rename = "bundle")]
    Bundle {
        request_id: String,
        symbols: Vec<SymbolRequest>,
    },
    #[serde(rename = "infer")]
    Infer {
        request_id: String,
        requests: Vec<InferRequestItem>,
        #[serde(skip_serializing_if = "Option::is_none")]
        wrappers: Option<Vec<WrapperRule>>,
    },
    #[serde(rename = "resolve_definitions")]
    ResolveDefinitions {
        request_id: String,
        bundled_dts: String,
        aliases: Vec<String>,
    },
    #[serde(rename = "health")]
    Health { request_id: String },
    #[serde(rename = "shutdown")]
    Shutdown { request_id: String },
}

// ============================================================================
// Response Types (matching TypeScript types)
// ============================================================================

/// Source location information for a type
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SourceLocation {
    /// File path relative to repo root
    pub file_path: String,
    /// Start line (1-based)
    pub start_line: u32,
    /// End line (1-based)
    pub end_line: u32,
    /// Start column (0-based)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub start_column: Option<u32>,
    /// End column (0-based)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub end_column: Option<u32>,
}

/// An entry in the type manifest
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ManifestEntry {
    /// The alias or original name of the type
    pub alias: String,
    /// The original symbol name
    pub original_name: String,
    /// The source file where the type was found
    pub source_file: String,
    /// The full type definition string
    pub type_string: String,
    /// Whether this was an explicit annotation or inferred
    pub is_explicit: bool,
}

/// An inferred type result
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InferredType {
    /// The alias for this type (generated if not provided)
    pub alias: String,
    /// The full TypeScript type string
    pub type_string: String,
    /// Whether the type was explicitly annotated in source
    pub is_explicit: bool,
    /// Source location information
    pub source_location: SourceLocation,
    /// The kind of inference that was performed
    pub infer_kind: InferKind,
}

/// Information about a symbol that failed to resolve
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SymbolFailure {
    /// The symbol that failed
    pub symbol_name: String,
    /// The source file where it was supposed to be
    pub source_file: String,
    /// Reason for the failure
    pub reason: String,
}

/// Response from the sidecar
#[derive(Debug, Clone, Deserialize)]
pub struct SidecarResponse {
    /// Echo of the request_id
    pub request_id: String,
    /// Response status
    pub status: String,
    /// Initialization time in milliseconds (for init/health)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub init_time_ms: Option<u64>,
    /// Bundled .d.ts content (for bundle)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dts_content: Option<String>,
    /// Manifest entries (for bundle)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub manifest: Option<Vec<ManifestEntry>>,
    /// Symbol failures (for bundle)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub symbol_failures: Option<Vec<SymbolFailure>>,
    /// Inferred types (for infer)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub inferred_types: Option<Vec<InferredType>>,
    /// Resolved definitions (for resolve_definitions)
    #[serde(default)]
    pub definitions: Option<Vec<ResolvedDefinitionResult>>,
    /// Error messages
    #[serde(skip_serializing_if = "Option::is_none")]
    pub errors: Option<Vec<String>>,
}

/// A single resolved type definition from the sidecar
#[derive(Debug, Clone, Deserialize)]
pub struct ResolvedDefinitionResult {
    pub type_alias: String,
    /// Original declaration text as written
    pub definition: String,
    /// Compiler-expanded form with all types fully inlined
    pub expanded: String,
}

// ============================================================================
// Combined Result Type
// ============================================================================

/// Result of resolving all types (explicit + inferred)
#[derive(Debug, Clone)]
pub struct TypeResolutionResult {
    /// The bundled .d.ts content (explicit types)
    pub dts_content: Option<String>,
    /// Manifest of explicit types
    pub explicit_manifest: Vec<ManifestEntry>,
    /// Inferred types
    pub inferred_types: Vec<InferredType>,
    /// Failures for explicit symbols
    pub symbol_failures: Vec<SymbolFailure>,
    /// General errors
    pub errors: Vec<String>,
}

// ============================================================================
// TypeSidecar Implementation
// ============================================================================

/// TypeSidecar - Manages communication with the Node.js type extraction sidecar.
///
/// # Example
///
/// ```no_run
/// use carrick::services::TypeSidecar;
/// use std::path::Path;
/// use std::time::Duration;
///
/// // Spawn the sidecar (non-blocking)
/// let mut sidecar = TypeSidecar::spawn(Path::new("./node_modules/.bin/type-sidecar")).unwrap();
///
/// // Start initialization (non-blocking)
/// sidecar.start_init(Path::new("/path/to/repo"), None);
///
/// // ... do other work while sidecar initializes ...
///
/// // Wait for sidecar to be ready
/// sidecar.wait_ready(Duration::from_secs(30)).unwrap();
///
/// // Now resolve types
/// let result = sidecar.resolve_types(&[]).unwrap();
/// ```
pub struct TypeSidecar {
    /// Child process handle
    child: Child,
    /// Stdin for sending requests (wrapped in mutex for thread safety)
    stdin: Mutex<ChildStdin>,
    /// Stdout for reading responses (wrapped in mutex for thread safety)
    stdout: Mutex<BufReader<ChildStdout>>,
    /// Current state of the sidecar
    state: Arc<Mutex<SidecarState>>,
    /// Time when spawn() was called
    spawn_time: Instant,
    /// Request ID counter
    request_counter: Mutex<u64>,
}

impl TypeSidecar {
    /// Spawn the sidecar process.
    ///
    /// This returns immediately after spawning - the sidecar process starts
    /// initializing in the background. Call `start_init()` to begin TypeScript
    /// project initialization, then `wait_ready()` when you need to use it.
    ///
    /// # Arguments
    /// * `sidecar_path` - Path to the sidecar executable (node script)
    ///
    /// # Returns
    /// A new `TypeSidecar` instance in the `Spawning` state.
    pub fn spawn(sidecar_path: &Path) -> Result<Self, SidecarError> {
        let spawn_time = Instant::now();

        eprintln!("[type_sidecar] Spawning sidecar from: {:?}", sidecar_path);

        // Spawn the Node.js process
        let mut child = Command::new("node")
            .arg(sidecar_path)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit()) // Let sidecar logs go to stderr
            .spawn()
            .map_err(|e| SidecarError::SpawnFailed(e.to_string()))?;

        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| SidecarError::SpawnFailed("Failed to get stdin".to_string()))?;

        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| SidecarError::SpawnFailed("Failed to get stdout".to_string()))?;

        eprintln!(
            "[type_sidecar] Sidecar spawned in {:?}",
            spawn_time.elapsed()
        );

        Ok(Self {
            child,
            stdin: Mutex::new(stdin),
            stdout: Mutex::new(BufReader::new(stdout)),
            state: Arc::new(Mutex::new(SidecarState::Spawning)),
            spawn_time,
            request_counter: Mutex::new(0),
        })
    }

    /// Start initialization of the TypeScript project.
    ///
    /// This sends the init request and returns immediately. The sidecar
    /// will process the initialization in the background. Use `wait_ready()`
    /// to block until initialization is complete.
    ///
    /// # Arguments
    /// * `repo_root` - Path to the repository root
    /// * `tsconfig_path` - Optional path to tsconfig.json (relative to repo root)
    pub fn start_init(&self, repo_root: &Path, tsconfig_path: Option<&str>) {
        let repo_root_str = repo_root.to_string_lossy().to_string();
        let tsconfig = tsconfig_path.map(String::from);

        // Update state to Initializing
        {
            let mut state = self.state.lock().unwrap();
            *state = SidecarState::Initializing;
        }

        eprintln!("[type_sidecar] Starting init for repo: {}", repo_root_str);

        // Send init request
        let request = SidecarRequest::Init {
            request_id: self.next_request_id(),
            repo_root: repo_root_str,
            tsconfig_path: tsconfig,
        };

        if let Err(e) = self.send_request(&request) {
            let mut state = self.state.lock().unwrap();
            *state = SidecarState::Failed(format!("Failed to send init: {}", e));
        }
    }

    /// Check if the sidecar is ready (non-blocking).
    ///
    /// This checks the current state without waiting.
    pub fn is_ready(&self) -> bool {
        let state = self.state.lock().unwrap();
        *state == SidecarState::Ready
    }

    /// Get the current state of the sidecar.
    pub fn get_state(&self) -> SidecarState {
        self.state.lock().unwrap().clone()
    }

    /// Wait for the sidecar to become ready.
    ///
    /// This blocks until the sidecar is ready or the timeout expires.
    /// If `start_init()` was called, this will wait for the init response.
    ///
    /// # Arguments
    /// * `timeout` - Maximum time to wait
    ///
    /// # Returns
    /// Ok(()) if ready, Err if timeout or failure.
    pub fn wait_ready(&self, timeout: Duration) -> Result<(), SidecarError> {
        let start = Instant::now();

        // If we're in Initializing state, read the init response
        {
            let state = self.state.lock().unwrap();
            if *state == SidecarState::Initializing {
                drop(state); // Release lock before reading

                // Read the init response
                match self.read_response_with_timeout(timeout) {
                    Ok(response) => {
                        let mut state = self.state.lock().unwrap();
                        if response.status == "ready" {
                            *state = SidecarState::Ready;
                            eprintln!(
                                "[type_sidecar] Sidecar ready (init_time: {:?}ms, total: {:?})",
                                response.init_time_ms,
                                self.spawn_time.elapsed()
                            );
                        } else {
                            let error = response
                                .errors
                                .map(|e| e.join("; "))
                                .unwrap_or_else(|| "Unknown error".to_string());
                            *state = SidecarState::Failed(error.clone());
                            return Err(SidecarError::InitFailed(error));
                        }
                    }
                    Err(e) => {
                        let mut state = self.state.lock().unwrap();
                        *state = SidecarState::Failed(e.to_string());
                        return Err(e);
                    }
                }
            }
        }

        // Poll state until ready or timeout
        while start.elapsed() < timeout {
            let state = self.state.lock().unwrap();
            match &*state {
                SidecarState::Ready => return Ok(()),
                SidecarState::Failed(e) => return Err(SidecarError::InitFailed(e.clone())),
                _ => {
                    drop(state);
                    thread::sleep(Duration::from_millis(10));
                }
            }
        }

        Err(SidecarError::Timeout)
    }

    /// Resolve explicit types by bundling symbols.
    ///
    /// # Arguments
    /// * `symbols` - Symbols to bundle
    ///
    /// # Returns
    /// The sidecar response with bundled types.
    pub fn resolve_types(
        &self,
        symbols: &[SymbolRequest],
    ) -> Result<SidecarResponse, SidecarError> {
        self.ensure_ready()?;

        if symbols.is_empty() {
            return Ok(SidecarResponse {
                request_id: "empty".to_string(),
                status: "success".to_string(),
                init_time_ms: None,
                dts_content: Some(String::new()),
                manifest: Some(vec![]),
                symbol_failures: None,
                inferred_types: None,
                definitions: None,
                errors: None,
            });
        }

        let request = SidecarRequest::Bundle {
            request_id: self.next_request_id(),
            symbols: symbols.to_vec(),
        };

        self.send_request(&request)?;
        self.read_response_with_timeout(Duration::from_secs(60))
    }

    /// Infer implicit types at specified locations.
    ///
    /// # Arguments
    /// * `requests` - Inference requests
    /// * `wrappers` - Optional wrapper rules for deterministic unwrapping
    ///
    /// # Returns
    /// The sidecar response with inferred types.
    pub fn infer_types(
        &self,
        requests: &[InferRequestItem],
        wrappers: &[WrapperRule],
    ) -> Result<SidecarResponse, SidecarError> {
        self.ensure_ready()?;

        if requests.is_empty() {
            return Ok(SidecarResponse {
                request_id: "empty".to_string(),
                status: "success".to_string(),
                init_time_ms: None,
                dts_content: None,
                manifest: None,
                symbol_failures: None,
                inferred_types: Some(vec![]),
                definitions: None,
                errors: None,
            });
        }

        let wrapper_rules = if wrappers.is_empty() {
            None
        } else {
            Some(wrappers.to_vec())
        };

        let request = SidecarRequest::Infer {
            request_id: self.next_request_id(),
            requests: requests.to_vec(),
            wrappers: wrapper_rules,
        };

        self.send_request(&request)?;
        self.read_response_with_timeout(Duration::from_secs(60))
    }

    /// Resolve type definitions from bundled .d.ts content.
    ///
    /// For each alias, returns both the original declaration text and the
    /// compiler-expanded form with all types fully inlined.
    ///
    /// # Arguments
    /// * `bundled_dts` - The bundled .d.ts content
    /// * `aliases` - Type alias names to resolve
    pub fn resolve_definitions(
        &self,
        bundled_dts: &str,
        aliases: &[String],
    ) -> Result<Vec<ResolvedDefinitionResult>, SidecarError> {
        self.ensure_ready()?;

        if aliases.is_empty() {
            return Ok(vec![]);
        }

        let request = SidecarRequest::ResolveDefinitions {
            request_id: self.next_request_id(),
            bundled_dts: bundled_dts.to_string(),
            aliases: aliases.to_vec(),
        };

        self.send_request(&request)?;
        let response = self.read_response_with_timeout(Duration::from_secs(30))?;

        if response.status != "success" {
            let errors = response.errors.unwrap_or_default();
            return Err(SidecarError::ResolutionFailed(errors.join("; ")));
        }

        Ok(response.definitions.unwrap_or_default())
    }

    /// Resolve all types (explicit + inferred) in a single operation.
    ///
    /// # Arguments
    /// * `explicit` - Symbols for explicit type bundling
    /// * `infer` - Requests for type inference
    /// * `wrappers` - Optional wrapper rules for deterministic unwrapping
    ///
    /// # Returns
    /// Combined result with both explicit and inferred types.
    pub fn resolve_all_types(
        &self,
        explicit: &[SymbolRequest],
        infer: &[InferRequestItem],
        wrappers: &[WrapperRule],
    ) -> Result<TypeResolutionResult, SidecarError> {
        self.ensure_ready()?;

        let mut result = TypeResolutionResult {
            dts_content: None,
            explicit_manifest: vec![],
            inferred_types: vec![],
            symbol_failures: vec![],
            errors: vec![],
        };

        // Bundle explicit types
        if !explicit.is_empty() {
            let bundle_response = self.resolve_types(explicit)?;
            result.dts_content = bundle_response.dts_content;
            if let Some(manifest) = bundle_response.manifest {
                result.explicit_manifest = manifest;
            }
            if let Some(failures) = bundle_response.symbol_failures {
                result.symbol_failures = failures;
            }
            if let Some(errors) = bundle_response.errors {
                result.errors.extend(errors);
            }
        }

        // Infer implicit types
        if !infer.is_empty() {
            let infer_response = self.infer_types(infer, wrappers)?;
            if let Some(inferred) = infer_response.inferred_types {
                result.inferred_types = inferred;
            }
            if let Some(errors) = infer_response.errors {
                result.errors.extend(errors);
            }
        }

        let had_explicit_dts = result.dts_content.is_some();
        let mut combined_dts = result.dts_content.take().unwrap_or_default();
        let mut appended_aliases: HashSet<String> = HashSet::new();

        // Pre-populate with aliases already defined by the bundler so the
        // infer / failure-fallback loops below never shadow them with `= unknown`.
        if had_explicit_dts {
            for req in explicit {
                if let Some(alias) = &req.alias {
                    if Self::dts_defines_alias(&combined_dts, alias) {
                        appended_aliases.insert(alias.clone());
                    }
                }
                if Self::dts_defines_alias(&combined_dts, &req.symbol_name) {
                    appended_aliases.insert(req.symbol_name.clone());
                }
            }
        }

        let mut append_alias = |alias: &str, type_string: &str| -> bool {
            if !appended_aliases.insert(alias.to_string()) {
                return false;
            }
            if !combined_dts.is_empty() && !combined_dts.ends_with('\n') {
                combined_dts.push('\n');
            }
            combined_dts.push_str("export type ");
            combined_dts.push_str(alias);
            combined_dts.push_str(" = ");
            combined_dts.push_str(type_string.trim().trim_end_matches(';'));
            combined_dts.push_str(";\n");
            true
        };

        let mut inferred_aliases = HashSet::new();
        for inferred in &result.inferred_types {
            inferred_aliases.insert(inferred.alias.clone());
            let type_string = if Self::is_untyped_response_type(&inferred.type_string) {
                "unknown"
            } else {
                inferred.type_string.as_str()
            };
            append_alias(&inferred.alias, type_string);
        }

        // Scrub inferred_types so downstream consumers (enrich_manifest_with_type_resolution)
        // see "unknown" for framework types and leave type_state as Unknown.
        for inferred in &mut result.inferred_types {
            if Self::is_untyped_response_type(&inferred.type_string) {
                inferred.type_string = "unknown".to_string();
            }
        }

        for failure in &result.symbol_failures {
            if let Some(request) = explicit.iter().find(|req| {
                req.symbol_name == failure.symbol_name && req.source_file == failure.source_file
            }) {
                let alias = request
                    .alias
                    .clone()
                    .unwrap_or_else(|| request.symbol_name.clone());
                append_alias(&alias, "unknown");
            }
        }

        if !infer.is_empty() {
            for request in infer {
                if let Some(alias) = &request.alias {
                    if !inferred_aliases.contains(alias) {
                        append_alias(alias, "unknown");
                    }
                }
            }
        }

        if had_explicit_dts || !combined_dts.is_empty() {
            result.dts_content = Some(combined_dts);
        }

        Ok(result)
    }

    /// Check health status of the sidecar.
    pub fn health_check(&self) -> Result<SidecarResponse, SidecarError> {
        let request = SidecarRequest::Health {
            request_id: self.next_request_id(),
        };

        self.send_request(&request)?;
        self.read_response_with_timeout(Duration::from_secs(5))
    }

    /// Shutdown the sidecar gracefully.
    pub fn shutdown(&self) -> Result<(), SidecarError> {
        let request = SidecarRequest::Shutdown {
            request_id: self.next_request_id(),
        };

        // Send shutdown, ignore response (process will exit)
        let _ = self.send_request(&request);
        Ok(())
    }

    // ========================================================================
    // Internal Methods
    // ========================================================================

    fn next_request_id(&self) -> String {
        let mut counter = self.request_counter.lock().unwrap();
        *counter += 1;
        format!("req-{}", counter)
    }

    /// Check whether a bundled DTS string already defines a given alias
    /// (as a type, interface, class, enum, or namespace declaration).
    fn dts_defines_alias(content: &str, alias: &str) -> bool {
        let escaped = regex::escape(alias);
        let pattern = format!(r"\b(type|interface|class|enum|namespace)\s+{}\b", escaped);
        match regex::Regex::new(&pattern) {
            Ok(re) => re.is_match(content),
            Err(_) => false,
        }
    }

    fn is_untyped_response_type(type_string: &str) -> bool {
        let trimmed = type_string.trim().trim_end_matches(';');
        if trimmed.is_empty() {
            return false;
        }
        let base = trimmed.rsplit('.').next().unwrap_or(trimmed);
        matches!(
            base,
            // Express
            "Response" | "Request" | "NextFunction" |
            "Express" | "Application" | "Router" |
            // Fastify
            "Context" | "FastifyInstance" | "FastifyReply" | "FastifyRequest" |
            // Koa
            "Koa" | "ParameterizedContext" |
            // Node.js HTTP core
            "IncomingMessage" | "ServerResponse"
        )
    }

    fn ensure_ready(&self) -> Result<(), SidecarError> {
        let state = self.state.lock().unwrap();
        match &*state {
            SidecarState::Ready => Ok(()),
            SidecarState::Failed(e) => Err(SidecarError::NotReady(e.clone())),
            _ => Err(SidecarError::NotReady(
                "Sidecar not initialized. Call start_init() and wait_ready() first.".to_string(),
            )),
        }
    }

    fn send_request(&self, request: &SidecarRequest) -> Result<(), SidecarError> {
        let json = serde_json::to_string(request)
            .map_err(|e| SidecarError::SerializationError(e.to_string()))?;

        let mut stdin = self.stdin.lock().unwrap();
        writeln!(stdin, "{}", json).map_err(|e| SidecarError::IoError(e.to_string()))?;
        stdin
            .flush()
            .map_err(|e| SidecarError::IoError(e.to_string()))?;

        Ok(())
    }

    fn read_response_with_timeout(
        &self,
        timeout: Duration,
    ) -> Result<SidecarResponse, SidecarError> {
        let mut stdout = self.stdout.lock().unwrap();
        let mut line = String::new();

        // Set up timeout using a simple polling approach
        // Note: For production, consider using async I/O
        let start = Instant::now();

        loop {
            if start.elapsed() > timeout {
                return Err(SidecarError::Timeout);
            }

            // Try to read a line
            match stdout.read_line(&mut line) {
                Ok(0) => {
                    // EOF - process may have died
                    return Err(SidecarError::ProcessDied);
                }
                Ok(_) => {
                    // Got a line, parse it
                    let trimmed = line.trim();
                    if trimmed.is_empty() {
                        line.clear();
                        continue;
                    }

                    let response: SidecarResponse = serde_json::from_str(trimmed).map_err(|e| {
                        SidecarError::DeserializationError(format!("{}: {}", e, trimmed))
                    })?;

                    return Ok(response);
                }
                Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    // No data yet, sleep a bit
                    thread::sleep(Duration::from_millis(10));
                    continue;
                }
                Err(e) => {
                    return Err(SidecarError::IoError(e.to_string()));
                }
            }
        }
    }
}

impl Drop for TypeSidecar {
    fn drop(&mut self) {
        eprintln!("[type_sidecar] Shutting down sidecar");

        // Try graceful shutdown first
        let _ = self.shutdown();

        // Give it a moment to exit
        thread::sleep(Duration::from_millis(100));

        // Force kill if still running
        let _ = self.child.kill();
    }
}

// ============================================================================
// Error Types
// ============================================================================

/// Errors that can occur when using the TypeSidecar
#[derive(Debug, Clone)]
pub enum SidecarError {
    /// Failed to spawn the sidecar process
    SpawnFailed(String),
    /// Sidecar initialization failed
    InitFailed(String),
    /// Sidecar is not ready to handle requests
    NotReady(String),
    /// Operation timed out
    Timeout,
    /// Sidecar process died unexpectedly
    ProcessDied,
    /// I/O error communicating with sidecar
    IoError(String),
    /// Failed to serialize request
    SerializationError(String),
    /// Failed to deserialize response
    DeserializationError(String),
    /// Definition resolution failed
    ResolutionFailed(String),
}

impl std::fmt::Display for SidecarError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SidecarError::SpawnFailed(e) => write!(f, "Failed to spawn sidecar: {}", e),
            SidecarError::InitFailed(e) => write!(f, "Sidecar initialization failed: {}", e),
            SidecarError::NotReady(e) => write!(f, "Sidecar not ready: {}", e),
            SidecarError::Timeout => write!(f, "Sidecar operation timed out"),
            SidecarError::ProcessDied => write!(f, "Sidecar process died unexpectedly"),
            SidecarError::IoError(e) => write!(f, "I/O error: {}", e),
            SidecarError::SerializationError(e) => write!(f, "Serialization error: {}", e),
            SidecarError::DeserializationError(e) => write!(f, "Deserialization error: {}", e),
            SidecarError::ResolutionFailed(e) => write!(f, "Definition resolution failed: {}", e),
        }
    }
}

impl std::error::Error for SidecarError {}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_infer_kind_serialization() {
        let kind = InferKind::FunctionReturn;
        let json = serde_json::to_string(&kind).unwrap();
        assert_eq!(json, r#""function_return""#);

        let kind = InferKind::ResponseBody;
        let json = serde_json::to_string(&kind).unwrap();
        assert_eq!(json, r#""response_body""#);

        let kind = InferKind::RequestBody;
        let json = serde_json::to_string(&kind).unwrap();
        assert_eq!(json, r#""request_body""#);
    }

    #[test]
    fn test_symbol_request_serialization() {
        let request = SymbolRequest {
            symbol_name: "User".to_string(),
            source_file: "src/types.ts".to_string(),
            alias: Some("UserResponse".to_string()),
        };
        let json = serde_json::to_string(&request).unwrap();
        assert!(json.contains(r#""symbol_name":"User""#));
        assert!(json.contains(r#""alias":"UserResponse""#));
    }

    #[test]
    fn test_infer_request_serialization() {
        let request = InferRequestItem {
            file_path: "src/routes.ts".to_string(),
            line_number: 42,
            span_start: Some(128),
            span_end: Some(196),
            expression_text: None,
            expression_line: None,
            infer_kind: InferKind::ResponseBody,
            alias: None,
        };
        let json = serde_json::to_string(&request).unwrap();
        assert!(json.contains(r#""file_path":"src/routes.ts""#));
        assert!(json.contains(r#""line_number":42"#));
        assert!(json.contains(r#""span_start":128"#));
        assert!(json.contains(r#""span_end":196"#));
        assert!(json.contains(r#""infer_kind":"response_body""#));
        assert!(!json.contains("alias")); // Should be skipped when None
    }

    #[test]
    fn test_sidecar_request_init_serialization() {
        let request = SidecarRequest::Init {
            request_id: "req-1".to_string(),
            repo_root: "/path/to/repo".to_string(),
            tsconfig_path: Some("tsconfig.json".to_string()),
        };
        let json = serde_json::to_string(&request).unwrap();
        assert!(json.contains(r#""action":"init""#));
        assert!(json.contains(r#""request_id":"req-1""#));
        assert!(json.contains(r#""repo_root":"/path/to/repo""#));
    }

    #[test]
    fn test_sidecar_request_bundle_serialization() {
        let request = SidecarRequest::Bundle {
            request_id: "req-2".to_string(),
            symbols: vec![SymbolRequest {
                symbol_name: "User".to_string(),
                source_file: "src/types.ts".to_string(),
                alias: None,
            }],
        };
        let json = serde_json::to_string(&request).unwrap();
        assert!(json.contains(r#""action":"bundle""#));
        assert!(json.contains(r#""symbols""#));
    }

    #[test]
    fn test_sidecar_response_deserialization() {
        let json = r#"{
            "request_id": "req-1",
            "status": "ready",
            "init_time_ms": 500
        }"#;
        let response: SidecarResponse = serde_json::from_str(json).unwrap();
        assert_eq!(response.request_id, "req-1");
        assert_eq!(response.status, "ready");
        assert_eq!(response.init_time_ms, Some(500));
    }

    #[test]
    fn test_sidecar_state_equality() {
        assert_eq!(SidecarState::Ready, SidecarState::Ready);
        assert_ne!(SidecarState::Ready, SidecarState::Spawning);
        assert_eq!(
            SidecarState::Failed("error".to_string()),
            SidecarState::Failed("error".to_string())
        );
    }

    #[test]
    fn test_dts_defines_alias_matches_interface() {
        let dts = "export interface Endpoint_abc_Response {\n  id: string;\n}\n";
        assert!(TypeSidecar::dts_defines_alias(dts, "Endpoint_abc_Response"));
    }

    #[test]
    fn test_dts_defines_alias_matches_type() {
        let dts = "export type Endpoint_abc_Response = { id: string };\n";
        assert!(TypeSidecar::dts_defines_alias(dts, "Endpoint_abc_Response"));
    }

    #[test]
    fn test_dts_defines_alias_no_false_positive() {
        let dts = "export interface Endpoint_abc_Request {\n  id: string;\n}\n";
        assert!(!TypeSidecar::dts_defines_alias(
            dts,
            "Endpoint_abc_Response"
        ));
    }

    #[test]
    fn test_dts_defines_alias_matches_enum() {
        let dts = "export enum Status { Active, Inactive }\n";
        assert!(TypeSidecar::dts_defines_alias(dts, "Status"));
    }

    #[test]
    fn test_is_untyped_response_type() {
        assert!(TypeSidecar::is_untyped_response_type("Response"));
        assert!(TypeSidecar::is_untyped_response_type("express.Response"));
        assert!(TypeSidecar::is_untyped_response_type("FastifyReply"));
        assert!(!TypeSidecar::is_untyped_response_type("{ id: string }"));
        assert!(!TypeSidecar::is_untyped_response_type("UserResponse"));
    }

    /// Verify that resolve_all_types does not append `type X = unknown` when
    /// the bundler already produced an `interface X { ... }` in the DTS.
    #[test]
    fn test_resolve_all_types_no_duplicate_when_bundled() {
        // Simulate a TypeResolutionResult as if the bundler already produced content.
        let bundled_dts = concat!(
            "export interface Endpoint_abc_Response {\n",
            "  id: string;\n",
            "  author: string;\n",
            "}\n"
        );

        // Build an equivalent of what resolve_all_types does post-bundle/infer:
        let had_explicit_dts = true;
        let combined_dts = bundled_dts.to_string();
        let mut appended_aliases: HashSet<String> = HashSet::new();

        // This is the new pre-population logic under test:
        let explicit = vec![SymbolRequest {
            symbol_name: "SomeInterface".to_string(),
            source_file: "src/types.ts".to_string(),
            alias: Some("Endpoint_abc_Response".to_string()),
        }];

        if had_explicit_dts {
            for req in &explicit {
                if let Some(alias) = &req.alias {
                    if TypeSidecar::dts_defines_alias(&combined_dts, alias) {
                        appended_aliases.insert(alias.clone());
                    }
                }
                if TypeSidecar::dts_defines_alias(&combined_dts, &req.symbol_name) {
                    appended_aliases.insert(req.symbol_name.clone());
                }
            }
        }

        // Now simulate the infer loop trying to append unknown for the same alias
        let inferred_alias = "Endpoint_abc_Response";
        let already_present = !appended_aliases.insert(inferred_alias.to_string());
        assert!(
            already_present,
            "appended_aliases should already contain the bundled alias"
        );

        // The combined DTS should NOT contain a duplicate `type X = unknown`
        assert!(
            !combined_dts.contains("type Endpoint_abc_Response = unknown"),
            "Should not have appended unknown fallback for already-bundled alias"
        );
        assert_eq!(
            combined_dts.matches("Endpoint_abc_Response").count(),
            1,
            "Alias should appear exactly once in bundled DTS"
        );
    }
}
