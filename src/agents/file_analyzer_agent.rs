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
use std::collections::HashMap;

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
    pub candidate_id: String,
    pub line_number: i32,
    pub owner_node: String,
    pub method: String,
    pub path: String,
    pub handler_name: String,
    pub pattern_matched: String,
    /// Start byte offset of the endpoint definition call expression (from SWC via apply_candidate_map)
    #[serde(default)]
    pub call_expression_span_start: Option<u32>,
    /// End byte offset of the endpoint definition call expression (from SWC via apply_candidate_map)
    #[serde(default)]
    pub call_expression_span_end: Option<u32>,
    /// Verbatim code text of the request payload expression (from Gemini)
    pub payload_expression_text: Option<String>,
    /// Line number where the payload expression starts (from Gemini)
    pub payload_expression_line: Option<i32>,
    /// Verbatim code text of the response emission expression (from Gemini)
    pub response_expression_text: Option<String>,
    /// Line number where the response expression starts (from Gemini)
    pub response_expression_line: Option<i32>,
    /// The primary type symbol name without wrappers (e.g., "User" from "Response<User[]>")
    pub primary_type_symbol: Option<String>,
    /// Import path where the type is defined (e.g., "./types/user"), null if inline or same file
    pub type_import_source: Option<String>,
}

/// Result of analyzing a single data-fetching call
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DataCallResult {
    pub candidate_id: String,
    pub line_number: i32,
    pub target: String,
    pub method: Option<String>,
    pub pattern_matched: String,
    /// Start byte offset of the data call expression (from SWC via apply_candidate_map)
    #[serde(default)]
    pub call_expression_span_start: Option<u32>,
    /// End byte offset of the data call expression (from SWC via apply_candidate_map)
    #[serde(default)]
    pub call_expression_span_end: Option<u32>,
    /// Verbatim code text of the call expression itself (from Gemini)
    #[serde(default)]
    pub call_expression_text: Option<String>,
    /// Line number where the call expression starts (from Gemini)
    #[serde(default)]
    pub call_expression_line: Option<i32>,
    /// Verbatim code text of the request payload expression (from Gemini)
    pub payload_expression_text: Option<String>,
    /// Line number where the payload expression starts (from Gemini)
    pub payload_expression_line: Option<i32>,
    /// The primary type symbol name without wrappers (e.g., "User" from "Promise<User>")
    pub primary_type_symbol: Option<String>,
    /// Import path where the type is defined (e.g., "./types/user"), null if inline or same file
    pub type_import_source: Option<String>,
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
        self.analyze_file_with_candidates(
            file_path,
            file_content,
            guidance,
            &[],
            &[],
            &HashMap::new(),
        )
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
    /// * `candidate_contexts` - Structured candidate details (JSON strings)
    ///
    /// # Returns
    /// A `FileAnalysisResult` containing all detected mounts, endpoints, and data calls.
    pub async fn analyze_file_with_candidates(
        &self,
        file_path: &str,
        file_content: &str,
        guidance: &FrameworkGuidance,
        candidate_hints: &[String],
        candidate_contexts: &[String],
        import_map: &HashMap<String, String>,
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
            candidate_contexts,
            import_map,
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
            .analyze_code_with_schema(&user_message, &system_message, Some(schema.clone()))
            .await?;

        println!("=== RAW FILE ANALYSIS RESPONSE ===");
        println!("{}", response);
        println!("=== END RAW RESPONSE ===");

        let mut result: FileAnalysisResult = serde_json::from_str(&response).map_err(|e| {
            format!(
                "Failed to parse file analysis response: {}. Raw response: {}",
                e, response
            )
        })?;

        // Sanitize LLM response: Gemini sometimes returns "+null" as a string instead of null
        let needs_retry = Self::sanitize_result(&mut result);
        let initial_result = result.clone();
        if needs_retry {
            println!("[FileAnalyzerAgent] Suspicious fields detected in LLM output; retrying once");
            let response = self
                .agent_service
                .analyze_code_with_schema(&user_message, &system_message, Some(schema))
                .await?;

            println!("=== RAW FILE ANALYSIS RESPONSE ===");
            println!("{}", response);
            println!("=== END RAW RESPONSE ===");

            let mut retry_result: FileAnalysisResult =
                serde_json::from_str(&response).map_err(|e| {
                    format!(
                        "Failed to parse file analysis response: {}. Raw response: {}",
                        e, response
                    )
                })?;

            Self::sanitize_result(&mut retry_result);
            let chosen = Self::choose_best_result(initial_result, retry_result);

            println!(
                "File analysis complete: {} mounts, {} endpoints, {} data_calls",
                chosen.mounts.len(),
                chosen.endpoints.len(),
                chosen.data_calls.len()
            );

            return Ok(chosen);
        }

        println!(
            "File analysis complete: {} mounts, {} endpoints, {} data_calls",
            result.mounts.len(),
            result.endpoints.len(),
            result.data_calls.len()
        );

        Ok(result)
    }

    fn result_score(result: &FileAnalysisResult) -> usize {
        result.mounts.len() + result.endpoints.len() + result.data_calls.len()
    }

    fn choose_best_result(
        initial: FileAnalysisResult,
        retry: FileAnalysisResult,
    ) -> FileAnalysisResult {
        let initial_score = Self::result_score(&initial);
        let retry_score = Self::result_score(&retry);

        if retry_score >= initial_score {
            retry
        } else {
            println!("[FileAnalyzerAgent] Retry produced fewer findings; keeping original result");
            initial
        }
    }

    /// Sanitize the LLM response to fix common issues like "+null" strings
    fn sanitize_result(result: &mut FileAnalysisResult) -> bool {
        // Helper to check if a string represents null
        fn is_null_string(s: &str) -> bool {
            s == "+null" || s == "null" || s == "NULL" || s == "-" || s.is_empty()
        }

        fn normalize_optional_string(value: &mut Option<String>) {
            let Some(current) = value.as_deref() else {
                return;
            };
            let trimmed = current.trim();
            if is_null_string(trimmed) {
                *value = None;
            } else if trimmed != current {
                *value = Some(trimmed.to_string());
            }
        }

        fn is_suspicious_import_source(value: &str) -> bool {
            if value.contains("primary_type_symbol") || value.contains("type_import_source") {
                return true;
            }

            value.chars().any(|ch| {
                ch.is_whitespace()
                    || ch == '"'
                    || ch == '\''
                    || ch == '`'
                    || ch == '{'
                    || ch == '}'
                    || ch == '('
                    || ch == ')'
                    || ch == ':'
                    || ch == ','
                    || ch == '['
                    || ch == ']'
            })
        }

        fn is_invalid_relative_source(value: &str) -> bool {
            value.starts_with('.')
                && !value.starts_with("./")
                && !value.starts_with("../")
                && !value.starts_with(".\\")
                && !value.starts_with("..\\")
        }

        fn is_valid_identifier(value: &str) -> bool {
            let mut chars = value.chars();
            let Some(first) = chars.next() else {
                return false;
            };
            if !(first.is_ascii_alphabetic() || first == '_' || first == '$') {
                return false;
            }
            chars.all(|ch| ch.is_ascii_alphanumeric() || ch == '_' || ch == '$')
        }

        fn normalize_import_source(value: &mut Option<String>) -> bool {
            let mut suspicious = false;
            normalize_optional_string(value);
            if let Some(source) = value.as_deref() {
                if source.starts_with("node:") {
                    *value = None;
                } else if is_suspicious_import_source(source) || is_invalid_relative_source(source)
                {
                    *value = None;
                    suspicious = true;
                }
            }
            suspicious
        }

        let mut needs_retry = false;

        // Sanitize endpoints
        for endpoint in &mut result.endpoints {
            normalize_optional_string(&mut endpoint.primary_type_symbol);
            if normalize_import_source(&mut endpoint.type_import_source) {
                needs_retry = true;
            }
            if let Some(ref symbol) = endpoint.primary_type_symbol {
                if !is_valid_identifier(symbol) {
                    endpoint.primary_type_symbol = None;
                }
            }
        }

        // Sanitize data calls
        for data_call in &mut result.data_calls {
            // Fix method field
            if let Some(ref mut s) = data_call.method {
                let trimmed = s.trim();
                if is_null_string(trimmed) {
                    data_call.method = None;
                } else if trimmed != s.as_str() {
                    *s = trimmed.to_string();
                }
            }
            normalize_optional_string(&mut data_call.primary_type_symbol);
            if normalize_import_source(&mut data_call.type_import_source) {
                needs_retry = true;
            }
            if let Some(ref symbol) = data_call.primary_type_symbol {
                if !is_valid_identifier(symbol) {
                    data_call.primary_type_symbol = None;
                }
            }
        }

        needs_retry
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
You function purely as a **Pattern Matcher** and **Alias Resolver**. You do NOT possess inherent knowledge of specific frameworks; you must rely strictly on the **ACTIVE PATTERNS** provided in the input. For data_calls, you must use the provided call-chain context and import table to decide whether the call is a downstream HTTP consumer or just parsing; do not infer from client names alone. If the structured context lacks a path/method, set them to null and mark as non-consumer.

### INPUT DATA
1. **Full Source Code**: The complete file content for context (imports, definitions).
2. **Candidate Targets**: A list of specific lines where an AST parser detected potential API activity.
3. **Import Table**: AST-derived mapping of local identifiers to module sources (do not invent new sources).
4. **Active Patterns**: The specific code patterns to classify (e.g., Mounts, Endpoints).

### CORE OBJECTIVE
Analyze the **Full Source Code**. Focus specifically on the **Candidate Targets** to classify them, but use the surrounding code to resolve variables and imports.

### 1. ANALYSIS RULES

#### A. Strict Pattern Matching
* **Endpoints:** If a target matches an `endpoint_pattern`, extract it.
* **Mounts:** If a target matches a `mount_pattern`, extract it.
* **Data Calls:** If a target matches a `data_fetching_pattern`, extract it. Use the provided call-chain context and import table; decide if it is a downstream HTTP consumer vs. pure parsing. Do not rely on hardcoded client heuristics.
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
* Always include the candidate_id from the candidate context for each endpoint/data_call.
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
* **response.json()/.text():** Treat these as data_calls only when they are part of an actual downstream HTTP consumer. Use the provided call-chain context (upstream call, path/method literal, enclosing function) to decide; avoid framework/client-specific heuristics.

### 5. TYPE LOCATION EXTRACTION (CRITICAL FOR TYPE CHECKING)
For endpoints and data calls, emit **expression text + line number** to tell the compiler where to infer types.
The source code is displayed with line-number prefixes (e.g., "  42| res.json(users)"). Read the number directly.

#### A. Response Body Expressions (MANDATORY for endpoints)
Identify the expression that sends/returns the response body (res.json(...), reply.send(...), return ...).
* You MUST emit `response_expression_text` and `response_expression_line` for EVERY endpoint that sends a response.
* Emit `response_expression_text` as the verbatim code text (e.g., `res.json(users)`)
* Emit `response_expression_line` as the line number where this expression starts
* CRITICAL: Copy the expression EXACTLY as it appears in the source code. Do not paraphrase or modify it.
* If unsure about the exact expression, emit your best match — an approximate match is far better than null.

#### B. Request Payload Expressions (MANDATORY when present)
Identify the expression representing request payloads:
* Endpoints: req.body / ctx.request.body or payload forwarded into downstream calls.
* Data calls: the payload argument passed to fetch/axios/etc.
* You MUST emit `payload_expression_text` and `payload_expression_line` for EVERY endpoint/data_call that receives a payload.
* Emit `payload_expression_text` as the verbatim code text (e.g., `req.body`)
* Emit `payload_expression_line` as the line number where this expression starts
* If unsure about the exact expression, emit your best match — an approximate match is far better than null.

#### C. Call Expression Text (MANDATORY for data calls)
For data calls, emit `call_expression_text` and `call_expression_line` for the HTTP call expression itself.
* Emit `call_expression_text` as the verbatim code text of the fetch/axios call (e.g., `fetch("/api/users")`)
* Emit `call_expression_line` as the line number where the call expression starts
* This tells the compiler where to find the call expression for return-type inference.

#### D. Explicit Type Symbols (optional)
If you see explicit TypeScript type annotations, extract:
* primary_type_symbol (core identifier, no wrappers)
* type_import_source (matching import source, or null if local)
Do NOT emit full type strings. If no explicit annotation is found, set both to null."#.to_string()
    }

    /// Build the dynamic user message with patterns and file content (legacy).
    #[allow(dead_code)]
    fn build_user_message(
        &self,
        file_path: &str,
        file_content: &str,
        guidance: &FrameworkGuidance,
    ) -> String {
        self.build_user_message_with_candidates(
            file_path,
            file_content,
            guidance,
            &[],
            &[],
            &HashMap::new(),
        )
    }

    /// Build the dynamic user message with patterns, file content, and candidate targets.
    fn build_user_message_with_candidates(
        &self,
        file_path: &str,
        file_content: &str,
        guidance: &FrameworkGuidance,
        candidate_hints: &[String],
        candidate_contexts: &[String],
        import_map: &HashMap<String, String>,
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

        let candidate_contexts_section = if candidate_contexts.is_empty() {
            "No structured candidate contexts provided.".to_string()
        } else {
            format!(
                "Structured candidate contexts (JSON, one per candidate):\n{}",
                candidate_contexts.join("\n")
            )
        };

        // Deterministic import map formatting
        let mut imports: Vec<_> = import_map.iter().collect();
        imports.sort_by(|a, b| a.0.cmp(b.0));
        let imports_section = if imports.is_empty() {
            "No imports detected by AST; treat all symbols as local unless resolved in code."
                .to_string()
        } else {
            let lines: Vec<String> = imports
                .iter()
                .map(|(local, source)| format!(r#"  - "{}" -> "{}""#, local, source))
                .collect();
            format!(
                "Resolved import table (AST-derived):\n{}\nUse this table for symbol grounding; do NOT invent sources.",
                lines.join("\n")
            )
        };

        // Add line-number prefixes to file content so Gemini can read line numbers directly
        let mut numbered_content =
            String::with_capacity(file_content.len() + file_content.lines().count() * 7);
        for (i, line) in file_content.lines().enumerate() {
            use std::fmt::Write;
            if i > 0 {
                numbered_content.push('\n');
            }
            let _ = write!(numbered_content, "{:4}| {}", i + 1, line);
        }

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

### CANDIDATE CONTEXT (Structured JSON)
{}  // Use these JSON blobs to decide method/path/consumer vs non-consumer. If missing path/method, set them to null.

### IMPORT TABLE (Do not hallucinate sources)
{}

### FRAMEWORK-SPECIFIC HINTS
{}

### FILE CONTENT (Path: {})
Lines are prefixed with line numbers. Use these numbers for *_expression_line fields.
```
{}
```

Analyze this file and return a JSON object with:
- "mounts": array of mount relationships found
- "endpoints": array of HTTP endpoint definitions found
- "data_calls": array of data fetching calls found

For each mount, include: line_number, parent_node, child_node, mount_path, import_source (null if local), pattern_matched

For each endpoint, include: candidate_id, line_number, owner_node, method, path, handler_name, pattern_matched,
response_expression_text, response_expression_line, payload_expression_text, payload_expression_line,
primary_type_symbol, type_import_source
  - Echo candidate_id from the candidate context
  - MUST emit response_expression_text: copy the EXACT expression text that sends the response (e.g., "res.json(users)")
  - MUST emit response_expression_line: read the line number from the prefix in the source code
  - For payload_expression_text: copy the EXACT expression for the request payload (e.g., "req.body")
  - For payload_expression_line: read the line number from the prefix

For each data_call, include: candidate_id, line_number, target, method (null if unknown), pattern_matched,
call_expression_text, call_expression_line, payload_expression_text, payload_expression_line,
primary_type_symbol, type_import_source
  - Echo candidate_id from the candidate context
  - MUST emit call_expression_text: copy the EXACT text of the fetch/axios/HTTP call (e.g., 'fetch("/api/users")')
  - MUST emit call_expression_line: read the line number from the prefix
  - For payload_expression_text: copy the EXACT payload argument text if detected
  - For payload_expression_line: read the line number from the prefix

Return ONLY the JSON object, no explanations."#,
            mount_patterns,
            endpoint_patterns,
            data_patterns,
            candidates_section,
            candidate_contexts_section,
            imports_section,
            guidance.triage_hints,
            file_path,
            numbered_content
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
    use std::collections::HashMap;

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
            candidate_id: "span:100-140".to_string(),
            line_number: 15,
            owner_node: "router".to_string(),
            method: "GET".to_string(),
            path: "/users/:id".to_string(),
            handler_name: "getUserById".to_string(),
            pattern_matched: ".get(".to_string(),
            call_expression_span_start: None,
            call_expression_span_end: None,
            payload_expression_text: None,
            payload_expression_line: None,
            response_expression_text: None,
            response_expression_line: None,
            primary_type_symbol: Some("User".to_string()),
            type_import_source: Some("./types/user".to_string()),
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
            candidate_id: "span:200-260".to_string(),
            line_number: 25,
            target: "https://api.example.com/data".to_string(),
            method: Some("POST".to_string()),
            pattern_matched: "fetch(".to_string(),
            call_expression_span_start: None,
            call_expression_span_end: None,
            call_expression_text: None,
            call_expression_line: None,
            payload_expression_text: None,
            payload_expression_line: None,
            primary_type_symbol: Some("Comment".to_string()),
            type_import_source: None,
        };

        let json = serde_json::to_string(&data_call).unwrap();
        assert!(json.contains("target"));
        assert!(json.contains("POST"));

        let deserialized: DataCallResult = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.target, "https://api.example.com/data");
        assert_eq!(deserialized.method, Some("POST".to_string()));
    }

    #[test]
    fn test_sanitize_result_filters_placeholders() {
        let mut result = FileAnalysisResult {
            mounts: vec![],
            endpoints: vec![EndpointResult {
                candidate_id: "span:10-50".to_string(),
                line_number: 1,
                owner_node: "app".to_string(),
                method: "GET".to_string(),
                path: "/test".to_string(),
                handler_name: "handler".to_string(),
                pattern_matched: ".get(".to_string(),
                call_expression_span_start: None,
                call_expression_span_end: None,
                payload_expression_text: None,
                payload_expression_line: None,
                response_expression_text: None,
                response_expression_line: None,
                primary_type_symbol: Some("-".to_string()),
                type_import_source: Some(".repo-a_types.ts".to_string()),
            }],
            data_calls: vec![DataCallResult {
                candidate_id: "span:60-120".to_string(),
                line_number: 2,
                target: "https://example.com".to_string(),
                method: Some("  POST  ".to_string()),
                pattern_matched: "fetch(".to_string(),
                call_expression_span_start: None,
                call_expression_span_end: None,
                call_expression_text: None,
                call_expression_line: None,
                payload_expression_text: None,
                payload_expression_line: None,
                primary_type_symbol: Some("NULL".to_string()),
                type_import_source: Some("bad import (oops)".to_string()),
            }],
        };

        let needs_retry = FileAnalyzerAgent::sanitize_result(&mut result);
        assert!(needs_retry);

        let endpoint = &result.endpoints[0];
        assert!(endpoint.primary_type_symbol.is_none());
        assert!(endpoint.type_import_source.is_none());

        let data_call = &result.data_calls[0];
        assert_eq!(data_call.method, Some("POST".to_string()));
        assert!(data_call.primary_type_symbol.is_none());
        assert!(data_call.type_import_source.is_none());
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
                candidate_id: "span:80-120".to_string(),
                line_number: 10,
                owner_node: "router".to_string(),
                method: "GET".to_string(),
                path: "/health".to_string(),
                handler_name: "healthCheck".to_string(),
                pattern_matched: ".get(".to_string(),
                call_expression_span_start: None,
                call_expression_span_end: None,
                payload_expression_text: None,
                payload_expression_line: None,
                response_expression_text: None,
                response_expression_line: None,
                primary_type_symbol: None,
                type_import_source: None,
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
    fn test_choose_best_result_prefers_richer_output() {
        let initial = FileAnalysisResult {
            mounts: vec![MountResult {
                line_number: 1,
                parent_node: "app".to_string(),
                child_node: "router".to_string(),
                mount_path: "/api".to_string(),
                import_source: None,
                pattern_matched: "app.use".to_string(),
            }],
            endpoints: vec![EndpointResult {
                candidate_id: "span:130-180".to_string(),
                line_number: 2,
                owner_node: "router".to_string(),
                method: "GET".to_string(),
                path: "/users".to_string(),
                handler_name: "handler".to_string(),
                pattern_matched: ".get(".to_string(),
                call_expression_span_start: None,
                call_expression_span_end: None,
                payload_expression_text: None,
                payload_expression_line: None,
                response_expression_text: None,
                response_expression_line: None,
                primary_type_symbol: None,
                type_import_source: None,
            }],
            data_calls: vec![DataCallResult {
                candidate_id: "span:190-240".to_string(),
                line_number: 3,
                target: "/users".to_string(),
                method: Some("GET".to_string()),
                pattern_matched: "fetch(".to_string(),
                call_expression_span_start: None,
                call_expression_span_end: None,
                call_expression_text: None,
                call_expression_line: None,
                payload_expression_text: None,
                payload_expression_line: None,
                primary_type_symbol: None,
                type_import_source: None,
            }],
        };

        let retry = FileAnalysisResult {
            mounts: vec![],
            endpoints: vec![],
            data_calls: vec![],
        };

        let chosen = FileAnalyzerAgent::choose_best_result(initial.clone(), retry);
        assert_eq!(chosen.mounts.len(), initial.mounts.len());
        assert_eq!(chosen.endpoints.len(), initial.endpoints.len());
        assert_eq!(chosen.data_calls.len(), initial.data_calls.len());
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
    fn test_build_user_message_includes_import_table_and_candidates() {
        let agent = FileAnalyzerAgent::new(AgentService::new("mock".to_string()));
        let guidance = create_test_guidance();
        let file_content = r#"
import { User } from './types';
const data = await fetch('/api/users').then(resp => resp.json());
"#;

        let candidates = vec!["- Line 3: fetch(...) - `fetch('/api/users')`".to_string()];
        let candidate_contexts: Vec<String> =
            vec![r#"{"line":3,"callee":"fetch","path":"/api/users","fn":"getData"}"#.to_string()];
        let mut import_map = HashMap::new();
        import_map.insert("User".to_string(), "./types".to_string());
        import_map.insert("useUsers".to_string(), "../hooks".to_string());

        let message = agent.build_user_message_with_candidates(
            "test.ts",
            file_content,
            &guidance,
            &candidates,
            &candidate_contexts,
            &import_map,
        );

        assert!(message.contains("IMPORT TABLE"));
        assert!(message.contains(r#""User" -> "./types""#));
        assert!(message.contains("CANDIDATE TARGETS"));
        assert!(message.contains("Line 3"));
        assert!(message.contains("CANDIDATE CONTEXT"));
        assert!(message.contains("/api/users"));
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
        assert!(system_message.contains("call-chain context"));
        assert!(system_message.contains("Import Table"));

        // Verify it emphasizes pattern-based matching
        assert!(system_message.contains("Strict Pattern Matching"));
        assert!(system_message.contains("Filter Noise"));
    }
}
