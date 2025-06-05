const { DynamoDBClient } = require('@aws-sdk/client-dynamodb');
const { DynamoDBDocumentClient, GetCommand } = require('@aws-sdk/lib-dynamodb');
const { S3Client, PutObjectCommand } = require('@aws-sdk/client-s3');
const { getSignedUrl } = require('@aws-sdk/s3-request-presigner');

const dynamoClient = new DynamoDBClient();
const docClient = DynamoDBDocumentClient.from(dynamoClient);
const s3Client = new S3Client();

// Fail fast if required environment variables are missing
if (!process.env.S3_BUCKET) {
    throw new Error('S3_BUCKET environment variable is required');
}
if (!process.env.DYNAMODB_TABLE) {
    throw new Error('DYNAMODB_TABLE environment variable is required');
}
if (!process.env.VALID_API_KEYS) {
    throw new Error('VALID_API_KEYS environment variable is required');
}

const BUCKET_NAME = process.env.S3_BUCKET;
const TABLE_NAME = process.env.DYNAMODB_TABLE;
const VALID_API_KEYS = process.env.VALID_API_KEYS.split(',').filter(k => k.length > 0);

// Validate that at least one API key was provided
if (VALID_API_KEYS.length === 0) {
    throw new Error('VALID_API_KEYS must contain at least one valid API key');
}

exports.handler = async (event) => {
    console.log('Event:', JSON.stringify(event, null, 2));
    
    try {
        // Parse request body
        let body;
        try {
            body = JSON.parse(event.body || '{}');
        } catch (e) {
            return {
                statusCode: 400,
                headers: {
                    'Content-Type': 'application/json',
                    'Access-Control-Allow-Origin': '*'
                },
                body: JSON.stringify({ error: 'Invalid JSON in request body' })
            };
        }

        const { repo, org, hash, filename } = body;

        // Validate required fields
        if (!repo || !org || !hash || !filename) {
            return {
                statusCode: 400,
                headers: {
                    'Content-Type': 'application/json',
                    'Access-Control-Allow-Origin': '*'
                },
                body: JSON.stringify({ 
                    error: 'Missing required fields: repo, org, hash, filename' 
                })
            };
        }

        // Extract API key from Authorization header
        const authHeader = event.headers?.authorization || event.headers?.Authorization;
        if (!authHeader) {
            return {
                statusCode: 401,
                headers: {
                    'Content-Type': 'application/json',
                    'Access-Control-Allow-Origin': '*'
                },
                body: JSON.stringify({ error: 'Authorization header is required' })
            };
        }

        // Extract Bearer token
        const tokenMatch = authHeader.match(/^Bearer\s+(.+)$/);
        if (!tokenMatch) {
            return {
                statusCode: 401,
                headers: {
                    'Content-Type': 'application/json',
                    'Access-Control-Allow-Origin': '*'
                },
                body: JSON.stringify({ error: 'Authorization header must be in format: Bearer <token>' })
            };
        }

        const apiKey = tokenMatch[1];

        // Authenticate API key
        if (!VALID_API_KEYS.includes(apiKey)) {
            return {
                statusCode: 401,
                headers: {
                    'Content-Type': 'application/json',
                    'Access-Control-Allow-Origin': '*'
                },
                body: JSON.stringify({ error: 'Invalid API key' })
            };
        }

        // Check DynamoDB for existing types
        const pk = `repo#${org}/${repo}`;
        const sk = `types#${hash}`;

        console.log(`Checking DynamoDB for PK: ${pk}, SK: ${sk}`);

        const getCommand = new GetCommand({
            TableName: TABLE_NAME,
            Key: {
                pk: pk,
                sk: sk
            }
        });

        const result = await docClient.send(getCommand);

        if (result.Item) {
            // Types already exist
            console.log('Types found in DynamoDB:', result.Item);
            return {
                statusCode: 200,
                headers: {
                    'Content-Type': 'application/json',
                    'Access-Control-Allow-Origin': '*'
                },
                body: JSON.stringify({
                    exists: true,
                    s3Url: result.Item.s3Url
                })
            };
        }

        // Types don't exist, generate pre-signed upload URL
        const s3Key = `${org}/${repo}/${hash}/${filename}`;
        console.log(`Generating pre-signed URL for S3 key: ${s3Key}`);

        const putObjectCommand = new PutObjectCommand({
            Bucket: BUCKET_NAME,
            Key: s3Key,
            ContentType: 'text/plain'
        });

        // Generate pre-signed PUT URL (valid for 15 minutes)
        const uploadUrl = await getSignedUrl(s3Client, putObjectCommand, {
            expiresIn: 900 // 15 minutes
        });

        const s3Url = `https://${BUCKET_NAME}.s3.amazonaws.com/${s3Key}`;

        return {
            statusCode: 200,
            headers: {
                'Content-Type': 'application/json',
                'Access-Control-Allow-Origin': '*'
            },
            body: JSON.stringify({
                exists: false,
                uploadUrl: uploadUrl,
                s3Url: s3Url,
                hash: hash
            })
        };

    } catch (error) {
        console.error('Error:', error);
        return {
            statusCode: 500,
            headers: {
                'Content-Type': 'application/json',
                'Access-Control-Allow-Origin': '*'
            },
            body: JSON.stringify({ 
                error: 'Internal server error',
                details: error.message
            })
        };
    }
};