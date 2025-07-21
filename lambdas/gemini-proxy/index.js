const { GoogleGenerativeAI } = require("@google/generative-ai");

// Initialize Gemini client
const genAI = new GoogleGenerativeAI(process.env.GEMINI_API_KEY);

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

  if (totalLength > 1048576) {
    // 1MB limit
    return {
      valid: false,
      error: "Request too large (max 1MB)",
    };
  }

  // Validate model if specified
  const allowedModels = ["gemini-2.5-flash", "gemini-1.5-flash", "gemini-pro"];
  if (body.model && !allowedModels.includes(body.model)) {
    return {
      valid: false,
      error: `Invalid model. Allowed: ${allowedModels.join(", ")}`,
    };
  }

  return { valid: true };
}

// Convert Carrick message format to Gemini format
function convertMessages(messages) {
  // For code analysis, combine all messages into a single user prompt
  let combinedText = "";

  for (const msg of messages) {
    if (typeof msg === "string") {
      combinedText += msg + "\n\n";
    } else if (msg.role === "system") {
      combinedText += msg.content + "\n\n";
    } else {
      combinedText += msg.content || msg.text || "";
    }
  }

  return [
    {
      role: "user",
      parts: [{ text: combinedText.trim() }],
    },
  ];
}

// Main Lambda handler
exports.handler = async (event) => {
  const startTime = Date.now();

  console.log("Gemini proxy request:", {
    method: event.httpMethod,
    path: event.path,
    userAgent: event.headers?.["User-Agent"],
  });

  // CORS headers
  const corsHeaders = {
    "Access-Control-Allow-Origin": "*",
    "Access-Control-Allow-Headers": "Content-Type,Authorization",
    "Access-Control-Allow-Methods": "POST,OPTIONS",
    "Content-Type": "application/json",
  };

  // Handle preflight requests
  if (event.httpMethod === "OPTIONS") {
    return {
      statusCode: 200,
      headers: corsHeaders,
      body: "",
    };
  }

  // Only allow POST requests
  if (event.httpMethod !== "POST") {
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

    // Check API key authentication
    const apiKey =
      event.headers?.authorization?.replace(/^Bearer /, "") ?? null;

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

    // Prepare Gemini request
    const model = requestBody.model || "gemini-2.5-flash";
    const geminiModel = genAI.getGenerativeModel({ model });

    // Convert messages to Gemini format
    const geminiMessages = convertMessages(requestBody.messages);

    // Prepare generation config - use Gemini defaults unless specified
    const generationConfig = {
      // Match original genai configuration
      reasoningEffort: "low",
    };

    if (requestBody.options?.temperature !== undefined) {
      generationConfig.temperature = requestBody.options.temperature;
    }
    if (requestBody.options?.maxOutputTokens !== undefined) {
      generationConfig.maxOutputTokens = requestBody.options.maxOutputTokens;
    }
    if (requestBody.options?.reasoningEffort !== undefined) {
      generationConfig.reasoningEffort = requestBody.options.reasoningEffort;
    }
    // Don't set topK and topP - let Gemini use its defaults

    console.log("Calling Gemini API:", {
      model,
      messageCount: geminiMessages.length,
      dailyRemaining: limitCheck.remaining,
    });

    // Make the Gemini API call - each request is independent
    const result = await geminiModel.generateContent({
      contents: geminiMessages,
      generationConfig,
    });
    const response = await result.response;
    const text = response.text();

    const endTime = Date.now();
    const duration = endTime - startTime;

    console.log("Gemini API success:", {
      duration: `${duration}ms`,
      responseLength: text.length,
      tokensUsed: response.usageMetadata || "unknown",
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
        usage: response.usageMetadata,
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
