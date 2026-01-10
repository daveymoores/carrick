//! File-centric analyzer agent for high-speed static analysis using Gemini 3.0 Flash.
//!
//! NOTE: This module is part of a refactoring effort. The public API will be integrated
//! with the orchestrator in subsequent commits.
#![allow(dead_code)]
//!
//! This agent implements a "one-shot" analysis approach where an entire file is sent
//! to the LLM along with framework-specific patterns. The LLM returns structured
//! findings that can be directly deserialized into graph structures.
//!
//! Key features:
//! - Framework agnostic: All detection logic derives from injected patterns
//! - Alias resolution: Import sources are tracked for cross-file linking
//! - Flat schema: Avoids recursion errors and ensures deterministic parsing

use crate::{
    agent_service::AgentService,
    agents::{framework_guidance_agent::FrameworkGuidance, schemas::AgentSchemas},
};
use serde::{Deserialize, Serialize};

/// Result of analyzing a single mount relationship
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MountResult {
    pub line_number: i32,
    pub parent_node: String,
    pub child_node: String,
    pub mount_path: String,
    pub import_source: Option<String>,
    pub pattern_matched: String,
}

/// Result of analyzing a single endpoint definition
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EndpointResult {
    pub line_number: i32,
    pub owner_node: String,
    pub method: String,
    pub path: String,
    pub handler_name: String,
    pub pattern_matched: String,
}

/// Result of analyzing a single data-fetching call
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DataCallResult {
    pub line_number: i32,
    pub target: String,
    pub method: Option<String>,
    pub pattern_matched: String,
}

/// Complete analysis result for a single file
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct FileAnalysisResult {
    pub mounts: Vec<MountResult>,
    pub endpoints: Vec<EndpointResult>,
    pub data_calls: Vec<DataCallResult>,
}

/// Agent that performs file-centric analysis using framework-agnostic patterns.
///
/// This agent sends the full content of a file to the LLM along with patterns
/// derived from framework guidance. The LLM acts purely as a pattern matcher
/// and alias resolver.
pub struct FileAnalyzerAgent {
    agent_service: AgentService,
}

impl FileAnalyzerAgent {
    pub fn new(agent_service: AgentService) -> Self {
        Self { agent_service }
    }

    /// Analyze a single file with the given framework patterns.
    ///
    /// # Arguments
    /// * `file_path` - Path to the file being analyzed (for context)
    /// * `file_content` - Full content of the file
    /// * `guidance` - Framework-specific patterns to use for matching
    ///
    /// # Returns
    /// A `FileAnalysisResult` containing all detected mounts, endpoints, and data calls.
    pub async fn analyze_file(
        &self,
        file_path: &str,
        file_content: &str,
        guidance: &FrameworkGuidance,
    ) -> Result<FileAnalysisResult, Box<dyn std::error::Error>> {
        // Skip empty files
        if file_content.trim().is_empty() {
            return Ok(FileAnalysisResult::default());
        }

        let system_message = self.build_system_message();
        let user_message = self.build_user_message(file_path, file_content, guidance);

        println!("=== FILE ANALYZER AGENT ===");
        println!("Analyzing file: {}", file_path);
        println!(
            "File size: {} chars, {} lines",
            file_content.len(),
            file_content.lines().count()
        );

        let schema = AgentSchemas::file_analysis_schema();
        let response = self
            .agent_service
            .analyze_code_with_schema(&user_message, &system_message, Some(schema))
            .await?;

        println!("=== RAW FILE ANALYSIS RESPONSE ===");
        println!("{}", response);
        println!("=== END RAW RESPONSE ===");

        let result: FileAnalysisResult = serde_json::from_str(&response).map_err(|e| {
            format!(
                "Failed to parse file analysis response: {}. Raw response: {}",
                e, response
            )
        })?;

        println!(
            "File analysis complete: {} mounts, {} endpoints, {} data_calls",
            result.mounts.len(),
            result.endpoints.len(),
            result.data_calls.len()
        );

        Ok(result)
    }

