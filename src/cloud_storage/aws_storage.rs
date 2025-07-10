use crate::cloud_storage::{CloudRepoData, CloudStorage, StorageError};
use crate::utils::get_repository_name;
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
    #[serde(rename = "cloudRepoData")]
    cloud_repo_data: Option<CloudRepoData>,
    #[serde(rename = "s3Url")]
    #[serde(skip_serializing_if = "Option::is_none")]
    s3_url: Option<String>,
}

#[derive(Deserialize)]
struct LambdaResponse {
    #[allow(dead_code)]
    exists: bool,
    #[serde(rename = "s3Url")]
    s3_url: String,
    #[serde(rename = "uploadUrl")]
    #[allow(dead_code)]
    upload_url: Option<String>,
    #[allow(dead_code)]
    hash: String,
    #[serde(default)]
    #[allow(dead_code)]
    adjacent: Vec<AdjacentRepo>,
}

#[derive(Deserialize)]
struct StoreMetadataResponse {
    #[allow(dead_code)]
    success: bool,
    #[allow(dead_code)]
    message: String,
}

#[derive(Deserialize)]
struct AdjacentRepo {
    repo: String,
    hash: String,
    #[serde(rename = "s3Url")]
    s3_url: String,
    #[allow(dead_code)]
    filename: String,
    metadata: Option<CloudRepoData>, // Now includes full metadata!
    #[serde(rename = "lastUpdated")]
    #[allow(dead_code)]
    last_updated: Option<String>,
}

#[derive(Deserialize)]
struct CrossRepoResponse {
    repos: Vec<AdjacentRepo>,
}

#[derive(Serialize)]
struct GetCrossRepoRequest {
    action: String,
    org: String,
}

impl AwsStorage {
    pub fn new() -> Result<Self, StorageError> {
        let api_endpoint = env!("CARRICK_API_ENDPOINT");
        let lambda_url = format!("{}/types/check-or-upload", api_endpoint);

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

        let status = response.status();

        if !response.status().is_success() {
            let error_text = response
                .text()
                .await
                .unwrap_or_else(|_| "Unknown error".to_string());
            return Err(StorageError::ConnectionError(format!(
                "Lambda returned error {}: {}",
                status, error_text
            )));
        }

        let response_text = response.text().await.map_err(|e| {
            StorageError::ConnectionError(format!("Failed to read response: {}", e))
        })?;

        let lambda_response: T = serde_json::from_str(&response_text).map_err(|e| {
            StorageError::SerializationError(format!(
                "Failed to parse lambda response for action '{}': {}. Raw response: {}",
                request.action, e, response_text
            ))
        })?;

        Ok(lambda_response)
    }

