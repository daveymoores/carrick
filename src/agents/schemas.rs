use serde_json::{Value, json};

/// Agent schema types for structured output
/// These correspond to the Type constants from the LLM provider
#[allow(dead_code)]
pub struct AgentType;

#[allow(dead_code)]
impl AgentType {
    pub const ARRAY: &'static str = "ARRAY";
    pub const OBJECT: &'static str = "OBJECT";
    pub const STRING: &'static str = "STRING";
    pub const NUMBER: &'static str = "NUMBER";
    pub const BOOLEAN: &'static str = "BOOLEAN";
}

/// Agent-format schemas for structured output from each agent
pub struct AgentSchemas;

#[allow(dead_code)]
impl AgentSchemas {
    /// Schema for TriageAgent output - array of TriageResult
    pub fn triage_schema() -> Value {
        json!({
            "type": "ARRAY",
            "items": {
                "type": "OBJECT",
                "properties": {
                    "location": {
                        "type": "STRING",
                        "description": "File location in format file:line:column"
                    },
                    "classification": {
                        "type": "STRING",
                        "enum": ["HttpEndpoint", "DataFetchingCall", "Middleware", "RouterMount", "Irrelevant"],
                        "description": "Classification category"
                    },
                    "confidence": {
                        "type": "NUMBER",
                        "minimum": 0.0,
                        "maximum": 1.0,
                        "description": "Confidence score between 0 and 1"
                    }
                },
                "required": ["location", "classification", "confidence"]
            }
        })
    }

    /// Schema for EndpointAgent output - array of HttpEndpoint
    pub fn endpoint_schema() -> Value {
        json!({
            "type": "ARRAY",
            "items": {
                "type": "OBJECT",
                "properties": {
                    "method": {
                        "type": "STRING",
                        "description": "HTTP method (GET, POST, PUT, DELETE, etc.)"
                    },
                    "path": {
                        "type": "STRING",
                        "description": "Route path (e.g., /users, /users/:id, /api/v1/orders)"
                    },
                    "handler": {
                        "type": "STRING",
                        "description": "Handler function name or 'anonymous' for inline functions"
                    },
                    "node_name": {
                        "type": "STRING",
                        "description": "The callee object name (e.g., app, router, fastify)"
                    },
                    "location": {
                        "type": "STRING",
                        "description": "File location in format file:line:column"
                    },
                    "confidence": {
                        "type": "NUMBER",
                        "minimum": 0.0,
                        "maximum": 1.0,
                        "description": "Confidence score between 0 and 1"
                    },
                    "reasoning": {
                        "type": "STRING",
                        "description": "Brief explanation of the extraction"
                    }
                },
                "required": ["method", "path", "handler", "node_name", "location", "confidence", "reasoning"]
            }
        })
    }

    /// Schema for ConsumerAgent output - array of DataFetchingCall
    pub fn consumer_schema() -> Value {
        json!({
            "type": "ARRAY",
            "items": {
                "type": "OBJECT",
                "properties": {
                    "library": {
                        "type": "STRING",
                        "description": "Library name (fetch, axios, got, response_parsing, etc.)"
                    },
                    "url": {
                        "type": "STRING",
                        "nullable": true,
                        "description": "URL being called if detectable from string literals"
                    },
                    "method": {
                        "type": "STRING",
                        "nullable": true,
                        "description": "HTTP method (GET, POST, etc.) if detectable"
                    },
                    "location": {
                        "type": "STRING",
                        "description": "File location in format file:line:column"
                    },
                    "confidence": {
                        "type": "NUMBER",
                        "minimum": 0.0,
                        "maximum": 1.0,
                        "description": "Confidence score between 0 and 1"
                    },
                    "reasoning": {
                        "type": "STRING",
                        "description": "Brief explanation of the extraction"
                    }
                },
                "required": ["library", "url", "method", "location", "confidence", "reasoning"]
            }
        })
    }

