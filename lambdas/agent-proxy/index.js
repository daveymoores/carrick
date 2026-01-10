const Anthropic = require("@anthropic-ai/sdk");

// Initialize Anthropic client
const client = new Anthropic({
  apiKey: process.env.AGENT_API_KEY,
});

// VALID_API_KEYS will be checked in handler

// Simple daily usage tracking
let dailyUsage = {
  count: 0,
  date: new Date().toISOString().split("T")[0], // YYYY-MM-DD format
};

// Simple configuration - just protect against huge bills
const DAILY_LIMIT = 2000;

// Helper function to get client identifier
function getClientId(event) {
  // Use IP address as primary identifier
  const ip = event.requestContext?.identity?.sourceIp || "unknown";

  // Add user agent for additional uniqueness
  const userAgent = event.headers?.["User-Agent"] || "";
  const carrickVersion =
    userAgent.match(/carrick\/([0-9.]+)/)?.[1] || "unknown";

  return `${ip}:${carrickVersion}`;
}

// Simple daily limit check
function checkDailyLimit() {
  const today = new Date().toISOString().split("T")[0];

  // Reset counter if it's a new day
  if (dailyUsage.date !== today) {
    dailyUsage.count = 0;
    dailyUsage.date = today;
  }

  // Check if we've hit the daily limit
  if (dailyUsage.count >= DAILY_LIMIT) {
    const tomorrow = new Date();
    tomorrow.setDate(tomorrow.getDate() + 1);
    tomorrow.setHours(0, 0, 0, 0);

    return {
      allowed: false,
      resetTime: tomorrow.getTime(),
      message: `Daily limit of ${DAILY_LIMIT} requests exceeded. Resets at midnight UTC.`,
      remaining: 0,
    };
  }

  // Increment counter and allow request
  dailyUsage.count++;

  return {
    allowed: true,
    remaining: DAILY_LIMIT - dailyUsage.count,
  };
}

// Input validation
function validateRequest(body) {
  if (!body.messages || !Array.isArray(body.messages)) {
    return { valid: false, error: "Missing or invalid messages array" };
  }

  if (body.messages.length === 0) {
    return { valid: false, error: "Messages array cannot be empty" };
  }

  if (body.messages.length > 10) {
    return { valid: false, error: "Too many messages (max 10)" };
  }

  // Check message content length
  const totalLength = body.messages.reduce((sum, msg) => {
    return sum + (msg.content || "").length;
  }, 0);

  if (totalLength > 5242880) {
    // 5MB limit - allows for large codebases with many async functions
    return {
      valid: false,
      error: "Request too large (max 5MB)",
    };
  }

  // Validate response_schema if provided
  if (body.response_schema && typeof body.response_schema !== "object") {
    return { valid: false, error: "response_schema must be an object" };
  }

  return { valid: true };
}

// Convert Carrick message format to Anthropic format
// Supports cache_control for prompt caching
function convertMessages(messages) {
  const convertedMessages = [];
  let systemMessage = null;

  for (const msg of messages) {
    if (typeof msg === "string") {
      convertedMessages.push({
        role: "user",
        content: msg,
      });
    } else if (msg.role === "system") {
      // Anthropic uses a separate system parameter
      // Check if content has cache_control (array of content blocks)
      if (Array.isArray(msg.content)) {
        systemMessage = msg.content;
      } else {
        systemMessage = msg.content;
      }
    } else {
      // Map roles - Anthropic uses "assistant" not "model"
      const role = msg.role === "model" ? "assistant" : msg.role;

      // Check if content is an array of content blocks (for cache_control support)
      if (Array.isArray(msg.content)) {
        convertedMessages.push({
          role: role,
          content: msg.content,
        });
      } else {
        convertedMessages.push({
          role: role,
          content: msg.content || msg.text || "",
        });
      }
    }
  }

  return { messages: convertedMessages, system: systemMessage };
}

