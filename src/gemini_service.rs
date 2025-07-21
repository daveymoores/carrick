use crate::visitor::{Call, Json, TypeReference};
use reqwest::Client;
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

#[derive(Debug, Serialize)]
struct ProxyRequest {
    messages: Vec<ProxyMessage>,
    options: ProxyOptions,
}

#[derive(Debug, Serialize)]
struct ProxyMessage {
    role: String,
    content: String,
}

#[derive(Debug, Serialize)]
struct ProxyOptions {
    #[serde(skip_serializing_if = "Option::is_none")]
    temperature: Option<f32>,
    #[serde(rename = "maxOutputTokens", skip_serializing_if = "Option::is_none")]
    max_output_tokens: Option<u32>,
}

#[derive(Debug, Deserialize)]
struct ProxyResponse {
    success: bool,
    text: String,
}

pub async fn extract_calls_from_async_expressions(
    async_calls: Vec<AsyncCallContext>,
) -> Result<Vec<Call>, Box<dyn std::error::Error>> {
    // If CARRICK_MOCK_ALL is set, bypass the actual API call for testing purposes
    if env::var("CARRICK_MOCK_ALL").is_ok() {
        return Ok(vec![]);
    }

    // Emergency disable option for Gemini API
    if env::var("DISABLE_GEMINI").is_ok() {
        println!("Gemini API disabled via DISABLE_GEMINI environment variable");
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
        eprintln!(
            "Warning: Large amount of source code to analyze ({:.1}KB total). This may result in high token usage.",
            total_size as f64 / 1024.0
        );
    }

    println!(
        "Found {} async expressions, sending to Gemini Flash 2.5...",
        async_calls.len()
    );

    // Get proxy endpoint from CARRICK_API_ENDPOINT (compile-time)
    let api_base = env!("CARRICK_API_ENDPOINT");
    let proxy_endpoint = format!("{}/gemini/chat", api_base);

    let client = Client::new();
    let prompt = create_extraction_prompt(&async_calls);

    let system_message = r#"You are an expert at analyzing JavaScript/TypeScript async calls for API route extraction and TypeScript type extraction.

CRITICAL REQUIREMENTS:
1. Extract ONLY HTTP requests (fetch, axios, request libraries) - ignore setTimeout, file I/O, database calls
2. Return ONLY valid JSON array starting with [ and ending with ]
3. Each object must have: route (string), method (string), request_body (object or null), has_response_type (boolean), request_type_info (object or null), response_type_info (object or null)

IMPORTANT: When analyzing Express route handlers, IGNORE the types of the handler parameters (such as req: Request<T>, res: Response<T>). These describe incoming HTTP requests to the server, NOT outgoing HTTP calls made by the server.
When extracting HTTP calls (fetch, axios, etc.), infer the request and response types from the data passed to the HTTP call and the expected result, NOT from the Express handler signature.

TYPE EXTRACTION REQUIREMENTS:
4. For outgoing HTTP calls (fetch, axios, etc.):
   a. For the response type, extract ONLY ONE type per unique HTTP call. Extract the type that is assigned directly to the result of the HTTP call (such as `const data: MyType = await response.json();`). DO NOT extract types from variables that are filtered, mapped, type-checked, or otherwise transformed versions of the raw response. Ignore all intermediate or final variables that are transformations of the original response; focus ONLY on the type as it is received directly from the HTTP call.

   CRITICAL:
   - If the result of the HTTP call is assigned to multiple variables, extract ONLY the type from the variable that is assigned the result of `await response.json()` (or equivalent), NOT from variables that are filtered, mapped, or type-checked versions of the response.
   - DO NOT extract multiple types for the same HTTP call, even if the result is assigned to multiple variables.
   - If multiple assignments are made from the same HTTP call, extract only the first assignment (the one closest to the HTTP call).

   BAD EXAMPLE:
     const raw: Foo[] = await resp.json();
     const filtered: Foo[] = filter(raw);
     // Only extract Foo[], not both.

   BAD EXAMPLE:
     const a: Bar[] = await resp.json();
     const b: Bar[] = a.filter(...);
     // Only extract Bar[], not both.

   GOOD EXAMPLE:
     const commentsRaw: {{ id: string; order_id: string }}[] = await commentsResp.json();
     const comments: {{ id: string; order_id: string }}[] = isCommentArray(commentsRaw) ? commentsRaw : [];
     // Only extract {{ id: string; order_id: string }}[] for this HTTP call.
   b. For the request type, use the type of the data passed as the request body or parameters in the HTTP call.
5. NEVER use Express handler parameter types (e.g., req: Request<T>, res: Response<T>) for outgoing HTTP calls—these describe incoming server requests, not outgoing client requests.
6. Calculate approximate character position where the type appears in the source
7. Generate meaningful alias names following pattern: MethodRouteRequest/Response (e.g., "GetUsersResponse", "PostUserRequest")

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

NO MARKDOWN, NO EXPLANATIONS - ONLY JSON ARRAY."#;

    let proxy_request = ProxyRequest {
        messages: vec![
            ProxyMessage {
                role: "system".to_string(),
                content: system_message.to_string(),
            },
            ProxyMessage {
                role: "user".to_string(),
                content: prompt,
            },
        ],
        options: ProxyOptions {
            temperature: None,       // Use Gemini defaults
            max_output_tokens: None, // Use Gemini defaults
        },
    };

    let mut request_builder = client
        .post(&proxy_endpoint)
        .json(&proxy_request)
        .timeout(std::time::Duration::from_secs(60));

    // Add API key if available (for authentication with proxy)
    if let Ok(api_key) = env::var("CARRICK_API_KEY") {
        request_builder = request_builder.header("Authorization", format!("Bearer {}", api_key));
    }

    match request_builder.send().await {
        Ok(response) => {
            if response.status().is_success() {
                match response.json::<ProxyResponse>().await {
                    Ok(proxy_response) => {
                        if proxy_response.success {
                            println!(
                                "Gemini proxy call successful. Processing {} async expressions.",
                                async_calls.len()
                            );
                            Ok(parse_gemini_response(&proxy_response.text, &async_calls))
                        } else {
                            eprintln!("Gemini proxy returned unsuccessful response");
                            Ok(vec![])
                        }
                    }
                    Err(e) => {
                        eprintln!("Failed to parse proxy response: {}", e);
                        Ok(vec![])
                    }
                }
            } else {
                let status = response.status();
                match response.text().await {
                    Ok(error_text) => {
                        eprintln!(
                            "Gemini proxy call failed with status {}: {}",
                            status, error_text
                        );
                        if status == 429 {
                            eprintln!(
                                "Rate limit exceeded. Consider deploying your own proxy or try again later."
                            );
                        }
                    }
                    Err(_) => {
                        eprintln!("Gemini proxy call failed with status {}", status);
                    }
                }
                Ok(vec![])
            }
        }
        Err(e) => {
            eprintln!("Gemini proxy call failed: {}", e);
            eprintln!("Continuing analysis without AI-extracted calls...");
            Ok(vec![])
        }
    }
}

