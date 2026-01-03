use crate::{
    agents::{framework_guidance_agent::FrameworkGuidance, schemas::AgentSchemas},
    call_site_extractor::CallSite,
    framework_detector::DetectionResult,
    gemini_service::GeminiService,
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
        framework_guidance: &FrameworkGuidance,
    ) -> Result<Vec<Middleware>, Box<dyn std::error::Error>> {
        if call_sites.is_empty() {
            return Ok(Vec::new());
        }

        println!("=== MIDDLEWARE AGENT DEBUG ===");
        println!(
            "Analyzing {} pre-triaged middleware call sites",
            call_sites.len()
        );

        let prompt =
            self.build_middleware_prompt(call_sites, framework_detection, framework_guidance);
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

CONTEXT SLICE (IMPORTANT):
- Each call site may include a `context_slice` field containing a minimal source snippet from the same file.
- The `context_slice` includes the middleware registration call (anchor) plus local variable definitions/import statements relevant to its arguments.
- Use `context_slice` to infer path_prefix/handler details when they are not directly available from args or resolved fields.
- Do NOT assume cross-file values beyond what appears in the `context_slice` (imports are a boundary).

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
        framework_guidance: &FrameworkGuidance,
    ) -> String {
        let call_sites_json = serde_json::to_string_pretty(call_sites).unwrap_or_default();
        let frameworks_json = serde_json::to_string(framework_detection).unwrap_or_default();

        // Format framework-specific middleware patterns
        let middleware_patterns = framework_guidance
            .middleware_patterns
            .iter()
            .map(|p| format!("- {} -> {} ({})", p.pattern, p.description, p.framework))
            .collect::<Vec<_>>()
            .join("\n");

        let parsing_notes = &framework_guidance.parsing_notes;

        format!(
            r#"Extract detailed information from these pre-identified middleware call sites.

FRAMEWORK CONTEXT:
{frameworks_json}

FRAMEWORK-SPECIFIC MIDDLEWARE PATTERNS:
{middleware_patterns}

PARSING NOTES:
{parsing_notes}

MIDDLEWARE CALL SITES:
{call_sites_json}

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
- Use the framework-specific middleware patterns above to understand how middleware works in each framework
- Extract path prefix from string literals if present (e.g., app.use('/api', middleware))
- If path prefix is not a direct literal and cannot be resolved from args/resolved_value, use the `context_slice` field to infer it
- If no path prefix, set to null (applies to all routes)
- Extract node_name from the callee_object field (e.g., "app", "router", "server")
- Identify middleware type based on function name and context:
  - express.json, express.urlencoded = "body-parser"
  - cors() = "cors"
  - express.static() = "static"
  - fastify.addHook() = "lifecycle-hook" (Fastify)
  - app.use('*', fn) = middleware registration (Hono)
  - app.derive() = "context-derivation" (Elysia)
  - Custom function names = "custom"
- For handler, use the actual function name if available
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
    fn test_middleware_prompts_mention_context_slice() {
        let gemini_service = GeminiService::new("mock".to_string());
        let agent = MiddlewareAgent::new(gemini_service);

        let system_message = agent.build_system_message();
        assert!(system_message.contains("context_slice"));

        let prompt = agent.build_middleware_prompt(
            &[dummy_call_site()],
            &empty_detection(),
            &empty_guidance(),
        );
        assert!(prompt.contains("`context_slice`"));
    }
}
