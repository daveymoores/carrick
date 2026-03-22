const { GoogleGenerativeAI, SchemaType } = require("@google/generative-ai");

// Initialize Google Generative AI client
const genAI = new GoogleGenerativeAI(process.env.AGENT_API_KEY);

// Simple daily usage tracking
let dailyUsage = {
  count: 0,
  date: new Date().toISOString().split("T")[0], // YYYY-MM-DD format
};

// Simple configuration - just protect against huge bills
const DAILY_LIMIT = 2000;

function normalizeThinkingLevel(value) {
  if (typeof value !== "string") {
    return null;
  }

  const normalized = value.trim().toLowerCase();
  if (["minimal", "low", "medium", "high"].includes(normalized)) {
    return normalized;
  }

  return null;
}

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

// Convert Carrick message format to Gemini format
function convertMessages(messages) {
  const contents = [];
  let systemInstruction = null;

  for (const msg of messages) {
    if (typeof msg === "string") {
      contents.push({
        role: "user",
        parts: [{ text: msg }],
      });
    } else if (msg.role === "system") {
      // Gemini uses systemInstruction parameter
      if (Array.isArray(msg.content)) {
        // Handle content blocks format
        systemInstruction = msg.content.map((c) => c.text || c).join("\n");
      } else {
        systemInstruction = msg.content;
      }
    } else {
      // Map roles - Gemini uses "model" not "assistant"
      const role = msg.role === "assistant" ? "model" : msg.role;

      // Handle content
      let text = "";
      if (Array.isArray(msg.content)) {
        // Handle content blocks format
        text = msg.content.map((c) => c.text || c).join("\n");
      } else {
        text = msg.content || msg.text || "";
      }

      contents.push({
        role: role,
        parts: [{ text: text }],
      });
    }
  }

  return { contents, systemInstruction };
}

