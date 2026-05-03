use crate::{
    agent_service::AgentService, agents::schemas::AgentSchemas, framework_detector::DetectionResult,
};
use serde::{Deserialize, Serialize};
use tracing::debug;

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

/// Flattened response format using parallel arrays (faster for structured output)
#[derive(Debug, Clone, Serialize, Deserialize)]
struct FlatPatternResponse {
    patterns: Vec<String>,
    descriptions: Vec<String>,
    frameworks: Vec<String>,
}

impl FlatPatternResponse {
    /// Convert parallel arrays back to Vec<PatternExample>
    fn into_pattern_examples(self) -> Vec<PatternExample> {
        self.patterns
            .into_iter()
            .zip(self.descriptions)
            .zip(self.frameworks)
            .map(|((pattern, description), framework)| PatternExample {
                pattern,
                description,
                framework,
            })
            .collect()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct GeneralGuidanceResponse {
    triage_hints: String,
    parsing_notes: String,
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
    /// Uses parallel calls to the /framework-guidance lambda for each category.
    /// All prompt construction lives lambda-side now (see
    /// carrick-cloud/lambdas/framework-guidance/prompts.js).
    pub async fn generate_guidance(
        &self,
        framework_detection: &DetectionResult,
    ) -> Result<FrameworkGuidance, Box<dyn std::error::Error>> {
        debug!("=== FRAMEWORK GUIDANCE AGENT DEBUG ===");
        debug!(
            "Generating guidance for frameworks: {:?}",
            framework_detection.frameworks
        );
        debug!("Data fetchers: {:?}", framework_detection.data_fetchers);

        // Execute calls in parallel for speed (flattened schema makes this fast enough)
        debug!("  Fetching all patterns in parallel...");
        let mount_task = self.fetch_patterns("mount", framework_detection);
        let endpoint_task = self.fetch_patterns("endpoint", framework_detection);
        let middleware_task = self.fetch_patterns("middleware", framework_detection);
        let fetching_task = self.fetch_patterns("data_fetching", framework_detection);
        let general_task = self.fetch_general_guidance(framework_detection);

        // Wait for all tasks to complete
        let (
            mount_patterns,
            endpoint_patterns,
            middleware_patterns,
            data_fetching_patterns,
            general_guidance,
        ) = tokio::try_join!(
            mount_task,
            endpoint_task,
            middleware_task,
            fetching_task,
            general_task
        )?;

        let guidance = FrameworkGuidance {
            mount_patterns,
            endpoint_patterns,
            middleware_patterns,
            data_fetching_patterns,
            triage_hints: general_guidance.triage_hints,
            parsing_notes: general_guidance.parsing_notes,
        };

        debug!("Generated guidance with:");
        debug!("  - {} mount patterns", guidance.mount_patterns.len());
        debug!("  - {} endpoint patterns", guidance.endpoint_patterns.len());
        debug!(
            "  - {} middleware patterns",
            guidance.middleware_patterns.len()
        );
        debug!(
            "  - {} data fetching patterns",
            guidance.data_fetching_patterns.len()
        );

        Ok(guidance)
    }

    async fn fetch_patterns(
        &self,
        category: &str,
        framework_detection: &DetectionResult,
    ) -> Result<Vec<PatternExample>, Box<dyn std::error::Error>> {
        let schema = AgentSchemas::pattern_list_schema();
        let body = serde_json::json!({
            "task": "patterns",
            "category": category,
            "frameworks": framework_detection.frameworks,
            "data_fetchers": framework_detection.data_fetchers,
            "response_schema": schema,
        });

        let response = self
            .agent_service
            .post_to_lambda("/framework-guidance", &body, category)
            .await?;

        let parsed: FlatPatternResponse = serde_json::from_str(&response).map_err(|e| {
            format!(
                "Failed to parse {} patterns: {}. Raw response: {}",
                category, e, response
            )
        })?;

        Ok(parsed.into_pattern_examples())
    }

    async fn fetch_general_guidance(
        &self,
        framework_detection: &DetectionResult,
    ) -> Result<GeneralGuidanceResponse, Box<dyn std::error::Error>> {
        let schema = AgentSchemas::general_guidance_schema();
        let body = serde_json::json!({
            "task": "general",
            "frameworks": framework_detection.frameworks,
            "data_fetchers": framework_detection.data_fetchers,
            "response_schema": schema,
        });

        let response = self
            .agent_service
            .post_to_lambda("/framework-guidance", &body, "general")
            .await?;

        let parsed: GeneralGuidanceResponse = serde_json::from_str(&response).map_err(|e| {
            format!(
                "Failed to parse general guidance: {}. Raw response: {}",
                e, response
            )
        })?;

        Ok(parsed)
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

    // build_context_string tests removed: context-string formatting now
    // lives lambda-side (carrick-cloud/lambdas/framework-guidance/prompts.js
    // → buildContextString). Rust only forwards the frameworks +
    // data_fetchers arrays.
}
