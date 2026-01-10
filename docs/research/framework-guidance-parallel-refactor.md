# Research Document: FrameworkGuidanceAgent Parallel Refactor

---

## ⚠️ IMPORTANT: REQUIRED READING BEFORE IMPLEMENTING

**YOU MUST READ THESE FILES FIRST:**
1. `.thoughts/README.md` - Project overview and current status
2. `.thoughts/OUTSTANDING_WORK.md` - Prioritized next steps
3. `lambdas/README.md` - Lambda function documentation

---

## 🚨 CRITICAL DESIGN PRINCIPLE: COMPLETE LIBRARY AND FRAMEWORK AGNOSTICISM

**CARRICK MUST BE COMPLETELY LIBRARY AND FRAMEWORK AGNOSTIC.**

This means:
- **NO hardcoded framework patterns** (e.g., no Express-specific, Fastify-specific, or any other framework-specific code)
- **NO hardcoded library patterns** (e.g., no Axios-specific, fetch-specific, or any other HTTP client-specific code)
- **The LLM determines appropriate patterns** based solely on what it detects in the codebase
- **Prompts must remain generic** - do not prescribe specific patterns for specific frameworks/libraries
- **The system must work for ANY JavaScript/TypeScript web framework or HTTP client**, including ones that don't exist yet

The entire value proposition of Carrick is that it can analyze ANY codebase without prior knowledge of the specific frameworks or libraries used. The LLM's general knowledge provides the framework-specific expertise at runtime.

**DO NOT add framework-specific logic, patterns, or heuristics to the codebase.**

---

## Problem Statement

The Carrick API analysis system was experiencing **503 Service Unavailable errors** due to AWS API Gateway's hard 30-second timeout limit. The `FrameworkGuidanceAgent` was making LLM calls with complex nested JSON schemas, which caused Claude's structured output constrained decoding to take longer than 30 seconds.

### Root Cause

When using Anthropic's structured outputs feature, the LLM must generate JSON that conforms exactly to the provided schema. **Nested object structures significantly increase constrained decoding time.** The original schema with arrays of objects (each with 3 required fields) frequently exceeded the 30-second API Gateway limit.

---

## ✅ IMPLEMENTED SOLUTION

### Key Insight: Flatten JSON Schemas

**Nested objects in JSON schemas cause exponential slowdown in constrained decoding.** The solution is to use **parallel arrays of primitives** instead of arrays of objects.

### Schema Flattening

**Before (slow - nested objects):**
```json
{
  "type": "OBJECT",
  "properties": {
    "patterns": {
      "type": "ARRAY",
      "items": {
        "type": "OBJECT",
        "properties": {
          "pattern": { "type": "STRING" },
          "description": { "type": "STRING" },
          "framework": { "type": "STRING" }
        },
        "required": ["pattern", "description", "framework"]
      }
    }
  },
  "required": ["patterns"]
}
```

**After (fast - parallel arrays):**
```json
{
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
}
```

This changes the structure from `OBJECT → ARRAY → OBJECT → STRING` to `OBJECT → ARRAY → STRING`, removing one level of nesting.

### Response Handling

The flattened response is converted back to structured objects in Rust:

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
struct FlatPatternResponse {
    patterns: Vec<String>,
    descriptions: Vec<String>,
    frameworks: Vec<String>,
}

