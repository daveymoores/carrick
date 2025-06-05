# Carrick Lambda Deployment Guide

This guide walks you through deploying the Carrick type checking API infrastructure to AWS.

## Prerequisites

1. **AWS CLI** configured with appropriate permissions
2. **Terraform** installed (v1.0+)
3. **Node.js** installed (v18+)
4. **AWS Account** with the following services available:
   - Lambda
   - API Gateway v2 (HTTP API)
   - DynamoDB
   - S3
   - IAM

## Required AWS Permissions

### For Terraform Deployment (Your AWS User/Role)

Your AWS user/role needs the following permissions to deploy the infrastructure:

**Lambda:**
- `lambda:CreateFunction`
- `lambda:UpdateFunctionCode`
- `lambda:UpdateFunctionConfiguration`
- `lambda:DeleteFunction`
- `lambda:GetFunction`
- `lambda:ListFunctions`
- `lambda:AddPermission`
- `lambda:RemovePermission`

**API Gateway:**
- `apigateway:POST`
- `apigateway:GET`
- `apigateway:PUT`
- `apigateway:DELETE`
- `apigateway:PATCH`

**DynamoDB:**
- `dynamodb:CreateTable`
- `dynamodb:DeleteTable`
- `dynamodb:DescribeTable`
- `dynamodb:ListTables`

**S3:**
- `s3:CreateBucket`
- `s3:DeleteBucket`
- `s3:PutBucketPolicy`
- `s3:PutBucketPublicAccessBlock`
- `s3:GetBucketLocation`

**IAM:**
- `iam:CreateRole`
- `iam:DeleteRole`
- `iam:AttachRolePolicy`
- `iam:DetachRolePolicy`
- `iam:CreatePolicy`
- `iam:DeletePolicy`
- `iam:GetRole`
- `iam:PassRole`

**CloudWatch Logs:**
- `logs:CreateLogGroup`
- `logs:DeleteLogGroup`
- `logs:DescribeLogGroups`

> **💡 Tip:** For simpler setup, you can use AWS managed policies like `PowerUserAccess` or create a custom policy with the above permissions. For production, use the principle of least privilege with the specific permissions listed above.

### For Lambda Function Runtime (Managed by Terraform)

The Lambda functions receive these permissions automatically via the IAM role defined in `iam.tf`:
- DynamoDB: `GetItem`, `PutItem`, `Query` on the CarrickTypeFiles table
- S3: `PutObject`, `GetObject`, `ListBucket` on the carrick-type-cache bucket
- CloudWatch Logs: `CreateLogGroup`, `CreateLogStream`, `PutLogEvents`

## Step-by-Step Deployment

### 1. Build Lambda Functions

```bash
cd carrick/lambdas
./build.sh
```

This will:
- Install Node.js dependencies for both functions
- Create `check-or-upload.zip` and `complete-upload.zip`
- Display build results

### 2. Configure Terraform Variables

```bash
cd ../terraform
cp terraform.tfvars.example terraform.tfvars
```

Edit `terraform.tfvars` with your values:

```hcl
# Generate secure API keys (use openssl rand -hex 32)
carrick_api_keys = "your-secure-api-key-1,your-secure-api-key-2"

# AWS region
aws_region = "us-east-1"

# Environment
environment = "prod"
```

### 3. Initialize Terraform

```bash
terraform init
```

### 4. Plan Deployment

```bash
terraform plan
```

Review the planned resources:
- 2 Lambda functions
- 1 API Gateway HTTP API
- 1 DynamoDB table
- 1 S3 bucket
- IAM roles and policies
- CloudWatch log groups

### 5. Deploy Infrastructure

```bash
terraform apply
```

Type `yes` when prompted to confirm deployment.

### 6. Get API Endpoint

After deployment completes, note the API endpoint:

```bash
terraform output api_endpoint
```

Example output: `https://abc123def.execute-api.us-east-1.amazonaws.com`

## Testing the Deployment

### Test check-or-upload endpoint:

