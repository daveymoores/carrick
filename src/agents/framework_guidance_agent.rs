use crate::{
    agent_service::AgentService, agents::schemas::AgentSchemas, framework_detector::DetectionResult,
};
use serde::{Deserialize, Serialize};

/// A single pattern example for a specific framework
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PatternExample {
    /// The code pattern, e.g., "app.route('/path', subApp)"
    pub pattern: String,

    /// What this pattern represents
    pub description: String,

    /// Which framework this is for
    pub framework: String,
}

/// Framework-specific guidance for downstream agents
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FrameworkGuidance {
    /// Patterns for router/sub-app mounting
    pub mount_patterns: Vec<PatternExample>,

    /// Patterns for HTTP endpoint definitions
    pub endpoint_patterns: Vec<PatternExample>,

    /// Patterns for middleware registration
    pub middleware_patterns: Vec<PatternExample>,

    /// Patterns for outbound HTTP calls
    pub data_fetching_patterns: Vec<PatternExample>,

    /// Free-form hints for the triage agent
    pub triage_hints: String,

    /// Framework-specific notes that may affect parsing
    pub parsing_notes: String,
}

/// Agent that generates framework-specific patterns and guidance
/// for downstream agents to use in their prompts.
///
/// This agent is fully LLM-driven - it asks the LLM to provide
/// patterns for whatever frameworks are detected, without any
/// hardcoded framework knowledge.
pub struct FrameworkGuidanceAgent {
    agent_service: AgentService,
}

impl FrameworkGuidanceAgent {
    pub fn new(agent_service: AgentService) -> Self {
        Self { agent_service }
    }

    /// Generate framework-specific guidance based on detected frameworks.
    /// Always calls the LLM to get guidance - no hardcoded patterns.
    pub async fn generate_guidance(
        &self,
        framework_detection: &DetectionResult,
    ) -> Result<FrameworkGuidance, Box<dyn std::error::Error>> {
        println!("=== FRAMEWORK GUIDANCE AGENT DEBUG ===");
        println!(
            "Generating guidance for frameworks: {:?}",
            framework_detection.frameworks
        );
        println!("Data fetchers: {:?}", framework_detection.data_fetchers);

        let prompt = self.build_guidance_prompt(framework_detection);
        let system_message = self.build_system_message();

        let schema = AgentSchemas::framework_guidance_schema();
        let response = self
            .agent_service
            .analyze_code_with_schema(&prompt, &system_message, Some(schema))
            .await?;

        println!("=== RAW AGENT FRAMEWORK GUIDANCE RESPONSE ===");
        println!("{}", response);
        println!("=== END RAW RESPONSE ===");

        let guidance: FrameworkGuidance = serde_json::from_str(&response).map_err(|e| {
            format!(
                "Failed to parse framework guidance response: {}. Raw response: {}",
                e, response
            )
        })?;

        println!("Generated guidance with:");
        println!("  - {} mount patterns", guidance.mount_patterns.len());
        println!("  - {} endpoint patterns", guidance.endpoint_patterns.len());
        println!(
            "  - {} middleware patterns",
            guidance.middleware_patterns.len()
        );
        println!(
            "  - {} data fetching patterns",
            guidance.data_fetching_patterns.len()
        );

        Ok(guidance)
    }

    fn build_system_message(&self) -> String {
        r#"You are an expert in JavaScript/TypeScript web frameworks. Your task is to provide
framework-specific patterns and guidance that will help other agents correctly
identify and parse code from these frameworks.

You will be given a list of detected frameworks and data-fetching libraries.
For each one, provide concrete code patterns with explanations.

Return ONLY valid JSON matching the required schema."#
            .to_string()
    }

