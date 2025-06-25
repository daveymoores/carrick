use crate::visitor::{Call, Json};
use genai::Client;
use genai::chat::{ChatMessage, ChatRequest};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Serialize)]
pub struct AsyncCallContext {
    pub kind: String,
    pub callee: String,
    pub arguments: Vec<String>,
    pub file: String,
    pub line: u32,
    pub source_code: String,
}

#[derive(Debug, Deserialize)]
pub struct GeminiCallResponse {
    route: String,
    method: String,
    request_body: Option<serde_json::Value>,
    has_response_type: bool,
}

pub async fn extract_calls_from_async_expressions(async_calls: Vec<AsyncCallContext>) -> Vec<Call> {
    if async_calls.is_empty() {
        return vec![];
    }

    let client = Client::default();
    let prompt = create_extraction_prompt(&async_calls);

    let chat_req = ChatRequest::new(vec![
        ChatMessage::system(
            r#"You are an expert at analyzing JavaScript/TypeScript async calls.
Extract HTTP details and return ONLY a valid JSON array.
Response format: Start with [ and end with ]. No markdown, no explanation, just JSON.
Each object must have: route (string), method (string), request_body (object or null), has_response_type (boolean)."#,
        ),
        ChatMessage::user(prompt),
    ]);

    match client
        .exec_chat("gemini-2.0-flash-exp", chat_req, None)
        .await
    {
        Ok(response) => {
            let response_text = response.content_text_as_str().unwrap_or("");
            parse_gemini_response(response_text, &async_calls)
        }
        Err(e) => {
            eprintln!("Gemini API call failed: {}", e);
            vec![]
        }
    }
}

fn create_extraction_prompt(async_calls: &[AsyncCallContext]) -> String {
    let calls_json = serde_json::to_string_pretty(async_calls).unwrap_or_default();

    format!(
        r#"Extract HTTP calls from these async expressions:

{}

Rules:
- Only HTTP requests (fetch, axios, request libs) - ignore setTimeout, file ops, etc.
- Extract: route (URL path), method (HTTP verb), request_body (JSON or null), has_response_type (boolean)
- For template literals: resolve to actual paths
- For env vars: use format "ENV_VAR:VARIABLE_NAME"

Output JSON array only:
[{{"route":"/api/users","method":"GET","request_body":null,"has_response_type":false}}]"#,
        calls_json
    )
}

fn convert_gemini_responses_to_calls(
    gemini_calls: Vec<GeminiCallResponse>,
    contexts: &[AsyncCallContext],
) -> Vec<Call> {
    gemini_calls
        .into_iter()
        .enumerate()
        .map(|(i, gc)| {
            let file_path = contexts
                .get(i)
                .map(|c| PathBuf::from(&c.file))
                .unwrap_or_default();

            Call {
                route: gc.route,
                method: gc.method.to_uppercase(),
                response: Json::Null,
                request: gc.request_body.and_then(|v| serde_json::from_value(v).ok()),
                response_type: None,
                request_type: None,
                call_file: file_path,
                call_id: None,
                call_number: None,
                common_type_name: None,
            }
        })
        .collect()
}

fn parse_gemini_response(response: &str, contexts: &[AsyncCallContext]) -> Vec<Call> {
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
        if let Ok(gemini_calls) = serde_json::from_str::<Vec<GeminiCallResponse>>(attempt) {
            return convert_gemini_responses_to_calls(gemini_calls, contexts);
        }
    }

    eprintln!("All JSON parsing attempts failed. Response: {}", json_str);
    vec![]
}

fn extract_from_code_block(text: &str) -> &str {
    if let Some(start) = text.find("```json") {
        if let Some(end) = text[start + 7..].find("```") {
            return text[start + 7..start + 7 + end].trim();
        }
    } else if let Some(start) = text.find("```") {
        if let Some(end) = text[start + 3..].find("```") {
            return text[start + 3..start + 3 + end].trim();
        }
    }
    text
}

fn extract_json_array(text: &str) -> &str {
    if let Some(start) = text.find('[') {
        if let Some(end) = text.rfind(']') {
            if end > start {
                return &text[start..=end];
            }
        }
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