// Convert Carrick schema format to Anthropic JSON schema format
function convertSchemaToAnthropic(schema) {
  if (!schema) return null;

  // Recursively convert type strings to lowercase as Anthropic expects
  function normalizeSchema(obj) {
    if (Array.isArray(obj)) {
      return obj.map(normalizeSchema);
    }

    if (typeof obj !== "object" || obj === null) {
      return obj;
    }

    const normalized = { ...obj };

    // Convert TYPE constants (ARRAY, OBJECT, STRING, etc.) to lowercase
    if (normalized.type && typeof normalized.type === "string") {
      normalized.type = normalized.type.toLowerCase();
    }

    // Claude requires additionalProperties: false for all object types
    if (normalized.type === "object" && normalized.properties) {
      normalized.additionalProperties = false;
    }

    // Claude doesn't support minimum/maximum for number types
    if (normalized.type === "number" || normalized.type === "integer") {
      delete normalized.minimum;
      delete normalized.maximum;
    }

    // Recursively normalize nested objects
    if (normalized.items) {
      normalized.items = normalizeSchema(normalized.items);
    }

    if (normalized.properties) {
      const newProperties = {};
      for (const [key, value] of Object.entries(normalized.properties)) {
        newProperties[key] = normalizeSchema(value);
      }
      normalized.properties = newProperties;
    }

    return normalized;
  }

  return normalizeSchema(schema);
}

