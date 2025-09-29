use crate::{
    agents::schemas::AgentSchemas, call_site_extractor::CallSite,
    framework_detector::DetectionResult, gemini_service::GeminiService,
};
use serde::{Deserialize, Serialize};

/// Represents a detected middleware registration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Middleware {
    pub middleware_type: String,
    pub path_prefix: Option<String>,
    pub handler: String,
    pub node_name: String, // The actual callee_object from source code (framework-agnostic)
    pub location: String,
    pub confidence: f32,
    pub reasoning: String,
}

/// Specialized agent for detecting middleware registrations
pub struct MiddlewareAgent {
    gemini_service: GeminiService,
}

impl MiddlewareAgent {
    pub fn new(gemini_service: GeminiService) -> Self {
        Self { gemini_service }
    }

    /// Extract details from pre-triaged middleware call sites
    pub async fn detect_middleware(
        &self,
        call_sites: &[CallSite],
        framework_detection: &DetectionResult,
    ) -> Result<Vec<Middleware>, Box<dyn std::error::Error>> {
        if call_sites.is_empty() {
            return Ok(Vec::new());
        }

        println!("=== MIDDLEWARE AGENT DEBUG ===");
        println!(
            "Analyzing {} pre-triaged middleware call sites",
            call_sites.len()
        );

        let prompt = self.build_middleware_prompt(call_sites, framework_detection);
        let system_message = self.build_system_message();

        let schema = AgentSchemas::middleware_schema();
        let response = self
            .gemini_service
            .analyze_code_with_schema(&prompt, &system_message, Some(schema))
            .await?;

        let middleware: Vec<Middleware> = serde_json::from_str(&response)
            .map_err(|e| format!("Failed to parse middleware detection response: {}", e))?;

        Ok(middleware)
    }

    fn build_system_message(&self) -> String {
        r#"You are an expert at extracting detailed information from middleware registrations.

These call sites have already been identified as middleware registrations by a triage process. Your task is to extract the specific details from each one.

EXTRACTION GOALS:
- Middleware type (body-parser, cors, auth, custom, etc.)
- Path prefix (if middleware applies to specific paths)
- Handler function name or description
- Location information

CRITICAL REQUIREMENTS:
1. Return ONLY valid JSON array
2. Extract details from ALL provided call sites (they're all middleware registrations)
3. Set confidence based on how clearly you can extract the details
4. Provide brief reasoning for each extraction

NO EXPLANATIONS - ONLY JSON ARRAY."#.to_string()
    }

    fn build_middleware_prompt(
        &self,
        call_sites: &[CallSite],
        framework_detection: &DetectionResult,
    ) -> String {
        let call_sites_json = serde_json::to_string_pretty(call_sites).unwrap_or_default();
        let frameworks_json = serde_json::to_string(framework_detection).unwrap_or_default();

        format!(
            r#"Extract detailed information from these pre-identified middleware call sites.

FRAMEWORK CONTEXT:
{}

MIDDLEWARE CALL SITES:
{}

For each middleware call site, extract:
1. Middleware type (body-parser, cors, auth, static, custom, etc.)
2. Path prefix (if the middleware applies to specific paths, otherwise null)
3. Handler function name or description
4. File location

Return JSON array with this structure:
[
  {{
    "middleware_type": "body-parser",
    "path_prefix": null,
    "handler": "express.json",
    "node_name": "app",
    "location": "server.ts:15:0",
    "confidence": 0.95,
    "reasoning": "Express JSON body parser middleware"
  }},
  {{
    "middleware_type": "custom",
    "path_prefix": "/api",
    "handler": "authMiddleware",
    "node_name": "router",
    "location": "server.ts:22:0",
    "confidence": 0.90,
    "reasoning": "Custom middleware mounted on /api path"
  }},
  ...
]

GUIDELINES:
- These are all middleware registrations (already triaged)
- Extract path prefix from string literals if present (e.g., app.use('/api', middleware))
- If no path prefix, set to null (applies to all routes)
- Extract node_name from the callee_object field (e.g., "app", "router", "server")
- Identify middleware type based on function name and context:
  - express.json, express.urlencoded = "body-parser"
  - cors() = "cors"
  - express.static() = "static"
  - Custom function names = "custom"
- For handler, use the actual function name if available
- Set confidence high (0.9+) for clear patterns, lower for ambiguous cases"#,
            frameworks_json, call_sites_json
        )
    }
}
