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

resource "aws_lambda_function" "gemini_proxy" {
  function_name    = "carrick-gemini-proxy"
  role             = aws_iam_role.lambda_exec.arn
  handler          = "index.handler"
  runtime          = "nodejs22.x"
  filename         = "../lambdas/gemini-proxy.zip"
  source_code_hash = filebase64sha256("../lambdas/gemini-proxy.zip")
  timeout          = 60 # Gemini API calls can take longer

  environment {
    variables = {
      GEMINI_API_KEY = var.gemini_api_key
      VALID_API_KEYS = var.carrick_api_keys
    }
  }
}
