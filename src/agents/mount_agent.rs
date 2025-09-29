use crate::{
    agents::schemas::AgentSchemas, call_site_extractor::CallSite,
    framework_detector::DetectionResult, gemini_service::GeminiService,
};
use serde::{Deserialize, Serialize};

/// Represents a detected mount relationship between nodes
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MountRelationship {
    pub parent_node: String, // The object doing the mounting (e.g., "app", "router")
    pub child_node: String,  // The object being mounted (e.g., "apiRouter", "userRouter")
    pub mount_path: String,  // The path where it's mounted (e.g., "/api", "/users")
    pub location: String,    // File location
    pub confidence: f32,
    pub reasoning: String,
}

/// Specialized agent for detecting router mount relationships
pub struct MountAgent {
    gemini_service: GeminiService,
}

impl MountAgent {
    pub fn new(gemini_service: GeminiService) -> Self {
        Self { gemini_service }
    }

    /// Extract mount relationships from pre-triaged RouterMount call sites
    pub async fn detect_mounts(
        &self,
        call_sites: &[CallSite],
        framework_detection: &DetectionResult,
    ) -> Result<Vec<MountRelationship>, Box<dyn std::error::Error>> {
        if call_sites.is_empty() {
            return Ok(Vec::new());
        }

        println!("=== MOUNT AGENT DEBUG ===");
        println!(
            "Analyzing {} pre-triaged router mount call sites",
            call_sites.len()
        );

        let prompt = self.build_mount_prompt(call_sites, framework_detection);
        let system_message = self.build_system_message();

        let schema = AgentSchemas::mount_schema();
        let response = self
            .gemini_service
            .analyze_code_with_schema(&prompt, &system_message, Some(schema))
            .await?;

        let mounts: Vec<MountRelationship> = serde_json::from_str(&response)
            .map_err(|e| format!("Failed to parse mount detection response: {}", e))?;

        Ok(mounts)
    }

    fn build_system_message(&self) -> String {
        r#"You are an expert at extracting router mount relationships from JavaScript/TypeScript code.

These call sites have already been identified as router mounts by a triage process. Your task is to extract the specific mount details from each one.

MOUNT RELATIONSHIPS TO EXTRACT:
- Parent node: The object doing the mounting (app, router, server, etc.)
- Child node: The object being mounted (router variable name, sub-app, etc.)
- Mount path: The path prefix where the child is mounted

EXAMPLES OF MOUNT PATTERNS:
- app.use('/api', apiRouter) -> parent: "app", child: "apiRouter", path: "/api"
- router.use('/users', userRouter) -> parent: "router", child: "userRouter", path: "/users"
- server.mount('/v1', v1Router) -> parent: "server", child: "v1Router", path: "/v1"

CRITICAL REQUIREMENTS:
1. Return ONLY valid JSON array
2. Extract details from ALL provided call sites (they're all router mounts)
3. Set confidence based on how clearly you can extract the details
4. Provide brief reasoning for each extraction

NO EXPLANATIONS - ONLY JSON ARRAY."#.to_string()
    }

    fn build_mount_prompt(
        &self,
        call_sites: &[CallSite],
        framework_detection: &DetectionResult,
    ) -> String {
        let call_sites_json = serde_json::to_string_pretty(call_sites).unwrap_or_default();
        let frameworks_json = serde_json::to_string(framework_detection).unwrap_or_default();

        format!(
            r#"Extract mount relationship details from these pre-identified router mount call sites.

FRAMEWORK CONTEXT:
{}

ROUTER MOUNT CALL SITES:
{}

For each router mount call site, extract:
1. Parent node (the object doing the mounting)
2. Child node (the router/sub-app being mounted)
3. Mount path (the path prefix where it's mounted)
4. File location

Return JSON array with this structure:
[
  {{
    "parent_node": "app",
    "child_node": "apiRouter",
    "mount_path": "/api",
    "location": "server.ts:25:0",
    "confidence": 0.95,
    "reasoning": "Clear mount relationship - app mounting apiRouter at /api"
  }},
  {{
    "parent_node": "router",
    "child_node": "userRouter",
    "mount_path": "/users",
    "location": "routes.ts:15:0",
    "confidence": 0.90,
    "reasoning": "Router mounting sub-router at /users path"
  }},
  ...
]

GUIDELINES:
- These are all router mount relationships (already triaged)
- Extract parent_node from callee_object field
- Extract child_node from the second argument (usually an Identifier)
- Extract mount_path from the first argument (usually a StringLiteral)
- Common patterns:
  - app.use('/path', router) -> parent: app, child: router, path: /path
  - router.use('/path', subRouter) -> parent: router, child: subRouter, path: /path
  - server.mount('/path', handler) -> parent: server, child: handler, path: /path
- Set confidence high (0.9+) for clear patterns, lower for ambiguous cases"#,
            frameworks_json, call_sites_json
        )
    }
}