    /// Build the system message for the Carrick Static Analysis Engine.
    /// This prompt is strictly framework-agnostic.
    fn build_system_message(&self) -> String {
        r#"You are the **Carrick Static Analysis Engine**.
Your mission is to analyze a single source code file to extract structural API relationships.
You function purely as a **Pattern Matcher** and **Alias Resolver**. You do NOT possess inherent knowledge of specific frameworks; you must rely strictly on the **ACTIVE PATTERNS** provided in the input.

### CORE OBJECTIVE
Scan the provided **FILE CONTENT** and extract items matching the **ACTIVE PATTERNS**. Return a structured JSON object containing flat lists of findings.

### 1. ANALYSIS RULES

#### A. Strict Pattern Matching
* **Endpoints:** If code matches an `endpoint_pattern` from the input, extract it.
* **Mounts:** If code matches a `mount_pattern` from the input, extract it.
* **Data Calls:** If code matches a `data_fetching_pattern` from the input, extract it.
* **No Hallucinations:** Do not infer endpoints from comments or vague code. If it doesn't match a pattern, ignore it.

#### B. Variable & Alias Resolution (CRITICAL)
Your extraction must be useful for a graph builder. You must resolve variable names to their definitions:
* **Imports:** If a generic router is mounted (e.g., `parent.mount('/', child)`), and `child` is imported from `'./auth'`, you MUST record `'./auth'` as the `import_source`. This is the ONLY way we link files.
* **Inline:** If a variable is defined in this file (e.g., `const api = createRouter()`), track that it is local (import_source = null).
* **Chaining:** If a pattern is chained (e.g., `createApp().plugin(...)`), the `parent_node` is the root object (e.g., `createApp()`).

### 2. OUTPUT REQUIREMENTS (Flat Schema)
* Do not nest details inside generic objects.
* Every finding must be a top-level item in its respective list.
* Strings should be exact literals from the code.
* Line numbers are 1-based.
* For HTTP methods, use uppercase: GET, POST, PUT, DELETE, PATCH, HEAD, OPTIONS, ALL.

### 3. IMPORT TRACKING
When you see an import statement like:
* `import userRouter from './routes/users'` - record import_source as './routes/users'
* `const auth = require('./auth')` - record import_source as './auth'
* `import { apiRoutes } from '../api'` - record import_source as '../api'

When a variable is used in a mount and that variable was imported, include the import source.

### 4. SPECIAL CASES
* **Default exports:** If the file exports a router/app as default and it's mounted elsewhere, the child_node should be the imported name.
* **Re-exports:** Track the original source, not intermediate re-exports.
* **Dynamic imports:** Record import_source as the string literal if available, otherwise null.
* **response.json()/.text():** These are data_calls when they appear after fetch/axios calls to parse response data."#.to_string()
    }

    /// Build the dynamic user message with patterns and file content.
    fn build_user_message(
        &self,
        file_path: &str,
        file_content: &str,
        guidance: &FrameworkGuidance,
    ) -> String {
        let mount_patterns = self.format_patterns(&guidance.mount_patterns);
        let endpoint_patterns = self.format_patterns(&guidance.endpoint_patterns);
        let data_patterns = self.format_patterns(&guidance.data_fetching_patterns);

        format!(
            r#"### ACTIVE PATTERNS (Derived from Framework Guidance)
{{
  "mount_patterns": [
    {}
  ],
  "endpoint_patterns": [
    {}
  ],
  "data_fetching_patterns": [
    {}
  ]
}}

### FRAMEWORK-SPECIFIC HINTS
{}

### FILE CONTENT (Path: {})
```
{}
```

Analyze this file and return a JSON object with:
- "mounts": array of mount relationships found
- "endpoints": array of HTTP endpoint definitions found
- "data_calls": array of data fetching calls found

For each mount, include: line_number, parent_node, child_node, mount_path, import_source (null if local), pattern_matched
For each endpoint, include: line_number, owner_node, method, path, handler_name, pattern_matched
For each data_call, include: line_number, target, method (null if unknown), pattern_matched

Return ONLY the JSON object, no explanations."#,
            mount_patterns,
            endpoint_patterns,
            data_patterns,
            guidance.triage_hints,
            file_path,
            file_content
        )
    }

