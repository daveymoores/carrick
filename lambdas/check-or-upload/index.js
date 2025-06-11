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

async function handleCheckOrUpload(payload) {
  const { repo, org, hash, filename } = payload;

  if (!repo || !org || !hash || !filename) {
    return response(400, {
      error: "Missing required fields: repo, org, hash, filename",
    });
  }

  const pk = `repo#${org}/${repo}`;
  const sk = `types#${hash}`;
  const s3Key = `${org}/${repo}/${hash}/${filename}`;
  let exists = false;
  let s3Url = `https://${BUCKET_NAME}.s3.amazonaws.com/${s3Key}`;

  try {
    const result = await docClient.send(
      new GetCommand({ TableName: TABLE_NAME, Key: { pk, sk } }),
    );
    if (result.Item) exists = true;
  } catch (err) {
    console.warn("Error checking DynamoDB:", err);
  }

  if (!exists) {
    try {
      await s3Client.send(
        new HeadObjectCommand({ Bucket: BUCKET_NAME, Key: s3Key }),
      );
      exists = true;
    } catch (err) {
      if (err.name !== "NotFound") throw err;
    }
  }

  let uploadUrl = null;
  if (!exists) {
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
      .filter((item) => item.pk !== pk || item.sk !== sk)
      .map((item) => {
        const [, repoPart] = item.pk.split("/");
        return {
          repo: repoPart,
          hash: item.sk.replace("types#", ""),
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
    exists,
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

  const pk = `repo#${org}/${repo}`;
  const sk = `types#${hash}`;
  const s3Key = `${org}/${repo}/${hash}/${filename}`;
  const s3Url = `https://${BUCKET_NAME}.s3.amazonaws.com/${s3Key}`;

  try {
    await docClient.send(
      new PutCommand({
        TableName: TABLE_NAME,
        Item: {
          pk,
          sk,
          apiKey,
          s3Url,
          filename,
          cloudRepoData, // Store the full CloudRepoData object
          lastUpdated: new Date().toISOString(),
          ttl: Math.floor(Date.now() / 1000) + 30 * 24 * 60 * 60, // 30 days TTL
        },
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
    const results = await docClient.send(
      new ScanCommand({
        TableName: TABLE_NAME,
        FilterExpression: "begins_with(pk, :orgPrefix) AND apiKey = :apiKey",
        ExpressionAttributeValues: {
          ":orgPrefix": `repo#${org}/`,
          ":apiKey": apiKey,
        },
      }),
    );

    const repos = (results.Items || []).map((item) => ({
      repo: item.pk.split("/")[1],
      hash: item.sk.replace("types#", ""),
      s3Url: item.s3Url,
      filename: item.filename,
      metadata: item.cloudRepoData,
      lastUpdated: item.lastUpdated,
    }));

    return response(200, { repos });
  } catch (err) {
    console.error("Error fetching cross-repo data:", err);
    return response(500, { error: "Failed to fetch cross-repo data" });
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

  // Validate s3Url format
  const expectedS3UrlPattern = new RegExp(
    `^https://${BUCKET_NAME}\\.s3\\.amazonaws\\.com/(.+)$`,
  );
  const match = s3Url.match(expectedS3UrlPattern);

  if (!match) {
    return response(400, { error: "Invalid s3Url format" });
  }

  const s3Key = match[1];
  const expectedKey = `${org}/${repo}/${hash}/${filename}`;

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

  // Store complete metadata
  const pk = `repo#${org}/${repo}`;
  const sk = `types#${hash}`;
  const now = new Date().toISOString();

  try {
    await docClient.send(
      new PutCommand({
        TableName: TABLE_NAME,
        Item: {
          pk,
          sk,
          s3Url,
          filename,
          org,
          repo,
          hash,
          apiKey,
          cloudRepoData,
          createdAt: now,
          updatedAt: now,
          ttl: Math.floor(Date.now() / 1000) + 30 * 24 * 60 * 60, // 30 days
        },
      }),
    );

    return response(200, {
      success: true,
      message: "Upload and metadata storage completed successfully",
      s3Url,
      metadata: { pk, sk, hasCloudRepoData: true, createdAt: now },
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

  // Validate s3Url format
  const expectedS3UrlPattern = new RegExp(
    `^https://${BUCKET_NAME}\\.s3\\.amazonaws\\.com/(.+)$`,
  );
  const match = s3Url.match(expectedS3UrlPattern);

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
  return {
    statusCode,
    headers: {
      "Content-Type": "application/json",
      "Access-Control-Allow-Origin": "*",
    },
    body: JSON.stringify(body),
  };
}
