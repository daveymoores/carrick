use async_trait::async_trait;
use bson::doc;
use futures::stream::TryStreamExt;
use mongodb::{Client, Collection};
use std::env;
use crate::cloud_storage::{CloudRepoData, CloudStorage, StorageError};

pub struct MongoStorage {
    collection: Collection<bson::Document>,
}

impl MongoStorage {
    pub async fn new() -> Result<Self, StorageError> {
        let uri = env::var("MONGODB_URI")
            .map_err(|_| StorageError::ConnectionError("MONGODB_URI environment variable not set".to_string()))?;
        
        let client = Client::with_uri_str(&uri)
            .await
            .map_err(|e| StorageError::ConnectionError(format!("Failed to connect to MongoDB: {}", e)))?;
        
        let db = client.database("carrick");
        let collection = db.collection::<bson::Document>("repo_data");
        
        Ok(Self { collection })
    }
}

#[async_trait]
impl CloudStorage for MongoStorage {
    async fn upload_repo_data(&self, token: &str, data: &CloudRepoData) -> Result<(), StorageError> {
        let filter = doc! { 
            "token": token, 
            "repo_name": &data.repo_name 
        };
        
        let mut doc_data = bson::to_document(data)
            .map_err(|e| StorageError::SerializationError(format!("Failed to serialize data: {}", e)))?;
        
        doc_data.insert("token", token);
        
        self.collection
            .replace_one(filter, doc_data)
            .upsert(true)
            .await
            .map_err(|e| StorageError::DatabaseError(format!("Failed to upload data: {}", e)))?;
            
        Ok(())
    }
    
    async fn download_all_repo_data(&self, token: &str) -> Result<Vec<CloudRepoData>, StorageError> {
        let filter = doc! { "token": token };
        
        let cursor = self.collection
            .find(filter)
            .await
            .map_err(|e| StorageError::DatabaseError(format!("Failed to query data: {}", e)))?;
        
        let docs: Vec<bson::Document> = cursor
            .try_collect()
            .await
            .map_err(|e| StorageError::DatabaseError(format!("Failed to collect documents: {}", e)))?;
        
        let mut results = Vec::new();
        for doc in docs {
            let repo_data: CloudRepoData = bson::from_document(doc)
                .map_err(|e| StorageError::SerializationError(format!("Failed to deserialize data: {}", e)))?;
            results.push(repo_data);
        }
        
        Ok(results)
    }
    
    async fn health_check(&self) -> Result<(), StorageError> {
        self.collection
            .find_one(doc! {})
            .await
            .map_err(|e| StorageError::ConnectionError(format!("Health check failed: {}", e)))?;
        
        Ok(())
    }
}