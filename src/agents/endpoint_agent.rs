use crate::{
    agent_service::AgentService,
    agents::{framework_guidance_agent::FrameworkGuidance, schemas::AgentSchemas},
    call_site_extractor::CallSite,
    framework_detector::DetectionResult,
};
use serde::{Deserialize, Serialize};

/// Represents a detected HTTP endpoint
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HttpEndpoint {
    pub method: String,
    pub path: String,
    pub handler: String,
    pub node_name: String, // The actual callee_object from source code (framework-agnostic)
    pub location: String,
    pub confidence: f32,
    pub reasoning: String,
    pub response_type_file: Option<String>,
    pub response_type_position: Option<u32>,
    pub response_type_string: Option<String>,
}

/// Specialized agent for detecting HTTP endpoints (routes)
pub struct EndpointAgent {
    agent_service: AgentService,
}

impl EndpointAgent {
    pub fn new(agent_service: AgentService) -> Self {
        Self { agent_service }
    }

    /// Detect HTTP endpoints from call sites
    /// Uses prompt caching to reduce costs when processing multiple batches
    pub async fn detect_endpoints(
        &self,
        call_sites: &[CallSite],
        framework_detection: &DetectionResult,
        framework_guidance: &FrameworkGuidance,
    ) -> Result<Vec<HttpEndpoint>, Box<dyn std::error::Error>> {
        if call_sites.is_empty() {
            return Ok(Vec::new());
        }

        println!("=== ENDPOINT AGENT DEBUG ===");
        println!(
            "Analyzing {} pre-triaged endpoint call sites",
            call_sites.len()
        );

        let system_message = self.build_system_message();

        // Batch size for parallel processing
        const BATCH_SIZE: usize = 10;
        let mut all_endpoints = Vec::new();
        let mut join_set = tokio::task::JoinSet::new();
        let total_batches = call_sites.len().div_ceil(BATCH_SIZE);

        for (batch_idx, batch) in call_sites.chunks(BATCH_SIZE).enumerate() {
            let batch_num = batch_idx + 1;
            println!(
                "Preparing endpoint batch {} of {} ({} call sites)",
                batch_num,
                total_batches,
                batch.len()
            );

            let prompt = self.build_endpoint_prompt(batch, framework_detection, framework_guidance);
            let system_message_clone = system_message.clone();
            let agent_service = self.agent_service.clone();

            join_set.spawn(async move {
                let schema = AgentSchemas::endpoint_schema();
                let response = agent_service
                    .analyze_code_with_schema(&prompt, &system_message_clone, Some(schema))
                    .await
                    .map_err(|e| {
                        format!("Agent API error in endpoint batch {}: {}", batch_num, e)
                    })?;

                println!("=== RAW AGENT ENDPOINT RESPONSE BATCH {} ===", batch_num);
                println!("{}", response);
                println!("=== END RAW RESPONSE ===");

                let endpoints: Vec<HttpEndpoint> =
                    serde_json::from_str(&response).map_err(|e| {
                        format!(
                            "Failed to parse endpoint detection response for batch {}: {}",
                            batch_num, e
                        )
                    })?;

                Ok::<Vec<HttpEndpoint>, String>(endpoints)
            });
        }

        while let Some(res) = join_set.join_next().await {
            match res {
                Ok(Ok(endpoints)) => all_endpoints.extend(endpoints),
                Ok(Err(e)) => return Err(e.into()),
                Err(e) => return Err(Box::new(e)),
            }
        }

        println!("Extracted {} endpoints:", all_endpoints.len());
        for (i, endpoint) in all_endpoints.iter().enumerate() {
            println!(
                "  {}. {} {} (owner: {})",
                i + 1,
                endpoint.method,
                endpoint.path,
                endpoint.node_name
            );
        }

        Ok(all_endpoints)
    }

