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
    visitor::{ImportedSymbol, SymbolKind},
};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use tracing::{debug, warn};

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
    /// Verbatim code text of the response payload subexpression (from Gemini).
    /// This is the value whose type we want — e.g., `users` in `res.json(users)`,
    /// `ctx.body = users`, `h.response(users)`, `return users`, `reply.send(users)`,
    /// `c.json(users)`. Framework-agnostic; set to null for payload-less handlers
    /// (redirects, 204s, streaming).
    pub response_expression_text: Option<String>,
    /// Line number where the payload expression starts (from Gemini)
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
        imported_symbols: &HashMap<String, ImportedSymbol>,
    ) -> Result<FileAnalysisResult, Box<dyn std::error::Error>> {
        // Skip empty files
        if file_content.trim().is_empty() {
            return Ok(FileAnalysisResult::default());
        }

        let user_message = self.build_user_message_with_candidates(
            file_path,
            file_content,
            guidance,
            candidate_hints,
            candidate_contexts,
            imported_symbols,
        );

        debug!("=== FILE ANALYZER AGENT (AST-GATED) ===");
        debug!("Analyzing file: {}", file_path);
        debug!(
            "File size: {} chars, {} lines",
            file_content.len(),
            file_content.lines().count()
        );
        debug!("Candidate targets: {}", candidate_hints.len());

        let schema = AgentSchemas::file_analysis_schema();
        let response = self
            .agent_service
            .analyze_with_lambda("/analyze-file", &user_message, Some(schema.clone()))
            .await?;

        debug!("=== RAW FILE ANALYSIS RESPONSE ===");
        debug!("{}", response);
        debug!("=== END RAW RESPONSE ===");

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
            warn!("[FileAnalyzerAgent] Suspicious fields detected in LLM output; retrying once");
            let response = self
                .agent_service
                .analyze_with_lambda("/analyze-file", &user_message, Some(schema))
                .await?;

            debug!("=== RAW FILE ANALYSIS RESPONSE ===");
            debug!("{}", response);
            debug!("=== END RAW RESPONSE ===");

            let mut retry_result: FileAnalysisResult =
                serde_json::from_str(&response).map_err(|e| {
                    format!(
                        "Failed to parse file analysis response: {}. Raw response: {}",
                        e, response
                    )
                })?;

            Self::sanitize_result(&mut retry_result);
            let chosen = Self::choose_best_result(initial_result, retry_result);

            debug!(
                "File analysis complete: {} mounts, {} endpoints, {} data_calls",
                chosen.mounts.len(),
                chosen.endpoints.len(),
                chosen.data_calls.len()
            );

            return Ok(chosen);
        }

        debug!(
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
            warn!("[FileAnalyzerAgent] Retry produced fewer findings; keeping original result");
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
            if let Some(ref symbol) = endpoint.primary_type_symbol
                && !is_valid_identifier(symbol)
            {
                endpoint.primary_type_symbol = None;
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
            if let Some(ref symbol) = data_call.primary_type_symbol
                && !is_valid_identifier(symbol)
            {
                data_call.primary_type_symbol = None;
            }
        }

        needs_retry
    }

    // The system prompt for the Carrick Static Analysis Engine lives in
    // the private carrick-cloud file-analyzer lambda
    // (lambdas/file-analyzer/system_prompt.txt). This orchestrator only
    // builds the structured user_message and POSTs to /analyze-file via
    // AgentService::analyze_with_lambda.

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
        imported_symbols: &HashMap<String, ImportedSymbol>,
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

        let imports_section = Self::format_import_table(imported_symbols);

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

