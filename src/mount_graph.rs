use crate::call_site_classifier::{CallSiteType, ClassifiedCallSite};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};

/// Represents a node in the mount graph (router or app)
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct GraphNode {
    pub name: String,
    pub node_type: NodeType,
    pub creation_site: Option<String>,
    pub file_location: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum NodeType {
    App,
    Router,
    Unknown,
}

/// Represents a mount relationship between nodes
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MountEdge {
    pub parent: String,
    pub child: String,
    pub path_prefix: String,
    pub middleware_stack: Vec<String>,
}

/// Represents an HTTP endpoint with its full computed path
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResolvedEndpoint {
    pub method: String,
    pub path: String,
    pub full_path: String,
    pub handler: Option<String>,
    pub owner: String,
    pub file_location: String,
    pub middleware_chain: Vec<String>,
}

/// Represents a data-fetching call with its target
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DataFetchingCall {
    pub method: String,
    pub target_url: String,
    pub client: String,
    pub file_location: String,
}

/// The complete mount and endpoint graph
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MountGraph {
    pub nodes: HashMap<String, GraphNode>,
    pub mounts: Vec<MountEdge>,
    pub endpoints: Vec<ResolvedEndpoint>,
    pub data_calls: Vec<DataFetchingCall>,
}

impl MountGraph {
    pub fn new() -> Self {
        Self {
            nodes: HashMap::new(),
            mounts: Vec::new(),
            endpoints: Vec::new(),
            data_calls: Vec::new(),
        }
    }

    /// Build the mount graph from classified call sites
    pub fn build_from_classified_sites(classified_sites: Vec<ClassifiedCallSite>) -> Self {
        let mut graph = Self::new();
        
        // First pass: collect all nodes and basic information
        graph.collect_nodes(&classified_sites);
        
        // Second pass: build mount relationships
        graph.build_mount_edges(&classified_sites);
        
        // Third pass: collect endpoints and data calls
        graph.collect_endpoints(&classified_sites);
        graph.collect_data_calls(&classified_sites);
        
        // Fourth pass: resolve full paths for all endpoints
        graph.resolve_endpoint_paths();
        
        graph
    }

    fn collect_nodes(&mut self, classified_sites: &[ClassifiedCallSite]) {
        for site in classified_sites {
            let node_name = site.call_site.callee_object.clone();
            
            if !self.nodes.contains_key(&node_name) {
                let node_type = self.infer_node_type(&site.call_site.definition);
                
                self.nodes.insert(node_name.clone(), GraphNode {
                    name: node_name,
                    node_type,
                    creation_site: site.call_site.definition.clone(),
                    file_location: site.call_site.location.clone(),
                });
            }
        }
    }

    fn infer_node_type(&self, definition: &Option<String>) -> NodeType {
        if let Some(def) = definition {
            if def.contains("express()") || def.contains("app()") || def.contains("fastify()") {
                NodeType::App
            } else if def.contains("Router()") || def.contains("router()") {
                NodeType::Router
            } else {
                NodeType::Unknown
            }
        } else {
            NodeType::Unknown
        }
    }

    fn build_mount_edges(&mut self, classified_sites: &[ClassifiedCallSite]) {
        for site in classified_sites {
            if matches!(site.classification, CallSiteType::RouterMount) {
                if let Some(mount) = self.extract_mount_relationship(site) {
                    self.mounts.push(mount);
                }
            }
        }
    }

    fn extract_mount_relationship(&self, site: &ClassifiedCallSite) -> Option<MountEdge> {
        // Use LLM-extracted mount information instead of heuristic parsing
        let parent = site.mount_parent.as_ref()?;
        let child = site.mount_child.as_ref()?;
        let path_prefix = site.mount_prefix.as_ref().unwrap_or(&"/".to_string()).clone();
        
        // TODO: Extract middleware stack from arguments if needed
        // For now, use empty middleware stack since LLM doesn't extract this yet
        let middleware_stack = Vec::new();
        
        Some(MountEdge {
            parent: parent.clone(),
            child: child.clone(),
            path_prefix,
            middleware_stack,
        })
    }

    fn collect_endpoints(&mut self, classified_sites: &[ClassifiedCallSite]) {
        for site in classified_sites {
            if matches!(site.classification, CallSiteType::HttpEndpoint) {
                if let Some(endpoint) = self.extract_endpoint(site) {
                    self.endpoints.push(endpoint);
                }
            }
        }
    }

    fn extract_endpoint(&self, site: &ClassifiedCallSite) -> Option<ResolvedEndpoint> {
        let method = site.call_site.callee_property.to_uppercase();
        let owner = site.call_site.callee_object.clone();
        
        // Extract path from first string argument (fallback to heuristic if needed)
        let path = site.call_site.args.iter()
            .find_map(|arg| {
                if matches!(arg.arg_type, crate::call_site_extractor::ArgumentType::StringLiteral) {
                    arg.value.clone()
                } else {
                    None
                }
            })
            .unwrap_or_else(|| "/".to_string());
        
        // Use LLM-extracted handler name instead of placeholder
        let handler = site.handler_name.clone();

        Some(ResolvedEndpoint {
            method,
            path: path.clone(),
            full_path: path, // Will be resolved later by resolve_endpoint_paths()
            handler,
            owner,
            file_location: site.call_site.location.clone(),
            middleware_chain: Vec::new(), // TODO: Could extract from handler_args if needed
        })
    }

    fn collect_data_calls(&mut self, classified_sites: &[ClassifiedCallSite]) {
        for site in classified_sites {
            if matches!(site.classification, CallSiteType::DataFetchingCall) {
                if let Some(call) = self.extract_data_call(site) {
                    self.data_calls.push(call);
                }
            }
        }
    }

    fn extract_data_call(&self, site: &ClassifiedCallSite) -> Option<DataFetchingCall> {
        let method = site.call_site.callee_property.to_uppercase();
        let client = site.call_site.callee_object.clone();
        
        // Extract target URL from first string argument
        let target_url = site.call_site.args.iter()
            .find_map(|arg| {
                if matches!(arg.arg_type, crate::call_site_extractor::ArgumentType::StringLiteral) {
                    arg.value.clone()
                } else {
                    None
                }
            })
            .unwrap_or_else(|| "unknown".to_string());

        Some(DataFetchingCall {
            method,
            target_url,
            client,
            file_location: site.call_site.location.clone(),
        })
    }

    fn resolve_endpoint_paths(&mut self) {
        let endpoints_to_resolve: Vec<(String, String)> = self.endpoints.iter()
            .map(|endpoint| (endpoint.owner.clone(), endpoint.path.clone()))
            .collect();
        
        for (i, (owner, path)) in endpoints_to_resolve.iter().enumerate() {
            let full_path = self.compute_full_path(owner, path);
            self.endpoints[i].full_path = full_path;
        }
    }

    /// Compute the full path by walking up the mount graph
    pub fn compute_full_path(&self, owner: &str, path: &str) -> String {
        let mut full_path = path.to_string();
        let mut current_node = owner;
        let mut visited = HashSet::new();
        
        // Walk up the mount chain
        while let Some(mount) = self.find_mount_for_child(current_node) {
            if visited.contains(current_node) {
                break; // Prevent cycles
            }
            visited.insert(current_node);
            
            full_path = self.join_paths(&mount.path_prefix, &full_path);
            current_node = &mount.parent;
            
            // Stop if we reach an app (root level)
            if let Some(node) = self.nodes.get(current_node) {
                if matches!(node.node_type, NodeType::App) {
                    break;
                }
            }
        }
        
        self.normalize_path(&full_path)
    }

    fn find_mount_for_child(&self, child: &str) -> Option<&MountEdge> {
        self.mounts.iter().find(|mount| mount.child == child)
    }

    fn join_paths(&self, prefix: &str, path: &str) -> String {
        let normalized_prefix = prefix.trim_end_matches('/');
        let normalized_path = path.trim_start_matches('/');
        
        if normalized_prefix.is_empty() {
            format!("/{}", normalized_path)
        } else if normalized_path.is_empty() {
            normalized_prefix.to_string()
        } else {
            format!("{}/{}", normalized_prefix, normalized_path)
        }
    }

    fn normalize_path(&self, path: &str) -> String {
        if path.is_empty() || path == "/" {
            "/".to_string()
        } else {
            // Remove duplicate slashes and ensure single leading slash
            let clean_path = path.split('/')
                .filter(|segment| !segment.is_empty())
                .collect::<Vec<_>>()
                .join("/");
            format!("/{}", clean_path)
        }
    }

    /// Get all endpoints with their resolved full paths
    pub fn get_resolved_endpoints(&self) -> &[ResolvedEndpoint] {
        &self.endpoints
    }

    /// Get all data-fetching calls
    pub fn get_data_calls(&self) -> &[DataFetchingCall] {
        &self.data_calls
    }

    /// Get mount relationships
    pub fn get_mounts(&self) -> &[MountEdge] {
        &self.mounts
    }

    /// Get all nodes in the graph
    pub fn get_nodes(&self) -> &HashMap<String, GraphNode> {
        &self.nodes
    }

    /// Find all endpoints that match a given path pattern
    pub fn find_matching_endpoints(&self, path: &str, method: &str) -> Vec<&ResolvedEndpoint> {
        self.endpoints.iter()
            .filter(|endpoint| {
                endpoint.method.eq_ignore_ascii_case(method) && 
                self.paths_match(&endpoint.full_path, path)
            })
            .collect()
    }

    fn paths_match(&self, endpoint_path: &str, call_path: &str) -> bool {
        // Simple path matching - could be enhanced with parameter matching
        endpoint_path == call_path || 
        self.path_matches_with_params(endpoint_path, call_path)
    }

    fn path_matches_with_params(&self, endpoint_path: &str, call_path: &str) -> bool {
        let endpoint_segments: Vec<&str> = endpoint_path.split('/').collect();
        let call_segments: Vec<&str> = call_path.split('/').collect();
        
        if endpoint_segments.len() != call_segments.len() {
            return false;
        }
        
        for (endpoint_seg, call_seg) in endpoint_segments.iter().zip(call_segments.iter()) {
            if endpoint_seg.starts_with(':') {
                continue; // Parameter segment matches anything
            }
            if endpoint_seg != call_seg {
                return false;
            }
        }
        
        true
    }
}

impl Default for MountGraph {
    fn default() -> Self {
        Self::new()
    }
}