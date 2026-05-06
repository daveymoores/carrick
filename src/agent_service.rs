use crate::visitor::{Call, Json, TypeReference};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::env;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::Semaphore;
use tokio::time::{Duration, sleep};
use tracing::{debug, warn};

/// Reusable service for making Agent API calls
#[derive(Debug, Clone)]
pub struct AgentService {
    api_key: String,
    client: Client,
    semaphore: Arc<Semaphore>,
}

impl AgentService {
    pub fn new(api_key: String) -> Self {
        // Limit concurrent requests to avoid rate limits
        // Paid tier allows higher limits, but let's be safe with 20 concurrent requests
        let concurrency_limit = env::var("CARRICK_CONCURRENCY_LIMIT")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(20);
        let use_system_proxy = env::var("CARRICK_USE_SYSTEM_PROXY").is_ok();
        let mut client_builder = Client::builder();
        if !use_system_proxy {
            client_builder = client_builder.no_proxy();
        }
        let client = client_builder
            .build()
            .expect("Failed to build agent HTTP client");

        Self {
            api_key,
            client,
            semaphore: Arc::new(Semaphore::new(concurrency_limit)),
        }
    }

    /// Per-task lambda call where the lambda just needs a user_message +
    /// schema (e.g. file-analyzer). The lambda owns the system prompt.
    /// `task_path` is the API Gateway route, e.g. "/analyze-file".
    pub async fn analyze_with_lambda(
        &self,
        task_path: &str,
        user_message: &str,
        response_schema: Option<serde_json::Value>,
    ) -> Result<String, Box<dyn std::error::Error>> {
        let request = LambdaRequest {
            user_message: user_message.to_string(),
            response_schema,
        };
        self.post_to_lambda(task_path, &request, user_message).await
    }

    /// Lower-level per-task lambda call for arbitrary structured payloads
    /// (e.g. framework-guidance which sends task+category+frameworks).
    /// `mock_seed` is used in mock mode to pick the right canned response.
    pub async fn post_to_lambda<B: Serialize + ?Sized>(
        &self,
        task_path: &str,
        body: &B,
        mock_seed: &str,
    ) -> Result<String, Box<dyn std::error::Error>> {
        let _permit = self
            .semaphore
            .acquire()
            .await
            .map_err(|e| format!("Failed to acquire semaphore permit: {}", e))?;

        if env::var("CARRICK_MOCK_ALL").is_ok() {
            return Ok(generate_mock_for_task(task_path, body, mock_seed));
        }

        self.post_with_retry(task_path, body).await
    }

    /// Shared HTTP + retry implementation for all lambda calls. Sends
    /// the version header, parses the structured error envelope, and
    /// only consumes a backoff attempt when the error is marked
    /// retriable=true (or on bare network failures).
    async fn post_with_retry<B>(
        &self,
        path: &str,
        body: &B,
    ) -> Result<String, Box<dyn std::error::Error>>
    where
        B: Serialize + ?Sized,
    {
        let api_base = env!("CARRICK_API_ENDPOINT");
        let endpoint = format!("{}{}", api_base, path);

        let request_builder = self
            .client
            .post(&endpoint)
            .json(body)
            .timeout(std::time::Duration::from_secs(60))
            .header("X-Carrick-Scanner-Version", env!("CARGO_PKG_VERSION"))
            .header("X-Carrick-Run-Id", crate::logging::run_id())
            .header("Authorization", format!("Bearer {}", self.api_key));

        // Retry logic for transient failures with exponential backoff.
        // 7 attempts: 2s, 4s, 8s, 16s, 32s, 64s. The lambda's structured
        // error envelope (`error.retriable`) is the source of truth for
        // application-level errors. We additionally retry on transient
        // *gateway* errors (429/502/503/504) where the body may not
        // even be a parseable JSON envelope (API Gateway timeouts return
        // non-envelope responses).
        let max_retries = 7;
        for attempt in 1..=max_retries {
            match request_builder.try_clone().unwrap().send().await {
                Ok(response) => {
                    let status = response.status();
                    let is_transient_gateway_status =
                        matches!(status.as_u16(), 429 | 502 | 503 | 504);

                    let body: AgentResponse = match response.json().await {
                        Ok(b) => b,
                        Err(e) => {
                            // Body wasn't a parseable envelope. If the status
                            // is a known transient gateway code, retry —
                            // otherwise fail fast (server-side bug).
                            if is_transient_gateway_status && attempt < max_retries {
                                let wait_time = Duration::from_secs(2u64.pow(attempt as u32));
                                warn!(
                                    "Gateway status {} with non-envelope body: {}. Retrying in {:?} (attempt {}/{})",
                                    status, e, wait_time, attempt, max_retries
                                );
                                sleep(wait_time).await;
                                continue;
                            }
                            return Err(format!(
                                "Agent proxy returned status {} with unparseable body: {}",
                                status, e
                            )
                            .into());
                        }
                    };

                    if status.is_success() && body.success {
                        return Ok(body.text.unwrap_or_default());
                    }

                    let err = match body.error {
                        Some(err) => err,
                        None => {
                            return Err(format!(
                                "Agent proxy status {} success={} but no error envelope",
                                status, body.success
                            )
                            .into());
                        }
                    };

                    if err.retriable && attempt < max_retries {
                        let wait_time = Duration::from_secs(2u64.pow(attempt as u32));
                        warn!(
                            "Agent error '{}' is retriable, retrying in {:?} (attempt {}/{}): {}",
                            err.code, wait_time, attempt, max_retries, err.message
                        );
                        sleep(wait_time).await;
                        continue;
                    }

                    return Err(format!(
                        "Agent error '{}' (retriable={}): {}",
                        err.code, err.retriable, err.message
                    )
                    .into());
                }
                Err(e) => {
                    // Bare network failure (no response received) — retriable by definition.
                    if attempt < max_retries {
                        let wait_time = Duration::from_secs(2u64.pow(attempt as u32));
                        warn!(
                            "Agent proxy network error: {}, retrying in {:?} (attempt {}/{})",
                            e, wait_time, attempt, max_retries
                        );
                        sleep(wait_time).await;
                        continue;
                    }

                    return Err(format!("Agent proxy call failed: {}", e).into());
                }
            }
        }

        Err("Maximum retry attempts exceeded".into())
    }
}

