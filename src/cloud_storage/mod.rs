use crate::{
    agents::{
        file_analyzer_agent::FileAnalysisResult, framework_guidance_agent::FrameworkGuidance,
    },
    analyzer::ApiEndpointDetails,
    app_context::AppContext,
    framework_detector::DetectionResult,
    mount_graph::MountGraph,
    multi_agent_orchestrator::MultiAgentAnalysisResult,
    packages::Packages,
    services::type_sidecar::InferKind,
    visitor::{FunctionDefinition, Mount, OwnerType},
};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::error::Error;
use std::path::PathBuf;

mod mock_storage;
pub use mock_storage::MockStorage;
mod aws_storage;
pub use aws_storage::AwsStorage;

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ManifestRole {
    Producer,
    Consumer,
}

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ManifestTypeKind {
    Request,
    Response,
}

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ManifestTypeState {
    Explicit,
    Implicit,
    Unknown,
}

/// Evidence metadata for how a manifest entry was derived.
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct TypeEvidence {
    /// Source file path where the type was found
    pub file_path: String,
    /// Start byte offset in the source file
    pub span_start: Option<u32>,
    /// End byte offset in the source file
    pub span_end: Option<u32>,
    /// Line number in the source file
    pub line_number: u32,
    /// Kind of inference performed for this type
    pub infer_kind: InferKind,
    /// Whether the type was explicitly annotated
    pub is_explicit: bool,
    /// Current state of the type extraction
    pub type_state: ManifestTypeState,
}