    /// Schema for MiddlewareAgent output - array of Middleware
    pub fn middleware_schema() -> Value {
        json!({
            "type": "ARRAY",
            "items": {
                "type": "OBJECT",
                "properties": {
                    "middleware_type": {
                        "type": "STRING",
                        "description": "Type of middleware (body-parser, cors, auth, static, custom, etc.)"
                    },
                    "path_prefix": {
                        "type": "STRING",
                        "nullable": true,
                        "description": "Path prefix if middleware applies to specific paths"
                    },
                    "handler": {
                        "type": "STRING",
                        "description": "Handler function name or description"
                    },
                    "node_name": {
                        "type": "STRING",
                        "description": "The callee object name (e.g., app, router, server)"
                    },
                    "location": {
                        "type": "STRING",
                        "description": "File location in format file:line:column"
                    },
                    "confidence": {
                        "type": "NUMBER",
                        "minimum": 0.0,
                        "maximum": 1.0,
                        "description": "Confidence score between 0 and 1"
                    },
                    "reasoning": {
                        "type": "STRING",
                        "description": "Brief explanation of the extraction"
                    }
                },
                "required": ["middleware_type", "path_prefix", "handler", "node_name", "location", "confidence", "reasoning"]
            }
        })
    }

    /// Flattened schema for parallel pattern fetching - uses parallel arrays instead of nested objects
    /// Used by FrameworkGuidanceAgent for individual category requests
    pub fn pattern_list_schema() -> Value {
        json!({
            "type": "OBJECT",
            "properties": {
                "patterns": {
                    "type": "ARRAY",
                    "items": { "type": "STRING" },
                    "description": "Code pattern examples"
                },
                "descriptions": {
                    "type": "ARRAY",
                    "items": { "type": "STRING" },
                    "description": "What each pattern represents (same order as patterns)"
                },
                "frameworks": {
                    "type": "ARRAY",
                    "items": { "type": "STRING" },
                    "description": "Which framework each pattern is for (same order as patterns)"
                }
            },
            "required": ["patterns", "descriptions", "frameworks"]
        })
    }

    /// Schema for general guidance (triage hints and parsing notes)
    /// Used by FrameworkGuidanceAgent for the general guidance request
    pub fn general_guidance_schema() -> Value {
        json!({
            "type": "OBJECT",
            "properties": {
                "triage_hints": {
                    "type": "STRING",
                    "description": "Free-form hints for distinguishing between categories"
                },
                "parsing_notes": {
                    "type": "STRING",
                    "description": "Framework-specific notes that may affect parsing"
                }
            },
            "required": ["triage_hints", "parsing_notes"]
        })
    }

