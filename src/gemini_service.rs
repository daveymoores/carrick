use crate::visitor::{Call, Json};
use genai::Client;
use genai::chat::{ChatMessage, ChatRequest};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Serialize)]
pub struct AsyncCallContext {
    pub kind: String,
    pub function_source: String,
    pub file: String,
    pub line: u32,
    pub function_name: String,
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
            r#"You are an expert at analyzing JavaScript/TypeScript async calls for API route extraction.

CRITICAL REQUIREMENTS:
1. Extract ONLY HTTP requests (fetch, axios, request libraries) - ignore setTimeout, file I/O, database calls
2. Return ONLY valid JSON array starting with [ and ending with ]
3. Each object must have: route (string), method (string), request_body (object or null), has_response_type (boolean)

ENVIRONMENT VARIABLE HANDLING:
- Format: "ENV_VAR:VARIABLE_NAME:path"
- process.env.API_URL + "/users" → "ENV_VAR:API_URL:/users"
- `${process.env.BASE_URL}/api/data` → "ENV_VAR:BASE_URL:/api/data"
- env.SERVICE_URL + "/health" → "ENV_VAR:SERVICE_URL:/health"

TEMPLATE LITERAL HANDLING:
- Keep variable placeholders: `/users/${userId}` → "/users/${userId}"
- Resolve when possible: `${baseUrl}/api` → "${baseUrl}/api"

URL CONSTRUCTION:
- String concatenation: "/api" + "/users" → "/api/users"
- Mixed env vars: process.env.API + "/v1" + path → "ENV_VAR:API:/v1" + path

NO MARKDOWN, NO EXPLANATIONS - ONLY JSON ARRAY."#,
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
        r#"Extract HTTP API calls from these JavaScript/TypeScript functions. Return ONLY a JSON array.

FUNCTIONS TO ANALYZE:
{}

EXTRACTION RULES:
1. Find HTTP calls: fetch(), axios.get/post/put/delete(), request(), etc.
2. Ignore: setTimeout, file operations, database calls, console.log
3. Extract exact route, method, request body

ENVIRONMENT VARIABLE FORMAT:
- process.env.API_URL + "/users" → "ENV_VAR:API_URL:/users"
- process.env.BASE + "/api" + path → "ENV_VAR:BASE:/api" + path
- `${{process.env.SERVICE}}/endpoint` → "ENV_VAR:SERVICE:/endpoint"
- CONSTANT_VAR + "/path" → "ENV_VAR:CONSTANT_VAR:/path"

TEMPLATE LITERALS:
- `/users/${{id}}` → "/users/${{id}}"
- `${{base}}/api` → "${{base}}/api"

STRING CONCATENATION:
- "/api" + "/users" → "/api/users"
- path + "/" + id → path + "/${{id}}"

EXAMPLES:
```js
fetch(process.env.API_URL + "/users")
// → {{"route":"ENV_VAR:API_URL:/users","method":"GET","request_body":null,"has_response_type":false}}

axios.post("/api/users", userData)
// → {{"route":"/api/users","method":"POST","request_body":{{"userData":"placeholder"}},"has_response_type":false}}
```

OUTPUT JSON ARRAY ONLY:"#,
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