### FRAMEWORK-SPECIFIC PARSING NOTES
These notes are generated per-scan by the framework guidance layer and describe how to correctly extract endpoints, mounts, owners, and prefixes for the exact framework(s) detected in this repo. Read them carefully — they override any generic rule in the system prompt when they conflict.
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
  - MUST emit response_expression_text: copy the EXACT payload subexpression (e.g., "users" from res.json(users), ctx.body = users, h.response(users), or return users). Emit null for payload-less handlers.
  - MUST emit response_expression_line: read the line number from the prefix in the source code (line where the payload subexpression appears)
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
            guidance.parsing_notes,
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

    /// Format the AST-derived imports grouped by source module with kind
    /// annotations. This is Move 3 (§9.3) in framework-coverage.md: richer
    /// per-file grounding so the LLM reads symbols against real imports
    /// rather than a pattern list.
    ///
    /// Format:
    /// ```text
    /// Imports resolved from the AST (grouped by source):
    ///   - From '@nestjs/common': Get, Post, Controller [named]
    ///   - From 'koa': Koa [default]
    ///   - From 'express': express [namespace]
    ///   - From './user.service': UserService [named]
    /// ```
    fn format_import_table(imported_symbols: &HashMap<String, ImportedSymbol>) -> String {
        if imported_symbols.is_empty() {
            return "No imports detected by AST; treat all symbols as local unless resolved in code.".to_string();
        }

        // Group by (source, kind). Named imports can batch per source;
        // default/namespace are one-per-source in practice but we handle it
        // uniformly for deterministic output.
        use std::collections::BTreeMap;
        let mut groups: BTreeMap<(String, &'static str), Vec<(String, String)>> = BTreeMap::new();
        for symbol in imported_symbols.values() {
            let kind_label = match symbol.kind {
                SymbolKind::Named => "named",
                SymbolKind::Default => "default",
                SymbolKind::Namespace => "namespace",
            };
            groups
                .entry((symbol.source.clone(), kind_label))
                .or_default()
                .push((symbol.local_name.clone(), symbol.imported_name.clone()));
        }

        // Stable per-group ordering by local name.
        for entries in groups.values_mut() {
            entries.sort();
        }

        let lines: Vec<String> = groups
            .iter()
            .map(|((source, kind_label), entries)| {
                let pretty: Vec<String> = entries
                    .iter()
                    .map(|(local, imported)| {
                        if local == imported {
                            local.clone()
                        } else {
                            // `import { Foo as Bar }` → Bar (as Foo)
                            format!("{local} (as {imported})")
                        }
                    })
                    .collect();
                format!(
                    "  - From '{source}': {names} [{kind_label}]",
                    names = pretty.join(", "),
                )
            })
            .collect();

        format!(
            "Imports resolved from the AST (grouped by source):\n{}\n\n\
             Use this table to interpret candidates. An identifier used in this file that appears above is the imported symbol from that module; an identifier NOT listed is either local to this file or globally available. Do NOT invent sources.",
            lines.join("\n"),
        )
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

    fn named(local: &str, source: &str) -> ImportedSymbol {
        ImportedSymbol {
            local_name: local.to_string(),
            imported_name: local.to_string(),
            source: source.to_string(),
            kind: SymbolKind::Named,
        }
    }

    fn default_import(local: &str, source: &str) -> ImportedSymbol {
        ImportedSymbol {
            local_name: local.to_string(),
            imported_name: local.to_string(),
            source: source.to_string(),
            kind: SymbolKind::Default,
        }
    }

    fn namespace_import(local: &str, source: &str) -> ImportedSymbol {
        ImportedSymbol {
            local_name: local.to_string(),
            imported_name: local.to_string(),
            source: source.to_string(),
            kind: SymbolKind::Namespace,
        }
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
        let mut imported = HashMap::new();
        imported.insert("User".to_string(), named("User", "./types"));
        imported.insert("useUsers".to_string(), named("useUsers", "../hooks"));

        let message = agent.build_user_message_with_candidates(
            "test.ts",
            file_content,
            &guidance,
            &candidates,
            &candidate_contexts,
            &imported,
        );

        assert!(message.contains("IMPORT TABLE"));
        // Grouped-by-source format: "From './types': User [named]".
        assert!(
            message.contains("From './types': User [named]"),
            "expected grouped import line, got: {message}"
        );
        assert!(message.contains("CANDIDATE TARGETS"));
        assert!(message.contains("Line 3"));
        assert!(message.contains("CANDIDATE CONTEXT"));
        assert!(message.contains("/api/users"));
    }

    #[test]
    fn test_format_import_table_groups_by_source_and_kind() {
        // Named imports from the same module batch together (NestJS style).
        let mut imports = HashMap::new();
        imports.insert("Get".to_string(), named("Get", "@nestjs/common"));
        imports.insert("Post".to_string(), named("Post", "@nestjs/common"));
        imports.insert(
            "Controller".to_string(),
            named("Controller", "@nestjs/common"),
        );
        imports.insert("Koa".to_string(), default_import("Koa", "koa"));
        imports.insert(
            "express".to_string(),
            namespace_import("express", "express"),
        );

        let out = FileAnalyzerAgent::format_import_table(&imports);
        // @nestjs/common should list all three named imports on one line, sorted.
        assert!(
            out.contains("From '@nestjs/common': Controller, Get, Post [named]"),
            "named imports should group & sort: {out}"
        );
        // Default & namespace annotated explicitly.
        assert!(
            out.contains("From 'koa': Koa [default]"),
            "default import annotation: {out}"
        );
        assert!(
            out.contains("From 'express': express [namespace]"),
            "namespace import annotation: {out}"
        );
    }

    #[test]
    fn test_format_import_table_aliased_named_import() {
        // `import { Foo as Bar } from 'mod'` — show both.
        let mut imports = HashMap::new();
        imports.insert(
            "Bar".to_string(),
            ImportedSymbol {
                local_name: "Bar".to_string(),
                imported_name: "Foo".to_string(),
                source: "mod".to_string(),
                kind: SymbolKind::Named,
            },
        );
        let out = FileAnalyzerAgent::format_import_table(&imports);
        assert!(
            out.contains("From 'mod': Bar (as Foo) [named]"),
            "aliased import should show `local (as imported)`: {out}"
        );
    }

    #[test]
    fn test_format_import_table_empty() {
        let imports = HashMap::new();
        let out = FileAnalyzerAgent::format_import_table(&imports);
        assert!(out.contains("No imports detected"));
    }

    // test_system_message_is_framework_agnostic was deleted: the system
    // prompt now lives in carrick-cloud/lambdas/file-analyzer/system_prompt.txt
    // and is no longer accessible from this Rust crate.
}
