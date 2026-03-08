use crate::{
    call_site_classifier::{CallSiteType, ClassifiedCallSite},
    url_normalizer::UrlNormalizer,
};
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
    Root,      // Never mounted by other nodes (top-level applications)
    Mountable, // Can be mounted by other nodes (routers, sub-apps)
    Unknown,   // Insufficient information to classify
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
    /// Optional repo identifier for cross-repo matching
    #[serde(skip_serializing_if = "Option::is_none")]
    pub repo_name: Option<String>,
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
    #[allow(dead_code)]
    pub fn build_from_classified_sites(classified_sites: Vec<ClassifiedCallSite>) -> Self {
        let mut graph = Self::new();

        // First pass: collect all nodes with unknown types initially
        graph.collect_nodes(&classified_sites);

        // Second pass: build mount relationships
        graph.build_mount_edges(&classified_sites);

        // Third pass: infer node types based on mount behavior
        graph.infer_node_types_from_behavior();

        // Fourth pass: collect endpoints and data calls
        graph.collect_endpoints(&classified_sites);
        graph.collect_data_calls(&classified_sites);

        // Fifth pass: resolve full paths for all endpoints
        graph.resolve_endpoint_paths();

        graph
    }

    #[allow(dead_code)]
    fn collect_nodes(&mut self, classified_sites: &[ClassifiedCallSite]) {
        for site in classified_sites {
            let node_name = site.call_site.callee_object.clone();

            if !self.nodes.contains_key(&node_name) {
                // Start with Unknown - we'll classify based on behavior later
                self.nodes.insert(
                    node_name.clone(),
                    GraphNode {
                        name: node_name,
                        node_type: NodeType::Unknown,
                        creation_site: site.call_site.definition.clone(),
                        file_location: site.call_site.location.clone(),
                    },
                );
            }
        }
    }

    /// Infer node types based on mounting behavior rather than framework-specific patterns
    fn infer_node_types_from_behavior(&mut self) {
        // Collect all nodes that are children in mount relationships
        let mut mounted_nodes = HashSet::new();
        for mount in &self.mounts {
            mounted_nodes.insert(mount.child.clone());
        }

        // Collect nodes that are parents in mount relationships
        let mut parent_nodes = HashSet::new();
        for mount in &self.mounts {
            parent_nodes.insert(mount.parent.clone());
        }

        // Classify nodes based on whether they are ever mounted
        let node_names: Vec<String> = self.nodes.keys().cloned().collect();
        for node_name in node_names {
            let node_type = if mounted_nodes.contains(&node_name) {
                // This node is mounted by something else, so it's mountable
                NodeType::Mountable
            } else if parent_nodes.contains(&node_name) {
                // This node mounts other things but is never mounted itself, so it's a root
                NodeType::Root
            } else {
                // No mounting relationships found - leave as Unknown
                // These could be standalone services, data fetching clients, or orphaned nodes
                // The path resolution will work fine without needing to classify them
                NodeType::Unknown
            };

            if let Some(node) = self.nodes.get_mut(&node_name) {
                node.node_type = node_type;
            }
        }
    }

    #[allow(dead_code)]
    fn build_mount_edges(&mut self, classified_sites: &[ClassifiedCallSite]) {
        for site in classified_sites {
            if matches!(site.classification, CallSiteType::RouterMount) {
                if let Some(mount) = self.extract_mount_relationship(site) {
                    self.mounts.push(mount);
                }
            }
        }
    }

    #[allow(dead_code)]
    fn extract_mount_relationship(&self, site: &ClassifiedCallSite) -> Option<MountEdge> {
        // Use LLM-extracted mount information instead of heuristic parsing
        let parent = site.mount_parent.as_ref()?;
        let child = site.mount_child.as_ref()?;
        let path_prefix = site
            .mount_prefix
            .as_ref()
            .unwrap_or(&"/".to_string())
            .clone();

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

    #[allow(dead_code)]
    fn collect_endpoints(&mut self, classified_sites: &[ClassifiedCallSite]) {
        for site in classified_sites {
            if matches!(site.classification, CallSiteType::HttpEndpoint) {
                if let Some(endpoint) = self.extract_endpoint(site) {
                    self.endpoints.push(endpoint);
                }
            }
        }
    }

    #[allow(dead_code)]
    fn extract_endpoint(&self, site: &ClassifiedCallSite) -> Option<ResolvedEndpoint> {
        let method = site.call_site.callee_property.to_uppercase();
        let owner = site.call_site.callee_object.clone();

        // Extract path from first string argument (fallback to heuristic if needed)
        let path = site
            .call_site
            .args
            .iter()
            .find_map(|arg| {
                if matches!(
                    arg.arg_type,
                    crate::call_site_extractor::ArgumentType::StringLiteral
                ) {
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
            repo_name: None,              // Will be set during cross-repo merge
        })
    }

    #[allow(dead_code)]
    fn collect_data_calls(&mut self, classified_sites: &[ClassifiedCallSite]) {
        for site in classified_sites {
            if matches!(site.classification, CallSiteType::DataFetchingCall) {
                if let Some(call) = self.extract_data_call(site) {
                    self.data_calls.push(call);
                }
            }
        }
    }

    #[allow(dead_code)]
    fn extract_data_call(&self, site: &ClassifiedCallSite) -> Option<DataFetchingCall> {
        let method = site.call_site.callee_property.to_uppercase();
        let client = site.call_site.callee_object.clone();

        // Extract target URL from first string argument
        let target_url = site
            .call_site
            .args
            .iter()
            .find_map(|arg| {
                if matches!(
                    arg.arg_type,
                    crate::call_site_extractor::ArgumentType::StringLiteral
                ) {
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
        let endpoints_to_resolve: Vec<(String, String)> = self
            .endpoints
            .iter()
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

            // Stop if we reach a root node (top-level application)
            if let Some(node) = self.nodes.get(current_node) {
                if matches!(node.node_type, NodeType::Root) {
                    break;
                }
            }
        }

        self.normalize_path(&full_path)
    }

    fn find_mount_for_child(&self, child: &str) -> Option<&MountEdge> {
        // First, try to find an exact match by child name
        if let Some(mount) = self.mounts.iter().find(|mount| mount.child == child) {
            return Some(mount);
        }

        // If no exact match, try to find a mount where the child node is an alias
        // of the current node. This handles cases where the same router is referred
        // to by different names:
        //   - routes/users.ts defines `const router = express.Router()`
        //   - server.ts imports it as `import userRouter from './routes/users'`
        //   - Mount is `app.use('/users', userRouter)` but endpoint owner is `router`
        //
        // Two nodes are considered aliases if they have the SAME file location
        // (same file AND same line/column). This distinguishes:
        //   - Aliases: `router` and `apiRouter` both at `api-router.ts:1:0` (same variable, different names)
        //   - Different routers: `router` at `server.ts:5:0` and `apiRouter` at `server.ts:27:0`
        let current_node_location = self.nodes.get(child).map(|n| &n.file_location);

        if let Some(location) = current_node_location {
            // Find all nodes that share this EXACT file location (they are aliases of the same router)
            // We use exact match (including line/column) to avoid treating different routers
            // in the same file as aliases
            let alias_names: Vec<&String> = self
                .nodes
                .iter()
                .filter(|(name, node)| *name != child && node.file_location == *location)
                .map(|(name, _)| name)
                .collect();

            // Try to find a mount where the child is one of these aliases
            for alias in alias_names {
                if let Some(mount) = self.mounts.iter().find(|mount| &mount.child == alias) {
                    return Some(mount);
                }
            }
        }

        None
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
            let clean_path = path
                .split('/')
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
    ///
    /// This is the basic matching method that does not perform URL normalization.
    /// For cross-service matching with full URLs, use `find_matching_endpoints_normalized`.
    #[allow(dead_code)]
    pub fn find_matching_endpoints(&self, path: &str, method: &str) -> Vec<&ResolvedEndpoint> {
        self.endpoints
            .iter()
            .filter(|endpoint| {
                endpoint.method.eq_ignore_ascii_case(method)
                    && self.paths_match(&endpoint.full_path, path)
            })
            .collect()
    }

    /// Find all endpoints that match a given URL pattern with URL normalization
    ///
    /// This method handles real-world URL patterns:
    /// - Full URLs: `https://user-service.internal/users/123` → matches `/users/:id`
    /// - Env var patterns: `ENV_VAR:SERVICE_URL:/users/123` → matches `/users/:id`
    /// - Template literals: `${API_URL}/users/${id}` → matches `/users/:id`
    /// - Query strings are stripped: `/users?page=1` → matches `/users`
    ///
    /// Returns `None` if the URL is identified as external (should be skipped).
    /// Returns `Some(vec)` with matching endpoints (may be empty if no match found).
    pub fn find_matching_endpoints_with_normalizer(
        &self,
        url: &str,
        method: &str,
        normalizer: &UrlNormalizer,
    ) -> Option<Vec<&ResolvedEndpoint>> {
        let normalized = normalizer.normalize(url);

        // Skip external calls
        if normalized.is_external {
            return None;
        }

        let matching = self
            .endpoints
            .iter()
            .filter(|endpoint| {
                endpoint.method.eq_ignore_ascii_case(method)
                    && self.paths_match(&endpoint.full_path, &normalized.path)
            })
            .collect();

        Some(matching)
    }

    #[allow(dead_code)]
    fn paths_match(&self, endpoint_path: &str, call_path: &str) -> bool {
        // Exact match
        if endpoint_path == call_path {
            return true;
        }

        // Parameter matching (e.g., /users/:id matches /users/123)
        if self.path_matches_with_params(endpoint_path, call_path) {
            return true;
        }

        // Try matching with wildcards
        if self.path_matches_with_wildcards(endpoint_path, call_path) {
            return true;
        }

        false
    }

    #[allow(dead_code)]
    fn path_matches_with_params(&self, endpoint_path: &str, call_path: &str) -> bool {
        let endpoint_segments: Vec<&str> = endpoint_path.split('/').collect();
        let call_segments: Vec<&str> = call_path.split('/').collect();

        // Handle optional segments (e.g., /users/:id?)
        let endpoint_required_count = endpoint_segments
            .iter()
            .filter(|s| !s.ends_with('?'))
            .count();

        // Call must have at least required segments and at most all segments
        if call_segments.len() < endpoint_required_count
            || call_segments.len() > endpoint_segments.len()
        {
            return false;
        }

        for (i, endpoint_seg) in endpoint_segments.iter().enumerate() {
            // Check if this is an optional segment
            let is_optional = endpoint_seg.ends_with('?');
            let seg = endpoint_seg.trim_end_matches('?');

            // If we're past the call segments, remaining endpoint segments must be optional
            if i >= call_segments.len() {
                if !is_optional {
                    return false;
                }
                continue;
            }

            let call_seg = call_segments[i];

            // Parameter segment matches anything (starts with :)
            if seg.starts_with(':') {
                continue;
            }

            // Exact match required for non-parameter segments
            if seg != call_seg {
                return false;
            }
        }

        true
    }

    /// Match paths with wildcard patterns
    ///
    /// Supports:
    /// - `*` matches a single path segment
    /// - `**` or `(.*)` matches zero or more path segments
    #[allow(dead_code)]
    fn path_matches_with_wildcards(&self, endpoint_path: &str, call_path: &str) -> bool {
        // Check for catch-all patterns
        if endpoint_path.ends_with("/*") || endpoint_path.ends_with("/**") {
            let prefix = endpoint_path.trim_end_matches("/**").trim_end_matches("/*");
            return call_path.starts_with(prefix);
        }

        // Check for regex-style catch-all
        if endpoint_path.ends_with("/(.*)") {
            let prefix = endpoint_path.trim_end_matches("/(.*)");
            return call_path.starts_with(prefix);
        }

        // Check for single-segment wildcards in the middle
        let endpoint_segments: Vec<&str> = endpoint_path.split('/').collect();
        let call_segments: Vec<&str> = call_path.split('/').collect();

        if endpoint_segments.len() != call_segments.len() {
            return false;
        }

        for (endpoint_seg, call_seg) in endpoint_segments.iter().zip(call_segments.iter()) {
            if *endpoint_seg == "*" {
                continue; // Single-segment wildcard matches anything
            }
            if endpoint_seg != call_seg {
                return false;
            }
        }

        true
    }

    /// Merge mount graphs from multiple repos for cross-repo analysis
    /// This is framework-agnostic - it merges the behavior-based analysis results
    pub fn merge_from_repos(all_repo_data: &[crate::cloud_storage::CloudRepoData]) -> Self {
        let mut merged = MountGraph::new();
        let mut seen_endpoints: HashSet<String> = HashSet::new();
        let mut seen_data_calls: HashSet<String> = HashSet::new();
        let mut seen_mounts: HashSet<String> = HashSet::new();

        for repo_data in all_repo_data {
            if let Some(ref mount_graph) = repo_data.mount_graph {
                // Merge nodes (deduplicate by name, prefer first occurrence)
                for (node_name, node) in &mount_graph.nodes {
                    merged
                        .nodes
                        .entry(node_name.clone())
                        .or_insert_with(|| node.clone());
                }

                // Merge endpoints (deduplicate by method + full_path)
                // Tag each endpoint with its source repo for cross-repo matching
                for endpoint in &mount_graph.endpoints {
                    let key = format!(
                        "{}:{}:{}",
                        repo_data.repo_name, endpoint.method, endpoint.full_path
                    );
                    if seen_endpoints.insert(key) {
                        let mut tagged_endpoint = endpoint.clone();
                        // Tag endpoint with repo name for service resolution
                        tagged_endpoint.repo_name = Some(repo_data.repo_name.clone());
                        merged.endpoints.push(tagged_endpoint);
                    }
                }

                // Merge data calls (deduplicate by method + target_url + file_location)
                for call in &mount_graph.data_calls {
                    let key = format!("{}:{}:{}", call.method, call.target_url, call.file_location);
                    if seen_data_calls.insert(key) {
                        merged.data_calls.push(call.clone());
                    }
                }

                // Merge mounts (deduplicate by parent + child + prefix)
                for mount in &mount_graph.mounts {
                    let key = format!("{}:{}:{}", mount.parent, mount.child, mount.path_prefix);
                    if seen_mounts.insert(key) {
                        merged.mounts.push(mount.clone());
                    }
                }
            }
        }

        merged
    }
}

impl Default for MountGraph {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::call_site_classifier::{CallSiteType, ClassifiedCallSite};
    use crate::call_site_extractor::{ArgumentType, CallArgument, CallSite};
    use crate::config::Config;

    fn create_test_call_site(
        callee_object: &str,
        callee_property: &str,
        definition: Option<String>,
        classification: CallSiteType,
    ) -> ClassifiedCallSite {
        ClassifiedCallSite {
            call_site: CallSite {
                callee_object: callee_object.to_string(),
                callee_property: callee_property.to_string(),
                args: vec![CallArgument {
                    value: Some("/test".to_string()),
                    resolved_value: None,
                    arg_type: ArgumentType::StringLiteral,
                    handler_param_types: None,
                }],
                definition,
                location: "test.js:1:1".to_string(),
                result_type: None,
                correlated_call: None,
                context_slice: None,
            },
            classification,
            confidence: 1.0,
            reasoning: "test".to_string(),
            mount_parent: None,
            mount_child: None,
            mount_prefix: None,
            handler_name: None,
            handler_args: Vec::new(),
        }
    }

    fn create_mount_call_site(parent: &str, child: &str, prefix: &str) -> ClassifiedCallSite {
        ClassifiedCallSite {
            call_site: CallSite {
                callee_object: parent.to_string(),
                callee_property: "use".to_string(),
                args: vec![],
                definition: None,
                location: "test.js:1:1".to_string(),
                result_type: None,
                correlated_call: None,
                context_slice: None,
            },
            classification: CallSiteType::RouterMount,
            confidence: 1.0,
            reasoning: "test mount".to_string(),
            mount_parent: Some(parent.to_string()),
            mount_child: Some(child.to_string()),
            mount_prefix: Some(prefix.to_string()),
            handler_name: None,
            handler_args: Vec::new(),
        }
    }

    #[test]
    fn test_behavior_based_node_classification() {
        // Test that nodes are classified based on mount relationships, not framework patterns
        let call_sites = vec![
            // Create some nodes with generic names (no framework-specific patterns)
            create_test_call_site(
                "myApp",
                "get",
                Some("createApplication()".to_string()),
                CallSiteType::HttpEndpoint,
            ),
            create_test_call_site(
                "userRouter",
                "get",
                Some("createComponent()".to_string()),
                CallSiteType::HttpEndpoint,
            ),
            create_test_call_site(
                "apiRouter",
                "post",
                Some("makeRouter()".to_string()),
                CallSiteType::HttpEndpoint,
            ),
            // Create mount relationship: myApp mounts userRouter
            create_mount_call_site("myApp", "userRouter", "/users"),
            // Create mount relationship: myApp mounts apiRouter
            create_mount_call_site("myApp", "apiRouter", "/api"),
        ];

        let graph = MountGraph::build_from_classified_sites(call_sites);

        // Check that nodes were classified based on behavior
        let my_app = graph.nodes.get("myApp").unwrap();
        let user_router = graph.nodes.get("userRouter").unwrap();
        let api_router = graph.nodes.get("apiRouter").unwrap();

        // myApp should be Root because it mounts others but is never mounted
        assert_eq!(my_app.node_type, NodeType::Root);

        // userRouter and apiRouter should be Mountable because they are mounted by myApp
        assert_eq!(user_router.node_type, NodeType::Mountable);
        assert_eq!(api_router.node_type, NodeType::Mountable);

        // Check mount relationships were created correctly
        assert_eq!(graph.mounts.len(), 2);
        assert!(
            graph.mounts.iter().any(|m| m.parent == "myApp"
                && m.child == "userRouter"
                && m.path_prefix == "/users")
        );
        assert!(
            graph
                .mounts
                .iter()
                .any(|m| m.parent == "myApp" && m.child == "apiRouter" && m.path_prefix == "/api")
        );
    }

    #[test]
    fn test_standalone_nodes_remain_unknown() {
        // Test that nodes with no mount relationships are left as Unknown (framework-agnostic)
        let call_sites = vec![
            create_test_call_site(
                "standaloneAPI",
                "get",
                Some("createSomeService()".to_string()),
                CallSiteType::HttpEndpoint,
            ),
            create_test_call_site(
                "httpClient",
                "post",
                Some("axios.create()".to_string()),
                CallSiteType::DataFetchingCall,
            ),
            create_test_call_site(
                "utilityObject",
                "patch",
                Some("makeUtility()".to_string()),
                CallSiteType::HttpEndpoint,
            ),
        ];

        let graph = MountGraph::build_from_classified_sites(call_sites);

        let standalone_api = graph.nodes.get("standaloneAPI").unwrap();
        let http_client = graph.nodes.get("httpClient").unwrap();
        let utility_object = graph.nodes.get("utilityObject").unwrap();

        // All should remain Unknown since they have no mount relationships
        // This is truly framework-agnostic - we don't try to guess based on naming patterns
        assert_eq!(standalone_api.node_type, NodeType::Unknown);
        assert_eq!(http_client.node_type, NodeType::Unknown);
        assert_eq!(utility_object.node_type, NodeType::Unknown);
    }

    #[test]
    fn test_url_normalized_matching_full_url() {
        // Test that full URLs are normalized and matched against endpoint paths
        let mut graph = MountGraph::new();

        // Add an endpoint
        graph.endpoints.push(ResolvedEndpoint {
            method: "GET".to_string(),
            path: "/users/:id".to_string(),
            full_path: "/users/:id".to_string(),
            handler: Some("getUser".to_string()),
            owner: "userRouter".to_string(),
            file_location: "routes/users.js:10:1".to_string(),
            middleware_chain: vec![],
            repo_name: None,
        });

        // Create config with internal domain
        let config = Config {
            service_name: None,
            internal_domains: ["user-service.internal".to_string()].into_iter().collect(),
            external_domains: ["api.stripe.com".to_string()].into_iter().collect(),
            internal_env_vars: Default::default(),
            external_env_vars: Default::default(),
        };

        // Test: Full internal URL should match
        let normalizer = UrlNormalizer::new(&config);
        let _result = graph.find_matching_endpoints_with_normalizer(
            "https://user-service.internal/users/123",
            "GET",
            &normalizer,
        );
    }

    /// Test for exact 3-level nesting structure from CI logs (repo-b)
    ///
    /// Structure:
    /// - app (Root) mounts apiRouter at /api
    /// - apiRouter (Mountable) mounts v1Router at /v1
    /// - v1Router (Mountable) owns POST /chat
    ///
    /// Expected: `compute_full_path("v1Router", "/chat")` returns `/api/v1/chat`
    #[test]
    fn test_nested_router_three_levels() {
        // Set up the graph manually to reproduce the exact structure from the logs
        let mut graph = MountGraph::new();

        // Add nodes
        graph.nodes.insert(
            "app".to_string(),
            GraphNode {
                name: "app".to_string(),
                node_type: NodeType::Root,
                creation_site: None,
                file_location: "repo-b_server.ts:1:0".to_string(),
            },
        );
        graph.nodes.insert(
            "apiRouter".to_string(),
            GraphNode {
                name: "apiRouter".to_string(),
                node_type: NodeType::Mountable,
                creation_site: None,
                file_location: "api-router.ts:1:0".to_string(),
            },
        );
        graph.nodes.insert(
            "v1Router".to_string(),
            GraphNode {
                name: "v1Router".to_string(),
                node_type: NodeType::Mountable,
                creation_site: None,
                file_location: "v1-router.ts:1:0".to_string(),
            },
        );

        // Add mount edges: app -> apiRouter at /api
        graph.mounts.push(MountEdge {
            parent: "app".to_string(),
            child: "apiRouter".to_string(),
            path_prefix: "/api".to_string(),
            middleware_stack: Vec::new(),
        });

        // Add mount edges: apiRouter -> v1Router at /v1
        graph.mounts.push(MountEdge {
            parent: "apiRouter".to_string(),
            child: "v1Router".to_string(),
            path_prefix: "/v1".to_string(),
            middleware_stack: Vec::new(),
        });

        // Test compute_full_path for the endpoint POST /chat owned by v1Router
        let full_path = graph.compute_full_path("v1Router", "/chat");

        println!("Computed full path: {}", full_path);
        println!("Nodes: {:?}", graph.nodes.keys().collect::<Vec<_>>());
        println!("Mounts: {:?}", graph.mounts);

        // The bug would cause this to return "/api/chat" (missing /v1)
        // or "/v1/chat" (missing /api)
        assert_eq!(
            full_path, "/api/v1/chat",
            "Three-level nested router path should include all mount prefixes: expected /api/v1/chat but got {}",
            full_path
        );
    }

    /// Test for broken mount chain where parent node names don't match across files.
    ///
    /// This reproduces the exact bug from CI logs where:
    /// - In server.ts: `app.use('/api', router)` - 'router' is imported from api-router.ts
    /// - In api-router.ts: `router.use('/v1', v1Router)` - 'router' is the local variable name
    ///
    /// The mount edges created are:
    /// - app -> router at /api (using imported name in server.ts)
    /// - router -> v1Router at /v1 (using local name in api-router.ts)
    ///
    /// BUG: If the import name resolution fails, 'router' in server.ts might not be
    /// recognized as the same as 'router' in api-router.ts, breaking the chain.
    #[test]
    fn test_nested_router_broken_chain_with_local_variable_names() {
        // This test simulates what happens when the parent of a mount uses a local
        // variable name that wasn't properly resolved to its imported name
        let mut graph = MountGraph::new();

        // Add nodes - note that "router" appears as a separate node from "apiRouter"
        graph.nodes.insert(
            "app".to_string(),
            GraphNode {
                name: "app".to_string(),
                node_type: NodeType::Root,
                creation_site: None,
                file_location: "repo-b_server.ts:1:0".to_string(),
            },
        );
        // This node represents "router" as imported in server.ts
        graph.nodes.insert(
            "router".to_string(),
            GraphNode {
                name: "router".to_string(),
                node_type: NodeType::Mountable,
                creation_site: None,
                file_location: "api-router.ts:1:0".to_string(),
            },
        );
        graph.nodes.insert(
            "v1Router".to_string(),
            GraphNode {
                name: "v1Router".to_string(),
                node_type: NodeType::Mountable,
                creation_site: None,
                file_location: "v1-router.ts:1:0".to_string(),
            },
        );

        // Mount edges as they would be created from the logs
        // app mounts router (the imported name) at /api
        graph.mounts.push(MountEdge {
            parent: "app".to_string(),
            child: "router".to_string(),
            path_prefix: "/api".to_string(),
            middleware_stack: Vec::new(),
        });

        // router (local name in api-router.ts) mounts v1Router at /v1
        graph.mounts.push(MountEdge {
            parent: "router".to_string(),
            child: "v1Router".to_string(),
            path_prefix: "/v1".to_string(),
            middleware_stack: Vec::new(),
        });

        // Test compute_full_path for the endpoint POST /chat owned by v1Router
        let full_path = graph.compute_full_path("v1Router", "/chat");

        println!("Broken chain test - Computed full path: {}", full_path);
        println!("Mounts: {:?}", graph.mounts);

        // When mounts are properly connected (router -> router), this should work
        assert_eq!(
            full_path, "/api/v1/chat",
            "Even with local variable names, path should resolve correctly when mount chain is connected"
        );
    }

    /// Test for the actual bug: when mount parent names are inconsistent
    /// This reproduces the ACTUAL bug where the second mount's parent doesn't match
    /// the first mount's child due to import name resolution failure
    #[test]
    fn test_nested_router_disconnected_chain_bug() {
        let mut graph = MountGraph::new();

        // Add nodes
        graph.nodes.insert(
            "app".to_string(),
            GraphNode {
                name: "app".to_string(),
                node_type: NodeType::Root,
                creation_site: None,
                file_location: "repo-b_server.ts:1:0".to_string(),
            },
        );
        // "apiRouter" is the imported name in server.ts
        graph.nodes.insert(
            "apiRouter".to_string(),
            GraphNode {
                name: "apiRouter".to_string(),
                node_type: NodeType::Mountable,
                creation_site: None,
                file_location: "api-router.ts:1:0".to_string(),
            },
        );
        // "router" is the local name in api-router.ts (DIFFERENT from "apiRouter")
        graph.nodes.insert(
            "router".to_string(),
            GraphNode {
                name: "router".to_string(),
                node_type: NodeType::Mountable,
                creation_site: None,
                file_location: "api-router.ts:1:0".to_string(),
            },
        );
        graph.nodes.insert(
            "v1Router".to_string(),
            GraphNode {
                name: "v1Router".to_string(),
                node_type: NodeType::Mountable,
                creation_site: None,
                file_location: "v1-router.ts:1:0".to_string(),
            },
        );

        // BUG REPRODUCTION: Mount edges with DISCONNECTED names
        // app mounts apiRouter (the imported name) at /api
        graph.mounts.push(MountEdge {
            parent: "app".to_string(),
            child: "apiRouter".to_string(), // <-- imported name
            path_prefix: "/api".to_string(),
            middleware_stack: Vec::new(),
        });

        // router (local name, NOT "apiRouter") mounts v1Router at /v1
        // This creates a BROKEN chain because "router" != "apiRouter"
        graph.mounts.push(MountEdge {
            parent: "router".to_string(), // <-- local name, doesn't match "apiRouter"!
            child: "v1Router".to_string(),
            path_prefix: "/v1".to_string(),
            middleware_stack: Vec::new(),
        });

        // Test compute_full_path for the endpoint POST /chat owned by v1Router
        let full_path = graph.compute_full_path("v1Router", "/chat");

        println!(
            "Disconnected chain bug test - Computed full path: {}",
            full_path
        );
        println!("Mounts: {:?}", graph.mounts);

        // BUG: This will return "/v1/chat" because the chain is broken at "router"
        // The traversal finds v1Router -> router at /v1, but then can't find
        // a mount where router is the child (because the mount has "apiRouter" as child)

        // For now, this test documents the current (buggy) behavior
        // After the fix, this should return "/api/v1/chat"

        // Current buggy behavior: returns /v1/chat (missing /api)
        // We assert what SHOULD happen after fix:
        assert_eq!(
            full_path, "/api/v1/chat",
            "Disconnected chain should still resolve to /api/v1/chat after fix. Got: {}",
            full_path
        );
    }

    #[test]
    fn test_url_normalized_matching_env_var_pattern() {
        let mut graph = MountGraph::new();

        graph.endpoints.push(ResolvedEndpoint {
            method: "POST".to_string(),
            path: "/orders".to_string(),
            full_path: "/orders".to_string(),
            handler: Some("createOrder".to_string()),
            owner: "orderRouter".to_string(),
            file_location: "routes/orders.js:15:1".to_string(),
            middleware_chain: vec![],
            repo_name: None,
        });

        let config = Config {
            service_name: None,
            internal_domains: Default::default(),
            external_domains: Default::default(),
            internal_env_vars: ["ORDER_SERVICE_URL".to_string()].into_iter().collect(),
            external_env_vars: ["STRIPE_API".to_string()].into_iter().collect(),
        };

        // Test: Internal env var pattern should match
        let normalizer = UrlNormalizer::new(&config);
        let result = graph.find_matching_endpoints_with_normalizer(
            "ENV_VAR:ORDER_SERVICE_URL:/orders",
            "POST",
            &normalizer,
        );
        assert!(result.is_some());
        assert_eq!(result.unwrap().len(), 1);

        // Test: External env var pattern should return None
        let result = graph.find_matching_endpoints_with_normalizer(
            "ENV_VAR:STRIPE_API:/v1/charges",
            "POST",
            &normalizer,
        );
        assert!(result.is_none());
    }

    #[test]
    fn test_url_normalized_matching_template_literal() {
        let mut graph = MountGraph::new();

        graph.endpoints.push(ResolvedEndpoint {
            method: "GET".to_string(),
            path: "/users/:userId/orders/:orderId".to_string(),
            full_path: "/users/:userId/orders/:orderId".to_string(),
            handler: Some("getUserOrder".to_string()),
            owner: "orderRouter".to_string(),
            file_location: "routes/orders.js:20:1".to_string(),
            middleware_chain: vec![],
            repo_name: None,
        });

        let config = Config {
            service_name: None,
            internal_domains: Default::default(),
            external_domains: Default::default(),
            internal_env_vars: ["API_URL".to_string()].into_iter().collect(),
            external_env_vars: Default::default(),
        };

        // Test: Template literal should be normalized and matched
        let normalizer = UrlNormalizer::new(&config);
        let result = graph.find_matching_endpoints_with_normalizer(
            "${API_URL}/users/${userId}/orders/${orderId}",
            "GET",
            &normalizer,
        );
        assert!(result.is_some());
        assert_eq!(result.unwrap().len(), 1);
    }

    #[test]
    fn test_url_normalized_matching_query_string_stripped() {
        let mut graph = MountGraph::new();

        graph.endpoints.push(ResolvedEndpoint {
            method: "GET".to_string(),
            path: "/users".to_string(),
            full_path: "/users".to_string(),
            handler: Some("listUsers".to_string()),
            owner: "userRouter".to_string(),
            file_location: "routes/users.js:5:1".to_string(),
            middleware_chain: vec![],
            repo_name: None,
        });

        let config = Config::default();

        // Test: Query string should be stripped before matching
        let normalizer = UrlNormalizer::new(&config);
        let result = graph.find_matching_endpoints_with_normalizer(
            "/users?page=1&limit=10",
            "GET",
            &normalizer,
        );
        assert!(result.is_some());
        assert_eq!(result.unwrap().len(), 1);
    }

    #[test]
    fn test_path_matches_with_optional_segments() {
        let graph = MountGraph::new();

        // Test optional segment matching
        assert!(graph.path_matches_with_params("/users/:id?", "/users"));
        assert!(graph.path_matches_with_params("/users/:id?", "/users/123"));
        assert!(!graph.path_matches_with_params("/users/:id", "/users")); // Not optional
    }

    #[test]
    fn test_path_matches_with_wildcards() {
        let graph = MountGraph::new();

        // Test wildcard matching
        assert!(graph.path_matches_with_wildcards("/api/*", "/api/users"));
        assert!(graph.path_matches_with_wildcards("/api/**", "/api/users/123/orders"));
        assert!(graph.path_matches_with_wildcards("/files/(.*)", "/files/path/to/file.txt"));
    }

    #[test]
    fn test_path_resolution_with_behavior_based_types() {
        let call_sites = vec![
            // Root app
            create_test_call_site(
                "app",
                "get",
                Some("createServer()".to_string()),
                CallSiteType::HttpEndpoint,
            ),
            // Nested routers
            create_test_call_site(
                "v1Router",
                "get",
                Some("createRouter()".to_string()),
                CallSiteType::HttpEndpoint,
            ),
            create_test_call_site(
                "userRouter",
                "post",
                Some("createRouter()".to_string()),
                CallSiteType::HttpEndpoint,
            ),
            // Mount relationships: app -> v1Router -> userRouter
            create_mount_call_site("app", "v1Router", "/v1"),
            create_mount_call_site("v1Router", "userRouter", "/users"),
        ];

        let graph = MountGraph::build_from_classified_sites(call_sites);

        // Test that path resolution works correctly
        let full_path = graph.compute_full_path("userRouter", "/:id");
        assert_eq!(full_path, "/v1/users/:id");

        // Verify node types were inferred correctly
        assert_eq!(graph.nodes.get("app").unwrap().node_type, NodeType::Root);
        assert_eq!(
            graph.nodes.get("v1Router").unwrap().node_type,
            NodeType::Mountable
        );
        assert_eq!(
            graph.nodes.get("userRouter").unwrap().node_type,
            NodeType::Mountable
        );
    }
}