    fn build_system_message(&self) -> String {
        r#"You are an expert at extracting detailed information from HTTP endpoint definitions.

These call sites have already been identified as HTTP endpoints by a triage process. Your task is to extract the specific details from each one.

EXTRACTION GOALS:
- HTTP method (GET, POST, PUT, DELETE, etc.)
- Route path (e.g., "/users", "/users/:id", "/api/v1/orders")
- Handler function name (or "anonymous" if inline function)
- Location information
- Response Type Information (CRITICAL for type checking):
  - Identify the return type of the handler (e.g., `Response<User[]>`)
  - Extract the file path where the type is used
  - Estimate the start position (character index) of the type annotation
  - Extract the type string itself

CONTEXT SLICE (IMPORTANT):
- Each call site may include a `context_slice` field containing a minimal source snippet from the same file.
- The `context_slice` includes the endpoint call itself (anchor) plus the local variable definitions/import statements that define identifiers used in the call.
- Use `context_slice` to resolve identifier-based route paths and handler references when the path is not directly a literal.
- Do NOT assume cross-file values beyond what appears in the `context_slice` (imports are a boundary).

CRITICAL REQUIREMENTS:
1. Return ONLY valid JSON array
2. Extract details from ALL provided call sites (they're all endpoints)
3. Set confidence based on how clearly you can extract the details
4. Provide brief reasoning for each extraction
5. For response types, look for `res: Response<Type>` annotations in Express handlers.

NO EXPLANATIONS - ONLY JSON ARRAY."#.to_string()
    }

    fn build_endpoint_prompt(
        &self,
        call_sites: &[CallSite],
        framework_detection: &DetectionResult,
        framework_guidance: &FrameworkGuidance,
    ) -> String {
        let call_sites_json = serde_json::to_string_pretty(call_sites).unwrap_or_default();
        let frameworks_json = serde_json::to_string(framework_detection).unwrap_or_default();

        // Format framework-specific endpoint patterns
        let endpoint_patterns = framework_guidance
            .endpoint_patterns
            .iter()
            .map(|p| format!("- {} -> {} ({})", p.pattern, p.description, p.framework))
            .collect::<Vec<_>>()
            .join("\n");

        let parsing_notes = &framework_guidance.parsing_notes;

        format!(
            r#"Extract detailed information from these pre-identified HTTP endpoint call sites.

FRAMEWORK CONTEXT:
{frameworks_json}

FRAMEWORK-SPECIFIC ENDPOINT PATTERNS:
{endpoint_patterns}

PARSING NOTES:
{parsing_notes}

HTTP ENDPOINT CALL SITES:
{call_sites_json}

For each endpoint call site, extract:
1. HTTP method (GET, POST, PUT, DELETE, etc.)
2. Route path (e.g., "/users", "/users/:id", "/api/v1/orders")
3. Handler function name (or "anonymous" if unnamed)
4. File location
5. Response Type Info (file, position, string) - look for `res: Response<T>`

Return JSON array with this structure:
[
  {{
    "method": "GET",
    "path": "/users/:id",
    "handler": "getUserProfile",
    "node_name": "app",
    "location": "server.ts:46:0",
    "confidence": 0.95,
    "reasoning": "Express route handler with path parameter",
    "response_type_file": "server.ts",
    "response_type_position": 1250,
    "response_type_string": "Response<UserProfile>"
  }},
  ...
]

GUIDELINES:
- These are all HTTP endpoint definitions (already triaged)
- Use the framework-specific patterns above to understand how endpoints are defined
- Extract the actual path string from string literals in arguments
- If an argument is an Identifier, use the "resolved_value" field if available to find the actual path string
- If the identifier path cannot be resolved from args/resolved_value, use the `context_slice` field to infer the concrete route string
- If an argument is a TemplateLiteral, use the "value" field which contains the reconstructed template string
- Extract node_name from the callee_object field (e.g., "app", "router", "fastify")
- If handler is an inline function, use "anonymous"
- If handler is a variable reference, use the variable name
- Infer HTTP method from the callee_property (get=GET, post=POST, etc.)
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
            callee_property: "get".to_string(),
            args: vec![],
            definition: None,
            location: "server.ts:1:1".to_string(),
            result_type: None,
            correlated_call: None,
            context_slice: None,
        }
    }

    #[test]
    fn test_endpoint_prompts_mention_context_slice() {
        let agent_service = crate::agent_service::AgentService::new("mock".to_string());
        let agent = EndpointAgent::new(agent_service);

        let system_message = agent.build_system_message();
        assert!(system_message.contains("context_slice"));

        let prompt = agent.build_endpoint_prompt(
            &[dummy_call_site()],
            &empty_detection(),
            &empty_guidance(),
        );
        assert!(prompt.contains("`context_slice`"));
    }
}
