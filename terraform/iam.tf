resource "aws_iam_role" "lambda_exec" {
  name = "carrick_lambda_exec"

  assume_role_policy = jsonencode({
    Version = "2012-10-17"
    Statement = [
      {
        Action = "sts:AssumeRole"
        Effect = "Allow"
        Principal = {
          Service = "lambda.amazonaws.com"
        }
      }
    ]
  })
}

resource "aws_iam_policy" "lambda_policy" {
  name = "carrick_lambda_policy"
  policy = jsonencode({
    Version = "2012-10-17",
    Statement: [
      {
        Action: [
          "s3:PutObject",
          "s3:GetObject",
          "s3:ListBucket"
        ],
        Effect: "Allow",
        Resource: [
          aws_s3_bucket.carrick_types.arn,
          "${aws_s3_bucket.carrick_types.arn}/*"
        ]
      },
      {
        Action: [
          "dynamodb:GetItem",
          "dynamodb:PutItem",
          "dynamodb:Query"
        ],
        Effect: "Allow",
        Resource: aws_dynamodb_table.type_metadata.arn
      },
      {
        Action: [
          "logs:CreateLogGroup",
          "logs:CreateLogStream",
          "logs:PutLogEvents"
        ],
        Effect: "Allow",
        Resource: "arn:aws:logs:*:*:*"
      }
    ]
  })
}

resource "aws_iam_role_policy_attachment" "lambda_policy_attach" {
  role       = aws_iam_role.lambda_exec.name
  policy_arn = aws_iam_policy.lambda_policy.arn
}
