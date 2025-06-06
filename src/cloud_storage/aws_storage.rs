use crate::cloud_storage::{CloudRepoData, CloudStorage, StorageError};
use async_trait::async_trait;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::env;

pub struct AwsStorage {
    lambda_url: String,
    http_client: Client,
    api_key: String,
}

#[derive(Serialize)]
struct LambdaRequest {
    action: String,
    repo: String,
    org: String,
    hash: String,
    filename: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    cloudRepoData: Option<CloudRepoData>,
    #[serde(skip_serializing_if = "Option::is_none")]
    s3Url: Option<String>,
}

#[derive(Deserialize)]
struct LambdaResponse {
    exists: bool,
    s3_url: String,
    upload_url: Option<String>,
    hash: String,
    adjacent: Vec<AdjacentRepo>,
}

#[derive(Deserialize)]
struct StoreMetadataResponse {
    success: bool,
    message: String,
}

#[derive(Deserialize)]
struct AdjacentRepo {
    repo: String,
    hash: String,
    s3_url: String,
    filename: String,
    metadata: Option<CloudRepoData>, // Now includes full metadata!
}

#[derive(Deserialize)]
struct CrossRepoResponse {
    repos: Vec<AdjacentRepo>,
}

impl AwsStorage {
    pub fn new() -> Result<Self, StorageError> {
        let lambda_url = env::var("CARRICK_LAMBDA_URL").map_err(|_| {
            StorageError::ConnectionError(
                "CARRICK_LAMBDA_URL environment variable not set".to_string(),
            )
        })?;

        let api_key = env::var("CARRICK_API_KEY").map_err(|_| {
            StorageError::ConnectionError(
                "CARRICK_API_KEY environment variable not set".to_string(),
            )
        })?;

        Ok(Self {
            lambda_url,
            http_client: Client::new(),
            api_key,
        })
    }

    async fn call_lambda<T>(&self, request: &LambdaRequest) -> Result<T, StorageError>
    where
        T: for<'de> serde::Deserialize<'de>,
    {
        let response = self
            .http_client
            .post(&self.lambda_url)
            .header("Authorization", format!("Bearer {}", self.api_key))
            .json(request)
            .send()
            .await
            .map_err(|e| StorageError::ConnectionError(format!("Lambda request failed: {}", e)))?;

        if !response.status().is_success() {
            let error_text = response
                .text()
                .await
                .unwrap_or_else(|_| "Unknown error".to_string());
            return Err(StorageError::ConnectionError(format!(
                "Lambda returned error {}: {}",
                response.status(),
                error_text
            )));
        }

        let lambda_response: T = response.json().await.map_err(|e| {
            StorageError::SerializationError(format!("Failed to parse lambda response: {}", e))
        })?;

        Ok(lambda_response)
    }

    async fn upload_to_s3(&self, upload_url: &str, content: &str) -> Result<(), StorageError> {
        let response = self
            .http_client
            .put(upload_url)
            .header("Content-Type", "text/plain")
            .body(content.to_string())
            .send()
            .await
            .map_err(|e| StorageError::ConnectionError(format!("S3 upload failed: {}", e)))?;

        if !response.status().is_success() {
            return Err(StorageError::ConnectionError(format!(
                "S3 upload returned error: {}",
                response.status()
            )));
        }

        Ok(())
    }

    async fn download_from_s3(&self, s3_url: &str) -> Result<String, StorageError> {
        let response = self
            .http_client
            .get(s3_url)
            .send()
            .await
            .map_err(|e| StorageError::ConnectionError(format!("S3 download failed: {}", e)))?;

        if !response.status().is_success() {
            return Err(StorageError::ConnectionError(format!(
                "S3 download returned error: {}",
                response.status()
            )));
        }

        let content = response.text().await.map_err(|e| {
            StorageError::SerializationError(format!("Failed to read S3 content: {}", e))
        })?;

        Ok(content)
    }

    fn extract_org_and_repo(&self, repo_name: &str) -> (String, String) {
        if let Some((org, repo)) = repo_name.split_once('/') {
            (org.to_string(), repo.to_string())
        } else {
            ("default".to_string(), repo_name.to_string())
        }
    }

    async fn store_repo_metadata(&self, data: &CloudRepoData) -> Result<(), StorageError> {
        let (org, repo) = self.extract_org_and_repo(&data.repo_name);

        let request = LambdaRequest {
            action: "store-metadata".to_string(),
            repo,
            org,
            hash: data.commit_hash.clone(),
            filename: "types.ts".to_string(),
            cloudRepoData: Some(data.clone()),
        };

        let _response: StoreMetadataResponse = self.call_lambda(&request).await?;
        println!("Successfully stored metadata for {}", data.repo_name);

        Ok(())
    }
}

