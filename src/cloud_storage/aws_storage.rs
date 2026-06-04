use crate::cloud_storage::{CloudRepoData, CloudStorage, StorageError};
use crate::oidc::OidcProvider;
use async_trait::async_trait;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use tracing::{debug, warn};

pub struct AwsStorage {
    lambda_url: String,
    http_client: Client,
    /// Whether the cloud advertises a service-aware index key (set from the
    /// health-check response). Until the cloud key includes a service
    /// discriminator this stays false, which gates multi-service uploads so
    /// they can't clobber each other.
    multi_service: std::sync::atomic::AtomicBool,
}

#[derive(Serialize)]
struct LambdaRequest {
    action: String,
    repo: String,
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
    /// Cloud capability: true once the index key includes a service
    /// discriminator, so multiple services per repo can coexist. Absent on
    /// older clouds, defaulting to false (gated).
    #[serde(default, rename = "multiService")]
    multi_service: bool,
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
}

impl AwsStorage {
    pub fn new() -> Result<Self, StorageError> {
        let api_endpoint = env!("CARRICK_API_ENDPOINT");
        let lambda_url = format!("{}/types/check-or-upload", api_endpoint);

        // Fail fast if OIDC isn't available — the cloud derives repo identity
        // from the signed OIDC claims, so there is no other way to authenticate.
        OidcProvider::global().map_err(|e| StorageError::ConnectionError(e.to_string()))?;

        Ok(Self {
            lambda_url,
            http_client: Client::new(),
            multi_service: std::sync::atomic::AtomicBool::new(false),
        })
    }

    /// POSTs a JSON body to the upload endpoint with the OIDC bearer header,
    /// returning the raw response body on success. OIDC tokens are short-lived,
    /// so on a 401 (token likely expired mid-run) we re-mint once and retry.
    async fn send_lambda<B>(&self, body: &B) -> Result<String, StorageError>
    where
        B: serde::Serialize + ?Sized,
    {
        let provider =
            OidcProvider::global().map_err(|e| StorageError::ConnectionError(e.to_string()))?;
        let mut token = provider
            .token()
            .await
            .map_err(|e| StorageError::ConnectionError(e.to_string()))?;

        let mut reminted = false;
        loop {
            let response = self
                .http_client
                .post(&self.lambda_url)
                .header("X-Carrick-OIDC", &token)
                .json(body)
                .send()
                .await
                .map_err(|e| {
                    StorageError::ConnectionError(format!("Lambda request failed: {}", e))
                })?;

            let status = response.status();
            let response_text = response.text().await.map_err(|e| {
                StorageError::ConnectionError(format!("Failed to read response: {}", e))
            })?;

            if status.as_u16() == 401 && !reminted {
                warn!("Upload returned 401; re-minting OIDC token and retrying once");
                token = provider
                    .remint()
                    .await
                    .map_err(|e| StorageError::ConnectionError(e.to_string()))?;
                reminted = true;
                continue;
            }

            if !status.is_success() {
                return Err(StorageError::ConnectionError(format!(
                    "Lambda returned error {}: {}",
                    status, response_text
                )));
            }

            return Ok(response_text);
        }
    }

    async fn call_lambda<T>(&self, request: &LambdaRequest) -> Result<T, StorageError>
    where
        T: for<'de> serde::Deserialize<'de>,
    {
        let response_text = self.send_lambda(request).await?;
        serde_json::from_str(&response_text).map_err(|e| {
            StorageError::SerializationError(format!(
                "Failed to parse lambda response for action '{}': {}. Raw response: {}",
                request.action, e, response_text
            ))
        })
    }

