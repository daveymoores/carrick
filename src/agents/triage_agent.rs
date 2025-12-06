use crate::{
    agents::schemas::AgentSchemas, call_site_extractor::CallSite,
    framework_detector::DetectionResult, gemini_service::GeminiService,
};
use serde::{Deserialize, Serialize};

/// Lean version of CallSite for triage - only essential fields needed for classification.
///
/// OPTIMIZATION STRATEGY:
/// This struct implements two key optimizations to reduce LLM prompt size:
/// 1. Send Only Necessary Data Fields - removes `args` and `definition` fields that aren't
///    needed for broad triage classification, reducing prompt size by 30-50%
/// 2. JSON Minification - when serialized, uses compact format without pretty-printing
///
/// DATA FLOW:
/// - Triage Agent: Uses LeanCallSite for classification (this optimization)
/// - Orchestrator: Matches TriageResult.location with original CallSite.location
/// - Downstream Agents: Get full CallSite objects with all fields intact
///
/// This ensures downstream agents still have access to args, definition, etc. while
/// optimizing the triage prompt for size and cost.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LeanCallSite {
    pub callee_object: String,
    pub callee_property: String,
    pub location: String, // Critical: must match original CallSite.location for orchestrator matching
    pub arg_count: usize, // Number of arguments - helps distinguish middleware from mounts
    pub first_arg_type: Option<String>, // Type of first argument (e.g., "StringLiteral", "Identifier")
    pub first_arg_value: Option<String>, // Value of first argument if available (e.g., "/api")
}

impl From<&CallSite> for LeanCallSite {
    fn from(call_site: &CallSite) -> Self {
        let (first_arg_type, first_arg_value) = call_site
            .args
            .first()
            .map(|arg| {
                (
                    Some(format!("{:?}", arg.arg_type)),
                    arg.value.clone().or_else(|| arg.resolved_value.clone()),
                )
            })
            .unwrap_or((None, None));

        Self {
            callee_object: call_site.callee_object.clone(),
            callee_property: call_site.callee_property.clone(),
            location: call_site.location.clone(),
            arg_count: call_site.args.len(),
            first_arg_type,
            first_arg_value,
        }
    }
}

