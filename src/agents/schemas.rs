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
                                "description": "The primary type symbol name without wrappers (e.g., 'User' from 'Response<User[]>'). Extract just the identifier, not the full type."
                            },
                            "type_import_source": {
                                "type": "STRING",
                                "nullable": true,
                                "description": "Import path where the type is defined (e.g., './types/user'), null if inline or defined in the same file. Look at import statements at the top of the file."
                            }
                        },
                        "required": ["candidate_id", "line_number", "owner_node", "method", "path", "handler_name", "pattern_matched", "emission_style"]
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
                                "description": "The primary type symbol name without wrappers (e.g., 'User' from 'Promise<User>'). Extract just the identifier, not the full type."
                            },
                            "type_import_source": {
                                "type": "STRING",
                                "nullable": true,
                                "description": "Import path where the type is defined (e.g., './types/user'), null if inline or defined in the same file. Look at import statements at the top of the file."
                            }
                        },
                        "required": ["candidate_id", "line_number", "target", "pattern_matched"]
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
    fn emission_style_schema_enum_matches_serde_wire_values() {
        // The schema's enum list and EmissionStyle's serde rename output must
        // stay in lockstep: a value the schema advertises but the enum can't
        // parse would be absorbed to None by the lenient deserializer and
        // silently lose the classification.
        use crate::agents::file_analyzer_agent::EmissionStyle;

        let schema = AgentSchemas::file_analysis_schema();
        let schema_values: Vec<String> = schema["properties"]["endpoints"]["items"]["properties"]
            ["emission_style"]["enum"]
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