    async fn call_lambda_generic<Req, Resp>(&self, request: &Req) -> Result<Resp, StorageError>
    where
        Req: serde::Serialize,
        Resp: for<'de> serde::Deserialize<'de>,
    {
        let response_text = self.send_lambda(request).await?;
        serde_json::from_str(&response_text).map_err(|e| {
            StorageError::SerializationError(format!("Failed to parse lambda response: {}", e))
        })
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
            // Always include the response body — S3 returns the actual cause
            // (AccessDenied, signature mismatch, missing header, etc.) in the
            // XML error document. A bare status code is rarely actionable.
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            return Err(StorageError::ConnectionError(format!(
                "S3 upload returned {}: {}",
                status, body
            )));
        }

        Ok(())
    }

    async fn store_repo_metadata(
        &self,
        data: &CloudRepoData,
        s3_url: &str,
    ) -> Result<(), StorageError> {
        let request = LambdaRequest {
            action: "store-metadata".to_string(),
            repo: data.repo_name.clone(),
            hash: data.commit_hash.clone(),
            filename: "types.d.ts".to_string(),
            cloud_repo_data: Some(data.clone()),
            s3_url: Some(s3_url.to_string()),
        };

        let _response: StoreMetadataResponse = self.call_lambda(&request).await?;
        debug!("Successfully stored metadata for {}", data.repo_name);

        Ok(())
    }
}