#[derive(Debug, Serialize)]
pub struct AsyncCallContext {
    pub kind: String,
    pub function_source: String,
    pub file: String,
    pub line: u32,
    pub function_name: String,
}

#[derive(Debug, Deserialize)]
pub struct AgentCallResponse {
    route: String,
    method: String,
    request_body: Option<serde_json::Value>,
    request_type_info: Option<TypeInfo>,
    response_type_info: Option<TypeInfo>,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct TypeInfo {
    pub file_path: String,
    pub start_position: u32,
    pub composite_type_string: String,
    pub alias: String,
}

/// Request body for per-task lambda endpoints (e.g. /analyze-file).
/// The lambda owns the system prompt; Rust just sends the user payload.
#[derive(Debug, Serialize)]
struct LambdaRequest {
    user_message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    response_schema: Option<serde_json::Value>,
}

/// Lambda response envelope. On success: `success=true, text="..."`.
/// On failure: `success=false, error=AgentError{...}`. The `retriable`
/// flag on the error is the source of truth for whether the scanner
/// should consume an exponential-backoff attempt.
#[derive(Debug, Deserialize)]
struct AgentResponse {
    success: bool,
    #[serde(default)]
    text: Option<String>,
    #[serde(default)]
    error: Option<AgentError>,
}

#[derive(Debug, Deserialize, Clone)]
struct AgentError {
    code: String,
    message: String,
    retriable: bool,
}

pub async fn extract_calls_from_async_expressions(
    async_calls: Vec<AsyncCallContext>,
    frameworks: &[String],
    data_fetchers: &[String],
) -> Result<Vec<Call>, Box<dyn std::error::Error>> {
    // If CARRICK_MOCK_ALL is set, bypass the actual API call for testing purposes
    if env::var("CARRICK_MOCK_ALL").is_ok() {
        return Ok(vec![]);
    }

    // Emergency disable option for Agent API
    if env::var("DISABLE_AGENT").is_ok() {
        debug!("Agent API disabled via DISABLE_AGENT environment variable");
        return Ok(vec![]);
    }

    if async_calls.is_empty() {
        return Ok(vec![]);
    }

    // Size protection: warn if function sources are very large (high token usage)
    let total_size: usize = async_calls
        .iter()
        .map(|call| call.function_source.len())
        .sum();

    const MAX_REASONABLE_SIZE: usize = 200_000; // 200KB - warn above this
    if total_size > MAX_REASONABLE_SIZE {
        warn!(
            "Warning: Large amount of source code to analyze ({:.1}KB total). This may result in high token usage.",
            total_size as f64 / 1024.0
        );
    }

    debug!(
        "Found {} async expressions, sending to /extract-calls lambda with framework context",
        async_calls.len()
    );

    // Get API key and create AgentService
    let api_key = env::var("CARRICK_API_KEY")
        .map_err(|_| "CARRICK_API_KEY environment variable must be set")?;
    let agent_service = AgentService::new(api_key);

    let payload = serde_json::json!({
        "async_calls": async_calls,
        "frameworks": frameworks,
        "data_fetchers": data_fetchers,
    });

    let response = agent_service
        .post_to_lambda("/extract-calls", &payload, "extract-calls")
        .await?;

    Ok(parse_agent_response(&response, &async_calls))
}

// create_extraction_system_message + create_extraction_prompt moved to
// carrick-cloud/lambdas/extract-calls/ (system_prompt.txt + buildUserMessage).
// Rust now sends {async_calls, frameworks, data_fetchers} to /extract-calls;
// the lambda assembles the prompt from those fields.

fn convert_agent_responses_to_calls(
    agent_calls: Vec<AgentCallResponse>,
    contexts: &[AsyncCallContext],
) -> Vec<Call> {
    agent_calls
        .into_iter()
        .enumerate()
        .map(|(i, gc)| {
            let file_path = contexts
                .get(i)
                .map(|c| PathBuf::from(&c.file))
                .unwrap_or_default();

            // Convert TypeInfo to TypeReference if present
            let request_type = gc.request_type_info.map(|type_info| {
                TypeReference {
                    file_path: PathBuf::from(type_info.file_path),
                    type_ann: None, // We don't have SWC AST node from Agent
                    start_position: type_info.start_position as usize,
                    composite_type_string: type_info.composite_type_string,
                    alias: type_info.alias,
                }
            });

            let response_type = gc.response_type_info.map(|type_info| {
                TypeReference {
                    file_path: PathBuf::from(type_info.file_path),
                    type_ann: None, // We don't have SWC AST node from Agent
                    start_position: type_info.start_position as usize,
                    composite_type_string: type_info.composite_type_string,
                    alias: type_info.alias,
                }
            });

            Call {
                route: gc.route,
                method: gc.method.to_uppercase(),
                response: Json::Null,
                request: gc.request_body.and_then(|v| serde_json::from_value(v).ok()),
                response_type,
                request_type,
                call_file: file_path,
                call_id: None,
                call_number: None,
                common_type_name: None,
            }
        })
        .collect()
}

fn parse_agent_response(response: &str, contexts: &[AsyncCallContext]) -> Vec<Call> {
    let json_str = response.trim();

    // Try multiple parsing strategies
    let cleaned = clean_response(json_str);
    let extraction_attempts = vec![
        // Direct parse (clean response)
        json_str,
        // Extract from code blocks
        extract_from_code_block(json_str),
        // Extract JSON array bounds
        extract_json_array(json_str),
        // Clean and retry
        &cleaned,
    ];

    for attempt in extraction_attempts {
        if let Ok(agent_calls) = serde_json::from_str::<Vec<AgentCallResponse>>(attempt) {
            return convert_agent_responses_to_calls(agent_calls, contexts);
        }
    }

    warn!("All JSON parsing attempts failed. Response: {}", json_str);
    vec![]
}

fn extract_from_code_block(text: &str) -> &str {
    if let Some(start) = text.find("```json") {
        if let Some(end) = text[start + 7..].find("```") {
            return text[start + 7..start + 7 + end].trim();
        }
    } else if let Some(start) = text.find("```")
        && let Some(end) = text[start + 3..].find("```")
    {
        return text[start + 3..start + 3 + end].trim();
    }
    text
}

fn extract_json_array(text: &str) -> &str {
    if let Some(start) = text.find('[')
        && let Some(end) = text.rfind(']')
        && end > start
    {
        return &text[start..=end];
    }
    "[]"
}

fn clean_response(text: &str) -> String {
    text.lines()
        .map(|line| line.trim())
        .filter(|line| !line.is_empty() && !line.starts_with("//"))
        .collect::<Vec<_>>()
        .join("")
}

/// Mock-mode dispatch by task path. Some lambdas don't send a
/// `response_schema` (e.g. /generate-intent ships only `{name, body,
/// called_intents}`), so falling through to schema-based dispatch
/// produces the wrong shape. This wrapper handles those tasks
/// explicitly before delegating to the generic schema-based mock.
fn generate_mock_for_task<B: Serialize + ?Sized>(
    task_path: &str,
    body: &B,
    mock_seed: &str,
) -> String {
    match task_path {
        "/generate-intent" => "Mock intent: function does something.".to_string(),
        "/extract-calls" => "[]".to_string(),
        _ => {
            // Tasks that send a schema (file-analyzer, framework-guidance)
            // dispatch by inspecting the schema shape. Tasks that don't but
            // happen to want the framework-detection-shaped fallback
            // (framework-detect) also land here — that's fine because the
            // default response_schema=None branch returns exactly that.
            let schema = serde_json::to_value(body)
                .ok()
                .and_then(|v| v.get("response_schema").cloned());
            generate_mock_response(&schema, mock_seed)
        }
    }
}

/// Generate mock response based on schema type
fn generate_mock_response(schema: &Option<serde_json::Value>, prompt: &str) -> String {
    match schema {
        Some(schema_val) => {
            // Check if schema is for an array
            if schema_val.get("type").and_then(|t| t.as_str()) == Some("ARRAY") {
                // Check what kind of array based on the items schema
                if let Some(items) = schema_val.get("items")
                    && let Some(props) = items.get("properties")
                {
                    // Triage schema - has location, classification, confidence
                    if props.get("classification").is_some() {
                        return generate_mock_triage_response(prompt);
                    }
                    // Endpoint schema - has method, path, handler, node_name
                    if props.get("node_name").is_some() && props.get("path").is_some() {
                        return generate_mock_endpoint_response(prompt);
                    }
                    // Consumer schema - has library, url, method
                    if props.get("library").is_some() {
                        return generate_mock_consumer_response(prompt);
                    }
                    // Mount schema - has parent_node, child_node, mount_path
                    if props.get("parent_node").is_some() && props.get("child_node").is_some() {
                        return generate_mock_mount_response(prompt);
                    }
                    // Middleware schema - has middleware_type
                    if props.get("middleware_type").is_some() {
                        return generate_mock_middleware_response(prompt);
                    }
                }
                // Default array response
                "[]".to_string()
            } else if schema_val.get("type").and_then(|t| t.as_str()) == Some("OBJECT") {
                if let Some(props) = schema_val.get("properties") {
                    // Check for file_analysis_schema - has mounts, endpoints, data_calls arrays
                    if props.get("mounts").is_some()
                        && props.get("endpoints").is_some()
                        && props.get("data_calls").is_some()
                    {
                        return generate_mock_file_analysis_response(prompt);
                    }
                    // Check for framework guidance schema - has mount_patterns, endpoint_patterns, etc.
                    if props.get("mount_patterns").is_some()
                        && props.get("endpoint_patterns").is_some()
                        && props.get("triage_hints").is_some()
                    {
                        return generate_mock_framework_guidance_response(prompt);
                    }
                    // Check for pattern_list_schema - has patterns, descriptions, frameworks arrays
                    if props.get("patterns").is_some()
                        && props.get("descriptions").is_some()
                        && props.get("frameworks").is_some()
                    {
                        return generate_mock_pattern_list_response();
                    }
                    // Check for general_guidance_schema - has triage_hints and parsing_notes
                    if props.get("triage_hints").is_some()
                        && props.get("parsing_notes").is_some()
                        && props.get("mount_patterns").is_none()
                    {
                        return generate_mock_general_guidance_response();
                    }
                }
                // Framework detection or other object schema
                r#"{"frameworks": ["express"], "data_fetchers": ["axios"], "notes": "Mock response"}"#.to_string()
            } else {
                // Framework detection or other object schema
                r#"{"frameworks": ["express"], "data_fetchers": ["axios"], "notes": "Mock response"}"#.to_string()
            }
        }
        None => {
            // No schema - return framework detection format
            r#"{"frameworks": ["express"], "data_fetchers": ["axios"], "notes": "Mock response"}"#
                .to_string()
        }
    }
}

/// Generate mock framework guidance response - returns empty structure for testing
/// The real LLM will provide actual patterns based on detected frameworks
fn generate_mock_framework_guidance_response(_prompt: &str) -> String {
    // In mock mode, return a valid but empty structure
    // The real LLM call will populate this with framework-specific patterns
    r#"{"mount_patterns":[],"endpoint_patterns":[],"middleware_patterns":[],"data_fetching_patterns":[],"triage_hints":"Mock mode - no guidance generated","parsing_notes":"Mock mode - no parsing notes"}"#.to_string()
}

/// Generate mock pattern list response for FrameworkGuidanceAgent pattern fetching
/// Returns basic patterns for common frameworks to enable testing
fn generate_mock_pattern_list_response() -> String {
    r#"{"patterns":["app.get('/path', handler)","app.post('/path', handler)","router.get('/path', handler)","app.use('/path', router)","fetch(url)","axios.get(url)"],"descriptions":["GET endpoint","POST endpoint","Router GET endpoint","Mount router","Fetch call","Axios GET"],"frameworks":["express","express","express","express","fetch","axios"]}"#.to_string()
}

/// Generate mock general guidance response for FrameworkGuidanceAgent
/// Returns empty triage hints and parsing notes
fn generate_mock_general_guidance_response() -> String {
    r#"{"triage_hints":"Mock mode - no triage hints","parsing_notes":"Mock mode - no parsing notes"}"#.to_string()
}

/// Generate mock file analysis response for FileAnalyzerAgent
/// Parses the file content from the prompt and extracts mock findings
fn generate_mock_file_analysis_response(prompt: &str) -> String {
    // Extract file path from prompt (format: "Path: path/to/file.ts")
    let file_path = prompt
        .lines()
        .find(|line| line.contains("Path:"))
        .and_then(|line| line.split("Path:").nth(1))
        .map(|s| s.trim().trim_end_matches(')'))
        .unwrap_or("unknown.ts");

    let mut candidate_by_line: HashMap<i32, (String, Option<u32>, Option<u32>)> = HashMap::new();
    let mut candidate_snippets: Vec<(String, Option<u32>, Option<u32>, String)> = Vec::new();
    for line in prompt.lines() {
        let trimmed = line.trim();
        if !trimmed.starts_with('{') || !trimmed.ends_with('}') {
            continue;
        }
        let Ok(value) = serde_json::from_str::<serde_json::Value>(trimmed) else {
            continue;
        };
        let Some(candidate_id) = value.get("candidate_id").and_then(|v| v.as_str()) else {
            continue;
        };
        let Some(line_number) = value.get("line_number").and_then(|v| v.as_i64()) else {
            continue;
        };
        let span_start = value
            .get("span_start")
            .and_then(|v| v.as_u64())
            .map(|v| v as u32);
        let span_end = value
            .get("span_end")
            .and_then(|v| v.as_u64())
            .map(|v| v as u32);
        if let Some(code_snippet) = value.get("code_snippet").and_then(|v| v.as_str()) {
            candidate_snippets.push((
                candidate_id.to_string(),
                span_start,
                span_end,
                code_snippet.to_string(),
            ));
        }
        candidate_by_line.insert(
            line_number as i32,
            (candidate_id.to_string(), span_start, span_end),
        );
    }

    // Look for common patterns in the file content to generate mock results
    let mut mounts = Vec::new();
    let mut endpoints = Vec::new();
    let mut data_calls = Vec::new();

    // Find where the actual FILE CONTENT section starts (after "### FILE CONTENT")
    // This avoids detecting patterns from the framework guidance examples
    let file_content_start = prompt
        .find("### FILE CONTENT")
        .or_else(|| prompt.find("FILE CONTENT"))
        .unwrap_or(0);

    let content_section = &prompt[file_content_start..];
    let content_to_analyze = if let Some(fence_start) = content_section.find("```") {
        let after_fence = &content_section[fence_start + 3..];
        if let Some(fence_end) = after_fence.find("```") {
            &after_fence[..fence_end]
        } else {
            after_fence
        }
    } else {
        content_section
    };

    let resolve_candidate = |line_number: i32, line_text: &str| {
        if let Some(entry) = candidate_by_line.get(&line_number) {
            return entry.clone();
        }
        let trimmed_line = line_text.trim();
        if !trimmed_line.is_empty()
            && let Some(entry) = candidate_snippets.iter().find(|(_, _, _, snippet)| {
                snippet.contains(trimmed_line) || trimmed_line.contains(snippet)
            })
        {
            return (entry.0.clone(), entry.1, entry.2);
        }
        (format!("line:{}", line_number), None, None)
    };

    // Simple pattern matching on prompt content for mock generation
    // Only look at lines that are likely actual code (not comments, not in strings)
    for (line_num, line) in content_to_analyze.lines().enumerate() {
        let line_number = (line_num + 1) as i32;
        let trimmed = line.trim();

        // Skip comments and empty lines
        if trimmed.starts_with("//") || trimmed.starts_with("*") || trimmed.is_empty() {
            continue;
        }

        // Skip lines that are clearly not endpoint definitions
        // (e.g., interface definitions, type annotations, etc.)
        if trimmed.starts_with("interface")
            || trimmed.starts_with("type ")
            || trimmed.starts_with("export type")
        {
            continue;
        }

        // Detect .use() mounts - must have a path string argument
        if (line.contains("app.use(")
            || line.contains("Router.use(")
            || line.contains("router.use(")
            || line.contains("apiRouter.use("))
            && (line.contains("\"/") || line.contains("'/"))
        {
            // Extract parent node name
            let parent = if line.contains("app.use") {
                "app"
            } else if line.contains("apiRouter.use") {
                "apiRouter"
            } else if line.contains("v1Router.use") {
                "v1Router"
            } else {
                "router"
            };

            // Try to extract the mount path
            let mount_path = extract_path_from_line(line).unwrap_or("/".to_string());

            mounts.push(serde_json::json!({
                "line_number": line_number,
                "parent_node": parent,
                "child_node": "childRouter",
                "mount_path": mount_path,
                "import_source": null,
                "pattern_matched": ".use("
            }));
        }

        // Detect endpoint patterns - must be on app/router object and have a path string
        // More specific patterns to avoid false positives
        let is_endpoint_call = (line.contains("app.get(")
            || line.contains("router.get(")
            || line.contains("v1Router.get(")
            || line.contains("apiRouter.get(")
            || line.contains("adminRouter.get("))
            && (line.contains("\"/") || line.contains("'/"));

        if is_endpoint_call {
            let owner = extract_owner_from_line(line, "get");
            let path = extract_path_from_line(line).unwrap_or("/".to_string());
            let (candidate_id, _span_start, _span_end) = resolve_candidate(line_number, line);
            endpoints.push(serde_json::json!({
                "candidate_id": candidate_id,
                "line_number": line_number,
                "owner_node": owner,
                "method": "GET",
                "path": path,
                "handler_name": "anonymous",
                "pattern_matched": ".get(",
                "payload_expression_text": null,
                "payload_expression_line": null,
                "response_expression_text": null,
                "response_expression_line": null,
                "primary_type_symbol": null,
                "type_import_source": null
            }));
        }

        let is_post_call = (line.contains("app.post(")
            || line.contains("router.post(")
            || line.contains("v1Router.post(")
            || line.contains("apiRouter.post(")
            || line.contains("adminRouter.post("))
            && (line.contains("\"/") || line.contains("'/"));

        if is_post_call {
            let owner = extract_owner_from_line(line, "post");
            let path = extract_path_from_line(line).unwrap_or("/".to_string());
            let (candidate_id, _span_start, _span_end) = resolve_candidate(line_number, line);
            endpoints.push(serde_json::json!({
                "candidate_id": candidate_id,
                "line_number": line_number,
                "owner_node": owner,
                "method": "POST",
                "path": path,
                "handler_name": "anonymous",
                "pattern_matched": ".post(",
                "payload_expression_text": null,
                "payload_expression_line": null,
                "response_expression_text": null,
                "response_expression_line": null,
                "primary_type_symbol": null,
                "type_import_source": null
            }));
        }

        // Detect DELETE endpoints
        let is_delete_call = (line.contains("app.delete(")
            || line.contains("router.delete(")
            || line.contains("v1Router.delete(")
            || line.contains("apiRouter.delete(")
            || line.contains("adminRouter.delete("))
            && (line.contains("\"/") || line.contains("'/"));

        if is_delete_call {
            let owner = extract_owner_from_line(line, "delete");
            let path = extract_path_from_line(line).unwrap_or("/".to_string());
            let (candidate_id, _span_start, _span_end) = resolve_candidate(line_number, line);
            endpoints.push(serde_json::json!({
                "candidate_id": candidate_id,
                "line_number": line_number,
                "owner_node": owner,
                "method": "DELETE",
                "path": path,
                "handler_name": "anonymous",
                "pattern_matched": ".delete(",
                "payload_expression_text": null,
                "payload_expression_line": null,
                "response_expression_text": null,
                "response_expression_line": null,
                "primary_type_symbol": null,
                "type_import_source": null
            }));
        }

        // Detect PUT endpoints
        let is_put_call = (line.contains("app.put(")
            || line.contains("router.put(")
            || line.contains("v1Router.put(")
            || line.contains("apiRouter.put(")
            || line.contains("adminRouter.put("))
            && (line.contains("\"/") || line.contains("'/"));

        if is_put_call {
            let owner = extract_owner_from_line(line, "put");
            let path = extract_path_from_line(line).unwrap_or("/".to_string());
            let (candidate_id, _span_start, _span_end) = resolve_candidate(line_number, line);
            endpoints.push(serde_json::json!({
                "candidate_id": candidate_id,
                "line_number": line_number,
                "owner_node": owner,
                "method": "PUT",
                "path": path,
                "handler_name": "anonymous",
                "pattern_matched": ".put(",
                "payload_expression_text": null,
                "payload_expression_line": null,
                "response_expression_text": null,
                "response_expression_line": null,
                "primary_type_symbol": null,
                "type_import_source": null
            }));
        }

        // Detect fetch calls - but not response.json() or similar
        if line.contains("fetch(") && !line.contains("response") && !line.contains("res.") {
            let target =
                extract_url_from_line(line).unwrap_or("https://api.example.com".to_string());
            let method = if line.contains("method:") && line.contains("POST") {
                "POST"
            } else {
                "GET"
            };
            let (candidate_id, _span_start, _span_end) = resolve_candidate(line_number, line);
            data_calls.push(serde_json::json!({
                "candidate_id": candidate_id,
                "line_number": line_number,
                "target": target,
                "method": method,
                "pattern_matched": "fetch(",
                "call_expression_text": null,
                "call_expression_line": null,
                "payload_expression_text": null,
                "payload_expression_line": null,
                "primary_type_symbol": null,
                "type_import_source": null
            }));
        }

        // Detect axios calls
        if line.contains("axios.get")
            || line.contains("axios.post")
            || line.contains("axios.put")
            || line.contains("axios.delete")
        {
            let method = if line.contains("axios.post") {
                "POST"
            } else if line.contains("axios.put") {
                "PUT"
            } else if line.contains("axios.delete") {
                "DELETE"
            } else {
                "GET"
            };
            let (candidate_id, _span_start, _span_end) = resolve_candidate(line_number, line);
            data_calls.push(serde_json::json!({
                "candidate_id": candidate_id,
                "line_number": line_number,
                "target": "https://api.example.com",
                "method": method,
                "pattern_matched": "axios.",
                "call_expression_text": null,
                "call_expression_line": null,
                "payload_expression_text": null,
                "payload_expression_line": null,
                "primary_type_symbol": null,
                "type_import_source": null
            }));
        }
    }

    // Log mock generation for debugging
    debug!(
        "Mock file analysis for {}: {} mounts, {} endpoints, {} data_calls",
        file_path,
        mounts.len(),
        endpoints.len(),
        data_calls.len()
    );

    serde_json::json!({
        "mounts": mounts,
        "endpoints": endpoints,
        "data_calls": data_calls
    })
    .to_string()
}

/// Helper to extract path from a line like: app.get("/users", handler)
fn extract_path_from_line(line: &str) -> Option<String> {
    // Try double quotes first
    if let Some(start) = line.find("\"")
        && let Some(end) = line[start + 1..].find("\"")
    {
        let path = &line[start + 1..start + 1 + end];
        if path.starts_with('/') {
            return Some(path.to_string());
        }
    }
    // Try single quotes
    if let Some(start) = line.find("'")
        && let Some(end) = line[start + 1..].find("'")
    {
        let path = &line[start + 1..start + 1 + end];
        if path.starts_with('/') {
            return Some(path.to_string());
        }
    }
    None
}

/// Helper to extract owner from a line like: router.get("/path", ...)
fn extract_owner_from_line(line: &str, method: &str) -> String {
    let pattern = format!(".{}(", method);
    if let Some(idx) = line.find(&pattern) {
        let before = &line[..idx];
        // Get the last word before the dot
        let words: Vec<&str> = before.split_whitespace().collect();
        if let Some(last) = words.last() {
            // Clean up any remaining characters
            let cleaned = last.trim_matches(|c: char| !c.is_alphanumeric() && c != '_');
            if !cleaned.is_empty() {
                return cleaned.to_string();
            }
        }
    }
    "router".to_string()
}

/// Helper to extract URL from fetch call
fn extract_url_from_line(line: &str) -> Option<String> {
    // Handle template literals and string literals
    if let Some(path) = extract_path_from_line(line) {
        return Some(path);
    }
    // Handle backtick template literals
    if let Some(start) = line.find('`')
        && let Some(end) = line[start + 1..].find('`')
    {
        return Some(line[start + 1..start + 1 + end].to_string());
    }
    None
}

/// Generate mock triage responses by extracting locations from prompt
fn generate_mock_triage_response(prompt: &str) -> String {
    let call_sites = extract_call_sites_from_prompt(prompt);

    let triage_results: Vec<serde_json::Value> = call_sites
        .iter()
        .map(|cs| {
            let location = cs.get("location").and_then(|l| l.as_str()).unwrap_or("");
            let callee_property = cs
                .get("callee_property")
                .and_then(|p| p.as_str())
                .unwrap_or("");
            let callee_object = cs
                .get("callee_object")
                .and_then(|o| o.as_str())
                .unwrap_or("");

            let args = cs.get("args").and_then(|a| a.as_array());
            let arg_count = cs
                .get("arg_count")
                .and_then(|c| c.as_u64())
                .map(|c| c as usize)
                .or_else(|| args.map(|a| a.len()))
                .unwrap_or(0);

            let has_correlated_call = cs
                .get("correlated_call")
                .map(|v| !v.is_null())
                .unwrap_or(false);

            let classification = if matches!(
                callee_property,
                "json" | "text" | "blob" | "arrayBuffer" | "formData"
            ) {
                if has_correlated_call {
                    "DataFetchingCall"
                } else {
                    "Irrelevant"
                }
            } else if callee_object == "global" && callee_property == "fetch" {
                "DataFetchingCall"
            } else if matches!(callee_property, "get" | "post" | "put" | "delete" | "patch") {
                if callee_object == "axios" || callee_object == "request" || callee_object == "http"
                {
                    "DataFetchingCall"
                } else {
                    "HttpEndpoint"
                }
            } else if callee_property == "use" {
                if arg_count >= 2 {
                    let first_is_string = cs
                        .get("first_arg_type")
                        .and_then(|t| t.as_str())
                        .map(|t| t == "StringLiteral")
                        .or_else(|| {
                            args.and_then(|a| a.first())
                                .and_then(|arg| arg.get("arg_type"))
                                .and_then(|t| t.as_str())
                                .map(|t| t == "StringLiteral")
                        })
                        .unwrap_or(false);

                    // For LeanCallSite we don't have second arg info, so we assume RouterMount
                    // if first arg is string and arg_count >= 2.
                    // For full CallSite we check second arg is Identifier.
                    let second_is_id = args
                        .and_then(|a| a.get(1))
                        .and_then(|arg| arg.get("arg_type"))
                        .and_then(|t| t.as_str())
                        == Some("Identifier");

                    if first_is_string && (args.is_none() || second_is_id) {
                        "RouterMount"
                    } else {
                        "Middleware"
                    }
                } else {
                    "Middleware"
                }
            } else if arg_count >= 2 {
                let first_is_id = args
                    .and_then(|a| a.first())
                    .and_then(|arg| arg.get("arg_type"))
                    .and_then(|t| t.as_str())
                    == Some("Identifier");

                let second_is_object = args
                    .and_then(|a| a.get(1))
                    .and_then(|arg| arg.get("arg_type"))
                    .and_then(|t| t.as_str())
                    == Some("ObjectLiteral");

                if first_is_id && second_is_object {
                    let context_slice = cs
                        .get("context_slice")
                        .and_then(|v| v.as_str())
                        .unwrap_or("");

                    if extract_path_prefix_from_context_slice(context_slice).is_some() {
                        "RouterMount"
                    } else {
                        "Irrelevant"
                    }
                } else {
                    "Irrelevant"
                }
            } else {
                "Irrelevant"
            };

            serde_json::json!({
                "location": location,
                "classification": classification,
                "confidence": 0.9
            })
        })
        .collect();

    serde_json::to_string(&triage_results).unwrap_or_else(|_| "[]".to_string())
}

/// Generate mock endpoint responses
fn generate_mock_endpoint_response(prompt: &str) -> String {
    let call_sites = extract_call_sites_from_prompt(prompt);
    let endpoints: Vec<serde_json::Value> = call_sites
        .iter()
        .filter_map(|cs| {
            let callee_property = cs
                .get("callee_property")
                .and_then(|p| p.as_str())
                .unwrap_or("");
            let callee_object = cs
                .get("callee_object")
                .and_then(|o| o.as_str())
                .unwrap_or("app");
            let location = cs.get("location").and_then(|l| l.as_str()).unwrap_or("");

            let raw_path = cs
                .get("args")
                .and_then(|args| args.as_array())
                .and_then(|arr| arr.first())
                .and_then(|arg| arg.get("resolved_value").or_else(|| arg.get("value")))
                .and_then(|v| v.as_str())
                .unwrap_or("/");

            let context_slice = cs
                .get("context_slice")
                .and_then(|v| v.as_str())
                .unwrap_or("");

            let inferred_prefix = if !context_slice.is_empty()
                && context_slice.contains(callee_object)
                && context_slice.contains("prefix")
            {
                extract_path_prefix_from_context_slice(context_slice)
            } else {
                None
            };

            let path = if let Some(prefix) = inferred_prefix {
                join_path_prefix(&prefix, raw_path)
            } else {
                raw_path.to_string()
            };

            if matches!(callee_property, "get" | "post" | "put" | "delete" | "patch") {
                Some(serde_json::json!({
                    "method": callee_property.to_uppercase(),
                    "path": path,
                    "handler": "handler",
                    "node_name": callee_object,
                    "location": location,
                    "confidence": 0.9,
                    "reasoning": "Mock endpoint extraction"
                }))
            } else {
                None
            }
        })
        .collect();

    serde_json::to_string(&endpoints).unwrap_or_else(|_| "[]".to_string())
}

/// Generate mock consumer (data fetching) responses
fn generate_mock_consumer_response(prompt: &str) -> String {
    let call_sites = extract_call_sites_from_prompt(prompt);

    let consumers: Vec<serde_json::Value> = call_sites
        .iter()
        .map(|cs| {
            let callee_property = cs
                .get("callee_property")
                .and_then(|p| p.as_str())
                .unwrap_or("");
            let callee_object = cs
                .get("callee_object")
                .and_then(|o| o.as_str())
                .unwrap_or("");
            let location = cs.get("location").and_then(|l| l.as_str()).unwrap_or("");

            let correlated = cs.get("correlated_call");
            let correlated_callee = correlated
                .and_then(|c| c.get("callee"))
                .and_then(|v| v.as_str());
            let correlated_url = correlated
                .and_then(|c| c.get("url"))
                .and_then(|v| v.as_str());
            let correlated_method = correlated
                .and_then(|c| c.get("method"))
                .and_then(|v| v.as_str());

            let args = cs.get("args").and_then(|a| a.as_array());
            let arg0_value = args
                .and_then(|a| a.first())
                .and_then(|arg| arg.get("resolved_value").or_else(|| arg.get("value")))
                .and_then(|v| v.as_str());

            let url: Option<String> = correlated_url
                .or(arg0_value)
                .map(|s| s.to_string())
                .filter(|s| !s.is_empty());

            let method: Option<String> =
                correlated_method
                    .map(|s| s.to_string())
                    .or_else(|| match callee_property {
                        "get" | "post" | "put" | "delete" | "patch" => {
                            Some(callee_property.to_uppercase())
                        }
                        _ => None,
                    });

            let is_decode_call = matches!(
                callee_property,
                "json" | "text" | "blob" | "arrayBuffer" | "formData"
            ) && args.map(|a| a.is_empty()).unwrap_or(false);

            let library = if is_decode_call {
                correlated_callee
                    .map(|s| s.to_string())
                    .unwrap_or_else(|| "response_parsing".to_string())
            } else if callee_object == "global" {
                callee_property.to_string()
            } else if let Some(callee) = correlated_callee {
                callee.to_string()
            } else {
                callee_object.to_string()
            };

            serde_json::json!({
                "library": library,
                "url": url,
                "method": method,
                "location": location,
                "confidence": 0.8,
                "reasoning": "Mock data fetching call"
            })
        })
        .collect();

    serde_json::to_string(&consumers).unwrap_or_else(|_| "[]".to_string())
}

/// Generate mock mount relationship responses
fn generate_mock_mount_response(prompt: &str) -> String {
    let call_sites = extract_call_sites_from_prompt(prompt);
    let mounts: Vec<serde_json::Value> = call_sites
        .iter()
        .filter_map(|cs| {
            let callee_property = cs
                .get("callee_property")
                .and_then(|p| p.as_str())
                .unwrap_or("");
            let callee_object = cs
                .get("callee_object")
                .and_then(|o| o.as_str())
                .unwrap_or("app");
            let location = cs.get("location").and_then(|l| l.as_str()).unwrap_or("");

            let args = cs.get("args").and_then(|a| a.as_array());

            if args.map(|a| a.len()).unwrap_or(0) >= 2 {
                let first_arg_type = args
                    .and_then(|a| a.first())
                    .and_then(|arg| arg.get("arg_type"))
                    .and_then(|t| t.as_str());

                let second_arg_type = args
                    .and_then(|a| a.get(1))
                    .and_then(|arg| arg.get("arg_type"))
                    .and_then(|t| t.as_str());

                if callee_property == "use"
                    && first_arg_type == Some("StringLiteral")
                    && second_arg_type == Some("Identifier")
                {
                    let path = args
                        .and_then(|a| a.first())
                        .and_then(|arg| arg.get("resolved_value").or_else(|| arg.get("value")))
                        .and_then(|v| v.as_str())
                        .unwrap_or("/");
                    let child = args
                        .and_then(|a| a.get(1))
                        .and_then(|arg| arg.get("value"))
                        .and_then(|v| v.as_str())
                        .unwrap_or("router");

                    return Some(serde_json::json!({
                        "parent_node": callee_object,
                        "child_node": child,
                        "mount_path": path,
                        "location": location,
                        "confidence": 0.9,
                        "reasoning": "Mock mount extraction"
                    }));
                }

                if first_arg_type == Some("Identifier") && second_arg_type == Some("ObjectLiteral")
                {
                    let child = args
                        .and_then(|a| a.first())
                        .and_then(|arg| arg.get("value"))
                        .and_then(|v| v.as_str())
                        .unwrap_or("router");

                    let context_slice = cs
                        .get("context_slice")
                        .and_then(|v| v.as_str())
                        .unwrap_or("");

                    if let Some(prefix) = extract_path_prefix_from_context_slice(context_slice) {
                        return Some(serde_json::json!({
                            "parent_node": callee_object,
                            "child_node": child,
                            "mount_path": prefix,
                            "location": location,
                            "confidence": 0.9,
                            "reasoning": "Mock mount extraction"
                        }));
                    }
                }
            }

            None
        })
        .collect();

    serde_json::to_string(&mounts).unwrap_or_else(|_| "[]".to_string())
}

/// Generate mock middleware responses
fn generate_mock_middleware_response(prompt: &str) -> String {
    let call_sites = extract_call_sites_from_prompt(prompt);
    let middleware: Vec<serde_json::Value> = call_sites
        .iter()
        .map(|cs| {
            let callee_property = cs
                .get("callee_property")
                .and_then(|p| p.as_str())
                .unwrap_or("");
            let callee_object = cs
                .get("callee_object")
                .and_then(|o| o.as_str())
                .unwrap_or("app");
            let location = cs.get("location").and_then(|l| l.as_str()).unwrap_or("");

            serde_json::json!({
                "middleware_type": "custom",
                "path_prefix": null,
                "handler": callee_property,
                "node_name": callee_object,
                "location": location,
                "confidence": 0.8,
                "reasoning": "Mock middleware"
            })
        })
        .collect();

    serde_json::to_string(&middleware).unwrap_or_else(|_| "[]".to_string())
}

fn extract_path_prefix_from_context_slice(context_slice: &str) -> Option<String> {
    extract_string_literal_after_key(context_slice, "prefix")
        .or_else(|| extract_string_literal_after_key(context_slice, "basePath"))
        .or_else(|| extract_string_literal_after_key(context_slice, "base_path"))
        .or_else(|| extract_string_literal_after_key(context_slice, "pathPrefix"))
        .or_else(|| extract_string_literal_after_key(context_slice, "path_prefix"))
        .filter(|v| v.starts_with('/'))
        .map(|v| v.to_string())
}

fn extract_string_literal_after_key(haystack: &str, key: &str) -> Option<String> {
    let hay = haystack.as_bytes();
    let key_bytes = key.as_bytes();
    let mut i = 0;

    while i + key_bytes.len() <= hay.len() {
        if &hay[i..i + key_bytes.len()] == key_bytes {
            let mut j = i + key_bytes.len();

            while j < hay.len() && hay[j].is_ascii_whitespace() {
                j += 1;
            }

            if j >= hay.len() || (hay[j] != b':' && hay[j] != b'=') {
                i += key_bytes.len();
                continue;
            }

            j += 1;
            while j < hay.len() && hay[j].is_ascii_whitespace() {
                j += 1;
            }

            if j >= hay.len() || (hay[j] != b'\'' && hay[j] != b'"') {
                i += key_bytes.len();
                continue;
            }

            let quote = hay[j];
            j += 1;
            let start_val = j;

            while j < hay.len() && hay[j] != quote {
                j += 1;
            }

            if j >= hay.len() {
                return None;
            }

            let value = String::from_utf8_lossy(&hay[start_val..j]).to_string();
            if !value.is_empty() {
                return Some(value);
            }
        }

        i += 1;
    }

    None
}

fn join_path_prefix(prefix: &str, path: &str) -> String {
    let normalized_prefix = prefix.trim_end_matches('/');
    let normalized_path = path.trim_start_matches('/');

    if normalized_prefix.is_empty() {
        format!("/{}", normalized_path)
    } else if normalized_path.is_empty() {
        normalized_prefix.to_string()
    } else {
        format!("{}/{}", normalized_prefix, normalized_path)
    }
}

/// Helper function to extract call sites from prompt JSON
fn extract_call_sites_from_prompt(prompt: &str) -> Vec<serde_json::Value> {
    // Try multiple search patterns for compact and pretty-printed JSON
    let patterns = [
        "[{\"callee_object\"",           // Compact JSON
        "[\n  {\n    \"callee_object\"", // Pretty-printed JSON
        "[\n  {\n   \"callee_object\"",  // Alternative indentation
    ];

    for pattern in &patterns {
        if let Some(start) = prompt.find(pattern) {
            // Find matching closing bracket
            if let Some(end_offset) = find_matching_bracket(&prompt[start..]) {
                let json_str = &prompt[start..start + end_offset];
                if let Ok(parsed) = serde_json::from_str::<Vec<serde_json::Value>>(json_str) {
                    return parsed;
                }
            }
        }
    }

    // Fallback: iterate through all JSON arrays to find one that looks like call sites
    // This handles cases where LeanCallSite serialization might differ slightly
    // and avoids picking up other arrays (like frameworks list)
    let mut current_pos = 0;
    while let Some(start) = prompt[current_pos..].find('[') {
        let abs_start = current_pos + start;
        if let Some(end_offset) = find_matching_bracket(&prompt[abs_start..]) {
            let json_str = &prompt[abs_start..abs_start + end_offset];
            if let Ok(parsed) = serde_json::from_str::<Vec<serde_json::Value>>(json_str)
                && !parsed.is_empty()
                && parsed[0].get("callee_object").is_some()
                && parsed[0].get("location").is_some()
            {
                return parsed;
            }
        }
        current_pos = abs_start + 1;
    }

    vec![]
}

/// Find the matching closing bracket for a JSON array
fn find_matching_bracket(s: &str) -> Option<usize> {
    let mut depth = 0;
    let mut in_string = false;
    let mut escape_next = false;

    for (i, ch) in s.char_indices() {
        if escape_next {
            escape_next = false;
            continue;
        }

        match ch {
            '\\' if in_string => escape_next = true,
            '"' => in_string = !in_string,
            '[' if !in_string => depth += 1,
            ']' if !in_string => {
                depth -= 1;
                if depth == 0 {
                    return Some(i + 1);
                }
            }
            _ => {}
        }
    }
    None
}
