use crate::cloud_storage::{CloudRepoData, CloudStorage, StorageError};
use async_trait::async_trait;
use std::collections::HashMap;
use std::sync::Mutex;

pub struct MockStorage {
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
    async fn upload_repo_data(
        &self,
        token: &str,
        data: &CloudRepoData,
    ) -> Result<(), StorageError> {
        println!(
            "MOCK: Uploading repo data for token: {}, repo: {}",
            token, data.repo_name
        );
        let mut storage = self.data.lock().unwrap();
        storage
            .entry(token.to_string())
            .or_insert_with(Vec::new)
            .push(data.clone());
        Ok(())
    }

    async fn upload_type_file(
        &self,
        token: &str,
        repo_name: &str,
        file_name: &str,
        content: &str,
    ) -> Result<(), StorageError> {
        println!(
            "MOCK: Uploading type file for token: {}, repo: {}, file: {}",
            token, repo_name, file_name
        );
        let key = format!("{}:{}:{}", token, repo_name, file_name);
        let mut type_files = self.type_files.lock().unwrap();
        type_files.insert(key, content.to_string());
        Ok(())
    }

    async fn download_all_repo_data(
        &self,
        token: &str,
    ) -> Result<Vec<CloudRepoData>, StorageError> {
        println!("MOCK: Downloading all repo data for token: {}", token);
        let storage = self.data.lock().unwrap();
        let result = storage.get(token).cloned().unwrap_or_default();
        println!("MOCK: Found {} repos for token {}", result.len(), token);
        Ok(result)
    }

    async fn health_check(&self) -> Result<(), StorageError> {
        println!("MOCK: Health check passed");
        Ok(())
    }
}
