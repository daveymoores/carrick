use crate::{
    agents::schemas::AgentSchemas, call_site_extractor::CallSite,
    framework_detector::DetectionResult, gemini_service::GeminiService,
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
}

/// Specialized agent for detecting HTTP endpoints (routes)
pub struct EndpointAgent {
    gemini_service: GeminiService,
}

impl EndpointAgent {
    pub fn new(gemini_service: GeminiService) -> Self {
        Self { gemini_service }
    }

    /// Detect HTTP endpoints from call sites
    pub async fn detect_endpoints(
        &self,
        call_sites: &[CallSite],
        framework_detection: &DetectionResult,
    ) -> Result<Vec<HttpEndpoint>, Box<dyn std::error::Error>> {
        if call_sites.is_empty() {
            return Ok(Vec::new());
        }

        println!("=== ENDPOINT AGENT DEBUG ===");
        println!(
            "Analyzing {} pre-triaged endpoint call sites",
            call_sites.len()
        );

        let prompt = self.build_endpoint_prompt(call_sites, framework_detection);
        let system_message = self.build_system_message();

        let schema = AgentSchemas::endpoint_schema();
        let response = self
            .gemini_service
            .analyze_code_with_schema(&prompt, &system_message, Some(schema))
            .await?;

        println!("=== RAW GEMINI ENDPOINT RESPONSE ===");
        println!("{}", response);
        println!("=== END RAW RESPONSE ===");

        let endpoints: Vec<HttpEndpoint> = serde_json::from_str(&response)
            .map_err(|e| format!("Failed to parse endpoint detection response: {}", e))?;

        println!("Extracted {} endpoints:", endpoints.len());
        for (i, endpoint) in endpoints.iter().enumerate() {
            println!(
                "  {}. {} {} (owner: {})",
                i + 1,
                endpoint.method,
                endpoint.path,
                endpoint.node_name
            );
        }

        Ok(endpoints)
    }

    fn build_system_message(&self) -> String {
        r#"You are an expert at extracting detailed information from HTTP endpoint definitions.

These call sites have already been identified as HTTP endpoints by a triage process. Your task is to extract the specific details from each one.

EXTRACTION GOALS:
- HTTP method (GET, POST, PUT, DELETE, etc.)
- Route path (e.g., "/users", "/users/:id", "/api/v1/orders")
- Handler function name (or "anonymous" if inline function)
- Location information

CRITICAL REQUIREMENTS:
1. Return ONLY valid JSON array
2. Extract details from ALL provided call sites (they're all endpoints)
3. Set confidence based on how clearly you can extract the details
4. Provide brief reasoning for each extraction

NO EXPLANATIONS - ONLY JSON ARRAY."#.to_string()
    }

    fn build_endpoint_prompt(
        &self,
        call_sites: &[CallSite],
        framework_detection: &DetectionResult,
    ) -> String {
        let call_sites_json = serde_json::to_string_pretty(call_sites).unwrap_or_default();
        let frameworks_json = serde_json::to_string(framework_detection).unwrap_or_default();

        format!(
            r#"Extract detailed information from these pre-identified HTTP endpoint call sites.

FRAMEWORK CONTEXT:
{}

HTTP ENDPOINT CALL SITES:
{}

For each endpoint call site, extract:
1. HTTP method (GET, POST, PUT, DELETE, etc.)
2. Route path (e.g., "/users", "/users/:id", "/api/v1/orders")
3. Handler function name (or "anonymous" if unnamed)
4. File location

Return JSON array with this structure:
[
  {{
    "method": "GET",
    "path": "/users/:id",
    "handler": "getUserProfile",
    "node_name": "app",
    "location": "server.ts:46:0",
    "confidence": 0.95,
    "reasoning": "Express route handler with path parameter"
  }},
  ...
]

GUIDELINES:
- These are all HTTP endpoint definitions (already triaged)
- Extract the actual path string from string literals in arguments
- If an argument is an Identifier, use the "resolved_value" field if available to find the actual path string
- If an argument is a TemplateLiteral, use the "value" field which contains the reconstructed template string
- Extract node_name from the callee_object field (e.g., "app", "router", "fastify")
- Extract node_name from the callee_object field (e.g., "app", "router", "fastify")
- If handler is an inline function, use "anonymous"
- If handler is a variable reference, use the variable name
- Infer HTTP method from the callee_property (get=GET, post=POST, etc.)
- Set confidence high (0.9+) for clear patterns, lower for ambiguous cases"#,
            frameworks_json, call_sites_json
        )
    }
}