```bash
curl -X POST https://your-api-endpoint.execute-api.us-east-1.amazonaws.com/types/check-or-upload \
  -H "Authorization: Bearer your-api-key" \
  -H "Content-Type: application/json" \
  -d '{
    "repo": "test-repo",
    "org": "test-org", 
    "hash": "abc123",
    "filename": "types.ts"
  }'
```

Expected response (first time):
```json
{
  "exists": false,
  "uploadUrl": "https://carrick-type-cache.s3.amazonaws.com/...",
  "s3Url": "https://carrick-type-cache.s3.amazonaws.com/test-org/test-repo/abc123/types.ts",
  "hash": "abc123"
}
```

### Test S3 upload:

```bash
# Upload a test file using the uploadUrl from above
curl -X PUT "https://carrick-type-cache.s3.amazonaws.com/..." \
  -H "Content-Type: text/plain" \
  -d "export interface TestType { id: string; }"
```

### Test complete-upload endpoint:

```bash
curl -X POST https://your-api-endpoint.execute-api.us-east-1.amazonaws.com/types/complete-upload \
  -H "Authorization: Bearer your-api-key" \
  -H "Content-Type: application/json" \
  -d '{
    "repo": "test-repo",
    "org": "test-org",
    "hash": "abc123", 
    "s3Url": "https://carrick-type-cache.s3.amazonaws.com/test-org/test-repo/abc123/types.ts",
    "filename": "types.ts"
  }'
```

Expected response:
```json
{
  "success": true,
  "message": "Upload completed successfully",
  "s3Url": "https://carrick-type-cache.s3.amazonaws.com/test-org/test-repo/abc123/types.ts",
  "metadata": {
    "pk": "repo#test-org/test-repo",
    "sk": "types#abc123",
    "createdAt": "2024-01-01T00:00:00.000Z"
  }
}
```

## Monitoring and Troubleshooting

### CloudWatch Logs

Lambda function logs are available in CloudWatch:
- `/aws/lambda/carrick-check-or-upload`
- `/aws/lambda/carrick-complete-upload`

### DynamoDB Console

Check stored metadata in the AWS DynamoDB console:
- Table: `CarrickTypeFiles`
- Look for items with PK pattern: `repo#org/repo`

### S3 Console

Verify uploaded files in the S3 console:
- Bucket: `carrick-type-cache`
- Path pattern: `org/repo/hash/filename`

### Common Issues

1. **"Invalid API key" errors**
   - Verify the API key in `terraform.tfvars` matches what you're sending
   - API keys are case-sensitive

2. **"File not found in S3" errors**
   - Ensure the S3 upload (step between check-or-upload and complete-upload) succeeded
   - Check CloudWatch logs for S3 upload errors

3. **Permission errors**
   - Verify your AWS credentials have the required permissions
   - Check IAM roles were created correctly by Terraform

4. **Lambda timeout errors**
   - Current timeout is 10 seconds, increase if needed in `lambda.tf`

## Security Considerations

1. **API Keys**: Store securely, rotate regularly
2. **S3 Access**: Bucket has public access blocked
3. **Pre-signed URLs**: Expire after 15 minutes
4. **Lambda Permissions**: Follow principle of least privilege

## Cleanup

To destroy all resources:

```bash
terraform destroy
```

**Warning**: This will delete all data in S3 and DynamoDB. Make sure you have backups if needed.

## Cost Estimation

Monthly costs (us-east-1, light usage):
- Lambda: ~$0.20 (100 requests/day)
- API Gateway: ~$1.00 (100 requests/day)
- DynamoDB: ~$0.25 (on-demand pricing)
- S3: ~$0.50 (10GB storage)
- **Total: ~$2.00/month**

Costs scale with usage. Monitor via AWS Cost Explorer.

## Next Steps

After successful deployment:
1. Update your GitHub Actions to use the new API endpoint
2. Add the API key as a GitHub secret
3. Test with a real repository
4. Set up monitoring alerts in CloudWatch
5. Consider setting up multiple environments (dev/staging/prod)