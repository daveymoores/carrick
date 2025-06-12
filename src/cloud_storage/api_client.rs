use crate::cloud_storage::{CloudRepoData, CloudStorage, StorageError};
use async_trait::async_trait;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::env;

#[derive(Serialize, Deserialize, Debug)]
struct CheckOrUploadRequest {
    repo: String,
    org: String,
    hash: String,
    filename: String,
}

#[derive(Serialize, Deserialize, Debug)]
struct CheckOrUploadResponse {
    exists: bool,
    #[serde(rename = "s3Url")]
    s3Url: Option<String>,
    #[serde(rename = "uploadUrl")]
    upload_url: Option<String>,
    hash: Option<String>,
}

#[derive(Serialize, Deserialize, Debug)]
struct CompleteUploadRequest {
    repo: String,
    org: String,
    hash: String,
    #[serde(rename = "s3Url")]
    s3Url: String,
    filename: String,
}

#[derive(Serialize, Deserialize, Debug)]
struct CompleteUploadResponse {
    success: bool,
    message: String,
    #[serde(rename = "s3Url")]
    s3Url: String,
}

pub struct ApiClient {
    client: Client,
    base_url: String,
    api_key: String,
}

impl ApiClient {
    pub fn new() -> Result<Self, StorageError> {
        let base_url = env!("CARRICK_API_ENDPOINT").to_string();

        let api_key = env::var("CARRICK_API_KEY").map_err(|_| {
            StorageError::ConnectionError(
                "CARRICK_API_KEY environment variable not set".to_string(),
            )
        })?;

        let client = Client::new();

        Ok(Self {
            client,
            base_url,
            api_key,
        })
    }

    pub async fn check_or_upload_types(
        &self,
        repo: &str,
        org: &str,
        hash: &str,
        filename: &str,
    ) -> Result<CheckOrUploadResponse, StorageError> {
        let url = format!("{}/types/check-or-upload", self.base_url);

        let request = CheckOrUploadRequest {
            repo: repo.to_string(),
            org: org.to_string(),
            hash: hash.to_string(),
            filename: filename.to_string(),
        };

        let response = self
            .client
            .post(&url)
            .header("Authorization", format!("Bearer {}", self.api_key))
            .header("Content-Type", "application/json")
            .json(&request)
            .send()
            .await
            .map_err(|e| StorageError::ConnectionError(format!("Failed to send request: {}", e)))?;

        if !response.status().is_success() {
            let status = response.status();
            let error_text = response
                .text()
                .await
                .unwrap_or_else(|_| "Unknown error".to_string());
            return Err(StorageError::DatabaseError(format!(
                "API request failed with status {}: {}",
                status, error_text
            )));
        }

        let check_response: CheckOrUploadResponse = response.json().await.map_err(|e| {
            StorageError::SerializationError(format!("Failed to parse response: {}", e))
        })?;

        Ok(check_response)
    }

    pub async fn upload_file_to_s3(
        &self,
        upload_url: &str,
        content: &str,
    ) -> Result<(), StorageError> {
        let response = self
            .client
            .put(upload_url)
            .header("Content-Type", "text/plain")
            .body(content.to_string())
            .send()
            .await
            .map_err(|e| StorageError::ConnectionError(format!("Failed to upload to S3: {}", e)))?;

        if !response.status().is_success() {
            let status = response.status();
            let error_text = response
                .text()
                .await
                .unwrap_or_else(|_| "Unknown error".to_string());
            return Err(StorageError::DatabaseError(format!(
                "S3 upload failed with status {}: {}",
                status, error_text
            )));
        }

        Ok(())
    }

