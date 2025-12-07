# Framework Guidance Agent Design

**Date**: January 2025  
**Status**: Proposed  
**Author**: AI Assistant  

---

## Problem Statement

The current multi-agent architecture passes detected frameworks to downstream agents, but the agents don't adapt their behavior based on which frameworks are actually in use. They use hardcoded Express-centric examples in their prompts:

```
- app.use('/api', apiRouter)
- router.use('/users', userRouter)
- app.get('/path', handler)
```

This works well for Express, Koa, and Fastify (which have similar APIs), but fails or produces poor results for:

- **Hono**: Uses `app.route('/path', subApp)` instead of `app.use()`
- **Elysia**: Uses `.group('/path', callback)` chaining
- **Fastify**: Uses `fastify.register(plugin, { prefix: '/api' })`
- **Hapi**: Uses `server.route({ method: 'GET', path: '/users/{id}', ... })`
- **NestJS**: Uses decorators `@Controller('path')`, `@Get('/:id')`

The system claims to be "framework-agnostic" but is actually "Express-compatible."

---

## Proposed Solution

Add a **Framework Guidance Agent** that:

1. Takes the detected frameworks as input
2. Generates framework-specific patterns and guidance
3. Passes this guidance to downstream agents to enhance their prompts

This makes the system truly framework-agnostic by having the LLM tell us how to handle each framework, rather than hardcoding patterns.

---

## Architecture

### Current Flow

```
FrameworkDetector 
    → DetectionResult { frameworks: ["hono"], data_fetchers: ["axios"] }
    → Serialized as JSON "FRAMEWORK CONTEXT" in prompts
    → Agents use hardcoded Express examples (ignored context)
```

### Proposed Flow

```
FrameworkDetector
    → DetectionResult { frameworks: ["hono"], data_fetchers: ["axios"] }
    ↓
FrameworkGuidanceAgent (NEW)
    → FrameworkGuidance {
        mount_patterns: [...],
        endpoint_patterns: [...],
        middleware_patterns: [...],
        data_fetching_patterns: [...],
        triage_hints: "..."
      }
    ↓
Downstream agents incorporate guidance into their prompts
```

---

## Data Structures

### Input

```rust
pub struct FrameworkGuidanceInput {
    pub frameworks: Vec<String>,      // e.g., ["hono", "zod"]
    pub data_fetchers: Vec<String>,   // e.g., ["axios", "ky"]
}
```

### Output

```rust
pub struct FrameworkGuidance {
    /// Patterns for router/sub-app mounting
    pub mount_patterns: Vec<PatternExample>,
    
    /// Patterns for HTTP endpoint definitions
    pub endpoint_patterns: Vec<PatternExample>,
    
    /// Patterns for middleware registration
    pub middleware_patterns: Vec<PatternExample>,
    
    /// Patterns for outbound HTTP calls
    pub data_fetching_patterns: Vec<PatternExample>,
    
    /// Free-form hints for the triage agent
    pub triage_hints: String,
    
    /// Framework-specific notes that may affect parsing
    pub parsing_notes: String,
}

pub struct PatternExample {
    /// The code pattern, e.g., "app.route('/path', subApp)"
    pub pattern: String,
    
    /// What this pattern represents
    pub description: String,
    
    /// Which framework this is for
    pub framework: String,
}
```

---

## Agent Prompt Design

### System Message

```
You are an expert in JavaScript/TypeScript web frameworks. Your task is to provide 
framework-specific patterns and guidance that will help other agents correctly 
identify and parse code from these frameworks.

You will be given a list of detected frameworks and data-fetching libraries. 
For each one, provide concrete code patterns with explanations.

Return ONLY valid JSON matching the required schema.
```

### User Prompt Template

```
Given the following detected frameworks and libraries, provide patterns and guidance 
for code analysis.

DETECTED FRAMEWORKS: {frameworks_list}
DETECTED DATA FETCHERS: {data_fetchers_list}

For each framework/library, provide:

1. MOUNT PATTERNS: How does this framework mount sub-routers or sub-applications?
   - Express: app.use('/api', router)
   - Hono: app.route('/api', subApp)
   - Fastify: fastify.register(routes, { prefix: '/api' })

2. ENDPOINT PATTERNS: How are HTTP endpoints defined?
   - Express: app.get('/users', handler)
   - Hapi: server.route({ method: 'GET', path: '/users', handler })
   - Elysia: app.get('/users', handler) or app.on('GET', '/users', handler)

3. MIDDLEWARE PATTERNS: How is middleware registered?
   - Express: app.use(cors())
   - Hono: app.use('*', middleware)
   - Fastify: fastify.addHook('onRequest', handler)

4. DATA FETCHING PATTERNS: How are outbound HTTP calls made?
   - axios: axios.get('/api/users') or axios({ method: 'get', url: '/api/users' })
   - ky: ky.get('/api/users').json()
   - got: got('/api/users').json()

5. TRIAGE HINTS: Any special notes for distinguishing between categories?
   - Example: "In Hono, .route() is for mounting sub-apps, .use() is middleware-only"
   - Example: "In Fastify, look for .register() with prefix option for mounts"

6. PARSING NOTES: Any AST/parsing considerations?
   - Example: "NestJS uses decorators - look for @Controller, @Get, @Post"
   - Example: "Elysia uses method chaining - .group().get() patterns"

Return JSON with this structure:
{
  "mount_patterns": [
    { "pattern": "app.route('/path', subApp)", "description": "Mount sub-application", "framework": "hono" }
  ],
  "endpoint_patterns": [...],
  "middleware_patterns": [...],
  "data_fetching_patterns": [...],
  "triage_hints": "Free-form guidance for the triage agent...",
  "parsing_notes": "Notes about AST structure or special syntax..."
}
```

