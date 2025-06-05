# Carrick Lambda Functions

This directory contains the AWS Lambda functions for the Carrick type checking API.

## Functions

### check-or-upload
- **Purpose**: Checks if types already exist for a given repo/hash combination, or generates a pre-signed S3 upload URL
- **Endpoint**: `POST /types/check-or-upload`
- **Runtime**: Node.js 20.x

**Request Body:**
```json
{
  "repo": "my-repo",
  "org": "my-org", 
  "hash": "abc123...",
  "filename": "types.ts"
}
```

**Headers:**
```
Authorization: Bearer your-api-key
Content-Type: application/json
```

**Response (exists):**
```json
{
  "exists": true,
  "s3Url": "https://carrick-type-cache.s3.amazonaws.com/my-org/my-repo/abc123.../types.ts"
}
```

**Response (doesn't exist):**
```json
{
  "exists": false,
  "uploadUrl": "https://carrick-type-cache.s3.amazonaws.com/...", 
  "s3Url": "https://carrick-type-cache.s3.amazonaws.com/my-org/my-repo/abc123.../types.ts",
  "hash": "abc123..."
}
```

### complete-upload
- **Purpose**: Validates S3 upload and saves metadata to DynamoDB
- **Endpoint**: `POST /types/complete-upload`
- **Runtime**: Node.js 22.x

**Request Body:**
```json
{
  "repo": "my-repo",
  "org": "my-org",
  "hash": "abc123...",
  "s3Url": "https://carrick-type-cache.s3.amazonaws.com/my-org/my-repo/abc123.../types.ts",
  "filename": "types.ts"
}
```

**Headers:**
```
Authorization: Bearer your-api-key
Content-Type: application/json
```

**Response:**
```json
{
  "success": true,
  "message": "Upload completed successfully",
  "s3Url": "https://carrick-type-cache.s3.amazonaws.com/...",
  "metadata": {
    "pk": "repo#my-org/my-repo",
    "sk": "types#abc123...",
    "createdAt": "2024-01-01T00:00:00.000Z"
  }
}
```

## Environment Variables

Both functions require these environment variables (no defaults - will fail if missing):

- `S3_BUCKET`: S3 bucket name for storing type files
- `DYNAMODB_TABLE`: DynamoDB table name for metadata storage  
- `VALID_API_KEYS`: Comma-separated list of valid API keys for authentication

### API Key Authentication

The `VALID_API_KEYS` environment variable contains a comma-separated list of valid API keys:
```
VALID_API_KEYS=dev-key-abc123,ci-key-def456,prod-key-xyz789
```

**Benefits:**
- **Secure access control** - only requests with valid keys are processed
- **Key rotation** - update keys without redeploying Lambda functions
- **Multiple environments** - different keys for dev/staging/prod
- **Revocation** - remove compromised keys from the list

**Example API request:**
```bash
curl -X POST https://api.carrick.dev/types/check-or-upload \
  -H "Authorization: Bearer dev-key-abc123" \
  -H "Content-Type: application/json" \
  -d '{
    "repo": "my-repo",
    "org": "my-org",
    "hash": "abc123",
    "filename": "types.ts"
  }'
```

**Key Management Best Practices:**
- Generate keys using: `openssl rand -hex 32`
- Store keys securely (GitHub Secrets, AWS Secrets Manager, etc.)
- Rotate keys regularly
- Use different keys per environment
- Never commit keys to version control

## DynamoDB Schema

The functions use a DynamoDB table with the following structure:

- **Partition Key (pk)**: `repo#{org}/{repo}`
- **Sort Key (sk)**: `types#{hash}`

**Item Structure:**
```json
{
  "pk": "repo#my-org/my-repo",
  "sk": "types#abc123...",
  "s3Url": "https://carrick-type-cache.s3.amazonaws.com/...",
  "filename": "types.ts",
  "org": "my-org",
  "repo": "my-repo", 
  "hash": "abc123...",
  "createdAt": "2024-01-01T00:00:00.000Z",
  "updatedAt": "2024-01-01T00:00:00.000Z"
}
```

## S3 Key Structure

Files are stored in S3 with the following key pattern:
```
{org}/{repo}/{hash}/{filename}
```

Example: `my-org/my-repo/abc123def456/types.ts`

## Building and Deployment

1. **Build the functions:**
   ```bash
   ./build.sh
   ```

2. **Deploy with Terraform:**
   ```bash
   cd ../terraform
   terraform plan
   terraform apply
   ```

## Security

- API key authentication on all endpoints
- Pre-signed URLs expire after 15 minutes
- S3 bucket has public access blocked
- Lambda functions have minimal IAM permissions

## Error Handling

Common error responses:

- `400`: Missing required fields or invalid request format
- `401`: Invalid API key
- `404`: File not found in S3 (for complete-upload)
- `500`: Internal server error

All responses include CORS headers for web client compatibility.

## Monitoring

The functions log to CloudWatch Logs with structured logging for debugging and monitoring.