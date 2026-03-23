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
  api_id                 = aws_apigatewayv2_api.carrick_api.id
  integration_type       = "AWS_PROXY"
  integration_uri        = aws_lambda_function.check_or_upload.invoke_arn
  integration_method     = "POST"
  payload_format_version = "2.0"
}

resource "aws_apigatewayv2_route" "check_upload_route" {
  api_id    = aws_apigatewayv2_api.carrick_api.id
  route_key = "POST /types/check-or-upload"
  target    = "integrations/${aws_apigatewayv2_integration.check_upload_integration.id}"
}

resource "aws_lambda_permission" "check_or_upload_api" {
  statement_id  = "AllowAPIGatewayInvokeCheck"
  action        = "lambda:InvokeFunction"
  function_name = aws_lambda_function.check_or_upload.function_name
  principal     = "apigateway.amazonaws.com"
  source_arn    = "${aws_apigatewayv2_api.carrick_api.execution_arn}/*/*"
}

resource "aws_apigatewayv2_integration" "mcp_server_integration" {
  api_id                 = aws_apigatewayv2_api.carrick_api.id
  integration_type       = "AWS_PROXY"
  integration_uri        = aws_lambda_function.mcp_server.invoke_arn
  integration_method     = "POST"
  payload_format_version = "2.0"
}

resource "aws_apigatewayv2_route" "mcp_post" {
  api_id    = aws_apigatewayv2_api.carrick_api.id
  route_key = "POST /mcp"
  target    = "integrations/${aws_apigatewayv2_integration.mcp_server_integration.id}"
}

resource "aws_apigatewayv2_route" "mcp_get" {
  api_id    = aws_apigatewayv2_api.carrick_api.id
  route_key = "GET /mcp"
  target    = "integrations/${aws_apigatewayv2_integration.mcp_server_integration.id}"
}

resource "aws_apigatewayv2_route" "mcp_delete" {
  api_id    = aws_apigatewayv2_api.carrick_api.id
  route_key = "DELETE /mcp"
  target    = "integrations/${aws_apigatewayv2_integration.mcp_server_integration.id}"
}

resource "aws_lambda_permission" "mcp_server_api" {
  statement_id  = "AllowAPIGatewayInvokeMCP"
  action        = "lambda:InvokeFunction"
  function_name = aws_lambda_function.mcp_server.function_name
  principal     = "apigateway.amazonaws.com"
  source_arn    = "${aws_apigatewayv2_api.carrick_api.execution_arn}/*/*"
}

resource "aws_apigatewayv2_integration" "agent_proxy_integration" {
  api_id                 = aws_apigatewayv2_api.carrick_api.id
  integration_type       = "AWS_PROXY"
  integration_uri        = aws_lambda_function.agent_proxy.invoke_arn
  integration_method     = "POST"
  payload_format_version = "2.0"
  timeout_milliseconds   = 30000 # API Gateway HTTP API max timeout is 30s
}

resource "aws_apigatewayv2_route" "agent_proxy_route" {
  api_id    = aws_apigatewayv2_api.carrick_api.id
  route_key = "POST /agent/chat"
  target    = "integrations/${aws_apigatewayv2_integration.agent_proxy_integration.id}"
}

resource "aws_apigatewayv2_route" "agent_proxy_options" {
  api_id    = aws_apigatewayv2_api.carrick_api.id
  route_key = "OPTIONS /agent/chat"
  target    = "integrations/${aws_apigatewayv2_integration.agent_proxy_integration.id}"
}

resource "aws_lambda_permission" "agent_proxy_api" {
  statement_id  = "AllowAPIGatewayInvokeAgent"
  action        = "lambda:InvokeFunction"
  function_name = aws_lambda_function.agent_proxy.function_name
  principal     = "apigateway.amazonaws.com"
  source_arn    = "${aws_apigatewayv2_api.carrick_api.execution_arn}/*/*"
}
