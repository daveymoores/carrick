resource "aws_lambda_function" "check_or_upload" {
  function_name    = "carrick-check-or-upload"
  role             = aws_iam_role.lambda_exec.arn
  handler          = "index.handler"
  runtime          = "nodejs22.x"
  filename         = "../lambdas/check-or-upload.zip"
  source_code_hash = filebase64sha256("../lambdas/check-or-upload.zip")
  timeout          = 30 # Increased timeout for multiple operations

  environment {
    variables = {
      S3_BUCKET      = aws_s3_bucket.carrick_types.bucket
      DYNAMODB_TABLE = aws_dynamodb_table.type_metadata.name
      VALID_API_KEYS = var.carrick_api_keys
    }
  }
}

resource "aws_lambda_function" "mcp_server" {
  function_name    = "carrick-mcp-server"
  role             = aws_iam_role.lambda_exec.arn
  handler          = "index.handler"
  runtime          = "nodejs22.x"
  filename         = "../lambdas/mcp-server.zip"
  source_code_hash = filebase64sha256("../lambdas/mcp-server.zip")
  timeout          = 30
  memory_size      = 256

  environment {
    variables = {
      CARRICK_API_ENDPOINT = "https://api.${var.domain_name}"
      CARRICK_API_KEY      = trimspace(split(",", var.carrick_api_keys)[0])
      CARRICK_ORG          = var.carrick_org
      VALID_API_KEYS       = var.carrick_api_keys
    }
  }
}

resource "aws_lambda_function" "agent_proxy" {
  function_name    = "carrick-agent-proxy"
  role             = aws_iam_role.lambda_exec.arn
  handler          = "index.handler"
  runtime          = "nodejs22.x"
  filename         = "../lambdas/agent-proxy.zip"
  source_code_hash = filebase64sha256("../lambdas/agent-proxy.zip")
  timeout          = 120 # Increased for parallel LLM calls - API Gateway has 30s limit but Lambda can continue processing

  environment {
    variables = {
      AGENT_API_KEY  = var.agent_api_key
      VALID_API_KEYS = var.carrick_api_keys
    }
  }
}
