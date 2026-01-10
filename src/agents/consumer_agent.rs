use crate::{
    agent_service::AgentService,
    agents::{framework_guidance_agent::FrameworkGuidance, schemas::AgentSchemas},
    call_site_extractor::CallSite,
    framework_detector::DetectionResult,
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
    agent_service: AgentService,
}

impl ConsumerAgent {
    pub fn new(agent_service: AgentService) -> Self {
        Self { agent_service }
    }

    /// Extract details from pre-triaged data fetching call sites
    pub async fn detect_data_fetching_calls(
        &self,
        call_sites: &[CallSite],
        framework_detection: &DetectionResult,
        framework_guidance: &FrameworkGuidance,
    ) -> Result<Vec<DataFetchingCall>, Box<dyn std::error::Error>> {
        if call_sites.is_empty() {
            return Ok(Vec::new());
        }

        println!("=== CONSUMER AGENT DEBUG ===");
        println!(
            "Analyzing {} pre-triaged data fetching call sites",
            call_sites.len()
        );

        let system_message = self.build_system_message();

        // Batch size for parallel processing
        const BATCH_SIZE: usize = 10;
        let mut all_calls = Vec::new();
        let mut join_set = tokio::task::JoinSet::new();
        let total_batches = call_sites.len().div_ceil(BATCH_SIZE);

        for (batch_idx, batch) in call_sites.chunks(BATCH_SIZE).enumerate() {
            let batch_num = batch_idx + 1;
            println!(
                "Preparing consumer batch {} of {} ({} call sites)",
                batch_num,
                total_batches,
                batch.len()
            );

            let prompt = self.build_fetching_prompt(batch, framework_detection, framework_guidance);
            let system_message_clone = system_message.clone();
            let agent_service = self.agent_service.clone();

            join_set.spawn(async move {
                let schema = AgentSchemas::consumer_schema();
                let response = agent_service
                    .analyze_code_with_schema(&prompt, &system_message_clone, Some(schema))
                    .await
                    .map_err(|e| {
                        format!("Agent API error in consumer batch {}: {}", batch_num, e)
                    })?;

                let calls: Vec<DataFetchingCall> =
                    serde_json::from_str(&response).map_err(|e| {
                        format!(
                            "Failed to parse data fetching detection response for batch {}: {}",
                            batch_num, e
                        )
                    })?;

                Ok::<Vec<DataFetchingCall>, String>(calls)
            });
        }

        while let Some(res) = join_set.join_next().await {
            match res {
                Ok(Ok(calls)) => all_calls.extend(calls),
                Ok(Err(e)) => return Err(e.into()),
                Err(e) => return Err(Box::new(e)),
            }
        }

        Ok(all_calls)
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

CONTEXT SLICE (IMPORTANT):
- Each call site may include a `context_slice` field containing a minimal source snippet from the same file.
- The `context_slice` includes the call itself (anchor) plus the local variable definitions/import statements that define identifiers used in the call.
- Use `context_slice` to infer URLs/methods when they are not directly available in args or resolved fields.
- Do NOT assume cross-file values beyond what appears in the `context_slice` (imports are a boundary).

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
        framework_guidance: &FrameworkGuidance,
    ) -> String {
        let call_sites_json = serde_json::to_string_pretty(call_sites).unwrap_or_default();
        let frameworks_json = serde_json::to_string(framework_detection).unwrap_or_default();

        // Format framework-specific data fetching patterns
        let data_fetching_patterns = framework_guidance
            .data_fetching_patterns
            .iter()
            .map(|p| format!("- {} -> {} ({})", p.pattern, p.description, p.framework))
            .collect::<Vec<_>>()
            .join("\n");

        let parsing_notes = &framework_guidance.parsing_notes;

        format!(
            r#"Extract detailed information from these pre-identified data fetching call sites.

FRAMEWORK CONTEXT:
{frameworks_json}

FRAMEWORK-SPECIFIC DATA FETCHING PATTERNS:
{data_fetching_patterns}

PARSING NOTES:
{parsing_notes}

DATA FETCHING CALL SITES:
{call_sites_json}

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
- Use the framework-specific data fetching patterns above to understand how each library makes HTTP calls
- Prefer SWC-extracted values if present (e.g., `correlated_call.url` on response parsing calls)
- Extract URL from string literals in arguments if present, otherwise set to null
- If arguments are Identifiers, check the "resolved_value" field for the actual string value
- If URL cannot be resolved from args/resolved_value/correlated_call, use the `context_slice` field to infer the URL
- If arguments are TemplateLiterals, use the "value" field which contains the reconstructed template string
- Infer HTTP method from function name (get=GET, post=POST, etc.)
- For response parsing calls (.json(), .text()), use "response_parsing" as library
- For direct HTTP client calls (axios.get, fetch), extract the client library name
- If callee_object is "global", the library name is the callee_property (e.g., "fetch")
- ky and got use .json() chaining for response parsing
- ofetch uses $fetch() function
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
            callee_object: "fetch".to_string(),
            callee_property: "fetch".to_string(),
            args: vec![],
            definition: None,
            location: "client.ts:1:1".to_string(),
            result_type: None,
            correlated_call: None,
            context_slice: None,
        }
    }

    #[test]
    fn test_consumer_prompts_mention_context_slice() {
        let agent_service = crate::agent_service::AgentService::new("mock".to_string());
        let agent = ConsumerAgent::new(agent_service);

        let system_message = agent.build_system_message();
        assert!(system_message.contains("context_slice"));

        let prompt = agent.build_fetching_prompt(
            &[dummy_call_site()],
            &empty_detection(),
            &empty_guidance(),
        );
        assert!(prompt.contains("`context_slice`"));
    }
}
