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
    /// Uses parallel calls to the LLM for each category for improved speed.
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

        let system_message = self.build_system_message();
        let prompt_context = self.build_context_string(framework_detection);

        // Execute calls in parallel for speed (flattened schema makes this fast enough)
        debug!("  Fetching all patterns in parallel...");
        let mount_task = self.fetch_patterns("mount", &prompt_context, &system_message);
        let endpoint_task = self.fetch_patterns("endpoint", &prompt_context, &system_message);
        let middleware_task = self.fetch_patterns("middleware", &prompt_context, &system_message);
        let fetching_task = self.fetch_patterns("data_fetching", &prompt_context, &system_message);
        let general_task = self.fetch_general_guidance(&prompt_context, &system_message);

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
        context: &str,
        system_message: &str,
    ) -> Result<Vec<PatternExample>, Box<dyn std::error::Error>> {
        let category_prompt = match category {
            "mount" => {
                "MOUNT PATTERNS: How does this framework mount sub-routers, sub-applications, or plugins?

Cover every realistic mount shape for each detected framework, including:
  (a) Top-level path-string mounts (e.g., `parent.use('/api', child)`).
  (b) Option-object mounts where the path prefix is a top-level key (e.g., `register(child, { prefix: '/api' })`).
  (c) Option-object mounts where the path prefix is NESTED inside the option object under a non-top-level key (e.g., a `routes: { prefix: '/api' }` shape, or any other wrapper the framework's register/mount function expects). Do NOT skip these — they are the most commonly missed shape.
  (d) Constructor-carried prefixes, where the child itself is built with a prefix and the mount site adds none (e.g., constructing a sub-router with a prefix option, then mounting it with no additional path).

For EACH pattern you return, the `description` field MUST tell a downstream analyzer exactly how to read the prefix from the call's arguments. Use this shape:
  - If the prefix is a top-level string argument: `\"Prefix is the 1st string argument.\"`
  - If the prefix is a top-level key in an options object: `\"Prefix is at options.prefix (2nd argument).\"`
  - If the prefix is nested inside an options object: name the exact dotted path, e.g. `\"Prefix is at options.routes.prefix (2nd argument).\"`
  - If the prefix is constructor-carried and the mount site contributes no prefix: `\"Prefix is carried by the child constructor; mount site contributes no prefix.\"`

The downstream file analyzer reads this description verbatim to locate the prefix in the AST. Vague descriptions like \"mounts a plugin with options\" will cause prefix extraction to fail."
            }
            "endpoint" => {
                "ENDPOINT PATTERNS: How are HTTP endpoints defined? Provide specific syntax for routes/handlers."
            }
            "middleware" => {
                "MIDDLEWARE PATTERNS: How is middleware registered? Provide specific syntax."
            }
            "data_fetching" => {
                "DATA FETCHING PATTERNS: How are outbound HTTP calls made? Provide specific syntax."
            }
            _ => "Provide relevant code patterns.",
        };

        let example_count = if category == "mount" { "3-5" } else { "2-3" };

        let prompt = format!(
            "{}\n\nTASK:\nProvide {} concise examples for: {}\n\nReturn JSON with three parallel arrays: patterns (code), descriptions (what each does — for mounts, see the required description format above), frameworks (which framework).",
            context, example_count, category_prompt
        );

        let schema = AgentSchemas::pattern_list_schema();
        let response = self
            .agent_service
            .analyze_code_with_schema(&prompt, system_message, Some(schema))
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
        context: &str,
        system_message: &str,
    ) -> Result<GeneralGuidanceResponse, Box<dyn std::error::Error>> {
        let prompt = format!(
            "{}\n\nTASK:\nProvide general guidance for a downstream static-analysis agent that reads a single source file and extracts endpoints, mounts, and data calls. The agent's correctness depends on your notes.\n\nReturn two fields:\n\n1. TRIAGE HINTS: How to distinguish mounts vs middleware vs endpoints for the detected framework(s).\n\n2. PARSING NOTES: Enumerate the framework-specific AST/parsing semantics the agent needs to extract endpoints correctly. For EACH detected framework, cover at minimum (omit a bullet if it genuinely does not apply):\n\n   (a) Owner attribution inside plugin/sub-app closures. If the framework registers routes inside a callback whose parameter shadows an outer identifier (e.g., `register: async (server) => {{ server.route(...) }}` in Hapi, `app.register(async (instance) => {{ instance.get(...) }})` in Fastify, or similar), state explicitly: the scoped parameter is a child instance; routes defined on it MUST be attributed to the plugin/child variable visible to the registration call, NOT to the parameter name. Name the typical parameter names the framework uses so the downstream agent can recognise the shape.\n\n   (b) Path-prefix location (ties to the MOUNT PATTERNS your mount call returns). State where the prefix lives for each registration idiom: constructor-carried (baked into the child at construction time) vs mount-site (provided in the registration call itself, including nested-options shapes). If constructor-carried, the endpoint's emitted path must include the prefix and the mount's prefix must be empty; if mount-site, vice versa. This is the highest-leverage correctness note — do not skip it.\n\n   (c) Decorator routing, if applicable (class-method decorators, param decorators). Name the routing decorators, the method-name → HTTP-verb mapping, and how class-level decorators contribute a path prefix.\n\n   (d) Method chaining quirks (e.g., `router.route('/x').get(...).post(...)`). State whether the chain's receiver anchors the owner.\n\n   (e) Any other shape the agent cannot infer from pattern examples alone — e.g., how handlers return responses in ways that affect which expression is the payload.\n\nBe concrete. Use the framework's real identifier names. Multi-framework repos: include a short section per framework. Vague notes like \"uses decorators\" will cause silent extraction bugs; notes like \"In NestJS, `@Controller('prefix')` on the class contributes a path prefix concatenated with the method decorator's argument\" are what the agent needs.\n\nReturn ONLY the JSON object.",
            context
        );

        let schema = AgentSchemas::general_guidance_schema();
        let response = self
            .agent_service
            .analyze_code_with_schema(&prompt, system_message, Some(schema))
            .await?;

        let parsed: GeneralGuidanceResponse = serde_json::from_str(&response).map_err(|e| {
            format!(
                "Failed to parse general guidance: {}. Raw response: {}",
                e, response
            )
        })?;

        Ok(parsed)
    }

    fn build_system_message(&self) -> String {
        "You are an expert at web frameworks and HTTP client libraries. Provide concise, realistic code pattern examples.".to_string()
    }

    fn build_context_string(&self, framework_detection: &DetectionResult) -> String {
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
            "DETECTED FRAMEWORKS: {}\nDETECTED DATA FETCHERS: {}",
            frameworks_list, data_fetchers_list
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
    fn test_build_context_with_frameworks() {
        let agent_service = crate::agent_service::AgentService::new("mock".to_string());
        let agent = FrameworkGuidanceAgent::new(agent_service);

        let detection = DetectionResult {
            frameworks: vec!["someframework".to_string()],
            data_fetchers: vec!["someclient".to_string()],
            notes: "test".to_string(),
        };

        let context = agent.build_context_string(&detection);

        assert!(context.contains("someframework"));
        assert!(context.contains("someclient"));
    }

    #[test]
    fn test_build_context_with_no_frameworks() {
        let agent_service = crate::agent_service::AgentService::new("mock".to_string());
        let agent = FrameworkGuidanceAgent::new(agent_service);

        let detection = DetectionResult {
            frameworks: vec![],
            data_fetchers: vec![],
            notes: "test".to_string(),
        };

        let context = agent.build_context_string(&detection);

        assert!(context.contains("None detected"));
    }
}
