resource "aws_lambda_function" "check_or_upload" {
  function_name    = "carrick-check-or-upload"
  role             = aws_iam_role.lambda_exec.arn
  handler          = "index.handler"
  runtime          = "nodejs22.x"
  filename         = "../lambdas/check-or-upload.zip"
  source_code_hash = filebase64sha256("../lambdas/check-or-upload.zip")
  timeout          = 10
  environment {
    variables = {
      S3_BUCKET      = aws_s3_bucket.carrick_types.bucket
      DYNAMODB_TABLE = aws_dynamodb_table.type_metadata.name
      VALID_API_KEYS = var.carrick_api_keys
    }
  }
}

resource "aws_lambda_function" "complete_upload" {
  function_name    = "carrick-complete-upload"
  role             = aws_iam_role.lambda_exec.arn
  handler          = "index.handler"
  runtime          = "nodejs22.x"
  filename         = "../lambdas/complete-upload.zip"
  source_code_hash = filebase64sha256("../lambdas/complete-upload.zip")
  timeout          = 10

  environment {
    variables = {
      S3_BUCKET      = aws_s3_bucket.carrick_types.bucket
      DYNAMODB_TABLE = aws_dynamodb_table.type_metadata.name
      VALID_API_KEYS = var.carrick_api_keys
    }
  }
}
