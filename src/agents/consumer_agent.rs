use crate::{
    agents::schemas::AgentSchemas, call_site_extractor::CallSite,
    framework_detector::DetectionResult, gemini_service::GeminiService,
};
use serde::{Deserialize, Serialize};

/// Represents a detected outbound API call
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DataFetchingCall {
    pub library: String,
    pub url: Option<String>,
    pub method: Option<String>,
    pub location: String,
    pub confidence: f32,
    pub reasoning: String,
    pub expected_type_file: Option<String>,
    pub expected_type_position: Option<u32>,
    pub expected_type_string: Option<String>,
}

/// Specialized agent for detecting outbound API/data fetching calls
pub struct ConsumerAgent {
    gemini_service: GeminiService,
}

impl ConsumerAgent {
    pub fn new(gemini_service: GeminiService) -> Self {
        Self { gemini_service }
    }

    /// Extract details from pre-triaged data fetching call sites
    pub async fn detect_data_fetching_calls(
        &self,
        call_sites: &[CallSite],
        framework_detection: &DetectionResult,
    ) -> Result<Vec<DataFetchingCall>, Box<dyn std::error::Error>> {
        if call_sites.is_empty() {
            return Ok(Vec::new());
        }

        println!("=== CONSUMER AGENT DEBUG ===");
        println!(
            "Analyzing {} pre-triaged data fetching call sites",
            call_sites.len()
        );

        let prompt = self.build_fetching_prompt(call_sites, framework_detection);
        let system_message = self.build_system_message();

        let schema = AgentSchemas::consumer_schema();
        let response = self
            .gemini_service
            .analyze_code_with_schema(&prompt, &system_message, Some(schema))
            .await?;

        let calls: Vec<DataFetchingCall> = serde_json::from_str(&response)
            .map_err(|e| format!("Failed to parse data fetching detection response: {}", e))?;

        Ok(calls)
    }

    fn build_system_message(&self) -> String {
        r#"You are an expert at extracting detailed information from data fetching and API calls.

These call sites have already been identified as data fetching calls by a triage process. Your task is to extract the specific details from each one.

EXTRACTION GOALS:
- Library name (fetch, axios, got, response parsing, etc.)
- URL being called (if detectable from string literals)
- HTTP method (if detectable)
- Location information
- Expected Response Type (CRITICAL for type checking):
  - Identify the type expected from the API call (e.g., `const data: User[] = await ...`)
  - Extract the file path where the type is used
  - Estimate the start position (character index) of the type annotation
  - Extract the type string itself

CRITICAL REQUIREMENTS:
1. Return ONLY valid JSON array
2. Extract details from ALL provided call sites (they're all data fetching calls)
3. Set confidence based on how clearly you can extract the details
4. Provide brief reasoning for each extraction
5. For expected types, look for variable type annotations (`const x: Type = ...`) or generic calls (`axios.get<Type>(...)`).

NO EXPLANATIONS - ONLY JSON ARRAY."#.to_string()
    }

    fn build_fetching_prompt(
        &self,
        call_sites: &[CallSite],
        framework_detection: &DetectionResult,
    ) -> String {
        let call_sites_json = serde_json::to_string_pretty(call_sites).unwrap_or_default();
        let frameworks_json = serde_json::to_string(framework_detection).unwrap_or_default();

        format!(
            r#"Extract detailed information from these pre-identified data fetching call sites.

FRAMEWORK CONTEXT:
{}

DATA FETCHING CALL SITES:
{}

For each data fetching call site, extract:
1. Library name (fetch, axios, got, etc.)
2. URL being called (if detectable from string literals)
3. HTTP method (GET, POST, etc. if detectable)
4. File location
5. Expected Response Type Info (file, position, string)

Return JSON array with this structure:
[
  {{
    "library": "fetch",
    "url": "http://localhost:3002/orders",
    "method": "GET",
    "location": "server.ts:58:11",
    "confidence": 0.95,
    "reasoning": "Direct fetch call with URL",
    "expected_type_file": "server.ts",
    "expected_type_position": 850,
    "expected_type_string": "Order[]"
  }},
  {{
    "library": "axios",
    "url": null,
    "method": "POST",
    "location": "api.ts:23:5",
    "confidence": 0.85,
    "reasoning": "Axios POST method call",
    "expected_type_file": null,
    "expected_type_position": null,
    "expected_type_string": null
  }},
  ...
]

GUIDELINES:
- These are all data fetching calls (already triaged)
- Extract URL from string literals in arguments if present, otherwise set to null
- If arguments are Identifiers, check the "resolved_value" field for the actual string value
- If arguments are TemplateLiterals, use the "value" field which contains the reconstructed template string
- Infer HTTP method from function name (get=GET, post=POST, etc.)
- Infer HTTP method from function name (get=GET, post=POST, etc.)
- For response parsing calls (.json(), .text()), use "response_parsing" as library
- For direct HTTP client calls (axios.get, fetch), extract the client library name
- If callee_object is "global", the library name is the callee_property (e.g., "fetch")
- Set confidence high (0.9+) for clear patterns, lower for ambiguous cases"#,
            frameworks_json, call_sites_json
        )
    }
}
