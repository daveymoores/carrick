use crate::cloud_storage::{CloudRepoData, CloudStorage, StorageError};
use async_trait::async_trait;
use std::collections::HashMap;
use std::sync::Mutex;

pub struct MockStorage {
    // Group data by org, then store repos
    data: Mutex<HashMap<String, Vec<CloudRepoData>>>,
    type_files: Mutex<HashMap<String, String>>,
}

impl MockStorage {
    pub fn new() -> Self {
        Self {
            data: Mutex::new(HashMap::new()),
            type_files: Mutex::new(HashMap::new()),
        }
    }
}

#[async_trait]
impl CloudStorage for MockStorage {
    async fn download_type_file_content(&self, s3_url: &str) -> Result<String, StorageError> {
        println!("MOCK: Downloading type file from S3 URL: {}", s3_url);
        // Return some mock TypeScript content
        Ok(format!(
            "// Mock TypeScript content for {}\nexport interface MockType {{ id: string; }}",
            s3_url
        ))
    }
    async fn upload_repo_data(&self, org: &str, data: &CloudRepoData) -> Result<(), StorageError> {
        println!(
            "MOCK: Uploading repo data for org: {}, repo: {}",
            org, data.repo_name
        );
        let mut storage = self.data.lock().unwrap();
        storage
            .entry(org.to_string())
            .or_default()
            .push(data.clone());
        Ok(())
    }

    async fn upload_type_file(
        &self,
        repo_name: &str,
        file_name: &str,
        content: &str,
    ) -> Result<(), StorageError> {
        println!(
            "MOCK: Uploading type file for repo: {}, file: {}",
            repo_name, file_name
        );
        let key = format!("{}:{}", repo_name, file_name);
        let mut type_files = self.type_files.lock().unwrap();
        type_files.insert(key, content.to_string());
        Ok(())
    }

    async fn download_all_repo_data(
        &self,
        org: &str,
    ) -> Result<(Vec<CloudRepoData>, HashMap<String, String>), StorageError> {
        println!("MOCK: Downloading all repo data for org: {}", org);
        let storage = self.data.lock().unwrap();
        let mut result = storage.get(org).cloned().unwrap_or_default();

        // Add some mock repos to simulate cross-repo scenario
        if result.len() <= 1 {
            // Create mock repos for testing
            let mock_repos = vec![
                CloudRepoData {
                    repo_name: "repo-a".to_string(),
                    endpoints: vec![],
                    calls: vec![],
                    mounts: vec![],
                    apps: HashMap::new(),
                    imported_handlers: vec![],
                    function_definitions: HashMap::new(),
                    config_json: None,
                    package_json: None,
                    last_updated: chrono::Utc::now(),
                    commit_hash: "mock-hash-a".to_string(),
                },
                CloudRepoData {
                    repo_name: "repo-b".to_string(),
                    endpoints: vec![],
                    calls: vec![],
                    mounts: vec![],
                    apps: HashMap::new(),
                    imported_handlers: vec![],
                    function_definitions: HashMap::new(),
                    config_json: None,
                    package_json: None,
                    last_updated: chrono::Utc::now(),
                    commit_hash: "mock-hash-b".to_string(),
                },
            ];
            result.extend(mock_repos);
        }

        // Create mock S3 URLs
        let mut mock_s3_urls = HashMap::new();
        for repo_data in &result {
            mock_s3_urls.insert(
                repo_data.repo_name.clone(),
                format!("https://mock-s3.com/{}", repo_data.repo_name),
            );
        }

        println!("MOCK: Found {} repos for org {}", result.len(), org);
        Ok((result, mock_s3_urls))
    }

    async fn health_check(&self) -> Result<(), StorageError> {
        println!("MOCK: Health check passed");
        Ok(())
    }
}
