use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::error::Error;
use std::fmt;

#[derive(Debug)]
pub enum LambdaError {
    NetworkError(reqwest::Error),
    SerializationError(serde_json::Error),
    ApiError(String),
    S3Error(String),
}

impl fmt::Display for LambdaError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            LambdaError::NetworkError(e) => write!(f, "Network error: {}", e),
            LambdaError::SerializationError(e) => write!(f, "Serialization error: {}", e),
            LambdaError::ApiError(msg) => write!(f, "API error: {}", msg),
            LambdaError::S3Error(msg) => write!(f, "S3 error: {}", msg),
        }
    }
}

impl Error for LambdaError {}

impl From<reqwest::Error> for LambdaError {
    fn from(error: reqwest::Error) -> Self {
        LambdaError::NetworkError(error)
    }
}

impl From<serde_json::Error> for LambdaError {
    fn from(error: serde_json::Error) -> Self {
        LambdaError::SerializationError(error)
    }
}

#[derive(Serialize)]
pub struct CheckOrUploadRequest {
    pub repo: String,
    pub hash: String,
    pub org: String,
    pub filename: String,
}

#[derive(Deserialize, Debug)]
pub struct CheckOrUploadResponse {
    pub exists: bool,
    #[serde(rename = "uploadUrl")]
    pub upload_url: Option<String>,
    #[serde(rename = "s3Url")]
    pub s3_url: Option<String>,
    pub hash: Option<String>,
    #[serde(default)]
    pub adjacent: Vec<AdjacentRepo>,
}

#[derive(Deserialize, Debug, Clone)]
pub struct AdjacentRepo {
    pub repo: String,
    pub hash: String,
    #[serde(rename = "s3Url")]
    pub s3_url: String,
    pub filename: String,
}

#[derive(Serialize)]
pub struct CompleteUploadRequest {
    pub repo: String,
    pub org: String,
    pub hash: String,
    #[serde(rename = "s3Url")]
    pub s3_url: String,
    pub filename: String,
}

#[derive(Deserialize)]
pub struct CompleteUploadResponse {
    pub success: bool,
    pub message: Option<String>,
}

pub struct LambdaClient {
    client: Client,
    base_url: String,
}

impl LambdaClient {
    pub fn new(base_url: String) -> Self {
        Self {
            client: Client::new(),
            base_url,
        }
    }

    pub async fn check_or_upload(
        &self,
        org: &str,
        repo_name: &str,
        commit_hash: &str,
        api_key: &str,
    ) -> Result<CheckOrUploadResponse, LambdaError> {
        let url = format!("{}/types/check-or-upload", self.base_url);
        
        let request = CheckOrUploadRequest {
            repo: repo_name.to_string(),
            hash: commit_hash.to_string(),
            org: org.to_string(),
            filename: "types.tar.gz".to_string(),
        };

        println!("Calling check-or-upload API for repo: {} ({})", repo_name, commit_hash);

        let response = self
            .client
            .post(&url)
            .header("Authorization", format!("Bearer {}", api_key))
            .json(&request)
            .send()
            .await?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            return Err(LambdaError::ApiError(format!(
                "HTTP {}: {}", status, body
            )));
        }