---

## Integration Points

### 1. Where to Call the Agent

In `multi_agent_orchestrator.rs`, after `FrameworkDetector` and before `CallSiteOrchestrator`:

```rust
// Stage 0: Framework Detection
let framework_detection = framework_detector
    .detect_frameworks_and_libraries(packages, imported_symbols)
    .await?;

// Stage 0.5: Framework Guidance (NEW)
let framework_guidance = framework_guidance_agent
    .generate_guidance(&framework_detection)
    .await?;

// Stage 1: Call Site Extraction
// ...

// Stage 2: Call Site Classification (pass guidance)
let analysis_results = orchestrator
    .analyze_call_sites(&call_sites, &framework_detection, &framework_guidance)
    .await?;
```

### 2. How Downstream Agents Use Guidance

Each agent's prompt builder would incorporate the relevant patterns:

```rust
fn build_mount_prompt(
    &self,
    call_sites: &[CallSite],
    framework_detection: &DetectionResult,
    framework_guidance: &FrameworkGuidance,  // NEW
) -> String {
    let mount_examples = framework_guidance.mount_patterns
        .iter()
        .map(|p| format!("- {} -> {}", p.pattern, p.description))
        .collect::<Vec<_>>()
        .join("\n");

    format!(r#"
FRAMEWORK-SPECIFIC MOUNT PATTERNS:
{mount_examples}

SPECIAL NOTES:
{triage_hints}

ROUTER MOUNT CALL SITES:
{call_sites_json}
...
"#)
}
```

### 3. Caching Consideration

Framework guidance can be cached per project since detected frameworks don't change during analysis. This avoids redundant LLM calls:

```rust
pub struct CachedFrameworkGuidance {
    pub guidance: FrameworkGuidance,
    pub frameworks_hash: u64,  // Hash of input frameworks for cache invalidation
}
```

---

## File Structure

```
src/agents/
├── mod.rs                      # Add framework_guidance_agent module
├── framework_guidance_agent.rs # NEW: The guidance agent
├── orchestrator.rs             # Update to pass guidance
├── triage_agent.rs             # Update to use guidance
├── endpoint_agent.rs           # Update to use guidance
├── mount_agent.rs              # Update to use guidance
├── consumer_agent.rs           # Update to use guidance
└── middleware_agent.rs         # Update to use guidance
```

---

## JSON Schema for LLM Response

```json
{
  "type": "object",
  "required": ["mount_patterns", "endpoint_patterns", "middleware_patterns", 
               "data_fetching_patterns", "triage_hints", "parsing_notes"],
  "properties": {
    "mount_patterns": {
      "type": "array",
      "items": {
        "type": "object",
        "required": ["pattern", "description", "framework"],
        "properties": {
          "pattern": { "type": "string" },
          "description": { "type": "string" },
          "framework": { "type": "string" }
        }
      }
    },
    "endpoint_patterns": { "$ref": "#/properties/mount_patterns" },
    "middleware_patterns": { "$ref": "#/properties/mount_patterns" },
    "data_fetching_patterns": { "$ref": "#/properties/mount_patterns" },
    "triage_hints": { "type": "string" },
    "parsing_notes": { "type": "string" }
  }
}
```

---

## Testing Strategy

### Unit Tests

1. **Pattern extraction**: Verify guidance is generated for known frameworks
2. **Unknown frameworks**: Verify graceful handling when framework is not recognized
3. **Multiple frameworks**: Verify guidance combines patterns from all detected frameworks

### Integration Tests

1. **Hono project**: Verify mount patterns use `.route()` not `.use()`
2. **Fastify project**: Verify `.register()` with prefix is detected as mount
3. **Mixed project**: Verify Express + axios patterns are both included

### Test Cases