#[async_trait]
impl CloudStorage for AwsStorage {
    async fn upload_repo_data(&self, data: &CloudRepoData) -> Result<(), StorageError> {
        let repo = &data.repo_name;

        // Step 1: Check if we need to upload type file
        let check_request = LambdaRequest {
            action: "check-or-upload".to_string(),
            repo: repo.clone(),
            hash: data.commit_hash.clone(),
            filename: "types.d.ts".to_string(),
            cloud_repo_data: None,
            s3_url: None,
        };

        let lambda_response: LambdaResponse = self.call_lambda(&check_request).await?;

        // Step 2: Upload type file if needed
        if let Some(upload_url) = lambda_response.upload_url {
            if let Some(bundled_types) = data.bundled_types.as_ref() {
                debug!("Uploading bundled types to S3...");
                self.upload_to_s3(&upload_url, bundled_types).await?;

                // Step 3: Complete the upload by storing metadata
                let complete_request = LambdaRequest {
                    action: "complete-upload".to_string(),
                    repo: repo.clone(),
                    hash: data.commit_hash.clone(),
                    filename: "types.d.ts".to_string(),
                    cloud_repo_data: Some(data.clone()),
                    s3_url: Some(lambda_response.s3_url),
                };

                let _complete_response: serde_json::Value =
                    self.call_lambda(&complete_request).await?;
                debug!("Successfully completed upload and stored metadata");
            } else {
                debug!(
                    "No bundled types available for {}; storing metadata only",
                    repo
                );
                self.store_repo_metadata(data, &lambda_response.s3_url)
                    .await?;
            }
        } else {
            debug!("Type file already exists, just updating metadata");
            self.store_repo_metadata(data, &lambda_response.s3_url)
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
        let commit_hash = crate::cloud_storage::get_current_commit_hash(".");

        let request = LambdaRequest {
            action: "check-or-upload".to_string(),
            repo: repo_name.to_string(),
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
    ) -> Result<(Vec<CloudRepoData>, HashMap<String, String>), StorageError> {
        let request = GetCrossRepoRequest {
            action: "get-cross-repo-data".to_string(),
        };

        let response: CrossRepoResponse = self.call_lambda_generic(&request).await?;

        let mut all_repo_data = Vec::new();
        let mut repo_s3_urls = HashMap::new();

        for adjacent in response.repos {
            if let Some(metadata) = adjacent.metadata {
                debug!("Processing repo: {} with full metadata", adjacent.repo);
                repo_s3_urls.insert(metadata.repo_name.clone(), adjacent.s3_url);
                all_repo_data.push(metadata);
            } else {
                warn!("No metadata found for repo: {}", adjacent.repo);
                let repo_data = CloudRepoData {
                    repo_name: adjacent.repo.clone(),
                    service_name: None,
                    endpoints: Vec::new(),
                    calls: Vec::new(),
                    mounts: Vec::new(),
                    apps: HashMap::new(),
                    imported_handlers: Vec::new(),
                    function_definitions: HashMap::new(),
                    config_json: None,
                    package_json: None,
                    packages: None,
                    last_updated: chrono::Utc::now(),
                    commit_hash: adjacent.hash,
                    mount_graph: None,
                    bundled_types: None,
                    type_manifest: None,
                    file_results: None,
                    cached_detection: None,
                    cached_guidance: None,
                    package_json_hash: None,
                    cache_version: None,
                };
                repo_s3_urls.insert(adjacent.repo.clone(), adjacent.s3_url);
                all_repo_data.push(repo_data);
            }
        }

        Ok((all_repo_data, repo_s3_urls))
    }

    async fn upload_logs(&self, repo: &str, log_content: &str) -> Result<(), StorageError> {
        let timestamp = chrono::Utc::now().format("%Y-%m-%dT%H-%M-%S").to_string();

        #[derive(Serialize)]
        struct UploadLogsRequest {
            action: String,
            repo: String,
            timestamp: String,
        }

        #[derive(Deserialize)]
        struct UploadLogsResponse {
            #[serde(rename = "uploadUrl")]
            upload_url: String,
        }

        let request = UploadLogsRequest {
            action: "upload-logs".to_string(),
            repo: repo.to_string(),
            timestamp,
        };

        let resp: UploadLogsResponse = self.call_lambda_generic(&request).await?;
        self.upload_to_s3(&resp.upload_url, log_content).await?;

        Ok(())
    }

    async fn post_pr_comment(
        &self,
        repo: &str,
        pr_number: u64,
        run_id: &str,
        body: &str,
    ) -> Result<(), StorageError> {
        // Dedicated action: unlike store-metadata/complete-upload it writes no
        // index data — the cloud only gates on the project's pr_comments_enabled
        // toggle and upserts the marked comment via the GitHub App. We keep the
        // rendered markdown as the source of truth here and let the cloud relay
        // it verbatim. `run_id` lets the cloud re-run this PR's workflow later
        // when a sibling repo's main changes.
        #[derive(Serialize)]
        struct PostPrCommentRequest<'a> {
            action: &'a str,
            repo: &'a str,
            pr_number: u64,
            #[serde(skip_serializing_if = "str::is_empty")]
            run_id: &'a str,
            pr_comment_body: &'a str,
        }

        let request = PostPrCommentRequest {
            action: "post-pr-comment",
            repo,
            pr_number,
            run_id,
            pr_comment_body: body,
        };

        // Best-effort by contract (caller logs and swallows), but surface the
        // transport error so the caller can log a useful message.
        self.send_lambda(&request).await?;
        debug!("Posted PR comment for {} (PR #{})", repo, pr_number);
        Ok(())
    }

    async fn health_check(&self) -> Result<(), StorageError> {
        let request = LambdaRequest {
            action: "check-or-upload".to_string(),
            repo: "health".to_string(),
            hash: "health-check".to_string(),
            filename: "health.ts".to_string(),
            cloud_repo_data: None,
            s3_url: None,
        };

        match self.call_lambda::<LambdaResponse>(&request).await {
            Ok(resp) => {
                // Record whether the cloud advertises a service-aware key, so
                // the multi-service upload gate can open without a scanner
                // release once the cloud deploys the key change.
                self.multi_service
                    .store(resp.multi_service, std::sync::atomic::Ordering::Relaxed);
                Ok(())
            }
            Err(StorageError::ConnectionError(msg))
                if msg.contains("401") || msg.contains("403") =>
            {
                Ok(()) // Lambda is responding, just rejecting our health check
            }
            Err(e) => Err(e),
        }
    }

    fn supports_multi_service(&self) -> bool {
        self.multi_service
            .load(std::sync::atomic::Ordering::Relaxed)
    }
}