    /// Schema for file-centric analysis output - flat structure with mounts, endpoints, and data_calls
    /// Used by FileAnalyzerAgent for one-shot file analysis with Gemini 3.0 Flash
    #[allow(dead_code)]
    pub fn file_analysis_schema() -> Value {
        json!({
            "type": "OBJECT",
            "properties": {
                "mounts": {
                    "type": "ARRAY",
                    "items": {
                        "type": "OBJECT",
                        "properties": {
                            "line_number": {
                                "type": "INTEGER",
                                "description": "Line number in the source file"
                            },
                            "parent_node": {
                                "type": "STRING",
                                "description": "Name of variable accepting the mount (e.g., app, router)"
                            },
                            "child_node": {
                                "type": "STRING",
                                "description": "Name of variable being mounted (e.g., apiRouter, userRoutes)"
                            },
                            "mount_path": {
                                "type": "STRING",
                                "description": "The string literal path prefix (e.g., /api, /users)"
                            },
                            "import_source": {
                                "type": "STRING",
                                "nullable": true,
                                "description": "File path if child_node is imported (e.g., './routes/users'), otherwise null for local definitions"
                            },
                            "pattern_matched": {
                                "type": "STRING",
                                "description": "The specific pattern that triggered this result (e.g., .use(, .register()"
                            }
                        },
                        "required": ["line_number", "parent_node", "child_node", "mount_path", "pattern_matched"]
                    }
                },
                "endpoints": {
                    "type": "ARRAY",
                    "items": {
                        "type": "OBJECT",
                        "properties": {
                            "candidate_id": {
                                "type": "STRING",
                                "description": "Stable identifier for the AST call site candidate"
                            },
                            "line_number": {
                                "type": "INTEGER",
                                "description": "Line number in the source file"
                            },
                            "owner_node": {
                                "type": "STRING",
                                "description": "Variable the endpoint is attached to (e.g., app, router, fastify)"
                            },
                            "method": {
                                "type": "STRING",
                                "enum": ["GET", "POST", "PUT", "DELETE", "PATCH", "HEAD", "OPTIONS", "ALL"],
                                "description": "HTTP method"
                            },
                            "path": {
                                "type": "STRING",
                                "description": "Route path (e.g., /users, /users/:id)"
                            },
                            "handler_name": {
                                "type": "STRING",
                                "description": "Function name or 'anonymous' for inline handlers"
                            },
                            "pattern_matched": {
                                "type": "STRING",
                                "description": "The specific pattern that triggered this result"
                            },
                            "payload_expression_text": {
                                "type": "STRING",
                                "nullable": true,
                                "description": "Verbatim code text of the request payload expression (e.g., 'req.body'), copied EXACTLY as it appears in the source code"
                            },
                            "payload_expression_line": {
                                "type": "INTEGER",
                                "nullable": true,
                                "description": "Line number where the payload expression starts (read from the line-number prefix in the source code)"
                            },
                            "response_expression_text": {
                                "type": "STRING",
                                "nullable": true,
                                "description": "Verbatim code text of the response PAYLOAD SUBEXPRESSION — the value whose type is the response body. Emit the inner value, not the surrounding call: e.g. 'users' from 'res.json(users)', 'ctx.body = users', 'h.response(users)', 'return users', 'reply.send(users)', or 'c.json(users)'. Null for payload-less handlers (redirects, 204s)."
                            },
                            "response_expression_line": {
                                "type": "INTEGER",
                                "nullable": true,
                                "description": "Line number where the payload subexpression starts (read from the line-number prefix in the source code)"
                            },
                            "emission_style": {
                                "type": "STRING",
                                "enum": ["imperative-send", "return-value", "no-payload"],
                                "nullable": true,
                                "description": "How the handler emits its response payload. 'imperative-send': the payload is the argument of a send call — res.json(users), reply.send(users), and also return c.json(users) / return NextResponse.json(users) (the payload is the argument, never the framework Response). 'return-value': the handler's return value IS the payload (e.g. Fastify 'return users'). 'no-payload': zero-arg sends (res.json()), streams/buffers handed to send calls, or payloads written by helper functions (renderUsers(res)). Pairing: imperative-send and return-value require non-null response_expression_text; no-payload requires response_expression_text and response_expression_line to be null."
                            },
                            "primary_type_symbol": {
                                "type": "STRING",
                                "nullable": true,
                                "description": "The named type of the RESPONSE payload value ONLY — the exact value emitted as `response_expression_text` — as a bare identifier. ALWAYS extract it when that response value has a determinable named type; do not skip it. Trace ONLY that sent or returned value to its declared type: a local variable annotation (`const u: User = ...; res.json(u)` gives `User`), a function return annotation (`(): Promise<User>` gives `User`), a cast on the payload (`res.json(u as User)` gives `User`), or a generic type argument (`get<User>(...)` gives `User`). An imperative send like `res.json(u)` does not suppress this; extract it the same as a returned value. Unwrap `Promise<...>`, arrays (`User[]`), and response wrappers (`Response<User>`) down to the core identifier. NEVER borrow a named type from elsewhere in the handler — in particular, do NOT put the REQUEST body's type here (the `T` in `req.body as T`); the request type is carried only by `payload_expression_text` and resolved downstream. Null for untyped, inline-object-literal, or otherwise un-annotated response payloads."
                            },
                            "type_import_source": {
                                "type": "STRING",
                                "nullable": true,
                                "description": "Import path where the `primary_type_symbol` type is defined (e.g., './types/user'), or null if it is declared in the same file. Null whenever `primary_type_symbol` is null. Read the import statements at the top of the file."
                            }
                        },
                        "required": ["candidate_id", "line_number", "owner_node", "method", "path", "handler_name", "pattern_matched", "emission_style", "payload_expression_text", "payload_expression_line"]
                    }
                },
                "data_calls": {
                    "type": "ARRAY",
                    "items": {
                        "type": "OBJECT",
                        "properties": {
                            "candidate_id": {
                                "type": "STRING",
                                "description": "Stable identifier for the AST call site candidate"
                            },
                            "line_number": {
                                "type": "INTEGER",
                                "description": "Line number in the source file"
                            },
                            "target": {
                                "type": "STRING",
                                "description": "The URL or resource being accessed"
                            },
                            "method": {
                                "type": "STRING",
                                "nullable": true,
                                "description": "HTTP method if detectable (GET, POST, etc.)"
                            },
                            "call_kind": {
                                "type": "STRING",
                                "enum": ["internal_http", "external_http", "sdk", "unresolved"],
                                "nullable": true,
                                "description": "How the call target was classified: internal_http (a call to another service's HTTP API), external_http (a third-party or public HTTP API), sdk (a cloud-service or datastore client call), or unresolved (could not be classified)."
                            },
                            "pattern_matched": {
                                "type": "STRING",
                                "description": "The specific pattern that triggered this result"
                            },
                            "call_expression_text": {
                                "type": "STRING",
                                "nullable": true,
                                "description": "Verbatim code text of the fetch/axios/HTTP call expression (e.g., 'fetch(\"/api/users\")'), copied EXACTLY as it appears in the source code"
                            },
                            "call_expression_line": {
                                "type": "INTEGER",
                                "nullable": true,
                                "description": "Line number where the call expression starts (read from the line-number prefix in the source code)"
                            },
                            "payload_expression_text": {
                                "type": "STRING",
                                "nullable": true,
                                "description": "Verbatim code text of the request payload expression (e.g., '{ name, email }'), copied EXACTLY as it appears in the source code"
                            },
                            "payload_expression_line": {
                                "type": "INTEGER",
                                "nullable": true,
                                "description": "Line number where the payload expression starts (read from the line-number prefix in the source code)"
                            },
                            "primary_type_symbol": {
                                "type": "STRING",
                                "nullable": true,
                                "description": "The named type the call expects back, as a bare identifier. ALWAYS extract it when the result has a determinable named type; do not skip it. Trace the call result to its declared type: an `as` cast on the returned promise (`res.json() as Promise<User>` gives `User`), an annotation on the awaited value (`const u: User = await res.json()` gives `User`), or a generic type argument on the call. Unwrap `Promise<...>`, arrays (`User[]`), and response wrappers (`Response<User>`) down to the core identifier. Null only for untyped results with no named type."
                            },
                            "type_import_source": {
                                "type": "STRING",
                                "nullable": true,
                                "description": "Import path where the `primary_type_symbol` type is defined (e.g., './types/user'), or null if it is declared in the same file. Read the import statements at the top of the file."
                            }
                        },
                        "required": ["candidate_id", "line_number", "target", "pattern_matched"]
                    }
                },
                "graphql_operations": {
                    "type": "ARRAY",
                    "items": {
                        "type": "OBJECT",
                        "properties": {
                            "kind": {
                                "type": "STRING",
                                "enum": ["query", "mutation", "subscription"],
                                "description": "The GraphQL root operation this resolver implements: query, mutation, or subscription."
                            },
                            "field": {
                                "type": "STRING",
                                "description": "The schema field name this resolver answers (e.g., 'order', 'createUser')."
                            },
                            "resolver_function": {
                                "type": "STRING",
                                "description": "Name of the resolver function implementing this field (e.g., 'resolveOrder')."
                            },
                            "resolver_line": {
                                "type": "INTEGER",
                                "description": "Line number where the resolver function is defined (read from the line-number prefix in the source code)."
                            },
                            "primary_type_symbol": {
                                "type": "STRING",
                                "nullable": true,
                                "description": "The named return type of the resolver as a bare identifier (e.g., 'ApiResponse'). Unwrap Promise<...>, arrays, and async-iterator wrappers down to the core identifier. Null for untyped, inline-object-literal, or otherwise un-annotated resolver returns."
                            },
                            "type_import_source": {
                                "type": "STRING",
                                "nullable": true,
                                "description": "Import path where the `primary_type_symbol` type is defined (e.g., './types/order'), or null if it is declared in the same file. Null whenever `primary_type_symbol` is null."
                            }
                        },
                        "required": ["kind", "field", "resolver_function", "resolver_line"]
                    }
                },
                "pubsub_operations": {
                    "type": "ARRAY",
                    "items": {
                        "type": "OBJECT",
                        "properties": {
                            "topic": {
                                "type": "STRING",
                                "description": "The exact topic or channel name the operation targets, as a literal string (e.g., 'metrics.page_view'). Copy it verbatim from the source; skip operations whose topic is a runtime variable with no literal value."
                            },
                            "role": {
                                "type": "STRING",
                                "enum": ["subscriber", "publisher"],
                                "description": "subscriber when the code registers a handler that receives messages on the topic; publisher when the code sends a message to the topic."
                            },
                            "line_number": {
                                "type": "INTEGER",
                                "description": "Line number where the operation appears (read from the line-number prefix in the source code)."
                            },
                            "primary_type_symbol": {
                                "type": "STRING",
                                "nullable": true,
                                "description": "The named type of the message payload as a bare identifier (e.g., 'PageViewEvent'). Unwrap arrays and promise wrappers down to the core identifier. Null for untyped or inline-object payloads."
                            },
                            "type_import_source": {
                                "type": "STRING",
                                "nullable": true,
                                "description": "Import path where the `primary_type_symbol` type is defined (e.g., './types/events'), or null if it is declared in the same file. Null whenever `primary_type_symbol` is null."
                            },
                            "broker": {
                                "type": "STRING",
                                "nullable": true,
                                "description": "Diagnostic only: the pub/sub library or transport the call uses if evident (e.g., 'redis'), or null. Not part of the operation's identity."
                            }
                        },
                        "required": ["topic", "role", "line_number"]
                    }
                }
            },
            "required": ["mounts", "endpoints", "data_calls"]
        })
    }