fn create_extraction_prompt(async_calls: &[AsyncCallContext]) -> String {
    let calls_json = serde_json::to_string_pretty(async_calls).unwrap_or_default();

    format!(
        r#"Extract HTTP API calls from these JavaScript/TypeScript functions. Return ONLY a JSON array.

IMPORTANT: When analyzing outgoing HTTP calls (fetch, axios, etc.) inside Express route handlers:
- For the response type:
   - Extract ONLY ONE type per unique HTTP call. Extract the type that is assigned directly to the result of the HTTP call (such as `const data: MyType = await response.json();`). DO NOT extract types from variables that are filtered, mapped, type-checked, or otherwise transformed versions of the raw response. Ignore all intermediate or final variables that are transformations of the original response; focus ONLY on the type as it is received directly from the HTTP call.

   CRITICAL:
   - If the result of the HTTP call is assigned to multiple variables, extract ONLY the type from the variable that is assigned the result of `await response.json()` (or equivalent), NOT from variables that are filtered, mapped, or type-checked versions of the response.
   - DO NOT extract multiple types for the same HTTP call, even if the result is assigned to multiple variables.
   - If multiple assignments are made from the same HTTP call, extract only the first assignment (the one closest to the HTTP call).

   BAD EXAMPLE:
     const raw: Foo[] = await resp.json();
     const filtered: Foo[] = filter(raw);
     // Only extract Foo[], not both.

   BAD EXAMPLE:
     const a: Bar[] = await resp.json();
     const b: Bar[] = a.filter(...);
     // Only extract Bar[], not both.

   GOOD EXAMPLE:
     const commentsRaw: {{ id: string; order_id: string }}[] = await commentsResp.json();
     const comments: {{ id: string; order_id: string }}[] = isCommentArray(commentsRaw) ? commentsRaw : [];
     // Only extract {{ id: string; order_id: string }}[] for this HTTP call.
- For the request type, use the type of the data passed as the request body or parameters in the HTTP call.
- NEVER use Express handler parameter types (e.g., req: Request<T>, res: Response<T>) for outgoing HTTP calls—these describe incoming server requests, not outgoing client requests.

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