        let result: CheckOrUploadResponse = response.json().await?;
        Ok(result)
    }

    pub async fn complete_upload(
        &self,
        org: &str,
        repo_name: &str,
        commit_hash: &str,
        s3_url: &str,
        api_key: &str,
    ) -> Result<CompleteUploadResponse, LambdaError> {
        let url = format!("{}/types/complete-upload", self.base_url);
        
        let request = CompleteUploadRequest {
            repo: repo_name.to_string(),
            hash: commit_hash.to_string(),
            org: org.to_string(),
            s3_url: s3_url.to_string(),
            filename: "types.tar.gz".to_string(),
        };

        println!("Calling complete-upload API for repo: {} ({})", repo_name, commit_hash);

        let response = self
            .client
            .post(&url)
            .header("Authorization", format!("Bearer {}", api_key))
            .json(&request)
            .send()
            .await?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            return Err(LambdaError::ApiError(format!(
                "HTTP {}: {}", status, body
            )));
        }

        let result: CompleteUploadResponse = response.json().await?;
        Ok(result)
    }

    pub async fn upload_types_to_s3(
        &self,
        upload_url: &str,
        output_dir: &str,
    ) -> Result<(), LambdaError> {
        use std::path::Path;
        use std::process::Command;

        let output_path = Path::new(output_dir);
        if !output_path.exists() {
            return Err(LambdaError::S3Error(format!(
                "Output directory does not exist: {}", output_dir
            )));
        }

        // Create a tarball of the output directory
        let tarball_path = "types_output.tar.gz";
        let tar_result = Command::new("tar")
            .args(&["-czf", tarball_path, "-C", output_dir, "."])
            .output()
            .map_err(|e| LambdaError::S3Error(format!("Failed to create tarball: {}", e)))?;

        if !tar_result.status.success() {
            let stderr = String::from_utf8_lossy(&tar_result.stderr);
            return Err(LambdaError::S3Error(format!(
                "Tar command failed: {}", stderr
            )));
        }

        // Read the tarball
        let tarball_data = std::fs::read(tarball_path)
            .map_err(|e| LambdaError::S3Error(format!("Failed to read tarball: {}", e)))?;

        println!("Uploading {} bytes to S3", tarball_data.len());

        // Upload to S3 using presigned URL
        let response = self
            .client
            .put(upload_url)
            .header("Content-Type", "application/gzip")
            .body(tarball_data)
            .send()
            .await?;

        // Clean up tarball
        let _ = std::fs::remove_file(tarball_path);

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            return Err(LambdaError::S3Error(format!(
                "S3 upload failed with status {}: {}", status, body
            )));
        }

        println!("Successfully uploaded types to S3");
        Ok(())
    }

    pub async fn download_adjacent_repo_types(
        &self,
        adjacent: &[AdjacentRepo],
        output_dir: &str,
    ) -> Result<(), LambdaError> {
        use std::path::Path;
        use std::process::Command;

        let output_path = Path::new(output_dir);
        
        // Ensure output directory exists
        if !output_path.exists() {
            std::fs::create_dir_all(output_path)
                .map_err(|e| LambdaError::S3Error(format!("Failed to create output directory: {}", e)))?;
        }

        for adjacent_repo in adjacent {
            println!("Downloading types for adjacent repo: {}", adjacent_repo.repo);

            // Download tarball from S3
            let response = self
                .client
                .get(&adjacent_repo.s3_url)
                .send()
                .await?;

            if !response.status().is_success() {
                println!("Warning: Failed to download types for repo {}: HTTP {}", 
                    adjacent_repo.repo, response.status());
                continue;
            }

            let tarball_data = response.bytes().await?;
            let tarball_path = format!("{}_types.tar.gz", adjacent_repo.repo);

            // Write tarball to disk
            std::fs::write(&tarball_path, tarball_data)
                .map_err(|e| LambdaError::S3Error(format!("Failed to write tarball: {}", e)))?;

            // Extract tarball to output directory
            let extract_result = Command::new("tar")
                .args(&["-xzf", &tarball_path, "-C", output_dir])
                .output()
                .map_err(|e| LambdaError::S3Error(format!("Failed to extract tarball: {}", e)))?;

            // Clean up tarball
            let _ = std::fs::remove_file(&tarball_path);

            if !extract_result.status.success() {
                let stderr = String::from_utf8_lossy(&extract_result.stderr);
                println!("Warning: Failed to extract types for repo {}: {}", 
                    adjacent_repo.repo, stderr);
                continue;
            }

            println!("Successfully downloaded and extracted types for repo: {}", adjacent_repo.repo);
        }

        Ok(())
    }
}

// Utility function to extract repo name from path
pub fn extract_repo_name(repo_path: &str) -> String {
    repo_path
        .trim_end_matches('/')
        .split('/')
        .last()
        .unwrap_or("default")
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_repo_name() {
        assert_eq!(extract_repo_name("../test_repos/repo-a/"), "repo-a");
        assert_eq!(extract_repo_name("../test_repos/repo-b"), "repo-b");
        assert_eq!(extract_repo_name("repo-c"), "repo-c");
        assert_eq!(extract_repo_name("."), ".");
    }
}