const { DynamoDBClient } = require('@aws-sdk/client-dynamodb');
const { DynamoDBDocumentClient, PutCommand } = require('@aws-sdk/lib-dynamodb');
const { S3Client, HeadObjectCommand } = require('@aws-sdk/client-s3');

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

        const { repo, org, hash, s3Url, filename } = body;

        // Validate required fields
        if (!repo || !org || !hash || !s3Url || !filename) {
            return {
                statusCode: 400,
                headers: {
                    'Content-Type': 'application/json',
                    'Access-Control-Allow-Origin': '*'
                },
                body: JSON.stringify({ 
                    error: 'Missing required fields: repo, org, hash, s3Url, filename' 
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

        // Validate s3Url format and extract key
        const expectedS3UrlPattern = new RegExp(`^https://${BUCKET_NAME}\\.s3\\.amazonaws\\.com/(.+)$`);
        const match = s3Url.match(expectedS3UrlPattern);
        
        if (!match) {
            return {
                statusCode: 400,
                headers: {
                    'Content-Type': 'application/json',
                    'Access-Control-Allow-Origin': '*'
                },
                body: JSON.stringify({ error: 'Invalid s3Url format' })
            };
        }

        const s3Key = match[1];
        
        // Validate that the S3 key matches expected pattern
        const expectedKey = `${org}/${repo}/${hash}/${filename}`;
        if (s3Key !== expectedKey) {
            return {
                statusCode: 400,
                headers: {
                    'Content-Type': 'application/json',
                    'Access-Control-Allow-Origin': '*'
                },
                body: JSON.stringify({ 
                    error: 'S3 URL does not match expected pattern',
                    expected: expectedKey,
                    actual: s3Key
                })
            };
        }

        // Verify that the file exists in S3
        console.log(`Verifying S3 object exists: ${s3Key}`);
        
        try {
            const headCommand = new HeadObjectCommand({
                Bucket: BUCKET_NAME,
                Key: s3Key
            });
            
            const headResult = await s3Client.send(headCommand);
            console.log('S3 object verified:', headResult);
        } catch (error) {
            if (error.name === 'NotFound' || error.$metadata?.httpStatusCode === 404) {
                return {
                    statusCode: 400,
                    headers: {
                        'Content-Type': 'application/json',
                        'Access-Control-Allow-Origin': '*'
                    },
                    body: JSON.stringify({ 
                        error: 'File not found in S3. Upload may have failed.' 
                    })
                };
            }
            throw error; // Re-throw other S3 errors
        }

        // Save metadata to DynamoDB
        const pk = `repo#${org}/${repo}`;
        const sk = `types#${hash}`;
        const now = new Date().toISOString();

        console.log(`Saving to DynamoDB - PK: ${pk}, SK: ${sk}`);

        const putCommand = new PutCommand({
            TableName: TABLE_NAME,
            Item: {
                pk: pk,
                sk: sk,
                s3Url: s3Url,
                filename: filename,
                org: org,
                repo: repo,
                hash: hash,
                createdAt: now,
                updatedAt: now
            },
            // Ensure we don't overwrite existing records
            ConditionExpression: 'attribute_not_exists(pk)'
        });

        try {
            await docClient.send(putCommand);
            console.log('Successfully saved metadata to DynamoDB');
        } catch (error) {
            if (error.name === 'ConditionalCheckFailedException') {
                // Record already exists, that's fine
                console.log('Metadata already exists in DynamoDB');
            } else {
                throw error; // Re-throw other DynamoDB errors
            }
        }

        return {
            statusCode: 200,
            headers: {
                'Content-Type': 'application/json',
                'Access-Control-Allow-Origin': '*'
            },
            body: JSON.stringify({
                success: true,
                message: 'Upload completed successfully',
                s3Url: s3Url,
                metadata: {
                    pk: pk,
                    sk: sk,
                    createdAt: now
                }
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