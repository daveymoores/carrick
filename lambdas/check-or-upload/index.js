const { DynamoDBClient } = require("@aws-sdk/client-dynamodb");
const {
  DynamoDBDocumentClient,
  GetCommand,
  ScanCommand,
  PutCommand, // Add this
} = require("@aws-sdk/lib-dynamodb");
const {
  S3Client,
  HeadObjectCommand,
  PutObjectCommand,
  GetObjectCommand,
} = require("@aws-sdk/client-s3");
const { getSignedUrl } = require("@aws-sdk/s3-request-presigner");

const dynamoClient = new DynamoDBClient();
const docClient = DynamoDBDocumentClient.from(dynamoClient);
const s3Client = new S3Client();

if (
  !process.env.S3_BUCKET ||
  !process.env.DYNAMODB_TABLE ||
  !process.env.VALID_API_KEYS
) {
  throw new Error("Missing one or more required environment variables");
}

const BUCKET_NAME = process.env.S3_BUCKET;
const TABLE_NAME = process.env.DYNAMODB_TABLE;
const VALID_API_KEYS = process.env.VALID_API_KEYS.split(",").filter(Boolean);

exports.handler = async (event) => {
  console.log("Event:", JSON.stringify(event, null, 2));

  const apiKey = event.headers?.authorization?.replace(/^Bearer /, "") ?? null;

  let body;
  try {
    body = JSON.parse(event.body || "{}");
  } catch (err) {
    return response(400, { error: "Invalid JSON in request body" });
  }

  if (!apiKey || !VALID_API_KEYS.includes(apiKey)) {
    return response(401, { error: "Invalid API key" });
  }

  const { action, ...payload } = body;

  switch (action) {
    case "check-or-upload":
      return await handleCheckOrUpload(payload);
    case "store-metadata":
      return await handleStoreMetadata(payload, apiKey);
    case "complete-upload":
      return await handleCompleteUpload(payload, apiKey);
    case "get-cross-repo-data":
      return await handleGetCrossRepoData(payload, apiKey);
    case "download-file":
      return await handleDownloadFile(payload, apiKey);
    default:
      // Default behavior - backward compatibility
      return await handleCheckOrUpload(body);
  }
};

// S3 URL validation regex (bucket-specific)
const EXPECTED_S3_URL_PATTERN = new RegExp(
  `^https://${BUCKET_NAME}\\.s3\\.amazonaws\\.com/(.+)$`,
);

// Utility: Build S3 key and URL from org/repo/hash/filename
function buildS3Key(org, repo, hash, filename) {
  return `${org}/${repo}/${hash}/${filename}`;
}
function buildS3Url(org, repo, hash, filename) {
  return `https://${BUCKET_NAME}.s3.amazonaws.com/${buildS3Key(org, repo, hash, filename)}`;
}

// Utility: Get expected S3 key for validation
function getExpectedS3Key(org, repo, hash, filename) {
  return buildS3Key(org, repo, hash, filename);
}

// Utility function to create a standardized DynamoDB item
function createDynamoDbItem({
  org,
  repo,
  hash,
  apiKey,
  s3Url,
  filename,
  cloudRepoData,
  createdAt,
  updatedAt,
}) {
  const pk = `repo#${org}/${repo}`;
  const sk = `types`;
  return {
    pk,
    sk,
    org,
    repo,
    hash,
    apiKey,
    s3Url,
    filename,
    cloudRepoData,
    createdAt,
    updatedAt,
    ttl: Math.floor(Date.now() / 1000) + 30 * 24 * 60 * 60, // 30 days TTL
  };
}

