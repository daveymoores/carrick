use crate::visitor::{Call, Json, TypeReference};
use genai::Client;
use genai::chat::{ChatMessage, ChatRequest};
use serde::{Deserialize, Serialize};
use std::env;
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

pub async fn extract_calls_from_async_expressions(async_calls: Vec<AsyncCallContext>) -> Vec<Call> {
    // If CARRICK_MOCK_ALL is set, bypass the actual API call for testing purposes
    if env::var("CARRICK_MOCK_ALL").is_ok() {
        return vec![];
    }

    if async_calls.is_empty() {
        return vec![];
    }

    let client = Client::default();
    let prompt = create_extraction_prompt(&async_calls);

    let chat_req = ChatRequest::new(vec![
        ChatMessage::system(
            r#"You are an expert at analyzing JavaScript/TypeScript async calls for API route extraction and TypeScript type extraction.

CRITICAL REQUIREMENTS:
1. Extract ONLY HTTP requests (fetch, axios, request libraries) - ignore setTimeout, file I/O, database calls
2. Return ONLY valid JSON array starting with [ and ending with ]
3. Each object must have: route (string), method (string), request_body (object or null), has_response_type (boolean), request_type_info (object or null), response_type_info (object or null)

TYPE EXTRACTION REQUIREMENTS:
4. Look for TypeScript type annotations in function parameters and return types
5. Extract request types from first parameter annotations (req: RequestType, request: SomeType, etc.)
6. Extract response types from return type annotations or response variable types
7. Calculate approximate character position where the type appears in the source
8. Generate meaningful alias names following pattern: MethodRouteRequest/Response (e.g., "GetUsersResponse", "PostUserRequest")

TYPE INFO OBJECT FORMAT:
- file_path: The source file path
- start_position: Approximate character position of type annotation (number)
- composite_type_string: Full type string (e.g., "Response<User[]>", "CreateUserRequest")
- alias: Generated alias name (e.g., "GetUsersResponse", "PostUserRequest")

ENVIRONMENT VARIABLE HANDLING:
- Format: "ENV_VAR:VARIABLE_NAME:path"
- process.env.API_URL + "/users" → "ENV_VAR:API_URL:/users"
- `${process.env.BASE_URL}/api/data` → "ENV_VAR:BASE_URL:/api/data"
- env.SERVICE_URL + "/health" → "ENV_VAR:SERVICE_URL:/health"

TEMPLATE LITERAL HANDLING:
- Convert ALL template literals to :id: `/users/${userId}` → "/users/:id"
- Convert ALL template literals to :id: `/users/${user_id}` → "/users/:id"
- Convert ALL template literals to :id: `/orders/${orderId}/items/${itemId}` → "/orders/:id/items/:id"
- Remove query parameters from paths: `/orders?userId=${userId}` → "/orders"

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
4. Extract TypeScript type annotations from function parameters and return types

TYPE EXTRACTION RULES:
- Look for function parameter types: (req: RequestType) → extract "RequestType"
- Look for return type annotations: ): Promise<ResponseType> → extract "ResponseType"
- Look for response variable types: const result: UserData = await...
- Calculate character position by counting from start of function
- Generate aliases: GET /users → "GetUsersResponse", POST /users → "PostUsersRequest"

ENVIRONMENT VARIABLE FORMAT:
- process.env.API_URL + "/users" → "ENV_VAR:API_URL:/users"
- process.env.BASE + "/api" + path → "ENV_VAR:BASE:/api" + path
- `${{process.env.SERVICE}}/endpoint` → "ENV_VAR:SERVICE:/endpoint"
- CONSTANT_VAR + "/path" → "ENV_VAR:CONSTANT_VAR:/path"

TEMPLATE LITERALS:
- `/users/${{id}}` → "/users/:id"
- `/users/${{userId}}` → "/users/:id"
- `/orders/${{orderId}}` → "/orders/:id"
- `/api/comments?userId=${{userId}}` → "/api/comments"
- `${{base}}/users/${{id}}` → "ENV_VAR:base:/users/:id"

STRING CONCATENATION:
- "/api" + "/users" → "/api/users"
- path + "/" + id → path + "/${{id}}"

EXAMPLES:
```
async function getUsers(req: GetUsersRequest): Promise<GetUsersResponse> {{{{
  return fetch(process.env.API_URL + "/users")
}}}}
// → {{{{"route":"ENV_VAR:API_URL:/users","method":"GET","request_body":null,"has_response_type":true,"request_type_info":{{{{"file_path":"example.ts","start_position":25,"composite_type_string":"GetUsersRequest","alias":"GetUsersRequest"}}}}, "response_type_info":{{{{"file_path":"example.ts","start_position":50,"composite_type_string":"GetUsersResponse","alias":"GetUsersResponse"}}}}}}}}

fetch(\`/users/${{{{userId}}}}/comments\`)
// → {{{{"route":"/users/:id/comments","method":"GET","request_body":null,"has_response_type":false,"request_type_info":null,"response_type_info":null}}}}

axios.post("/api/users", userData)
// → {{{{"route":"/api/users","method":"POST","request_body":{{{{"userData":"placeholder"}}}},"has_response_type":false,"request_type_info":null,"response_type_info":null}}}}
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

            // Convert TypeInfo to TypeReference if present
            let request_type = gc.request_type_info.map(|type_info| {
                TypeReference {
                    file_path: PathBuf::from(type_info.file_path),
                    type_ann: None, // We don't have SWC AST node from Gemini
                    start_position: type_info.start_position as usize,
                    composite_type_string: type_info.composite_type_string,
                    alias: type_info.alias,
                }
            });

            let response_type = gc.response_type_info.map(|type_info| {
                TypeReference {
                    file_path: PathBuf::from(type_info.file_path),
                    type_ann: None, // We don't have SWC AST node from Gemini
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
