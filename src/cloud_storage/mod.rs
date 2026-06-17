use crate::{
    agents::{file_analyzer_agent::FileAnalysisResult, framework_guidance_agent::ProtocolGuidance},
    analyzer::ApiEndpointDetails,
    app_context::AppContext,
    framework_detector::DetectionResult,
    mount_graph::MountGraph,
    multi_agent_orchestrator::MultiAgentAnalysisResult,
    operation::OperationKey,
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
use tracing::debug;

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
    /// Operation identity. Flattened so the JSON keeps the flat
    /// `protocol`/`method`/`path` fields the ts_check matcher reads.
    #[serde(flatten)]
    pub key: OperationKey,
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
    /// Original declaration text as written (preserves named types for readability).
    /// Generated at CI time by the sidecar's DefinitionResolver.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resolved_definition: Option<String>,
    /// Compiler-expanded form with all types fully inlined.
    /// Generated at CI time via ts-morph's type.getText() with NoTruncation.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expanded_definition: Option<String>,
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
    /// Cached per-protocol framework guidance
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cached_guidance: Option<ProtocolGuidance>,
    /// Cached machinery-unwrap rules from the extraction_config task.
    /// Reusable under the same `package_json_hash` gate as detection/guidance
    /// (its inputs are the detected stack + dependency names).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cached_extraction_config: Option<crate::services::type_sidecar::ExtractionConfig>,
    /// Hash of package.json content — if it matches, cached detection/guidance are reusable
    #[serde(skip_serializing_if = "Option::is_none")]
    pub package_json_hash: Option<String>,
    /// Cache format version — discard cached data if mismatched
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cache_version: Option<u32>,
    /// Why type extraction was skipped or failed for this service, if it was.
    /// `None` means types were resolved normally. Set so the index records
    /// that this service's data is degraded (endpoints without types) instead
    /// of that being indistinguishable from "endpoints have no types".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub type_extraction_status: Option<String>,
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
                key: OperationKey::http(&endpoint.method, endpoint.full_path.clone()),
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
                key: OperationKey::http(&call.method, call.target_url.clone()),
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

        debug!(
            endpoints = endpoints.len(),
            calls = calls.len(),
            mounts = mounts.len(),
            function_definitions = function_definitions.len(),
            "Created CloudRepoData directly from multi-agent results"
        );

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
            cached_extraction_config: None,
            package_json_hash: None,
            cache_version: None,
            type_extraction_status: None,
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
    async fn upload_repo_data(&self, data: &CloudRepoData) -> Result<(), StorageError>;

    /// Whether this backend can store more than one service per git repo
    /// without collision. The production index keys on
    /// (workspace, project, repo) only — no service discriminator — so real
    /// uploads of multiple services from one repo would overwrite each other.
    /// Mock storage records uploads in memory and is safe, so it overrides
    /// this. Gates multi-service index upload in the engine.
    fn supports_multi_service(&self) -> bool {
        false
    }

    async fn download_all_repo_data(
        &self,
    ) -> Result<(Vec<CloudRepoData>, HashMap<String, String>), StorageError>;
    #[allow(dead_code)]
    async fn upload_type_file(
        &self,
        repo_name: &str,
        file_name: &str,
        content: &str,
    ) -> Result<(), StorageError>;
    async fn health_check(&self) -> Result<(), StorageError>;
    async fn upload_logs(&self, repo: &str, log_content: &str) -> Result<(), StorageError>;

    /// Relay a rendered PR-comment body to the cloud, which posts (and
    /// updates in place on later pushes) a single GitHub App comment on the
    /// PR. Only called on `pull_request` runs — index data is deliberately
    /// not uploaded there, so this is the one signal a PR run sends.
    ///
    /// `run_id` is the GitHub Actions run id of this PR check; the cloud
    /// records it so a later sibling main change can re-run this exact run and
    /// refresh the comment (see carrick-cloud docs/internal/fanout-pr-rerun.md).
    async fn post_pr_comment(
        &self,
        repo: &str,
        pr_number: u64,
        run_id: &str,
        body: &str,
    ) -> Result<(), StorageError>;
}

pub fn get_current_commit_hash(repo_path: &str) -> String {
    std::process::Command::new("git")
        .args(["rev-parse", "HEAD"])
        .current_dir(repo_path)
        .output()
        .map(|output| String::from_utf8_lossy(&output.stdout).trim().to_string())
        .unwrap_or_else(|_| "unknown".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::services::type_sidecar::InferKind;

    /// The flattened key is the wire contract with ts_check's manifest
    /// matcher: entries must serialize with flat `protocol`/`method`/`path`
    /// fields, and round-trip back into the tagged key.
    #[test]
    fn manifest_entry_serializes_flat_protocol_fields() {
        let key = OperationKey::http("GET", "/api/users/:id");
        let entry = TypeManifestEntry {
            key: key.clone(),
            role: ManifestRole::Producer,
            type_kind: ManifestTypeKind::Response,
            type_alias: "Endpoint_abc_Response".to_string(),
            file_path: "src/routes.ts".to_string(),
            line_number: 12,
            is_explicit: false,
            type_state: ManifestTypeState::Unknown,
            evidence: TypeEvidence {
                file_path: "src/routes.ts".to_string(),
                span_start: None,
                span_end: None,
                line_number: 12,
                infer_kind: InferKind::ResponseBody,
                is_explicit: false,
                type_state: ManifestTypeState::Unknown,
            },
            resolved_definition: None,
            expanded_definition: None,
        };

        let json: serde_json::Value = serde_json::to_value(&entry).unwrap();
        assert_eq!(json["protocol"], "http");
        assert_eq!(json["method"], "GET");
        assert_eq!(json["path"], "/api/users/:id");

        let back: TypeManifestEntry = serde_json::from_value(json).unwrap();
        assert_eq!(back.key, key);
    }
}
