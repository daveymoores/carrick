use crate::{
    analyzer::ApiEndpointDetails,
    app_context::AppContext,
    mount_graph::MountGraph,
    multi_agent_orchestrator::MultiAgentAnalysisResult,
    packages::Packages,
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

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct CloudRepoData {
    pub repo_name: String,
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
}

impl CloudRepoData {
    /// Create CloudRepoData directly from multi-agent analysis results
    /// This bypasses the legacy Analyzer adapter layer
    pub fn from_multi_agent_results(
        repo_name: String,
        analysis_result: &MultiAgentAnalysisResult,
        config_json: Option<String>,
        package_json: Option<String>,
        packages: Option<Packages>,
    ) -> Self {
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

        Self {
            repo_name,
            endpoints,
            calls,
            mounts,
            apps: HashMap::new(),
            imported_handlers: vec![],
            function_definitions: HashMap::new(),
            config_json,
            package_json,
            packages,
            last_updated: Utc::now(),
            commit_hash: get_current_commit_hash(),
            mount_graph: Some(mount_graph.clone()), // Store mount graph for cross-repo analysis
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
    async fn download_type_file_content(&self, s3_url: &str) -> Result<String, StorageError>;
}

pub fn get_current_commit_hash() -> String {
    std::process::Command::new("git")
        .args(["rev-parse", "HEAD"])
        .output()
        .map(|output| String::from_utf8_lossy(&output.stdout).trim().to_string())
        .unwrap_or_else(|_| "unknown".to_string())
}