    /// Schema for the framework-guidance `extraction_config` task: rules for
    /// unwrapping machinery/wrapper types around response payloads. Field
    /// names are camelCase to match the sidecar's `ExtractionRule`
    /// (`src/sidecar/src/types.ts`); semantics are taught by the cloud-side
    /// prompt. All rule fields are required so the model decides each one;
    /// empty arrays / null mean "not applicable".
    pub fn extraction_config_schema() -> Value {
        json!({
            "type": "OBJECT",
            "properties": {
                "rules": {
                    "type": "ARRAY",
                    "items": {
                        "type": "OBJECT",
                        "properties": {
                            "wrapperSymbols": {
                                "type": "ARRAY",
                                "items": { "type": "STRING" },
                                "description": "Exact wrapper type/symbol names to unwrap. Use only for distinctive names (AxiosResponse, ApiEnvelope). For generic names shared with the DOM or frameworks (Response, Request) ALWAYS pair with originModuleGlobs — when globs are present the symbol must also originate from a matching module."
                            },
                            "machineryIndicators": {
                                "type": "ARRAY",
                                "items": { "type": "STRING" },
                                "description": "Method/property names that mark a machinery type (e.g. statusCode, headers). Only applied together with originModuleGlobs."
                            },
                            "originModuleGlobs": {
                                "type": "ARRAY",
                                "items": { "type": "STRING" },
                                "description": "Package-path globs the wrapper's declaration must come from, resolved against node_modules (e.g. got/*, @types/node/*, typescript/lib/*). Emit multiple candidate globs when the origin is ambiguous; entries that match nothing are ignored. Leave empty for workspace-local wrapper types and rely on a distinctive wrapperSymbols name instead."
                            },
                            "payloadGenericIndex": {
                                "type": "INTEGER",
                                "nullable": true,
                                "description": "Index of the generic type argument holding the payload. Null when the wrapper is not generic; defaults to 0."
                            },
                            "payloadPropertyPath": {
                                "type": "ARRAY",
                                "items": { "type": "STRING" },
                                "description": "Property path to the payload when generics are unavailable (e.g. [\"body\"] for got's Response.body)."
                            },
                            "unwrapRecursively": {
                                "type": "BOOLEAN",
                                "nullable": true,
                                "description": "Recursively unwrap nested wrappers (Promise<AxiosResponse<T>> resolves to T). Null defaults to false."
                            },
                            "maxDepth": {
                                "type": "INTEGER",
                                "nullable": true,
                                "description": "Maximum nesting depth for recursive unwrapping. Null defaults to 4."
                            }
                        },
                        "required": ["wrapperSymbols", "machineryIndicators", "originModuleGlobs", "payloadGenericIndex", "payloadPropertyPath", "unwrapRecursively", "maxDepth"]
                    }
                }
            },
            "required": ["rules"]
        })
    }

