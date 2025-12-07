use serde_json::{Value, json};

/// Gemini schema types for structured output
/// These correspond to the Type constants from @google/genai
#[allow(dead_code)]
pub struct GeminiType;

#[allow(dead_code)]
impl GeminiType {
    pub const ARRAY: &'static str = "ARRAY";
    pub const OBJECT: &'static str = "OBJECT";
    pub const STRING: &'static str = "STRING";
    pub const NUMBER: &'static str = "NUMBER";
    pub const BOOLEAN: &'static str = "BOOLEAN";
}

/// Gemini-format schemas for structured output from each agent
pub struct AgentSchemas;

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
                    },
                    "response_type_file": {
                        "type": "STRING",
                        "nullable": true,
                        "description": "File path containing the response type definition"
                    },
                    "response_type_position": {
                        "type": "NUMBER",
                        "nullable": true,
                        "description": "Start position (index) of the response type definition in the file"
                    },
                    "response_type_string": {
                        "type": "STRING",
                        "nullable": true,
                        "description": "The type string itself (e.g. 'User[]', 'Response<Order>')"
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
                    },
                    "expected_type_file": {
                        "type": "STRING",
                        "nullable": true,
                        "description": "File path containing the expected response type definition"
                    },
                    "expected_type_position": {
                        "type": "NUMBER",
                        "nullable": true,
                        "description": "Start position (index) of the expected response type definition in the file"
                    },
                    "expected_type_string": {
                        "type": "STRING",
                        "nullable": true,
                        "description": "The type string expected (e.g. 'User[]')"
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

    /// Schema for FrameworkGuidanceAgent output
    pub fn framework_guidance_schema() -> Value {
        json!({
            "type": "OBJECT",
            "properties": {
                "mount_patterns": {
                    "type": "ARRAY",
                    "items": {
                        "type": "OBJECT",
                        "properties": {
                            "pattern": {
                                "type": "STRING",
                                "description": "Code pattern example, e.g., app.route('/path', subApp)"
                            },
                            "description": {
                                "type": "STRING",
                                "description": "What this pattern represents"
                            },
                            "framework": {
                                "type": "STRING",
                                "description": "Which framework this pattern is for"
                            }
                        },
                        "required": ["pattern", "description", "framework"]
                    },
                    "description": "Patterns for router/sub-app mounting"
                },
                "endpoint_patterns": {
                    "type": "ARRAY",
                    "items": {
                        "type": "OBJECT",
                        "properties": {
                            "pattern": { "type": "STRING" },
                            "description": { "type": "STRING" },
                            "framework": { "type": "STRING" }
                        },
                        "required": ["pattern", "description", "framework"]
                    },
                    "description": "Patterns for HTTP endpoint definitions"
                },
                "middleware_patterns": {
                    "type": "ARRAY",
                    "items": {
                        "type": "OBJECT",
                        "properties": {
                            "pattern": { "type": "STRING" },
                            "description": { "type": "STRING" },
                            "framework": { "type": "STRING" }
                        },
                        "required": ["pattern", "description", "framework"]
                    },
                    "description": "Patterns for middleware registration"
                },
                "data_fetching_patterns": {
                    "type": "ARRAY",
                    "items": {
                        "type": "OBJECT",
                        "properties": {
                            "pattern": { "type": "STRING" },
                            "description": { "type": "STRING" },
                            "framework": { "type": "STRING" }
                        },
                        "required": ["pattern", "description", "framework"]
                    },
                    "description": "Patterns for outbound HTTP calls"
                },
                "triage_hints": {
                    "type": "STRING",
                    "description": "Free-form hints for distinguishing between categories"
                },
                "parsing_notes": {
                    "type": "STRING",
                    "description": "Framework-specific notes that may affect parsing"
                }
            },
            "required": ["mount_patterns", "endpoint_patterns", "middleware_patterns",
                        "data_fetching_patterns", "triage_hints", "parsing_notes"]
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
        assert!(AgentSchemas::framework_guidance_schema().is_object());
    }

    #[test]
    fn test_framework_guidance_schema_structure() {
        let schema = AgentSchemas::framework_guidance_schema();
        assert_eq!(schema["type"], "OBJECT");
        assert!(schema["properties"]["mount_patterns"].is_object());
        assert!(schema["properties"]["endpoint_patterns"].is_object());
        assert!(schema["properties"]["middleware_patterns"].is_object());
        assert!(schema["properties"]["data_fetching_patterns"].is_object());
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
}