```rust
#[test]
fn test_hono_guidance_includes_route_pattern() {
    let input = FrameworkGuidanceInput {
        frameworks: vec!["hono".to_string()],
        data_fetchers: vec![],
    };
    
    let guidance = generate_guidance_sync(input);
    
    assert!(guidance.mount_patterns.iter().any(|p| 
        p.pattern.contains(".route(") && p.framework == "hono"
    ));
}

#[test]
fn test_fastify_guidance_includes_register_pattern() {
    let input = FrameworkGuidanceInput {
        frameworks: vec!["fastify".to_string()],
        data_fetchers: vec![],
    };
    
    let guidance = generate_guidance_sync(input);
    
    assert!(guidance.mount_patterns.iter().any(|p| 
        p.pattern.contains(".register(") && p.framework == "fastify"
    ));
}
```

---

## Implementation Steps

1. **Create `framework_guidance_agent.rs`**
   - Define `FrameworkGuidance` and `PatternExample` structs
   - Implement `FrameworkGuidanceAgent` with `generate_guidance()` method
   - Add JSON schema for structured output

2. **Update `agents/mod.rs`**
   - Export the new agent and types

3. **Update `orchestrator.rs`**
   - Accept `FrameworkGuidance` in `analyze_call_sites()`
   - Pass guidance to all downstream agents

4. **Update each downstream agent**
   - `triage_agent.rs`: Use `triage_hints` in classification prompt
   - `mount_agent.rs`: Use `mount_patterns` in extraction prompt
   - `endpoint_agent.rs`: Use `endpoint_patterns` in extraction prompt
   - `middleware_agent.rs`: Use `middleware_patterns` in extraction prompt
   - `consumer_agent.rs`: Use `data_fetching_patterns` in extraction prompt

5. **Update `multi_agent_orchestrator.rs`**
   - Call `FrameworkGuidanceAgent` after framework detection
   - Pass guidance through the pipeline

6. **Add tests**
   - Unit tests for guidance generation
   - Integration tests for end-to-end framework handling

---

## Open Questions

1. **Should we cache guidance?** 
   - Pro: Saves LLM calls for repeated runs
   - Con: Adds complexity, cache invalidation concerns

2. **Should guidance generation be optional?**
   - For Express-only projects, the default patterns work fine
   - Could skip guidance agent if only Express/Koa/Fastify detected

3. **How to handle decorator-based frameworks (NestJS)?**
   - The SWC extractor currently doesn't extract decorators
   - Guidance agent can note this, but won't fix the extraction gap
   - May need separate work to add decorator extraction to SWC

4. **How to handle file-based routing (Next.js, Remix)?**
   - This requires file path analysis, not call site analysis
   - Guidance agent can note this limitation
   - May need a separate "file router analyzer" component

---

## Prompt for Next Agent

Use this prompt to implement the Framework Guidance Agent:

```
You are implementing a new agent for the Carrick TypeScript API compatibility checker.

PROJECT CONTEXT:
- Carrick analyzes TypeScript/JavaScript codebases to extract API endpoints and data-fetching calls
- It uses a multi-agent architecture where specialized agents handle different tasks
- Currently, agents use hardcoded Express-style examples in their prompts
- We need to make the system truly framework-agnostic

YOUR TASK:
Create a new `FrameworkGuidanceAgent` that generates framework-specific patterns and guidance.

FILES TO CREATE:
1. `src/agents/framework_guidance_agent.rs` - The new agent

FILES TO MODIFY:
1. `src/agents/mod.rs` - Export the new agent
2. `src/agents/orchestrator.rs` - Pass guidance to downstream agents
3. `src/agents/triage_agent.rs` - Use triage_hints in prompt
4. `src/agents/mount_agent.rs` - Use mount_patterns in prompt
5. `src/agents/endpoint_agent.rs` - Use endpoint_patterns in prompt
6. `src/agents/middleware_agent.rs` - Use middleware_patterns in prompt
7. `src/agents/consumer_agent.rs` - Use data_fetching_patterns in prompt
8. `src/multi_agent_orchestrator.rs` - Call the new agent

REFERENCE FILES:
- Read `src/agents/triage_agent.rs` for agent structure patterns
- Read `src/framework_detector.rs` for how framework detection works
- Read `src/agents/schemas.rs` for JSON schema patterns
- Read `.thoughts/framework_guidance_agent_design.md` for full design spec

REQUIREMENTS:
1. Follow the existing agent patterns in the codebase
2. Use structured JSON output with schemas (see AgentSchemas)
3. Write tests for the new agent
4. Ensure backward compatibility - if guidance generation fails, fall back to current behavior

START BY:
1. Reading the design document at `.thoughts/framework_guidance_agent_design.md`
2. Reading existing agent implementations for patterns
3. Creating the new agent file
4. Adding tests
5. Integrating with the orchestrator
```

---

## Success Criteria

1. **Hono projects** correctly identify `.route()` as mount operations
2. **Fastify projects** correctly identify `.register()` with prefix as mounts
3. **Mixed framework projects** get combined guidance for all frameworks
4. **Unknown frameworks** don't break the system (graceful fallback)
5. **All existing tests** continue to pass
6. **New tests** cover framework-specific guidance generation