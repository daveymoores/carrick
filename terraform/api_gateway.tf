resource "aws_apigatewayv2_api" "carrick_api" {
  name          = "carrick-api"
  protocol_type = "HTTP"
}

resource "aws_apigatewayv2_stage" "default_stage" {
  api_id      = aws_apigatewayv2_api.carrick_api.id
  name        = "$default"
  auto_deploy = true
}

resource "aws_apigatewayv2_integration" "check_upload_integration" {
  api_id           = aws_apigatewayv2_api.carrick_api.id
  integration_type = "AWS_PROXY"
  integration_uri  = aws_lambda_function.check_or_upload.invoke_arn
  integration_method = "POST"
  payload_format_version = "2.0"
}

resource "aws_apigatewayv2_integration" "complete_upload_integration" {
  api_id           = aws_apigatewayv2_api.carrick_api.id
  integration_type = "AWS_PROXY"
  integration_uri  = aws_lambda_function.complete_upload.invoke_arn
  integration_method = "POST"
  payload_format_version = "2.0"
}

resource "aws_apigatewayv2_route" "check_upload_route" {
  api_id    = aws_apigatewayv2_api.carrick_api.id
  route_key = "POST /types/check-or-upload"
  target    = "integrations/${aws_apigatewayv2_integration.check_upload_integration.id}"
}

resource "aws_apigatewayv2_route" "complete_upload_route" {
  api_id    = aws_apigatewayv2_api.carrick_api.id
  route_key = "POST /types/complete-upload"
  target    = "integrations/${aws_apigatewayv2_integration.complete_upload_integration.id}"
}

resource "aws_lambda_permission" "check_or_upload_api" {
  statement_id  = "AllowAPIGatewayInvokeCheck"
  action        = "lambda:InvokeFunction"
  function_name = aws_lambda_function.check_or_upload.function_name
  principal     = "apigateway.amazonaws.com"
  source_arn    = "${aws_apigatewayv2_api.carrick_api.execution_arn}/*/*"
}

resource "aws_lambda_permission" "complete_upload_api" {
  statement_id  = "AllowAPIGatewayInvokeComplete"
  action        = "lambda:InvokeFunction"
  function_name = aws_lambda_function.complete_upload.function_name
  principal     = "apigateway.amazonaws.com"
  source_arn    = "${aws_apigatewayv2_api.carrick_api.execution_arn}/*/*"
}