    async fn call_lambda_generic<Req, Resp>(&self, request: &Req) -> Result<Resp, StorageError>
    where
        Req: serde::Serialize,
        Resp: for<'de> serde::Deserialize<'de>,
    {
        let response = self
            .http_client
            .post(&self.lambda_url)
            .header("Authorization", format!("Bearer {}", self.api_key))
            .json(request)
            .send()
            .await
            .map_err(|e| StorageError::ConnectionError(format!("Lambda request failed: {}", e)))?;

        let status = response.status();

        if !response.status().is_success() {
            let error_text = response
                .text()
                .await
                .unwrap_or_else(|_| "Unknown error".to_string());
            return Err(StorageError::ConnectionError(format!(
                "Lambda returned error {}: {}",
                status, error_text
            )));
        }

        let response_text = response.text().await.map_err(|e| {
            StorageError::ConnectionError(format!("Failed to read response: {}", e))
        })?;

        let lambda_response: Resp = serde_json::from_str(&response_text).map_err(|e| {
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
        #[derive(Serialize)]
        struct DownloadRequest {
            action: String,
            #[serde(rename = "s3Url")]
            s3_url: String,
        }

        #[derive(Deserialize)]
        struct DownloadResponse {
            content: String,
        }

        let request = DownloadRequest {
            action: "download-file".to_string(),
            s3_url: s3_url.to_string(),
        };

        let response: DownloadResponse = self.call_lambda_generic(&request).await?;
        Ok(response.content)
    }

    fn extract_org_and_repo(&self, repo_name: &str) -> (String, String) {
        if let Some((org, repo)) = repo_name.split_once('/') {
            (org.to_string(), repo.to_string())
        } else {
            ("default".to_string(), repo_name.to_string())
        }
    }

    async fn store_repo_metadata(
        &self,
        data: &CloudRepoData,
        s3_url: &str,
        org: &str, // Add org parameter
    ) -> Result<(), StorageError> {
        let request = LambdaRequest {
            action: "store-metadata".to_string(),
            repo: data.repo_name.clone(), // Use repo name as-is
            org: org.to_string(),         // Use passed org
            hash: data.commit_hash.clone(),
            filename: "types.ts".to_string(),
            cloud_repo_data: Some(data.clone()),
            s3_url: Some(s3_url.to_string()),
        };

        let _response: StoreMetadataResponse = self.call_lambda(&request).await?;
        println!("Successfully stored metadata for {}", data.repo_name);

        Ok(())
    }
}

#[async_trait]
impl CloudStorage for AwsStorage {
    async fn download_type_file_content(&self, s3_url: &str) -> Result<String, StorageError> {
        self.download_from_s3(s3_url).await
    }
    async fn upload_repo_data(&self, org: &str, data: &CloudRepoData) -> Result<(), StorageError> {
        let repo = &data.repo_name;

        // Step 1: Check if we need to upload type file
        let check_request = LambdaRequest {
            action: "check-or-upload".to_string(),
            repo: repo.clone(),
            org: org.to_string(),
            hash: data.commit_hash.clone(),
            filename: "types.ts".to_string(),
            cloud_repo_data: None,
            s3_url: None,
        };

        let lambda_response: LambdaResponse = self.call_lambda(&check_request).await?;

        // Step 2: Upload type file if needed
        if let Some(upload_url) = lambda_response.upload_url {
            if let Some(ts_file_path) = find_generated_typescript_file(&data.repo_name) {
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
                    repo: repo.clone(),
                    org: org.to_string(),
                    hash: data.commit_hash.clone(),
                    filename: "types.ts".to_string(),
                    cloud_repo_data: Some(data.clone()),
                    s3_url: Some(lambda_response.s3_url),
                };

                let _complete_response: serde_json::Value =
                    self.call_lambda(&complete_request).await?;
                println!("Successfully completed upload and stored metadata");
            }
        } else {
            println!("Type file already exists, just updating metadata");
            self.store_repo_metadata(data, &lambda_response.s3_url, org)
                .await?;
        }

        Ok(())
    }

    async fn upload_type_file(
        &self,
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
            cloud_repo_data: None,
            s3_url: None,
        };

        let lambda_response: LambdaResponse = self.call_lambda(&request).await?;

        if let Some(upload_url) = lambda_response.upload_url {
            self.upload_to_s3(&upload_url, content).await?;
        }

        Ok(())
    }

    async fn download_all_repo_data(
        &self,
        org: &str,
    ) -> Result<(Vec<CloudRepoData>, HashMap<String, String>), StorageError> {
        let request = GetCrossRepoRequest {
            action: "get-cross-repo-data".to_string(),
            org: org.to_string(),
        };

        let response: CrossRepoResponse = self.call_lambda_generic(&request).await?;

        let mut all_repo_data = Vec::new();
        let mut repo_s3_urls = HashMap::new();

        for adjacent in response.repos {
            if let Some(metadata) = adjacent.metadata {
                println!("Processing repo: {} with full metadata", adjacent.repo);
                repo_s3_urls.insert(metadata.repo_name.clone(), adjacent.s3_url);
                all_repo_data.push(metadata);
            } else {
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
                    last_updated: chrono::Utc::now(),
                    commit_hash: adjacent.hash,
                };
                repo_s3_urls.insert(adjacent.repo.clone(), adjacent.s3_url);
                all_repo_data.push(repo_data);
            }
        }

        Ok((all_repo_data, repo_s3_urls))
    }

    async fn health_check(&self) -> Result<(), StorageError> {
        let request = LambdaRequest {
            action: "check-or-upload".to_string(),
            repo: "health".to_string(),
            org: "check".to_string(),
            hash: "health-check".to_string(),
            filename: "health.ts".to_string(),
            cloud_repo_data: None,
            s3_url: None,
        };

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
fn find_generated_typescript_file(repo_name: &str) -> Option<String> {
    use std::path::Path;

    // Use the shared repository name extraction logic
    let actual_repo_name = get_repository_name(repo_name);

    let expected_filename = format!("{}_types.ts", actual_repo_name);
    let expected_path = Path::new(".")
        .join("ts_check/output")
        .join(&expected_filename);

    if expected_path.exists() {
        Some(expected_path.to_string_lossy().to_string())
    } else {
        None
    }
}