    pub async fn complete_upload(
        &self,
        repo: &str,
        org: &str,
        hash: &str,
        s3Url: &str,
        filename: &str,
    ) -> Result<CompleteUploadResponse, StorageError> {
        let url = format!("{}/types/complete-upload", self.base_url);

        let request = CompleteUploadRequest {
            repo: repo.to_string(),
            org: org.to_string(),
            hash: hash.to_string(),
            s3Url: s3Url.to_string(),
            filename: filename.to_string(),
        };

        let response = self
            .client
            .post(&url)
            .header("Authorization", format!("Bearer {}", self.api_key))
            .header("Content-Type", "application/json")
            .json(&request)
            .send()
            .await
            .map_err(|e| StorageError::ConnectionError(format!("Failed to send request: {}", e)))?;

        if !response.status().is_success() {
            let status = response.status();
            let error_text = response
                .text()
                .await
                .unwrap_or_else(|_| "Unknown error".to_string());
            return Err(StorageError::DatabaseError(format!(
                "API request failed with status {}: {}",
                status, error_text
            )));
        }

        let complete_response: CompleteUploadResponse = response.json().await.map_err(|e| {
            StorageError::SerializationError(format!("Failed to parse response: {}", e))
        })?;

        Ok(complete_response)
    }

    pub async fn download_file_from_s3(&self, s3_url: &str) -> Result<String, StorageError> {
        let response = self.client.get(s3_url).send().await.map_err(|e| {
            StorageError::ConnectionError(format!("Failed to download from S3: {}", e))
        })?;

        if !response.status().is_success() {
            let status = response.status();
            return Err(StorageError::DatabaseError(format!(
                "Failed to download file from S3, status: {}",
                status
            )));
        }

        let content = response.text().await.map_err(|e| {
            StorageError::SerializationError(format!("Failed to read S3 content: {}", e))
        })?;

        Ok(content)
    }
}

// For now, we'll implement a minimal CloudStorage trait for compatibility
// This will need to be refactored as the new approach doesn't directly map to the old interface
#[async_trait]
impl CloudStorage for ApiClient {
    async fn upload_repo_data(
        &self,
        _token: &str,
        _data: &CloudRepoData,
    ) -> Result<(), StorageError> {
        // This method will be deprecated as we move to the new type-focused approach
        // For now, return success to maintain compatibility
        Ok(())
    }

    async fn download_all_repo_data(
        &self,
        _token: &str,
    ) -> Result<Vec<CloudRepoData>, StorageError> {
        // This method will be deprecated as we move to the new type-focused approach
        // For now, return empty vec to maintain compatibility
        Ok(Vec::new())
    }

    async fn upload_type_file(
        &self,
        _token: &str,
        repo_name: &str,
        file_name: &str,
        content: &str,
    ) -> Result<(), StorageError> {
        // Extract org and repo from repo_name if it contains "/"
        let (org, repo) = if repo_name.contains('/') {
            let parts: Vec<&str> = repo_name.split('/').collect();
            if parts.len() >= 2 {
                (parts[0], parts[1])
            } else {
                ("default", repo_name)
            }
        } else {
            ("default", repo_name)
        };

        // Generate a hash for the content (simplified approach)
        let hash = format!("{:x}", md5::compute(content.as_bytes()));

        // Check if types already exist
        let check_response = self
            .check_or_upload_types(repo, org, &hash, file_name)
            .await?;

        if check_response.exists {
            println!("Types already exist in cache for {}/{}", org, repo);
            return Ok(());
        }

        // Upload to S3 if needed
        if let Some(upload_url) = check_response.upload_url {
            println!("Uploading types to S3 for {}/{}", org, repo);
            self.upload_file_to_s3(&upload_url, content).await?;

            // Complete the upload
            if let Some(s3_url) = check_response.s3_url {
                self.complete_upload(repo, org, &hash, &s3_url, file_name)
                    .await?;
                println!(
                    "Successfully uploaded and registered types for {}/{}",
                    org, repo
                );
            }
        }

        Ok(())
    }

    async fn health_check(&self) -> Result<(), StorageError> {
        // Simple health check by making a request to the base URL
        let response = self
            .client
            .get(&self.base_url)
            .timeout(std::time::Duration::from_secs(5))
            .send()
            .await
            .map_err(|e| StorageError::ConnectionError(format!("Health check failed: {}", e)))?;

        if response.status().is_success() || response.status().as_u16() == 404 {
            // 404 is acceptable for health check as the base URL might not have a handler
            Ok(())
        } else {
            Err(StorageError::ConnectionError(format!(
                "Health check failed with status: {}",
                response.status()
            )))
        }
    }
}