async function handleCheckOrUpload(payload) {
  const { repo, org, hash, filename } = payload;

  if (!repo || !org || !hash || !filename) {
    return response(400, {
      error: "Missing required fields: repo, org, hash, filename",
    });
  }

  const pk = `repo#${org}/${repo}`;
  const sk = `types`;
  const s3Key = buildS3Key(org, repo, hash, filename);
  let exists = false;
  let needsUpload = false;
  let s3Url = buildS3Url(org, repo, hash, filename);

  // Check if record exists for this repo
  try {
    const result = await docClient.send(
      new GetCommand({ TableName: TABLE_NAME, Key: { pk, sk } }),
    );
    if (result.Item) {
      exists = true;
      // Check if the hash has changed
      if (result.Item.hash !== hash) {
        needsUpload = true;
        // Update S3 URL for new hash
        s3Url = buildS3Url(org, repo, hash, filename);
      } else {
        // Same hash, file should exist
        s3Url = result.Item.s3Url;
      }
    } else {
      needsUpload = true;
    }
  } catch (err) {
    console.warn("Error checking DynamoDB:", err);
    needsUpload = true;
  }

  // If record doesn't exist or hash changed, check S3 and prepare upload URL
  if (!exists || needsUpload) {
    try {
      await s3Client.send(
        new HeadObjectCommand({ Bucket: BUCKET_NAME, Key: s3Key }),
      );
      // File exists in S3 but record is missing/outdated
      needsUpload = false;
    } catch (err) {
      if (err.name !== "NotFound") throw err;
      // File doesn't exist in S3, need to upload
      needsUpload = true;
    }
  }

  let uploadUrl = null;
  if (needsUpload) {
    uploadUrl = await getSignedUrl(
      s3Client,
      new PutObjectCommand({ Bucket: BUCKET_NAME, Key: s3Key }),
      { expiresIn: 60 * 5 },
    );
  }

  // Always return adjacent metadata for cross-repo analysis
  let adjacent = [];
  try {
    const adjacentResults = await docClient.send(
      new ScanCommand({
        TableName: TABLE_NAME,
        FilterExpression: "begins_with(pk, :orgPrefix)",
        ExpressionAttributeValues: {
          ":orgPrefix": `repo#${org}/`,
        },
      }),
    );

    adjacent = (adjacentResults.Items || [])
      .filter((item) => item.pk !== pk)
      .map((item) => {
        const [, repoPart] = item.pk.split("/");
        return {
          repo: repoPart,
          hash: item.hash,
          s3Url: item.s3Url,
          filename: item.filename,
          // Include the full CloudRepoData if it exists
          metadata: item.cloudRepoData || null,
        };
      });
  } catch (err) {
    console.warn("Could not fetch adjacent metadata:", err);
  }

  return response(200, {
    exists: exists && !needsUpload,
    s3Url,
    uploadUrl,
    hash,
    adjacent,
  });
}

async function handleStoreMetadata(payload, apiKey) {
  const { repo, org, hash, filename, cloudRepoData } = payload;

  if (!repo || !org || !hash || !filename || !cloudRepoData) {
    return response(400, {
      error:
        "Missing required fields: repo, org, hash, filename, cloudRepoData",
    });
  }

  // Build the s3Url consistently
  const s3Url = buildS3Url(org, repo, hash, filename);
  const now = new Date().toISOString();

  // Use the utility function to create a complete and standardized item
  const item = createDynamoDbItem({
    org,
    repo,
    hash,
    apiKey,
    s3Url,
    filename,
    cloudRepoData,
    createdAt: now, // Use createdAt and updatedAt for consistency
    updatedAt: now,
  });

  try {
    await docClient.send(
      new PutCommand({
        TableName: TABLE_NAME,
        Item: item, // Use the complete item from the utility function
      }),
    );

    return response(200, {
      success: true,
      message: "Metadata stored successfully",
    });
  } catch (err) {
    console.error("Error storing metadata:", err);
    return response(500, {
      error: "Failed to store metadata",
    });
  }
}

async function handleGetCrossRepoData(payload, apiKey) {
  const { org } = payload;

  if (!org) {
    return response(400, { error: "Missing required field: org" });
  }

  try {
    // Handle DynamoDB pagination to get all repos
    let allItems = [];
    let lastEvaluatedKey = undefined;
    let scanCount = 0;

    do {
      scanCount++;
      const scanParams = {
        TableName: TABLE_NAME,
        FilterExpression: "begins_with(pk, :orgPrefix)",
        ExpressionAttributeValues: {
          ":orgPrefix": `repo#${org}/`,
        },
      };

      if (lastEvaluatedKey) {
        scanParams.ExclusiveStartKey = lastEvaluatedKey;
      }

      const results = await docClient.send(new ScanCommand(scanParams));

      if (results.Items && results.Items.length > 0) {
        allItems = allItems.concat(results.Items);
      }

      lastEvaluatedKey = results.LastEvaluatedKey;
    } while (lastEvaluatedKey);

    console.log(
      `Cross-repo scan: Found ${allItems.length} repos using ${scanCount} scans`,
    );

    const repos = [];
    const errors = [];

    for (let i = 0; i < allItems.length; i++) {
      const item = allItems[i];
      try {
        const repoData = {
          repo: item.pk.split("/")[1],
          hash: item.hash,
          s3Url: item.s3Url,
          filename: item.filename,
          metadata: item.cloudRepoData,
          lastUpdated: item.lastUpdated,
        };

        repos.push(repoData);
      } catch (itemErr) {
        console.error(`Error processing repo ${item.repo}:`, itemErr.message);
        errors.push({
          repo: item.repo,
          error: itemErr.message,
          pk: item.pk,
        });
      }
    }

    if (errors.length > 0) {
      console.error(`Processing errors for ${errors.length} repos:`, errors);
    }

    const responseBody = { repos };
    if (errors.length > 0) {
      responseBody.processing_errors = errors;
    }

    return response(200, responseBody);
  } catch (err) {
    console.error("Error fetching cross-repo data:", err);
    return response(500, {
      error: "Failed to fetch cross-repo data",
      debug: err.message,
    });
  }
}