// Convert Carrick schema format to Gemini JSON schema format
function convertSchemaToGemini(schema) {
  if (!schema) return null;

  // Recursively convert schema to Gemini format
  function normalizeSchema(obj) {
    if (Array.isArray(obj)) {
      return obj.map(normalizeSchema);
    }

    if (typeof obj !== "object" || obj === null) {
      return obj;
    }

    const normalized = { ...obj };

    // Convert type strings to Gemini SchemaType format
    if (normalized.type && typeof normalized.type === "string") {
      const typeMap = {
        string: SchemaType.STRING,
        number: SchemaType.NUMBER,
        integer: SchemaType.INTEGER,
        boolean: SchemaType.BOOLEAN,
        array: SchemaType.ARRAY,
        object: SchemaType.OBJECT,
        STRING: SchemaType.STRING,
        NUMBER: SchemaType.NUMBER,
        INTEGER: SchemaType.INTEGER,
        BOOLEAN: SchemaType.BOOLEAN,
        ARRAY: SchemaType.ARRAY,
        OBJECT: SchemaType.OBJECT,
      };
      normalized.type = typeMap[normalized.type] || normalized.type;
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

// Retry wrapper for Gemini API calls with exponential backoff
async function callGeminiWithRetry(model, contents, generationConfig, maxRetries = 3) {
  for (let attempt = 1; attempt <= maxRetries; attempt++) {
    try {
      return await model.generateContent({
        contents: contents,
        generationConfig: generationConfig,
      });
    } catch (error) {
      console.log(`Gemini API attempt ${attempt}/${maxRetries} failed:`, {
        message: error.message,
        status: error.status,
        code: error.code,
        details: error.errorDetails || error.details,
      });

      const isRetryable =
        error.message?.includes("overloaded") ||
        error.message?.includes("503") ||
        error.message?.includes("RESOURCE_EXHAUSTED");

      if (isRetryable && attempt < maxRetries) {
        const waitTime = Math.pow(2, attempt) * 1000; // 2s, 4s
        console.log(`Retrying in ${waitTime}ms...`);
        await new Promise(resolve => setTimeout(resolve, waitTime));
        continue;
      }
      throw error;
    }
  }
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

    // Use Gemini 3 Flash preview model
    const modelName = "gemini-3.1-flash-lite-preview";

    // Convert messages to Gemini format
    const { contents, systemInstruction } = convertMessages(
      requestBody.messages
    );

    // Log prompt metrics for monitoring
    const totalPromptSize = (systemInstruction?.length || 0) +
      contents.reduce((sum, c) => sum + (c.parts?.[0]?.text?.length || 0), 0);

    // Get schema info for logging
    const schemaInfo = requestBody.response_schema ? {
      topLevelType: requestBody.response_schema.type,
      propertyCount: requestBody.response_schema.properties
        ? Object.keys(requestBody.response_schema.properties).length
        : 0,
      schemaSize: JSON.stringify(requestBody.response_schema).length,
    } : null;

    console.log("Prompt metrics:", {
      totalKB: Math.round(totalPromptSize / 1024),
      systemInstructionKB: Math.round((systemInstruction?.length || 0) / 1024),
      messageCount: contents.length,
      hasSchema: !!requestBody.response_schema,
      schemaInfo,
    });

    // Prepare model configuration
    const modelConfig = {};

    // Add system instruction if present
    if (systemInstruction) {
      modelConfig.systemInstruction = systemInstruction;
    }

    // Add structured output schema if provided
    if (requestBody.response_schema) {
      const normalizedSchema = convertSchemaToGemini(
        requestBody.response_schema
      );
      modelConfig.generationConfig = {
        responseMimeType: "application/json",
        responseSchema: normalizedSchema,
      };
    }

    // Get the model
    const model = genAI.getGenerativeModel({
      model: modelName,
      ...modelConfig,
    });

    // Prepare generation config
    const generationConfig = {
      maxOutputTokens: 8192,
      ...(modelConfig.generationConfig || {}),
    };

    const requestedThinkingLevel = normalizeThinkingLevel(
      requestBody.options?.thinkingLevel ?? requestBody.options?.reasoningEffort,
    );

    // Only enable thinking if explicitly requested by caller
    if (requestedThinkingLevel) {
      generationConfig.thinkingConfig = {
        thinkingLevel: requestedThinkingLevel,
      };
    }

    // Add temperature if specified
    if (requestBody.options?.temperature != null) {
      generationConfig.temperature = requestBody.options.temperature;
    }

    console.log("Calling Gemini API:", {
      model: modelName,
      messageCount: contents.length,
      dailyRemaining: limitCheck.remaining,
      httpMethod: httpMethod,
      hasSchema: !!requestBody.response_schema,
      hasSystem: !!systemInstruction,
      thinkingLevel: generationConfig.thinkingConfig?.thinkingLevel ?? "none",
    });

    // Make the Gemini API call with retry logic
    const result = await callGeminiWithRetry(model, contents, generationConfig);

    const response = result.response;
    const text = response.text();

    const endTime = Date.now();
    const duration = endTime - startTime;

    // Get usage metadata if available
    const usageMetadata = response.usageMetadata;

    console.log("Gemini API success:", {
      duration: `${duration}ms`,
      responseLength: text.length,
      tokensUsed: usageMetadata || "unknown",
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
        usage: usageMetadata
          ? {
            promptTokens: usageMetadata.promptTokenCount,
            candidatesTokens: usageMetadata.candidatesTokenCount,
            totalTokens: usageMetadata.totalTokenCount,
          }
          : null,
        responseTime: duration,
      }),
    };
  } catch (error) {
    const endTime = Date.now();
    const duration = endTime - startTime;

    console.error("Gemini API error:", {
      error: error.message,
      errorName: error.name,
      errorCode: error.code,
      errorDetails: JSON.stringify(error, null, 2),
      stack: error.stack,
      duration: `${duration}ms`,
    });

    // Handle specific Gemini API errors
    let statusCode = 500;
    let errorMessage = "Internal server error";
    let debugInfo = error.message;

    if (error.message?.includes("quota") || error.message?.includes("limit")) {
      statusCode = 429;
      errorMessage = "API quota exceeded. Please try again later.";
    } else if (
      error.message?.includes("API key") ||
      error.message?.includes("authentication")
    ) {
      statusCode = 401;
      errorMessage = "API authentication failed";
    } else if (
      error.message?.includes("content") ||
      error.message?.includes("safety") ||
      error.message?.includes("blocked")
    ) {
      statusCode = 400;
      errorMessage = "Content was blocked by safety filters";
    } else if (
      error.message?.includes("overloaded") ||
      error.message?.includes("503") ||
      error.message?.includes("RESOURCE_EXHAUSTED")
    ) {
      statusCode = 503;
      errorMessage = "Model is overloaded. Retries exhausted.";
    } else if (error.message?.includes("not found") || error.status === 404) {
      statusCode = 503;
      errorMessage = "Model not available. Please try again later.";
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
