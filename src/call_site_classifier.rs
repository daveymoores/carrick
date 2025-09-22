use crate::{
    call_site_extractor::CallSite,
    framework_detector::DetectionResult,
    gemini_service::GeminiService,
};
use serde::{Deserialize, Serialize};

/// Classification result for a call site
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClassifiedCallSite {
    #[serde(flatten)]
    pub call_site: CallSite,
    pub classification: CallSiteType,
    pub confidence: f32,
    pub reasoning: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum CallSiteType {
    RouterMount,
    Middleware,
    HttpEndpoint,
    DataFetchingCall,
    GraphQLCall,
    Irrelevant,
}

/// Service for classifying call sites using LLM with framework context
pub struct CallSiteClassifier {
    gemini_service: GeminiService,
}

impl CallSiteClassifier {
    pub fn new(gemini_service: GeminiService) -> Self {
        Self { gemini_service }
    }

    /// Classify all call sites using framework context
    pub async fn classify_call_sites(
        &self,
        call_sites: &[CallSite],
        framework_detection: &DetectionResult,
    ) -> Result<Vec<ClassifiedCallSite>, Box<dyn std::error::Error>> {
        let prompt = self.build_classification_prompt(call_sites, framework_detection);
        let system_message = self.build_system_message();

        let response = self.gemini_service.analyze_code(&prompt, &system_message).await?;

        let classified_sites: Vec<ClassifiedCallSite> = serde_json::from_str(&response)
            .map_err(|e| format!("Failed to parse LLM classification response: {}", e))?;

        Ok(classified_sites)
    }

    fn build_system_message(&self) -> String {
        r#"You are an expert at analyzing JavaScript/TypeScript call sites to classify them based on detected frameworks and data-fetching libraries.

Your task is to classify each call site as one of:
- RouterMount: Mounting or registering a router/sub-application with a routing framework
- Middleware: Registering middleware or interceptors with a framework
- HttpEndpoint: Defining HTTP endpoints/routes that handle requests
- DataFetchingCall: Making outgoing HTTP/API calls to external services
- GraphQLCall: GraphQL queries, mutations, or subscriptions
- Irrelevant: Unrelated to HTTP routing or API calls

CRITICAL REQUIREMENTS:
1. Use the provided framework and data-fetcher context to understand the specific APIs and patterns
2. Consider callee_object, callee_property, args, and definition together
3. Return ONLY valid JSON array starting with [ and ending with ]
4. Each object must have: call_site (original data), classification, confidence (0.0-1.0), reasoning

ANALYSIS APPROACH:
- Look at the detected frameworks to understand which routing/server libraries are in use
- Look at the detected data-fetchers to understand which HTTP client libraries are in use
- Use your knowledge of each framework's API patterns to classify call sites
- Consider the argument types: string paths, function handlers, objects, etc.
- Use the definition field to understand how objects were created/assigned

NO EXPLANATIONS - ONLY JSON ARRAY."#.to_string()
    }

    fn build_classification_prompt(&self, call_sites: &[CallSite], framework_detection: &DetectionResult) -> String {
        let call_sites_json = serde_json::to_string_pretty(call_sites).unwrap_or_default();
        let frameworks_json = serde_json::to_string(framework_detection).unwrap_or_default();

        format!(
            r#"Classify these JavaScript/TypeScript call sites based on the detected frameworks and data-fetching libraries.

FRAMEWORK CONTEXT:
{}

CALL SITES TO CLASSIFY:
{}

For each call site, analyze:
1. callee_object: The object being called on
2. callee_property: The method being called  
3. args: The arguments with their types (StringLiteral, Identifier, FunctionExpression, etc.)
4. definition: How the callee_object was created/assigned
5. location: File and line number

Use your knowledge of the detected frameworks and data-fetchers to classify each call site.

Return JSON array with this structure:
[
  {{
    "call_site": {{ /* original call site data */ }},
    "classification": "RouterMount|Middleware|HttpEndpoint|DataFetchingCall|GraphQLCall|Irrelevant",
    "confidence": 0.95,
    "reasoning": "Brief explanation based on framework knowledge"
  }},
  ...
]"#,
            frameworks_json, call_sites_json
        )
    }
}