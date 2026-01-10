//! Legacy data types for backward compatibility.
//!
//! These types were originally defined in the old agent modules (endpoint_agent,
//! consumer_agent, middleware_agent, mount_agent, orchestrator). They are preserved
//! here for tests and the mount_graph module that still use them.
//!
//! For new code, prefer using the file-centric types from `file_analyzer_agent.rs`.

use serde::{Deserialize, Serialize};

/// HTTP endpoint detected by analysis
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HttpEndpoint {
    pub method: String,
    pub path: String,
    pub handler: String,
    pub node_name: String,
    pub location: String,
    pub response_type_file: Option<String>,
    pub response_type_position: Option<usize>,
    pub response_type_string: Option<String>,
}

/// Data fetching call detected by analysis
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DataFetchingCall {
    pub callee: String,
    pub url: Option<String>,
    pub method: Option<String>,
    pub location: String,
    pub expected_type_file: Option<String>,
    pub expected_type_position: Option<usize>,
    pub expected_type_string: Option<String>,
}

/// Middleware registration detected by analysis
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Middleware {
    pub name: String,
    pub mount_path: Option<String>,
    pub node_name: String,
    pub location: String,
}

/// Mount relationship between routers/apps
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MountRelationship {
    pub parent_node: String,
    pub child_node: String,
    pub mount_path: String,
    pub location: String,
}

/// Statistics from the triage process (legacy)
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct TriageStats {
    pub total_call_sites: usize,
    pub endpoints_count: usize,
    pub data_fetching_count: usize,
    pub middleware_count: usize,
    pub router_mount_count: usize,
    pub irrelevant_count: usize,
}

/// Complete analysis results from all specialized agents (legacy format)
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AnalysisResults {
    pub endpoints: Vec<HttpEndpoint>,
    pub data_fetching_calls: Vec<DataFetchingCall>,
    pub middleware: Vec<Middleware>,
    pub mount_relationships: Vec<MountRelationship>,
    pub triage_stats: TriageStats,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_http_endpoint_serialization() {
        let endpoint = HttpEndpoint {
            method: "GET".to_string(),
            path: "/users".to_string(),
            handler: "getUsers".to_string(),
            node_name: "router".to_string(),
            location: "test.ts:10:5".to_string(),
            response_type_file: None,
            response_type_position: None,
            response_type_string: None,
        };

        let json = serde_json::to_string(&endpoint).unwrap();
        let deserialized: HttpEndpoint = serde_json::from_str(&json).unwrap();

        assert_eq!(deserialized.method, "GET");
        assert_eq!(deserialized.path, "/users");
    }

    #[test]
    fn test_analysis_results_default() {
        let results = AnalysisResults::default();

        assert!(results.endpoints.is_empty());
        assert!(results.data_fetching_calls.is_empty());
        assert!(results.middleware.is_empty());
        assert!(results.mount_relationships.is_empty());
        assert_eq!(results.triage_stats.total_call_sites, 0);
    }
}
