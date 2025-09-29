const { GoogleGenAI, Type } = require("@google/genai");

// Initialize Gemini client
const client = new GoogleGenAI({
  apiKey: process.env.GEMINI_API_KEY,
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

  // Model is hardcoded to gemini-2.5-flash for simplicity

  return { valid: true };
}

// Convert Carrick message format to Gemini format
function convertMessages(messages) {
  const convertedMessages = [];

  for (const msg of messages) {
    if (typeof msg === "string") {
      convertedMessages.push({
        role: "user",
        content: msg,
      });
    } else if (msg.role === "system") {
      // Convert system messages to user messages with prefix
      convertedMessages.push({
        role: "user",
        content: `System: ${msg.content}`,
      });
    } else {
      // Map roles to Gemini-compatible roles
      const role = msg.role === "assistant" ? "model" : "user";
      convertedMessages.push({
        role: role,
        content: msg.content || msg.text || "",
      });
    }
  }

  return convertedMessages;
}

// Main Lambda handler
exports.handler = async (event) => {
  const startTime = Date.now();

  console.log("Gemini proxy request:", {
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
    if (!process.env.GEMINI_API_KEY || !process.env.VALID_API_KEYS) {
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

    // Use hardcoded model for simplicity
    const model = "gemini-2.5-flash";

    // Convert messages to Gemini format
    const geminiMessages = convertMessages(requestBody.messages);

    // Prepare generation config - removing thinking config to avoid timeouts
    const generationConfig = {};

    if (requestBody.options?.temperature !== undefined) {
      generationConfig.temperature = requestBody.options.temperature;
    }
    // Remove custom options for simplicity - use defaults

    // Prepare config object for structured output
    const config = {
      generationConfig,
    };

    // Add structured output schema if provided
    if (requestBody.response_schema) {
      config.responseMimeType = "application/json";
      config.responseSchema = convertStringTypesToGeminiTypes(requestBody.response_schema);
    }

    console.log("Calling Gemini API:", {
      model,
      messageCount: geminiMessages.length,
      dailyRemaining: limitCheck.remaining,
      httpMethod: httpMethod,
      hasSchema: !!requestBody.response_schema,
    });

    // Make the Gemini API call using new @google/genai package
    const result = await client.models.generateContent({
      model,
      contents: geminiMessages.map((msg) => ({
        role: msg.role,
        parts: [{ text: msg.content }],
      })),
      config,
    });

    const text = result.text;

    const endTime = Date.now();
    const duration = endTime - startTime;

    console.log("Gemini API success:", {
      duration: `${duration}ms`,
      responseLength: text.length,
      tokensUsed: result.usage || "unknown",
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
        usage: result.usage,
        responseTime: duration,
      }),
    };
  } catch (error) {
    const endTime = Date.now();
    const duration = endTime - startTime;

    console.error("Gemini API error:", {
      error: error.message,
      stack: error.stack,
      duration: `${duration}ms`,
    });

    // Handle specific Gemini API errors
    let statusCode = 500;
    let errorMessage = "Internal server error";

    if (error.message?.includes("quota") || error.message?.includes("limit")) {
      statusCode = 429;
      errorMessage = "API quota exceeded. Please try again later.";
    } else if (error.message?.includes("API key")) {
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
        requestId: event.requestContext?.requestId,
      }),
    };
  }
};

// Convert string type names to Gemini Type constants
function convertStringTypesToGeminiTypes(schema) {
  if (Array.isArray(schema)) {
    return schema.map(convertStringTypesToGeminiTypes);
  }

  if (typeof schema !== 'object' || schema === null) {
    return schema;
  }

  const converted = { ...schema };

  // Convert type field from string to Type constant
  if (converted.type) {
    switch (converted.type) {
      case "ARRAY":
        converted.type = Type.ARRAY;
        break;
      case "OBJECT":
        converted.type = Type.OBJECT;
        break;
      case "STRING":
        converted.type = Type.STRING;
        break;
      case "NUMBER":
        converted.type = Type.NUMBER;
        break;
      case "BOOLEAN":
        converted.type = Type.BOOLEAN;
        break;
      // Keep as-is if already a Type constant or unknown
    }
  }

  // Recursively convert nested objects
  if (converted.items) {
    converted.items = convertStringTypesToGeminiTypes(converted.items);
  }

  if (converted.properties) {
    const newProperties = {};
    for (const [key, value] of Object.entries(converted.properties)) {
      newProperties[key] = convertStringTypesToGeminiTypes(value);
    }
    converted.properties = newProperties;
  }

  return converted;
}