impl FlatPatternResponse {
    fn into_pattern_examples(self) -> Vec<PatternExample> {
        self.patterns
            .into_iter()
            .zip(self.descriptions)
            .zip(self.frameworks)
            .map(|((pattern, description), framework)| PatternExample {
                pattern,
                description,
                framework,
            })
            .collect()
    }
}
```

### Parallel Execution

With the flattened schema, parallel execution now completes well within the 30-second timeout:

```rust
pub async fn generate_guidance(
    &self,
    framework_detection: &DetectionResult,
) -> Result<FrameworkGuidance, Box<dyn std::error::Error>> {
    let system_message = self.build_system_message();
    let prompt_context = self.build_context_string(framework_detection);

    // Execute calls in parallel (flattened schema makes this fast enough)
    let mount_task = self.fetch_patterns("mount", &prompt_context, &system_message);
    let endpoint_task = self.fetch_patterns("endpoint", &prompt_context, &system_message);
    let middleware_task = self.fetch_patterns("middleware", &prompt_context, &system_message);
    let fetching_task = self.fetch_patterns("data_fetching", &prompt_context, &system_message);
    let general_task = self.fetch_general_guidance(&prompt_context, &system_message);

    let (mount_patterns, endpoint_patterns, middleware_patterns, data_fetching_patterns, general_guidance) = 
        tokio::try_join!(mount_task, endpoint_task, middleware_task, fetching_task, general_task)?;

    Ok(FrameworkGuidance { /* ... */ })
}
```

---

## Prompt Caching - SKIPPED

### Why Caching Was Not Implemented

Anthropic prompt caching requires a **minimum of 1024 tokens (~4096 characters)** for caching to activate. 

The cached context for downstream agents (TriageAgent, EndpointAgent, etc.) typically contains:
- Framework detection JSON (~100 chars)
- LLM-generated framework patterns (~500-800 chars)
- Generic classification instructions (~1000-1500 chars)

**Total: ~1600-2400 characters (~400-600 tokens) - below the 1024 token minimum.**

### The Framework Agnosticism Constraint

To reach the 1024 token minimum, we would need to add more content to the cached context. However:

1. **Cannot add framework-specific examples** - This would violate the core design principle that Carrick must be completely framework-agnostic
2. **Cannot add hardcoded patterns** - The LLM should determine patterns based on what it detects, not from hardcoded examples
3. **Generic padding would be noise** - Adding meaningless content just to reach the token threshold would degrade prompt quality

### Decision

**Prompt caching is skipped for now** to maintain framework agnosticism. The cost/latency trade-off is acceptable given the core design principle.

### Future Considerations

Caching could be revisited if:
1. Anthropic lowers the minimum token threshold
2. A framework-agnostic way to generate 1024+ tokens of useful cached content is found
3. The project decides framework-specific optimizations are acceptable for certain use cases

### Lambda Proxy Support (Preserved)

The Lambda proxy still supports `cache_control` blocks in case caching is re-enabled later:

```javascript
// Check if content is an array of content blocks (for cache_control support)
if (Array.isArray(msg.content)) {
    convertedMessages.push({
        role: role,
        content: msg.content,
    });
}
```

---

## Infrastructure Configuration

### Lambda Timeout
```hcl
# terraform/lambda.tf
timeout = 120  # Increased for parallel LLM calls
```

### API Gateway Timeout
```hcl
# terraform/api_gateway.tf
timeout_milliseconds = 30000  # API Gateway HTTP API max is 30s
```

### Retry Logic

7 attempts with exponential backoff (2s, 4s, 8s, 16s, 32s, 64s):

```rust
let max_retries = 7;
for attempt in 1..=max_retries {
    // ... on 429 or 503:
    let wait_time = Duration::from_secs(2u64.pow(attempt as u32));
    sleep(wait_time).await;
}
```

---

## Files Modified

1. **`src/agents/framework_guidance_agent.rs`** - Parallel execution, flattened response handling
2. **`src/agents/schemas.rs`** - Flattened `pattern_list_schema()`, `general_guidance_schema()`
3. **`src/agent_service.rs`** - Improved retry logic (7 attempts, exponential backoff)
4. **`src/agents/triage_agent.rs`** - Result count validation
5. **`src/agents/endpoint_agent.rs`** - Standard prompt building
6. **`src/agents/consumer_agent.rs`** - Standard prompt building
7. **`src/agents/middleware_agent.rs`** - Standard prompt building
8. **`src/agents/mount_agent.rs`** - Standard prompt building
9. **`lambdas/agent-proxy/index.js`** - Cache control support (preserved for future use), cache stats logging
10. **`terraform/lambda.tf`** - Timeout increased to 120s
11. **`terraform/api_gateway.tf`** - Explicit 30s timeout

---

## Performance Results

| Metric | Before | After |
|--------|--------|-------|
| FrameworkGuidance latency | 30-60s (timeouts) | ~5-10s |
| 503 errors | Frequent | Rare |
| Schema complexity | Nested objects | Flat arrays |
| Parallel calls | Failed (timeout) | Working |

---

## Lessons Learned

1. **Nested JSON schemas kill performance** - Always prefer flat structures for structured output
2. **Parallel arrays are fast** - `OBJECT → ARRAY → STRING` is much faster than `OBJECT → ARRAY → OBJECT`
3. **API Gateway has hard limits** - 30s max for HTTP APIs, plan around this
4. **Cache minimum tokens** - Anthropic requires 1024+ tokens for caching to activate
5. **Exponential backoff is essential** - Rate limits and transient failures need robust retry logic
6. **Framework agnosticism has trade-offs** - Some optimizations (like prompt caching) may conflict with the core design principle

---

## Future Considerations

1. **Switch to Gemini** - May handle structured output more efficiently for complex schemas
2. **Lambda Function URLs** - Bypass API Gateway's 30s limit entirely
3. **Prompt caching revisit** - If Anthropic lowers minimum token threshold or we find agnostic ways to reach 1024 tokens
4. **Batch consolidation** - Reduce total API calls by increasing batch sizes where possible

---

## References

- Anthropic Structured Outputs: https://docs.anthropic.com/en/docs/build-with-claude/structured-outputs
- Anthropic Prompt Caching: https://docs.anthropic.com/en/docs/build-with-claude/prompt-caching
- Prompt Caching Minimum Tokens: https://docs.anthropic.com/en/docs/build-with-claude/prompt-caching#minimum-cacheable-prompt-length
- AWS API Gateway Timeout Limits: 30s max for HTTP APIs
