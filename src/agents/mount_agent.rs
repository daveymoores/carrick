use crate::{
    agent_service::AgentService,
    agents::{framework_guidance_agent::FrameworkGuidance, schemas::AgentSchemas},
    call_site_extractor::CallSite,
    framework_detector::DetectionResult,
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
    agent_service: AgentService,
}

impl MountAgent {
    pub fn new(agent_service: AgentService) -> Self {
        Self { agent_service }
    }

    /// Extract mount relationships from pre-triaged RouterMount call sites
    pub async fn detect_mounts(
        &self,
        call_sites: &[CallSite],
        framework_detection: &DetectionResult,
        framework_guidance: &FrameworkGuidance,
    ) -> Result<Vec<MountRelationship>, Box<dyn std::error::Error>> {
        if call_sites.is_empty() {
            return Ok(Vec::new());
        }

        println!("=== MOUNT AGENT DEBUG ===");
        println!(
            "Analyzing {} pre-triaged router mount call sites",
            call_sites.len()
        );

        let system_message = self.build_system_message();

        // Batch size for parallel processing
        const BATCH_SIZE: usize = 10;
        let mut all_mounts = Vec::new();
        let mut join_set = tokio::task::JoinSet::new();
        let total_batches = call_sites.len().div_ceil(BATCH_SIZE);

        for (batch_idx, batch) in call_sites.chunks(BATCH_SIZE).enumerate() {
            let batch_num = batch_idx + 1;
            println!(
                "Preparing mount batch {} of {} ({} call sites)",
                batch_num,
                total_batches,
                batch.len()
            );

            let prompt = self.build_mount_prompt(batch, framework_detection, framework_guidance);
            let system_message_clone = system_message.clone();
            let agent_service = self.agent_service.clone();

            join_set.spawn(async move {
                let schema = AgentSchemas::mount_schema();
                let response = agent_service
                    .analyze_code_with_schema(&prompt, &system_message_clone, Some(schema))
                    .await
                    .map_err(|e| format!("Agent API error in mount batch {}: {}", batch_num, e))?;

                println!("=== RAW GEMINI MOUNT RESPONSE BATCH {} ===", batch_num);
                println!("{}", response);
                println!("=== END RAW RESPONSE ===");

                let mounts: Vec<MountRelationship> =
                    serde_json::from_str(&response).map_err(|e| {
                        format!(
                            "Failed to parse mount detection response for batch {}: {}",
                            batch_num, e
                        )
                    })?;

                Ok::<Vec<MountRelationship>, String>(mounts)
            });
        }

        while let Some(res) = join_set.join_next().await {
            match res {
                Ok(Ok(mounts)) => all_mounts.extend(mounts),
                Ok(Err(e)) => return Err(e.into()),
                Err(e) => return Err(Box::new(e)),
            }
        }

        println!("Extracted {} mount relationships:", all_mounts.len());
        for (i, mount) in all_mounts.iter().enumerate() {
            println!(
                "  {}. {} mounts {} at {}",
                i + 1,
                mount.parent_node,
                mount.child_node,
                mount.mount_path
            );
        }

        Ok(all_mounts)
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

CONTEXT SLICE (IMPORTANT):
- Each call site may include a `context_slice` field containing a minimal source snippet from the same file.
- The `context_slice` includes the mount call itself (anchor) plus local variable definitions/import statements relevant to the mount arguments.
- Use `context_slice` to infer mount_path/child_node when they are not directly available from args or resolved fields.
- Do NOT assume cross-file values beyond what appears in the `context_slice` (imports are a boundary).

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
        framework_guidance: &FrameworkGuidance,
    ) -> String {
        let call_sites_json = serde_json::to_string_pretty(call_sites).unwrap_or_default();
        let frameworks_json = serde_json::to_string(framework_detection).unwrap_or_default();

        // Format framework-specific mount patterns
        let mount_patterns = framework_guidance
            .mount_patterns
            .iter()
            .map(|p| format!("- {} -> {} ({})", p.pattern, p.description, p.framework))
            .collect::<Vec<_>>()
            .join("\n");

        let parsing_notes = &framework_guidance.parsing_notes;

        format!(
            r#"Extract mount relationship details from these pre-identified router mount call sites.

FRAMEWORK CONTEXT:
{frameworks_json}

FRAMEWORK-SPECIFIC MOUNT PATTERNS:
{mount_patterns}

PARSING NOTES:
{parsing_notes}

ROUTER MOUNT CALL SITES:
{call_sites_json}

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
- If arguments are Identifiers, check the "resolved_value" field for the actual string value
- If mount_path cannot be resolved from args/resolved_value, use the `context_slice` field to infer the concrete mount path
- If arguments are TemplateLiterals, use the "value" field which contains the reconstructed template string
- Use the framework-specific mount patterns above to understand how mounts work in each framework
- Common patterns:
  - app.use('/path', router) -> parent: app, child: router, path: /path
  - router.use('/path', subRouter) -> parent: router, child: subRouter, path: /path
  - server.mount('/path', handler) -> parent: server, child: handler, path: /path
  - app.route('/path', subApp) -> parent: app, child: subApp, path: /path (Hono)
  - fastify.register(routes, {{ prefix: '/path' }}) -> parent: fastify, child: routes, path: /path (Fastify)
- Set confidence high (0.9+) for clear patterns, lower for ambiguous cases"#
        )
    }
}

#[cfg(test)]
mod prompt_tests {
    use super::*;

    fn empty_detection() -> DetectionResult {
        DetectionResult {
            frameworks: vec![],
            data_fetchers: vec![],
            notes: "test".to_string(),
        }
    }

    fn empty_guidance() -> FrameworkGuidance {
        FrameworkGuidance {
            mount_patterns: vec![],
            endpoint_patterns: vec![],
            middleware_patterns: vec![],
            data_fetching_patterns: vec![],
            triage_hints: String::new(),
            parsing_notes: String::new(),
        }
    }

    fn dummy_call_site() -> CallSite {
        CallSite {
            callee_object: "app".to_string(),
            callee_property: "use".to_string(),
            args: vec![],
            definition: None,
            location: "server.ts:1:1".to_string(),
            result_type: None,
            correlated_call: None,
            context_slice: None,
        }
    }

    #[test]
    fn test_mount_prompts_mention_context_slice() {
        let agent_service = AgentService::new("mock".to_string());
        let agent = MountAgent::new(agent_service);

        let system_message = agent.build_system_message();
        assert!(system_message.contains("context_slice"));

        let prompt =
            agent.build_mount_prompt(&[dummy_call_site()], &empty_detection(), &empty_guidance());
        assert!(prompt.contains("`context_slice`"));
    }
}
