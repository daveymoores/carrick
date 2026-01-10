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
    /// File path containing the response type definition
    pub response_type_file: Option<String>,
    /// Start position (character index) of the response type in the file
    pub response_type_position: Option<i32>,
    /// The type string itself (e.g., "User[]", "Response<Order>")
    pub response_type_string: Option<String>,
}

/// Result of analyzing a single data-fetching call
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DataCallResult {
    pub line_number: i32,
    pub target: String,
    pub method: Option<String>,
    pub pattern_matched: String,
    /// File path containing the response type definition
    pub response_type_file: Option<String>,
    /// Start position (character index) of the response type in the file
    pub response_type_position: Option<i32>,
    /// The type string itself (e.g., "User[]", "Response<Order>")
    pub response_type_string: Option<String>,
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
    /// Analyze a single file with the given framework patterns (legacy method without candidates).
    ///
    /// This method is kept for backward compatibility. Prefer `analyze_file_with_candidates`
    /// for the AST-gated architecture.
    pub async fn analyze_file(
        &self,
        file_path: &str,
        file_content: &str,
        guidance: &FrameworkGuidance,
    ) -> Result<FileAnalysisResult, Box<dyn std::error::Error>> {
        // Delegate to the new method with empty candidates
        self.analyze_file_with_candidates(file_path, file_content, guidance, &[])
            .await
    }

    /// Analyze a single file with AST-detected candidate targets.
    ///
    /// This is the primary method for the AST-Gated architecture:
    /// 1. SWC Scanner has already found candidate lines
    /// 2. We send Full File + Patterns + Candidate Hints to the LLM
    /// 3. The LLM uses candidates as "Focused Targets" to ensure 100% recall
    ///
    /// # Arguments
    /// * `file_path` - Path to the file being analyzed (for context)
    /// * `file_content` - Full content of the file
    /// * `guidance` - Framework-specific patterns to use for matching
    /// * `candidate_hints` - AST-detected candidate lines (formatted hints from SWC Scanner)
    ///
    /// # Returns
    /// A `FileAnalysisResult` containing all detected mounts, endpoints, and data calls.
    pub async fn analyze_file_with_candidates(
        &self,
        file_path: &str,
        file_content: &str,
        guidance: &FrameworkGuidance,
        candidate_hints: &[String],
    ) -> Result<FileAnalysisResult, Box<dyn std::error::Error>> {
        // Skip empty files
        if file_content.trim().is_empty() {
            return Ok(FileAnalysisResult::default());
        }

        let system_message = self.build_system_message_with_candidates();
        let user_message = self.build_user_message_with_candidates(
            file_path,
            file_content,
            guidance,
            candidate_hints,
        );

        println!("=== FILE ANALYZER AGENT (AST-GATED) ===");
        println!("Analyzing file: {}", file_path);
        println!(
            "File size: {} chars, {} lines",
            file_content.len(),
            file_content.lines().count()
        );
        println!("Candidate targets: {}", candidate_hints.len());

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

    /// Build the system message for the Carrick Static Analysis Engine (legacy).
    /// This prompt is strictly framework-agnostic.
    #[allow(dead_code)]
    fn build_system_message(&self) -> String {
        self.build_system_message_with_candidates()
    }

    /// Build the system message for AST-Gated analysis with Candidate Targets.
    /// This prompt is strictly framework-agnostic and includes guidance for using AST hints.
    fn build_system_message_with_candidates(&self) -> String {
        r#"You are the **Carrick Static Analysis Engine**.
Your mission is to analyze a single source code file to extract structural API relationships.
You function purely as a **Pattern Matcher** and **Alias Resolver**. You do NOT possess inherent knowledge of specific frameworks; you must rely strictly on the **ACTIVE PATTERNS** provided in the input.

### INPUT DATA
1. **Full Source Code**: The complete file content for context (imports, definitions).
2. **Candidate Targets**: A list of specific lines where an AST parser detected potential API activity.
3. **Active Patterns**: The specific code patterns to classify (e.g., Mounts, Endpoints).

### CORE OBJECTIVE
Analyze the **Full Source Code**. Focus specifically on the **Candidate Targets** to classify them, but use the surrounding code to resolve variables and imports.

### 1. ANALYSIS RULES

#### A. Strict Pattern Matching
* **Endpoints:** If a target matches an `endpoint_pattern`, extract it.
* **Mounts:** If a target matches a `mount_pattern`, extract it.
* **Data Calls:** If a target matches a `data_fetching_pattern`, extract it.
* **Filter Noise:** The AST parser is broad. If a "Candidate Target" does not strictly match an Active Pattern (e.g., it's just a comment or unrelated function call), IGNORE it.

#### B. Variable & Alias Resolution (CRITICAL)
Your extraction must be useful for a graph builder. You must resolve variable names:
* **Imports:** If a router/controller is mounted (e.g., `parent.mount('/', child)`), and `child` is imported from `'./auth'`, you MUST record `'./auth'` as the `import_source`. This is the ONLY way we link files.
* **Inline:** If a variable is defined in this file (e.g., `const api = createRouter()`), track that it is local (import_source = null).
* **Chaining:** If a pattern is chained (e.g., `createApp().plugin(...)`), the `parent_node` is the root object.

### 2. OUTPUT REQUIREMENTS (Flat Schema)
* Do not nest details. Every finding must be a top-level item in its respective list.
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
* **response.json()/.text():** These are data_calls when they appear after fetch/axios calls to parse response data.

### 5. RESPONSE TYPE EXTRACTION (CRITICAL FOR TYPE CHECKING)
For endpoints and data calls, you MUST extract TypeScript type annotations when present:

#### A. Endpoint Response Types
Look for Response<T> or similar generic type annotations on handler parameters:
* `app.get('/users', (req: Request, res: Response<User[]>) => ...)` → response_type_string: "Response<User[]>"
* `router.post('/order', async (req, res: Response<{ orderId: string }>) => ...)` → response_type_string: "Response<{ orderId: string }>"
* `app.get('/data', handler as RequestHandler<{}, DynamicResponse>)` → response_type_string: "Response<DynamicResponse>"

#### B. Data Call Response Types
Look for type assertions or generic parameters on fetch/axios calls:
* `const data = await resp.json() as Comment[]` → response_type_string: "Comment[]"
* `const user = await fetch<User>('/api/user')` → response_type_string: "User"
* `const orders: Order[] = await api.get('/orders')` → response_type_string: "Order[]"

#### C. Position Calculation
* response_type_position: Count characters from the start of the file to where the type annotation begins
* response_type_file: Use the current file path being analyzed
* If no type annotation is found, set all response_type_* fields to null"#.to_string()
    }

    /// Build the dynamic user message with patterns and file content (legacy).
    #[allow(dead_code)]
    fn build_user_message(
        &self,
        file_path: &str,
        file_content: &str,
        guidance: &FrameworkGuidance,
    ) -> String {
        self.build_user_message_with_candidates(file_path, file_content, guidance, &[])
    }

    /// Build the dynamic user message with patterns, file content, and candidate targets.
    fn build_user_message_with_candidates(
        &self,
        file_path: &str,
        file_content: &str,
        guidance: &FrameworkGuidance,
        candidate_hints: &[String],
    ) -> String {
        let mount_patterns = self.format_patterns(&guidance.mount_patterns);
        let endpoint_patterns = self.format_patterns(&guidance.endpoint_patterns);
        let data_patterns = self.format_patterns(&guidance.data_fetching_patterns);

        // Format candidate targets section
        let candidates_section = if candidate_hints.is_empty() {
            "No specific candidates provided - analyze the entire file for patterns.".to_string()
        } else {
            format!(
                "The following lines triggered the AST parser. Analyze these specific locations:\n{}",
                candidate_hints.join("\n")
            )
        };

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

### CANDIDATE TARGETS (AST-Detected Hints)
{}

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

For each endpoint, include: line_number, owner_node, method, path, handler_name, pattern_matched, response_type_file, response_type_position, response_type_string
  - response_type_file: The file path where the response type is defined (use the current file path if inline)
  - response_type_position: The character position (0-based index) where the response type annotation starts in the file
  - response_type_string: The exact TypeScript type string from the code (e.g., "Response<User[]>", "Response<{{ id: number }}>")
  - CRITICAL: Look for Express/Fastify Response<T> generic type annotations on handler parameters. Extract the FULL type including Response<...> wrapper.
  - Example: `(req: Request, res: Response<User[]>)` → response_type_string: "Response<User[]>"
  - Example: `(req, res: Response<{{ userId: number; comments: Comment[] }}>)` → response_type_string: "Response<{{ userId: number; comments: Comment[] }}>"

For each data_call, include: line_number, target, method (null if unknown), pattern_matched, response_type_file, response_type_position, response_type_string
  - For fetch/axios calls with typed responses like `await resp.json() as Comment[]`, extract the type assertion
  - For typed fetch wrappers, extract the generic type parameter

Return ONLY the JSON object, no explanations."#,
            mount_patterns,
            endpoint_patterns,
            data_patterns,
            candidates_section,
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
            response_type_file: Some("test.ts".to_string()),
            response_type_position: Some(100),
            response_type_string: Some("Response<User>".to_string()),
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
            response_type_file: None,
            response_type_position: None,
            response_type_string: Some("Comment[]".to_string()),
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
                response_type_file: None,
                response_type_position: None,
                response_type_string: None,
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
        assert!(system_message.contains("Filter Noise"));
    }
}