/// Simple classification result from triage
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TriageResult {
    pub location: String,
    pub classification: TriageClassification,
    pub confidence: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum TriageClassification {
    HttpEndpoint,
    DataFetchingCall,
    Middleware,
    RouterMount,
    Irrelevant,
}

/// First-pass agent that broadly classifies all call sites
pub struct TriageAgent {
    gemini_service: GeminiService,
}

impl TriageAgent {
    pub fn new(gemini_service: GeminiService) -> Self {
        Self { gemini_service }
    }

    /// Perform initial broad classification of all call sites with batching
    pub async fn classify_call_sites(
        &self,
        call_sites: &[CallSite],
        framework_detection: &DetectionResult,
    ) -> Result<Vec<TriageResult>, Box<dyn std::error::Error>> {
        if call_sites.is_empty() {
            return Ok(Vec::new());
        }

        println!("=== TRIAGE AGENT DEBUG ===");
        println!("Triaging {} call sites", call_sites.len());

        // Batch size to avoid 503 errors and rate limiting - reduced from 10 to 5
        const BATCH_SIZE: usize = 5;
        let mut all_results = Vec::new();

        for (batch_num, batch) in call_sites.chunks(BATCH_SIZE).enumerate() {
            println!(
                "Processing batch {} of {} ({} call sites)",
                batch_num + 1,
                call_sites.len().div_ceil(BATCH_SIZE),
                batch.len()
            );

            let prompt = self.build_triage_prompt(batch, framework_detection);
            let system_message = self.build_system_message();

            println!(
                "Batch {} prompt length: {} chars",
                batch_num + 1,
                prompt.len()
            );

            let schema = AgentSchemas::triage_schema();
            let response = self
                .gemini_service
                .analyze_code_with_schema(&prompt, &system_message, Some(schema))
                .await?;

            println!("=== RAW GEMINI RESPONSE BATCH {} ===", batch_num + 1);
            println!("{}", response);
            println!("=== END RAW RESPONSE ===");

            let batch_results: Vec<TriageResult> =
                serde_json::from_str(&response).map_err(|e| {
                    format!(
                        "Failed to parse triage response for batch {}: {}. Raw response: {}",
                        batch_num + 1,
                        e,
                        response
                    )
                })?;

            println!(
                "Batch {} classified {} call sites",
                batch_num + 1,
                batch_results.len()
            );
            all_results.extend(batch_results);

            // Delay between batches to avoid rate limiting - increased from 500ms to 1000ms
            if batch_num + 1 < call_sites.len().div_ceil(BATCH_SIZE) {
                tokio::time::sleep(std::time::Duration::from_millis(1000)).await;
            }
        }

        println!("Total triage classified {} call sites", all_results.len());

        // Debug: print classification breakdown
        let mut counts = std::collections::HashMap::new();
        for result in &all_results {
            *counts
                .entry(format!("{:?}", result.classification))
                .or_insert(0) += 1;
        }
        println!("Classification breakdown: {:?}", counts);

        Ok(all_results)
    }

    fn build_system_message(&self) -> String {
        r#"You are a JSON-only code analysis tool. Return ONLY a JSON array with no explanations, no markdown, no text before or after.

TASK: Classify each call site into exactly one category.

CATEGORIES:
- HttpEndpoint: Route definitions (app.get, router.post, etc.)
- DataFetchingCall: Outbound API calls (fetch, axios, response.json, etc.)
- Middleware: Middleware registration (app.use with single argument that is NOT a path)
- RouterMount: Router mounting (app.use('/path', router) - MUST have 2+ args where first is a path string)
- Irrelevant: Everything else (Array methods, console.log, etc.) AND response methods (res.send, res.json, res.status)

CRITICAL DISTINCTION - app.use() and router.use():
Use the arg_count, first_arg_type, and first_arg_value fields to distinguish:
- RouterMount: arg_count >= 2 AND first_arg_type == "StringLiteral" AND first_arg_value starts with "/"
  Examples: app.use('/api', router), router.use('/v1', v1Router)
- Middleware: arg_count == 1 OR first_arg_type != "StringLiteral" OR first_arg_value doesn't start with "/"
  Examples: app.use(cors()), app.use(express.json()), app.use(authMiddleware)

REQUIRED JSON FORMAT:
[
  {
    "location": "exact_location_string",
    "classification": "category_name",
    "confidence": 0.95
  }
]

CRITICAL:
- Return ONLY the JSON array
- No explanations, no markdown, no extra text
- Match EVERY input call site
- Use exact location strings from input"#.to_string()
    }

    fn build_triage_prompt(
        &self,
        call_sites: &[CallSite],
        framework_detection: &DetectionResult,
    ) -> String {
        // In mock mode, pass full call sites so mock generator can classify properly
        // In real mode, use lean call sites to reduce prompt size
        let call_sites_json = if std::env::var("CARRICK_MOCK_ALL").is_ok() {
            serde_json::to_string(call_sites).unwrap_or_default()
        } else {
            // Strategy 1: Send only necessary data fields - convert to lean call sites
            let lean_call_sites: Vec<LeanCallSite> =
                call_sites.iter().map(|cs| cs.into()).collect();
            // Strategy 2: Minify JSON - use compact serialization without pretty printing
            serde_json::to_string(&lean_call_sites).unwrap_or_default()
        };
        let frameworks_json = serde_json::to_string(framework_detection).unwrap_or_default();

        format!(
            r#"Perform initial triage classification of these JavaScript/TypeScript call sites.

FRAMEWORK CONTEXT:
{}

CALL SITES TO CLASSIFY:
{}

For each call site, assign it to one of these categories:
- HttpEndpoint: Defines routes that handle incoming HTTP requests
- DataFetchingCall: Makes outbound API calls or fetches data
- Middleware: Registers middleware (arg_count=1, or first arg is NOT a path string)
- RouterMount: Mounts routers (arg_count>=2, first_arg_type="StringLiteral", first_arg_value starts with "/")
- Irrelevant: Utility functions, logging, Array methods, and response methods (res.send, res.json, etc.)

IMPORTANT: Each call site includes arg_count, first_arg_type, and first_arg_value fields.
Use these to distinguish RouterMount from Middleware:
- If callee_property is "use" AND arg_count >= 2 AND first_arg_type == "StringLiteral" AND first_arg_value starts with "/" -> RouterMount
- If callee_property is "use" AND (arg_count == 1 OR first_arg_type != "StringLiteral") -> Middleware

Return JSON array with this structure:
[
  {{
    "location": "repo-a_server.ts:18:0",
    "classification": "HttpEndpoint",
    "confidence": 0.95
  }},
  {{
    "location": "repo-a_server.ts:59:37",
    "classification": "DataFetchingCall",
    "confidence": 0.85
  }},
  ...
]

GUIDELINES:
- Use the framework context to understand what libraries are in use
- app.get('/path', handler) = HttpEndpoint
- fetch('url') (global.fetch) or axios.get() = DataFetchingCall
- response.json() or resp.text() = DataFetchingCall (parsing API responses)
- app.use(middleware) where arg_count=1 = Middleware
- app.use('/api', router) where arg_count=2, first_arg_type="StringLiteral", first_arg_value="/api" = RouterMount
- router.use('/v1', v1Router) where arg_count=2, first_arg_type="StringLiteral" = RouterMount
- Array.isArray() or console.log() = Irrelevant
- Match EVERY input call site with exactly ONE classification
- Use the exact location strings from the input"#,
            frameworks_json, call_sites_json
        )
    }
}