    /// Format pattern examples as JSON array items.
    fn format_patterns(
        &self,
        patterns: &[crate::agents::framework_guidance_agent::PatternExample],
    ) -> String {
        if patterns.is_empty() {
            return "// No patterns provided".to_string();
        }

        patterns
            .iter()
            .map(|p| {
                format!(
                    r#"    // {} ({}): {}
    "{}""#,
                    p.framework, p.description, p.pattern, p.pattern
                )
            })
            .collect::<Vec<_>>()
            .join(",\n")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agents::framework_guidance_agent::PatternExample;

    fn create_test_guidance() -> FrameworkGuidance {
        FrameworkGuidance {
            mount_patterns: vec![
                PatternExample {
                    pattern: ".use(".to_string(),
                    description: "Mount middleware or router".to_string(),
                    framework: "express".to_string(),
                },
                PatternExample {
                    pattern: ".register(".to_string(),
                    description: "Register plugin or router".to_string(),
                    framework: "fastify".to_string(),
                },
            ],
            endpoint_patterns: vec![
                PatternExample {
                    pattern: ".get(".to_string(),
                    description: "GET endpoint".to_string(),
                    framework: "express".to_string(),
                },
                PatternExample {
                    pattern: ".post(".to_string(),
                    description: "POST endpoint".to_string(),
                    framework: "express".to_string(),
                },
            ],
            middleware_patterns: vec![],
            data_fetching_patterns: vec![
                PatternExample {
                    pattern: "fetch(".to_string(),
                    description: "Fetch API call".to_string(),
                    framework: "native".to_string(),
                },
                PatternExample {
                    pattern: "axios.".to_string(),
                    description: "Axios HTTP call".to_string(),
                    framework: "axios".to_string(),
                },
            ],
            triage_hints: "Look for router.use() for mounts, router.get/post/etc for endpoints"
                .to_string(),
            parsing_notes: "Express uses chained methods".to_string(),
        }
    }

    #[test]
    fn test_file_analysis_result_default() {
        let result = FileAnalysisResult::default();
        assert!(result.mounts.is_empty());
        assert!(result.endpoints.is_empty());
        assert!(result.data_calls.is_empty());
    }

    #[test]
    fn test_mount_result_serialization() {
        let mount = MountResult {
            line_number: 10,
            parent_node: "app".to_string(),
            child_node: "userRouter".to_string(),
            mount_path: "/users".to_string(),
            import_source: Some("./routes/users".to_string()),
            pattern_matched: ".use(".to_string(),
        };

        let json = serde_json::to_string(&mount).unwrap();
        assert!(json.contains("parent_node"));
        assert!(json.contains("import_source"));
        assert!(json.contains("./routes/users"));

        let deserialized: MountResult = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.parent_node, "app");
        assert_eq!(
            deserialized.import_source,
            Some("./routes/users".to_string())
        );
    }