async function handleCompleteUpload(payload, apiKey) {
  const { repo, org, hash, s3Url, filename, cloudRepoData } = payload;

  if (!repo || !org || !hash || !s3Url || !filename || !cloudRepoData) {
    return response(400, {
      error:
        "Missing required fields: repo, org, hash, s3Url, filename, cloudRepoData",
    });
  }

  // Validate s3Url format and key
  const match = s3Url.match(EXPECTED_S3_URL_PATTERN);

  if (!match) {
    return response(400, { error: "Invalid s3Url format" });
  }

  const s3Key = match[1];
  const expectedKey = getExpectedS3Key(org, repo, hash, filename);

  if (s3Key !== expectedKey) {
    return response(400, {
      error: "S3 URL does not match expected pattern",
      expected: expectedKey,
      actual: s3Key,
    });
  }

  // Verify file exists in S3
  try {
    await s3Client.send(
      new HeadObjectCommand({
        Bucket: BUCKET_NAME,
        Key: s3Key,
      }),
    );
  } catch (error) {
    if (error.name === "NotFound" || error.$metadata?.httpStatusCode === 404) {
      return response(400, {
        error: "File not found in S3. Upload may have failed.",
      });
    }
    throw error;
  }

  const now = new Date().toISOString();

  const item = createDynamoDbItem({
    org,
    repo,
    hash,
    apiKey,
    s3Url,
    filename,
    cloudRepoData,
    createdAt: now,
    updatedAt: now,
  });

  try {
    await docClient.send(
      new PutCommand({
        TableName: TABLE_NAME,
        Item: item,
      }),
    );

    return response(200, {
      success: true,
      message: "Upload and metadata storage completed successfully",
      s3Url,
      metadata: {
        pk: item.pk,
        sk: item.sk,
        hasCloudRepoData: true,
        createdAt: now,
      },
    });
  } catch (error) {
    console.error("Error storing metadata:", error);
    return response(500, { error: "Failed to store metadata" });
  }
}

async function handleDownloadFile(payload, apiKey) {
  const { s3Url } = payload;

  if (!s3Url) {
    return response(400, { error: "Missing required field: s3Url" });
  }

  // Validate s3Url format and extract key
  const match = s3Url.match(EXPECTED_S3_URL_PATTERN);

  if (!match) {
    return response(400, { error: "Invalid s3Url format" });
  }

  const s3Key = match[1];

  try {
    // Use S3 GetObject to download the file content
    const getObjectResponse = await s3Client.send(
      new GetObjectCommand({
        Bucket: BUCKET_NAME,
        Key: s3Key,
      }),
    );

    // Convert the stream to string
    const content = await getObjectResponse.Body.transformToString();

    return response(200, { content });
  } catch (error) {
    console.error("Error downloading file from S3:", error);
    if (error.name === "NoSuchKey" || error.$metadata?.httpStatusCode === 404) {
      return response(404, { error: "File not found" });
    }
    return response(500, { error: "Failed to download file" });
  }
}

function response(statusCode, body) {
  try {
    const responseBody = JSON.stringify(body);
    return {
      statusCode,
      headers: {
        "Content-Type": "application/json",
        "Access-Control-Allow-Origin": "*",
      },
      body: responseBody,
    };
  } catch (err) {
    console.error("Response serialization failed:", err.message);
    return {
      statusCode: 500,
      headers: {
        "Content-Type": "application/json",
        "Access-Control-Allow-Origin": "*",
      },
      body: JSON.stringify({
        error: "Response serialization failed",
        debug: err.message,
        originalStatusCode: statusCode,
      }),
    };
  }
}