// Main Lambda handler
exports.handler = async (event) => {
  const startTime = Date.now();

  console.log("Agent proxy request:", {
    method: event.requestContext?.http?.method || event.httpMethod,
    path: event.requestContext?.http?.path || event.path,
    userAgent: event.headers?.["User-Agent"] || event.headers?.["user-agent"],
  });

  // CORS headers
  const corsHeaders = {
    "Access-Control-Allow-Origin": "*",
    "Access-Control-Allow-Headers": "Content-Type,Authorization",
    "Access-Control-Allow-Methods": "POST,OPTIONS",
    "Content-Type": "application/json",
  };

  // Get HTTP method (API Gateway v2 vs v1 compatibility)
  const httpMethod = event.requestContext?.http?.method || event.httpMethod;

  // Handle preflight requests
  if (httpMethod === "OPTIONS") {
    return {
      statusCode: 200,
      headers: corsHeaders,
      body: "",
    };
  }

  // Only allow POST requests
  if (httpMethod !== "POST") {
    return {
      statusCode: 405,
      headers: corsHeaders,
      body: JSON.stringify({
        error: "Method not allowed",
        message: "Only POST requests are supported",
      }),
    };
  }

  try {
    // Check required environment variables
    if (!process.env.AGENT_API_KEY || !process.env.VALID_API_KEYS) {
      return {
        statusCode: 500,
        headers: corsHeaders,
        body: JSON.stringify({
          error: "Server configuration error",
          message: "Missing required environment variables",
        }),
      };
    }

    const VALID_API_KEYS =
      process.env.VALID_API_KEYS.split(",").filter(Boolean);

    // Check API key authentication (API Gateway v2 vs v1 compatibility)
    const authHeader =
      event.headers?.authorization || event.headers?.Authorization;
    const apiKey = authHeader?.replace(/^Bearer /, "") ?? null;

    if (!apiKey || !VALID_API_KEYS.includes(apiKey)) {
      return {
        statusCode: 401,
        headers: corsHeaders,
        body: JSON.stringify({
          error: "Authentication failed",
          message: "Valid API key required in Authorization header",
        }),
      };
    }

    // Parse request body
    let requestBody;
    try {
      requestBody = JSON.parse(event.body || "{}");
    } catch (parseError) {
      return {
        statusCode: 400,
        headers: corsHeaders,
        body: JSON.stringify({
          error: "Invalid JSON",
          message: "Request body must be valid JSON",
        }),
      };
    }

    // Validate request
    const validation = validateRequest(requestBody);
    if (!validation.valid) {
      return {
        statusCode: 400,
        headers: corsHeaders,
        body: JSON.stringify({
          error: "Validation failed",
          message: validation.error,
        }),
      };
    }

    // Check daily limit
    const limitCheck = checkDailyLimit();

    if (!limitCheck.allowed) {
      const resetIn = Math.ceil((limitCheck.resetTime - Date.now()) / 1000);
      return {
        statusCode: 429,
        headers: {
          ...corsHeaders,
          "Retry-After": resetIn.toString(),
          "X-Daily-Limit": DAILY_LIMIT.toString(),
          "X-Daily-Remaining": "0",
        },
        body: JSON.stringify({
          error: "Daily limit exceeded",
          message: limitCheck.message,
          resetTime: limitCheck.resetTime,
        }),
      };
    }

    // Use Claude Haiku 4.5 model
    const model = "claude-haiku-4-5";

    // Convert messages to Anthropic format
    const { messages: agentMessages, system: systemMessage } = convertMessages(
      requestBody.messages
    );

    // Prepare API call parameters
    const apiParams = {
      model: model,
      max_tokens: 4096,
      messages: agentMessages,
    };

    // Add system message if present
    if (systemMessage) {
      apiParams.system = systemMessage;
    }

    // Add temperature if specified (must be a valid number, not null)
    if (requestBody.options?.temperature != null) {
      apiParams.temperature = requestBody.options.temperature;
    }

    // Add structured output schema if provided
    if (requestBody.response_schema) {
      const normalizedSchema = convertSchemaToAnthropic(
        requestBody.response_schema
      );
      apiParams.output_format = {
        type: "json_schema",
        schema: normalizedSchema,
      };
    }

    // Check if any messages have cache_control for logging
    const hasCacheControl = agentMessages.some(
      (m) => Array.isArray(m.content) && m.content.some((c) => c.cache_control)
    ) || (Array.isArray(systemMessage) && systemMessage.some((c) => c.cache_control));

    console.log("Calling Agent API:", {
      model,
      messageCount: agentMessages.length,
      dailyRemaining: limitCheck.remaining,
      httpMethod: httpMethod,
      hasSchema: !!requestBody.response_schema,
      hasSystem: !!systemMessage,
      hasCacheControl: hasCacheControl,
    });

    // Make the Agent API call using Anthropic SDK with structured outputs
    const response = await client.beta.messages.create({
      ...apiParams,
      betas: ["structured-outputs-2025-11-13"],
    });

    // Extract text from response
    const text = response.content[0].text;

    const endTime = Date.now();
    const duration = endTime - startTime;

    // Log cache statistics if available
    const cacheStats = response.usage?.cache_creation_input_tokens !== undefined
      ? {
        cache_creation_input_tokens: response.usage.cache_creation_input_tokens,
        cache_read_input_tokens: response.usage.cache_read_input_tokens,
      }
      : null;

    console.log("Agent API success:", {
      duration: `${duration}ms`,
      responseLength: text.length,
      tokensUsed: response.usage || "unknown",
      cacheStats: cacheStats,
    });

    // Return successful response
    return {
      statusCode: 200,
      headers: {
        ...corsHeaders,
        "X-Response-Time": `${duration}ms`,
        "X-Daily-Remaining": limitCheck.remaining.toString(),
        "X-Daily-Limit": DAILY_LIMIT.toString(),
      },
      body: JSON.stringify({
        success: true,
        text: text,
        usage: response.usage,
        responseTime: duration,
      }),
    };
  } catch (error) {
    const endTime = Date.now();
    const duration = endTime - startTime;

    console.error("Agent API error:", {
      error: error.message,
      errorName: error.name,
      errorCode: error.code,
      errorDetails: JSON.stringify(error, null, 2),
      stack: error.stack,
      duration: `${duration}ms`,
    });

    // Handle specific Agent API errors
    let statusCode = 500;
    let errorMessage = "Internal server error";
    let debugInfo = error.message;

    if (error.message?.includes("quota") || error.message?.includes("limit")) {
      statusCode = 429;
      errorMessage = "API quota exceeded. Please try again later.";
    } else if (error.message?.includes("API key") || error.message?.includes("authentication")) {
      statusCode = 401;
      errorMessage = "API authentication failed";
    } else if (
      error.message?.includes("content") ||
      error.message?.includes("safety")
    ) {
      statusCode = 400;
      errorMessage = "Content was blocked by safety filters";
    }

    return {
      statusCode,
      headers: corsHeaders,
      body: JSON.stringify({
        error: "API Error",
        message: errorMessage,
        debugInfo: debugInfo,
        requestId: event.requestContext?.requestId,
      }),
    };
  }
};