    #[test]
    fn test_endpoint_result_serialization() {
        let endpoint = EndpointResult {
            line_number: 15,
            owner_node: "router".to_string(),
            method: "GET".to_string(),
            path: "/users/:id".to_string(),
            handler_name: "getUserById".to_string(),
            pattern_matched: ".get(".to_string(),
        };

        let json = serde_json::to_string(&endpoint).unwrap();
        assert!(json.contains("owner_node"));
        assert!(json.contains("GET"));

        let deserialized: EndpointResult = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.method, "GET");
        assert_eq!(deserialized.handler_name, "getUserById");
    }

    #[test]
    fn test_data_call_result_serialization() {
        let data_call = DataCallResult {
            line_number: 25,
            target: "https://api.example.com/data".to_string(),
            method: Some("POST".to_string()),
            pattern_matched: "fetch(".to_string(),
        };

        let json = serde_json::to_string(&data_call).unwrap();
        assert!(json.contains("target"));
        assert!(json.contains("POST"));

        let deserialized: DataCallResult = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.target, "https://api.example.com/data");
        assert_eq!(deserialized.method, Some("POST".to_string()));
    }

    #[test]
    fn test_file_analysis_result_serialization() {
        let result = FileAnalysisResult {
            mounts: vec![MountResult {
                line_number: 5,
                parent_node: "app".to_string(),
                child_node: "apiRouter".to_string(),
                mount_path: "/api".to_string(),
                import_source: Some("./api".to_string()),
                pattern_matched: ".use(".to_string(),
            }],
            endpoints: vec![EndpointResult {
                line_number: 10,
                owner_node: "router".to_string(),
                method: "GET".to_string(),
                path: "/health".to_string(),
                handler_name: "healthCheck".to_string(),
                pattern_matched: ".get(".to_string(),
            }],
            data_calls: vec![],
        };

        let json = serde_json::to_string(&result).unwrap();
        assert!(json.contains("mounts"));
        assert!(json.contains("endpoints"));
        assert!(json.contains("data_calls"));

        let deserialized: FileAnalysisResult = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.mounts.len(), 1);
        assert_eq!(deserialized.endpoints.len(), 1);
        assert!(deserialized.data_calls.is_empty());
    }

    #[test]
    fn test_format_patterns_empty() {
        let agent = FileAnalyzerAgent::new(AgentService::new("mock".to_string()));
        let result = agent.format_patterns(&[]);
        assert_eq!(result, "// No patterns provided");
    }

    #[test]
    fn test_format_patterns_with_items() {
        let agent = FileAnalyzerAgent::new(AgentService::new("mock".to_string()));
        let patterns = vec![
            PatternExample {
                pattern: ".get(".to_string(),
                description: "GET endpoint".to_string(),
                framework: "express".to_string(),
            },
            PatternExample {
                pattern: ".post(".to_string(),
                description: "POST endpoint".to_string(),
                framework: "express".to_string(),
            },
        ];

        let result = agent.format_patterns(&patterns);
        assert!(result.contains(".get("));
        assert!(result.contains(".post("));
        assert!(result.contains("express"));
        assert!(result.contains("GET endpoint"));
    }

    #[test]
    fn test_build_user_message() {
        let agent = FileAnalyzerAgent::new(AgentService::new("mock".to_string()));
        let guidance = create_test_guidance();
        let file_content = r#"
import express from 'express';
const app = express();
app.get('/health', (req, res) => res.json({ status: 'ok' }));
"#;

        let message = agent.build_user_message("test.ts", file_content, &guidance);

        assert!(message.contains("ACTIVE PATTERNS"));
        assert!(message.contains("mount_patterns"));
        assert!(message.contains("endpoint_patterns"));
        assert!(message.contains("data_fetching_patterns"));
        assert!(message.contains("test.ts"));
        assert!(message.contains("express"));
    }

    #[test]
    fn test_system_message_is_framework_agnostic() {
        let agent = FileAnalyzerAgent::new(AgentService::new("mock".to_string()));
        let system_message = agent.build_system_message();

        // Should NOT contain hardcoded framework names in the system message
        // The system message should be generic and rely on patterns
        assert!(system_message.contains("Pattern Matcher"));
        assert!(system_message.contains("Alias Resolver"));
        assert!(system_message.contains("ACTIVE PATTERNS"));
        assert!(system_message.contains("import_source"));

        // Verify it emphasizes pattern-based matching
        assert!(system_message.contains("Strict Pattern Matching"));
        assert!(system_message.contains("No Hallucinations"));
    }
}