    /// Schema for MountAgent output - array of MountRelationship
    pub fn mount_schema() -> Value {
        json!({
            "type": "ARRAY",
            "items": {
                "type": "OBJECT",
                "properties": {
                    "parent_node": {
                        "type": "STRING",
                        "description": "The object doing the mounting (e.g., app, router)"
                    },
                    "child_node": {
                        "type": "STRING",
                        "description": "The object being mounted (e.g., apiRouter, userRouter)"
                    },
                    "mount_path": {
                        "type": "STRING",
                        "description": "The path where it's mounted (e.g., /api, /users)"
                    },
                    "location": {
                        "type": "STRING",
                        "description": "File location in format file:line:column"
                    },
                    "confidence": {
                        "type": "NUMBER",
                        "minimum": 0.0,
                        "maximum": 1.0,
                        "description": "Confidence score between 0 and 1"
                    },
                    "reasoning": {
                        "type": "STRING",
                        "description": "Brief explanation of the extraction"
                    }
                },
                "required": ["parent_node", "child_node", "mount_path", "location", "confidence", "reasoning"]
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_all_schemas_are_valid_json() {
        // Test that all schemas can be serialized to JSON
        assert!(AgentSchemas::triage_schema().is_object());
        assert!(AgentSchemas::endpoint_schema().is_object());
        assert!(AgentSchemas::consumer_schema().is_object());
        assert!(AgentSchemas::middleware_schema().is_object());
        assert!(AgentSchemas::mount_schema().is_object());
        assert!(AgentSchemas::pattern_list_schema().is_object());
        assert!(AgentSchemas::general_guidance_schema().is_object());
    }

    #[test]
    fn test_pattern_list_schema_structure() {
        let schema = AgentSchemas::pattern_list_schema();
        assert_eq!(schema["type"], "OBJECT");
        // Flattened structure uses parallel arrays
        assert!(schema["properties"]["patterns"].is_object());
        assert_eq!(schema["properties"]["patterns"]["type"], "ARRAY");
        assert_eq!(schema["properties"]["patterns"]["items"]["type"], "STRING");
        assert!(schema["properties"]["descriptions"].is_object());
        assert_eq!(schema["properties"]["descriptions"]["type"], "ARRAY");
        assert!(schema["properties"]["frameworks"].is_object());
        assert_eq!(schema["properties"]["frameworks"]["type"], "ARRAY");
        assert!(schema["required"].is_array());
    }

    #[test]
    fn test_general_guidance_schema_structure() {
        let schema = AgentSchemas::general_guidance_schema();
        assert_eq!(schema["type"], "OBJECT");
        assert!(schema["properties"]["triage_hints"].is_object());
        assert!(schema["properties"]["parsing_notes"].is_object());
        assert!(schema["required"].is_array());
    }

    #[test]
    fn test_triage_schema_structure() {
        let schema = AgentSchemas::triage_schema();
        assert_eq!(schema["type"], "ARRAY");
        assert!(schema["items"]["properties"]["location"].is_object());
        assert!(schema["items"]["properties"]["classification"]["enum"].is_array());
        assert!(schema["items"]["required"].is_array());
    }

    #[test]
    fn extraction_rule_schema_fields_match_serde_wire_names() {
        // Three copies of the rule field names exist: this schema, the serde
        // struct in services::type_sidecar, and the sidecar's zod validator.
        // Every Rust field is #[serde(default)], so a drifted key would not
        // error anywhere — the rule would parse as an empty no-op and
        // unwrapping would silently go dead. Pin schema ↔ serde here (the
        // sidecar's own tests pin the zod side).
        use crate::services::type_sidecar::ExtractionRule;

        let rule = ExtractionRule {
            wrapper_symbols: vec!["AxiosResponse".to_string()],
            machinery_indicators: vec!["statusCode".to_string()],
            origin_module_globs: vec!["axios/*".to_string()],
            payload_generic_index: Some(0),
            payload_property_path: vec!["data".to_string()],
            unwrap_recursively: Some(true),
            max_depth: Some(4),
        };
        let serialized = serde_json::to_value(&rule).unwrap();
        let mut serde_keys: Vec<&str> = serialized
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect();
        serde_keys.sort_unstable();

        let schema = AgentSchemas::extraction_config_schema();
        let rule_schema = &schema["properties"]["rules"]["items"];
        let mut schema_keys: Vec<&str> = rule_schema["properties"]
            .as_object()
            .expect("rule properties must exist")
            .keys()
            .map(String::as_str)
            .collect();
        schema_keys.sort_unstable();
        assert_eq!(schema_keys, serde_keys);

        let mut required_keys: Vec<&str> = rule_schema["required"]
            .as_array()
            .expect("rule required list must exist")
            .iter()
            .map(|v| v.as_str().unwrap())
            .collect();
        required_keys.sort_unstable();
        assert_eq!(required_keys, serde_keys);
    }

    #[test]
    fn emission_style_schema_enum_matches_serde_wire_values() {
        // The schema's enum list and EmissionStyle's serde rename output must
        // stay in lockstep: a value the schema advertises but the enum can't
        // parse would be absorbed to None by the lenient deserializer and
        // silently lose the classification.
        use crate::agents::file_analyzer_agent::EmissionStyle;

        let schema = AgentSchemas::file_analysis_schema();
        let schema_values: Vec<String> =
            schema["properties"]["endpoints"]["items"]["properties"]["emission_style"]["enum"]
                .as_array()
                .expect("emission_style enum must exist")
                .iter()
                .map(|v| v.as_str().unwrap().to_string())
                .collect();

        let serde_values: Vec<String> = [
            EmissionStyle::ImperativeSend,
            EmissionStyle::ReturnValue,
            EmissionStyle::NoPayload,
        ]
        .iter()
        .map(|style| {
            serde_json::to_value(style)
                .unwrap()
                .as_str()
                .unwrap()
                .to_string()
        })
        .collect();

        assert_eq!(schema_values, serde_values);

        // The model must always decide (field is required); null stays legal.
        let required = schema["properties"]["endpoints"]["items"]["required"]
            .as_array()
            .unwrap();
        assert!(required.iter().any(|v| v == "emission_style"));
    }

    #[test]
    fn call_kind_schema_enum_matches_serde_wire_values() {
        // The data_calls call_kind enum and CallKind's serde output must stay in
        // lockstep, or a value the schema advertises but the enum can't parse is
        // silently absorbed to None and the classification is lost.
        use crate::operation::CallKind;

        let schema = AgentSchemas::file_analysis_schema();
        let schema_values: Vec<String> = schema["properties"]["data_calls"]["items"]["properties"]
            ["call_kind"]["enum"]
            .as_array()
            .expect("call_kind enum must exist")
            .iter()
            .map(|v| v.as_str().unwrap().to_string())
            .collect();

        let serde_values: Vec<String> = [
            CallKind::InternalHttp,
            CallKind::ExternalHttp,
            CallKind::Sdk,
            CallKind::Unresolved,
        ]
        .iter()
        .map(|k| {
            serde_json::to_value(k)
                .unwrap()
                .as_str()
                .unwrap()
                .to_string()
        })
        .collect();

        assert_eq!(schema_values, serde_values);

        // call_kind is optional (nullable, not required): a missing label is legal
        // and defaults to unclassified rather than forcing the model to guess.
        let required = schema["properties"]["data_calls"]["items"]["required"]
            .as_array()
            .unwrap();
        assert!(!required.iter().any(|v| v == "call_kind"));
    }

    #[test]
    fn graphql_operations_schema_enum_matches_serde_wire_values() {
        // The graphql_operations `kind` enum and GraphqlOperationKind's serde
        // output must stay in lockstep, or a value the schema advertises but the
        // enum can't parse is silently lost when the file-analyzer response is
        // deserialized (mirrors call_kind / emission_style).
        use crate::operation::GraphqlOperationKind;

        let schema = AgentSchemas::file_analysis_schema();
        let schema_values: Vec<String> =
            schema["properties"]["graphql_operations"]["items"]["properties"]["kind"]["enum"]
                .as_array()
                .expect("graphql_operations kind enum must exist")
                .iter()
                .map(|v| v.as_str().unwrap().to_string())
                .collect();

        let serde_values: Vec<String> = [
            GraphqlOperationKind::Query,
            GraphqlOperationKind::Mutation,
            GraphqlOperationKind::Subscription,
        ]
        .iter()
        .map(|k| {
            serde_json::to_value(k)
                .unwrap()
                .as_str()
                .unwrap()
                .to_string()
        })
        .collect();

        assert_eq!(schema_values, serde_values);

        // The four locating fields must always be present; the type slots
        // (primary_type_symbol / type_import_source) stay optional/nullable.
        let required = schema["properties"]["graphql_operations"]["items"]["required"]
            .as_array()
            .expect("graphql_operations item required array must exist");
        for field in ["kind", "field", "resolver_function", "resolver_line"] {
            assert!(
                required.iter().any(|v| v == field),
                "graphql_operations item required must contain {field}"
            );
        }

        // graphql_operations is optional at the top level: a model may omit it.
        let top_required = schema["required"].as_array().unwrap();
        assert!(!top_required.iter().any(|v| v == "graphql_operations"));
    }

    #[test]
    fn pubsub_operations_schema_enum_matches_serde_wire_values() {
        // The pubsub_operations `role` enum and PubsubRole's serde output must
        // stay in lockstep, or a value the schema advertises but the enum can't
        // parse is silently lost when the file-analyzer response is deserialized
        // (mirrors graphql_operations / call_kind / emission_style).
        use crate::operation::PubsubRole;

        let schema = AgentSchemas::file_analysis_schema();
        let schema_values: Vec<String> =
            schema["properties"]["pubsub_operations"]["items"]["properties"]["role"]["enum"]
                .as_array()
                .expect("pubsub_operations role enum must exist")
                .iter()
                .map(|v| v.as_str().unwrap().to_string())
                .collect();

        let serde_values: Vec<String> = [PubsubRole::Subscriber, PubsubRole::Publisher]
            .iter()
            .map(|r| {
                serde_json::to_value(r)
                    .unwrap()
                    .as_str()
                    .unwrap()
                    .to_string()
            })
            .collect();

        assert_eq!(schema_values, serde_values);

        // The three locating fields are always present; the type slots and the
        // diagnostic broker stay optional/nullable.
        let required = schema["properties"]["pubsub_operations"]["items"]["required"]
            .as_array()
            .expect("pubsub_operations item required array must exist");
        for field in ["topic", "role", "line_number"] {
            assert!(
                required.iter().any(|v| v == field),
                "pubsub_operations item required must contain {field}"
            );
        }

        // pubsub_operations is optional at the top level: a model may omit it.
        let top_required = schema["required"].as_array().unwrap();
        assert!(!top_required.iter().any(|v| v == "pubsub_operations"));
    }

    #[test]
    fn test_file_analysis_schema_structure() {
        let schema = AgentSchemas::file_analysis_schema();
        assert_eq!(schema["type"], "OBJECT");
        assert!(schema["properties"]["mounts"].is_object());
        assert!(schema["properties"]["endpoints"].is_object());
        assert!(schema["properties"]["data_calls"].is_object());
        assert!(
            schema["properties"]["endpoints"]["items"]["properties"]["candidate_id"].is_object()
        );
        assert!(
            schema["properties"]["endpoints"]["items"]["properties"]["payload_expression_text"]
                .is_object()
        );
        assert!(
            schema["properties"]["endpoints"]["items"]["properties"]["payload_expression_line"]
                .is_object()
        );
        assert!(
            schema["properties"]["endpoints"]["items"]["properties"]["response_expression_text"]
                .is_object()
        );
        assert!(
            schema["properties"]["endpoints"]["items"]["properties"]["response_expression_line"]
                .is_object()
        );
        assert!(
            schema["properties"]["data_calls"]["items"]["properties"]["candidate_id"].is_object()
        );
        assert!(
            schema["properties"]["data_calls"]["items"]["properties"]["call_expression_text"]
                .is_object()
        );
        assert!(
            schema["properties"]["data_calls"]["items"]["properties"]["call_expression_line"]
                .is_object()
        );
        assert!(
            schema["properties"]["data_calls"]["items"]["properties"]["payload_expression_text"]
                .is_object()
        );
        assert!(
            schema["properties"]["data_calls"]["items"]["properties"]["payload_expression_line"]
                .is_object()
        );
        assert!(schema["required"].is_array());
    }
}