#[async_trait]
impl CloudStorage for AwsStorage {
    async fn upload_repo_data(
        &self,
        _token: &str,
        data: &CloudRepoData,
    ) -> Result<(), StorageError> {
        let (org, repo) = self.extract_org_and_repo(&data.repo_name);

        // Step 1: Check if we need to upload type file
        let check_request = LambdaRequest {
            action: "check-or-upload".to_string(),
            repo: repo.clone(),
            org: org.clone(),
            hash: data.commit_hash.clone(),
            filename: "types.ts".to_string(),
            cloudRepoData: None,
            s3Url: None,
        };

        let lambda_response: LambdaResponse = self.call_lambda(&check_request).await?;

        // Step 2: Upload type file if needed
        if let Some(upload_url) = lambda_response.upload_url {
            if let Some(ts_file_path) = find_generated_typescript_file(".") {
                let type_file_content = std::fs::read_to_string(&ts_file_path).map_err(|e| {
                    StorageError::SerializationError(format!(
                        "Failed to read TypeScript file: {}",
                        e
                    ))
                })?;

                println!("Uploading type file to S3...");
                self.upload_to_s3(&upload_url, &type_file_content).await?;

                // Step 3: Complete the upload by storing metadata
                let complete_request = LambdaRequest {
                    action: "complete-upload".to_string(),
                    repo,
                    org,
                    hash: data.commit_hash.clone(),
                    filename: "types.ts".to_string(),
                    cloudRepoData: Some(data.clone()),
                    s3Url: Some(lambda_response.s3_url), // Provide the s3_url
                };

                let _complete_response: serde_json::Value =
                    self.call_lambda(&complete_request).await?;
                println!("Successfully completed upload and stored metadata");
            }
        } else {
            println!("Type file already exists, just updating metadata");
            // Use store-metadata instead of complete-upload for existing files
            self.store_repo_metadata(data).await?;
        }

        Ok(())
    }

    async fn upload_type_file(
        &self,
        _token: &str,
        repo_name: &str,
        file_name: &str,
        content: &str,
    ) -> Result<(), StorageError> {
        let (org, repo) = self.extract_org_and_repo(repo_name);
        let commit_hash = crate::cloud_storage::get_current_commit_hash();

        let request = LambdaRequest {
            action: "check-or-upload".to_string(),
            repo,
            org,
            hash: commit_hash,
            filename: file_name.to_string(),
            cloudRepoData: None,
        };

        let lambda_response: LambdaResponse = self.call_lambda(&request).await?;

        if let Some(upload_url) = lambda_response.upload_url {
            self.upload_to_s3(&upload_url, content).await?;
        }

        Ok(())
    }

    async fn download_all_repo_data(
        &self,
        _token: &str,
    ) -> Result<Vec<CloudRepoData>, StorageError> {
        // Get current repo info to determine org
        let current_repo_name = std::env::current_dir()
            .map_err(|_| {
                StorageError::ConnectionError("Could not determine current directory".to_string())
            })?
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("unknown")
            .to_string();

        let (org, _) = self.extract_org_and_repo(&current_repo_name);

        // Use the new get-cross-repo-data action
        let request = LambdaRequest {
            action: "get-cross-repo-data".to_string(),
            repo: "".to_string(), // Not needed for this action
            org,
            hash: "".to_string(),     // Not needed for this action
            filename: "".to_string(), // Not needed for this action
            cloudRepoData: None,
        };

        let response: CrossRepoResponse = self.call_lambda(&request).await?;

        let mut all_repo_data = Vec::new();

        for adjacent in response.repos {
            if let Some(metadata) = adjacent.metadata {
                // We have the full metadata! Just need to download type file if needed
                println!("Processing repo: {} with full metadata", adjacent.repo);
                all_repo_data.push(metadata);
            } else {
                // Fallback: create minimal CloudRepoData (shouldn't happen with new implementation)
                println!("Warning: No metadata found for repo: {}", adjacent.repo);
                let repo_data = CloudRepoData {
                    repo_name: adjacent.repo.clone(),
                    endpoints: Vec::new(),
                    calls: Vec::new(),
                    mounts: Vec::new(),
                    apps: HashMap::new(),
                    imported_handlers: Vec::new(),
                    function_definitions: HashMap::new(),
                    config_json: None,
                    package_json: None,
                    extracted_types: Vec::new(),
                    last_updated: chrono::Utc::now(),
                    commit_hash: adjacent.hash,
                };
                all_repo_data.push(repo_data);
            }
        }

        Ok(all_repo_data)
    }

    async fn health_check(&self) -> Result<(), StorageError> {
        let request = LambdaRequest {
            action: "check-or-upload".to_string(),
            repo: "health".to_string(),
            org: "check".to_string(),
            hash: "health-check".to_string(),
            filename: "health.ts".to_string(),
            cloudRepoData: None,
        };

        // We expect this to fail with 401/403, but not a connection error
        match self.call_lambda::<LambdaResponse>(&request).await {
            Ok(_) => Ok(()),
            Err(StorageError::ConnectionError(msg))
                if msg.contains("401") || msg.contains("403") =>
            {
                Ok(()) // Lambda is responding, just rejecting our health check
            }
            Err(e) => Err(e),
        }
    }
}

// Helper function
fn find_generated_typescript_file(repo_path: &str) -> Option<String> {
    use std::fs;
    use std::path::Path;

    let output_dir = Path::new(repo_path).join("ts_check/output");
    if output_dir.exists() {
        if let Ok(entries) = fs::read_dir(output_dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if let Some(ext) = path.extension() {
                    if ext == "ts" {
                        return Some(path.to_string_lossy().to_string());
                    }
                }
            }
        }
    }
    None
}
