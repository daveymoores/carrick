const { handler } = require("./index");

// Mock event helper
function createMockEvent(httpMethod = "POST", body = null, headers = {}) {
  return {
    httpMethod,
    path: "/agent-proxy",
    headers: {
      "Content-Type": "application/json",
      "User-Agent": "carrick/1.0.0",
      authorization: "Bearer test-api-key",
      ...headers,
    },
    body: body ? JSON.stringify(body) : null,
    requestContext: {
      requestId: "test-request-id",
      identity: {
        sourceIp: "127.0.0.1",
      },
    },
  };
}

// Test cases
async function runTests() {
  console.log("🧪 Running Agent Proxy Lambda Tests\n");

  // Set up test environment with mock values
  process.env.AGENT_API_KEY = "mock-agent-api-key-for-testing";
  process.env.VALID_API_KEYS = "test-api-key,another-key";
  console.log("✅ Mock environment variables set for testing\n");

  // Test 1: OPTIONS request (CORS preflight)
  console.log("1. Testing CORS preflight (OPTIONS)...");
  try {
    const event = createMockEvent("OPTIONS");
    const result = await handler(event);

    console.log(`   Status: ${result.statusCode}`);
    console.log(
      `   CORS headers: ${result.headers["Access-Control-Allow-Origin"] ? "✅" : "❌"}`,
    );

    if (result.statusCode === 200) {
      console.log("   ✅ CORS preflight test passed\n");
    } else {
      console.log("   ❌ CORS preflight test failed\n");
    }
  } catch (error) {
    console.log(`   ❌ Error: ${error.message}\n`);
  }

  // Test 2: Invalid HTTP method
  console.log("2. Testing invalid HTTP method (GET)...");
  try {
    const event = createMockEvent("GET");
    const result = await handler(event);

    console.log(`   Status: ${result.statusCode}`);
    const response = JSON.parse(result.body);
    console.log(`   Error message: ${response.message}`);

    if (result.statusCode === 405) {
      console.log("   ✅ Method validation test passed\n");
    } else {
      console.log("   ❌ Method validation test failed\n");
    }
  } catch (error) {
    console.log(`   ❌ Error: ${error.message}\n`);
  }

  // Test 3: Invalid JSON body
  console.log("3. Testing invalid JSON body...");
  try {
    const event = createMockEvent("POST");
    event.body = "invalid json{";
    const result = await handler(event);

    console.log(`   Status: ${result.statusCode}`);
    const response = JSON.parse(result.body);
    console.log(`   Error message: ${response.message}`);

    if (result.statusCode === 400 && response.error === "Invalid JSON") {
      console.log("   ✅ JSON validation test passed\n");
    } else {
      console.log("   ❌ JSON validation test failed\n");
    }
  } catch (error) {
    console.log(`   ❌ Error: ${error.message}\n`);
  }

  // Test 4: Missing messages
  console.log("4. Testing missing messages...");
  try {
    const event = createMockEvent("POST", {});
    const result = await handler(event);

    console.log(`   Status: ${result.statusCode}`);
    const response = JSON.parse(result.body);
    console.log(`   Error message: ${response.message}`);

    if (result.statusCode === 400 && response.message.includes("messages")) {
      console.log("   ✅ Message validation test passed\n");
    } else {
      console.log("   ❌ Message validation test failed\n");
    }
  } catch (error) {
    console.log(`   ❌ Error: ${error.message}\n`);
  }

  // Test 5: Empty messages array
  console.log("5. Testing empty messages array...");
  try {
    const event = createMockEvent("POST", { messages: [] });
    const result = await handler(event);

    console.log(`   Status: ${result.statusCode}`);
    const response = JSON.parse(result.body);
    console.log(`   Error message: ${response.message}`);

    if (result.statusCode === 400 && response.message.includes("empty")) {
      console.log("   ✅ Empty messages test passed\n");
    } else {
      console.log("   ❌ Empty messages test failed\n");
    }
  } catch (error) {
    console.log(`   ❌ Error: ${error.message}\n`);
  }

  // Test 6: Too many messages
  console.log("6. Testing too many messages...");
  try {
    const manyMessages = Array(15).fill({ role: "user", content: "test" });
    const event = createMockEvent("POST", { messages: manyMessages });
    const result = await handler(event);

    console.log(`   Status: ${result.statusCode}`);
    const response = JSON.parse(result.body);
    console.log(`   Error message: ${response.message}`);

    if (result.statusCode === 400 && response.message.includes("Too many")) {
      console.log("   ✅ Message limit test passed\n");
    } else {
      console.log("   ❌ Message limit test failed\n");
    }
  } catch (error) {
    console.log(`   ❌ Error: ${error.message}\n`);
  }

  // Test 7: Request too large
  console.log("7. Testing request size limit...");
  try {
    const largeContent = "x".repeat(1.5 * 1024 * 1024); // 1.5MB
    const event = createMockEvent("POST", {
      messages: [{ role: "user", content: largeContent }],
    });
    const result = await handler(event);

    console.log(`   Status: ${result.statusCode}`);
    const response = JSON.parse(result.body);
    console.log(`   Error message: ${response.message}`);

    if (result.statusCode === 400 && response.message.includes("too large")) {
      console.log("   ✅ Size limit test passed\n");
    } else {
      console.log("   ❌ Size limit test failed\n");
    }
  } catch (error) {
    console.log(`   ❌ Error: ${error.message}\n`);
  }

  // Test 8 removed - model is hardcoded in the Lambda, no validation needed

  // Test 8: Authentication failure
  console.log("8. Testing authentication failure...");
  try {
    const event = createMockEvent(
      "POST",
      {
        messages: [{ role: "user", content: "test" }],
      },
      { authorization: "" },
    ); // Remove auth header

    const result = await handler(event);
    console.log(`   Status: ${result.statusCode}`);

    if (result.statusCode === 401) {
      console.log("   ✅ Authentication test passed\n");
    } else {
      console.log("   ❌ Authentication test failed\n");
    }
  } catch (error) {
    console.log(`   ❌ Error: ${error.message}\n`);
  }

  // Test 9: Valid request structure
  console.log("9. Testing valid request structure...");
  try {
    const event = createMockEvent("POST", {
      messages: [
        { role: "system", content: "You are a helpful assistant." },
        { role: "user", content: "Say hello" },
      ],
      options: {
        temperature: 0.7,
        maxOutputTokens: 100,
        reasoningEffort: "low",
      },
    });

    const result = await handler(event);
    console.log(`   Status: ${result.statusCode}`);

    if (result.statusCode === 401) {
      console.log(
        "   ⚠️  Expected 401 (Mock AGENT_API_KEY used) - structure test passed",
      );
    } else if (result.statusCode === 200) {
      const response = JSON.parse(result.body);
      console.log(
        `   Success! Response length: ${response.text?.length || 0} chars`,
      );
      console.log(`   ✅ Full integration test passed`);
    } else {
      const response = JSON.parse(result.body);
      console.log(`   Response: ${response.message}`);
    }
    console.log("");
  } catch (error) {
    console.log(`   ❌ Error: ${error.message}\n`);
  }

  console.log("🏁 Test suite completed!");
  console.log("\n📝 Notes:");
  console.log(
    "   - Set AGENT_API_KEY environment variable for full integration testing",
  );
  console.log("   - Tests use 'test-api-key' for authentication");
  console.log("   - Deploy to AWS Lambda for production testing");
}

// Run tests if this file is executed directly
if (require.main === module) {
  runTests().catch(console.error);
}

module.exports = { createMockEvent, runTests };