    fn build_guidance_prompt(&self, framework_detection: &DetectionResult) -> String {
        let frameworks_list = if framework_detection.frameworks.is_empty() {
            "None detected".to_string()
        } else {
            framework_detection.frameworks.join(", ")
        };

        let data_fetchers_list = if framework_detection.data_fetchers.is_empty() {
            "None detected".to_string()
        } else {
            framework_detection.data_fetchers.join(", ")
        };

        format!(
            r#"Given the following detected frameworks and libraries, provide patterns and guidance
for code analysis.

DETECTED FRAMEWORKS: {frameworks_list}
DETECTED DATA FETCHERS: {data_fetchers_list}

For each framework/library detected, provide:

1. MOUNT PATTERNS: How does this framework mount sub-routers or sub-applications?
   Provide the specific syntax this framework uses.

2. ENDPOINT PATTERNS: How are HTTP endpoints defined?
   Provide the specific syntax for defining routes/handlers.

3. MIDDLEWARE PATTERNS: How is middleware registered?
   Provide the specific syntax for adding middleware.

4. DATA FETCHING PATTERNS: How are outbound HTTP calls made?
   Provide the specific syntax for making HTTP requests.

5. TRIAGE HINTS: Any special notes for distinguishing between categories?
   What makes a mount different from middleware in this framework?
   What distinguishes endpoints from other function calls?

6. PARSING NOTES: Any AST/parsing considerations?
   Does this framework use decorators, method chaining, config objects, etc.?

Return JSON with this structure:
{{
  "mount_patterns": [
    {{ "pattern": "<code example>", "description": "<what it does>", "framework": "<framework name>" }}
  ],
  "endpoint_patterns": [...],
  "middleware_patterns": [...],
  "data_fetching_patterns": [...],
  "triage_hints": "Free-form guidance for distinguishing between categories...",
  "parsing_notes": "Notes about AST structure or special syntax..."
}}

Include at least 2-3 patterns for each category based on the detected frameworks.
Be specific to the actual frameworks detected - provide their real syntax and idioms."#
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_pattern_example_serialization() {
        let pattern = PatternExample {
            pattern: "app.get('/test', handler)".to_string(),
            description: "Test endpoint".to_string(),
            framework: "someframework".to_string(),
        };

        let json = serde_json::to_string(&pattern).unwrap();
        assert!(json.contains("pattern"));
        assert!(json.contains("description"));
        assert!(json.contains("framework"));

        let deserialized: PatternExample = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.pattern, pattern.pattern);
        assert_eq!(deserialized.description, pattern.description);
        assert_eq!(deserialized.framework, pattern.framework);
    }

    #[test]
    fn test_framework_guidance_serialization() {
        let guidance = FrameworkGuidance {
            mount_patterns: vec![PatternExample {
                pattern: "test".to_string(),
                description: "test".to_string(),
                framework: "test".to_string(),
            }],
            endpoint_patterns: vec![],
            middleware_patterns: vec![],
            data_fetching_patterns: vec![],
            triage_hints: "some hints".to_string(),
            parsing_notes: "some notes".to_string(),
        };

        let json = serde_json::to_string(&guidance).unwrap();
        assert!(json.contains("mount_patterns"));
        assert!(json.contains("endpoint_patterns"));
        assert!(json.contains("triage_hints"));

        let deserialized: FrameworkGuidance = serde_json::from_str(&json).unwrap();
        assert_eq!(
            deserialized.mount_patterns.len(),
            guidance.mount_patterns.len()
        );
    }

    #[test]
    fn test_build_prompt_with_frameworks() {
        let agent_service = crate::agent_service::AgentService::new("mock".to_string());
        let agent = FrameworkGuidanceAgent::new(agent_service);

        let detection = DetectionResult {
            frameworks: vec!["someframework".to_string()],
            data_fetchers: vec!["someclient".to_string()],
            notes: "test".to_string(),
        };

        let prompt = agent.build_guidance_prompt(&detection);

        assert!(prompt.contains("someframework"));
        assert!(prompt.contains("someclient"));
    }

    #[test]
    fn test_build_prompt_with_no_frameworks() {
        let agent_service = crate::agent_service::AgentService::new("mock".to_string());
        let agent = FrameworkGuidanceAgent::new(agent_service);

        let detection = DetectionResult {
            frameworks: vec![],
            data_fetchers: vec![],
            notes: "test".to_string(),
        };

        let prompt = agent.build_guidance_prompt(&detection);

        assert!(prompt.contains("None detected"));
    }
}
