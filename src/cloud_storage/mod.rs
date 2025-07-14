use crate::{
    analyzer::ApiEndpointDetails,
    app_context::AppContext,
    visitor::{FunctionDefinition, Mount},
};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::error::Error;

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
    pub last_updated: DateTime<Utc>,
    pub commit_hash: String,
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