/// Entry in the type manifest mapping endpoints to their type information
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct TypeManifestEntry {
    /// HTTP method (GET, POST, PUT, DELETE, etc.)
    pub method: String,
    /// API path (e.g., /api/users/:id)
    pub path: String,
    /// Whether this is a producer or consumer
    pub role: ManifestRole,
    /// Whether this entry represents request or response
    pub type_kind: ManifestTypeKind,
    /// The type alias used in the bundled .d.ts file
    pub type_alias: String,
    /// Source file path where the type was found
    pub file_path: String,
    /// Line number in the source file
    pub line_number: u32,
    /// Whether the type was explicitly annotated
    pub is_explicit: bool,
    /// Current state of the type extraction
    pub type_state: ManifestTypeState,
    /// Evidence metadata for this entry
    pub evidence: TypeEvidence,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct CloudRepoData {
    pub repo_name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub service_name: Option<String>, // Service name from carrick.json for cross-repo resolution
    pub endpoints: Vec<ApiEndpointDetails>,
    pub calls: Vec<ApiEndpointDetails>,
    pub mounts: Vec<Mount>,
    pub apps: HashMap<String, AppContext>,
    pub imported_handlers: Vec<(String, String, String, String)>,
    pub function_definitions: HashMap<String, FunctionDefinition>,
    pub config_json: Option<String>,
    pub package_json: Option<String>,
    pub packages: Option<Packages>, // Structured package data for dependency analysis
    pub last_updated: DateTime<Utc>,
    pub commit_hash: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mount_graph: Option<MountGraph>, // Mount graph for framework-agnostic analysis
    /// Bundled TypeScript type definitions (.d.ts content)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bundled_types: Option<String>,
    /// Type manifest mapping endpoints/calls to their type aliases
    #[serde(skip_serializing_if = "Option::is_none")]
    pub type_manifest: Option<Vec<TypeManifestEntry>>,
    /// Cached per-file LLM analysis results for incremental re-analysis
    #[serde(skip_serializing_if = "Option::is_none")]
    pub file_results: Option<HashMap<String, FileAnalysisResult>>,
    /// Cached framework detection result
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cached_detection: Option<DetectionResult>,
    /// Cached framework guidance result
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cached_guidance: Option<FrameworkGuidance>,
    /// Hash of package.json content — if it matches, cached detection/guidance are reusable
    #[serde(skip_serializing_if = "Option::is_none")]
    pub package_json_hash: Option<String>,
    /// Cache format version — discard cached data if mismatched
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cache_version: Option<u32>,
}

impl CloudRepoData {
    /// Create CloudRepoData directly from multi-agent analysis results
    /// This bypasses the legacy Analyzer adapter layer
    pub fn from_multi_agent_results(
        repo_name: String,
        repo_path: &str,
        analysis_result: &MultiAgentAnalysisResult,
        config_json: Option<String>,
        package_json: Option<String>,
        packages: Option<Packages>,
        function_definitions: HashMap<String, FunctionDefinition>,
    ) -> Self {
        // Extract service_name from config_json if present
        let service_name = config_json.as_ref().and_then(|json| {
            serde_json::from_str::<serde_json::Value>(json)
                .ok()
                .and_then(|v| {
                    v.get("serviceName")
                        .and_then(|s| s.as_str())
                        .map(String::from)
                })
        });
        let mount_graph = &analysis_result.mount_graph;

        // Convert ResolvedEndpoints to ApiEndpointDetails
        let endpoints: Vec<ApiEndpointDetails> = mount_graph
            .get_resolved_endpoints()
            .iter()
            .map(|endpoint| ApiEndpointDetails {
                owner: Some(OwnerType::App(endpoint.owner.clone())),
                route: endpoint.full_path.clone(),
                method: endpoint.method.clone(),
                params: vec![],
                request_body: None,
                response_body: None,
                handler_name: endpoint.handler.clone(),
                request_type: None,
                response_type: None,
                file_path: PathBuf::from(&endpoint.file_location),
            })
            .collect();

        // Convert DataFetchingCalls to ApiEndpointDetails
        let calls: Vec<ApiEndpointDetails> = mount_graph
            .get_data_calls()
            .iter()
            .map(|call| ApiEndpointDetails {
                owner: None,
                route: call.target_url.clone(),
                method: call.method.clone(),
                params: vec![],
                request_body: None,
                response_body: None,
                handler_name: Some(call.client.clone()),
                request_type: None,
                response_type: None,
                file_path: PathBuf::from(&call.file_location),
            })
            .collect();

        // Convert MountEdges to Mount
        let mounts: Vec<Mount> = mount_graph
            .get_mounts()
            .iter()
            .map(|mount| Mount {
                parent: OwnerType::App(mount.parent.clone()),
                child: OwnerType::Router(mount.child.clone()),
                prefix: mount.path_prefix.clone(),
            })
            .collect();

        println!("Created CloudRepoData directly from multi-agent results:");
        println!("  - {} endpoints", endpoints.len());
        println!("  - {} calls", calls.len());
        println!("  - {} mounts", mounts.len());
        println!("  - {} function definitions", function_definitions.len());

        Self {
            repo_name,
            service_name,
            endpoints,
            calls,
            mounts,
            apps: HashMap::new(),
            imported_handlers: vec![],
            function_definitions,
            config_json,
            package_json,
            packages,
            last_updated: Utc::now(),
            commit_hash: get_current_commit_hash(repo_path),
            mount_graph: Some(mount_graph.clone()), // Store mount graph for cross-repo analysis
            bundled_types: None,
            type_manifest: None,
            file_results: None,
            cached_detection: None,
            cached_guidance: None,
            package_json_hash: None,
            cache_version: None,
        }
    }
}

#[derive(Debug)]
pub enum StorageError {
    ConnectionError(String),
    SerializationError(String),
    #[allow(dead_code)]
    NotFound(String),
    #[allow(dead_code)]
    DatabaseError(String),
}

impl std::fmt::Display for StorageError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            StorageError::ConnectionError(msg) => write!(f, "Connection error: {}", msg),
            StorageError::SerializationError(msg) => write!(f, "Serialization error: {}", msg),
            StorageError::NotFound(msg) => write!(f, "Not found: {}", msg),
            StorageError::DatabaseError(msg) => write!(f, "Database error: {}", msg),
        }
    }
}

impl Error for StorageError {}

#[async_trait]
pub trait CloudStorage {
    async fn upload_repo_data(&self, org: &str, data: &CloudRepoData) -> Result<(), StorageError>;
    async fn download_all_repo_data(
        &self,
        org: &str,
    ) -> Result<(Vec<CloudRepoData>, HashMap<String, String>), StorageError>; // Updated return type
    #[allow(dead_code)]
    async fn upload_type_file(
        &self,
        repo_name: &str,
        file_name: &str,
        content: &str,
    ) -> Result<(), StorageError>;
    async fn health_check(&self) -> Result<(), StorageError>;
}

pub fn get_current_commit_hash(repo_path: &str) -> String {
    std::process::Command::new("git")
        .args(["rev-parse", "HEAD"])
        .current_dir(repo_path)
        .output()
        .map(|output| String::from_utf8_lossy(&output.stdout).trim().to_string())
        .unwrap_or_else(|_| "unknown".to_string())
}
