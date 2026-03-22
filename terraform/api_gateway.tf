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

# ─── Graph API ────────────────────────────────────────────────────────────────

resource "aws_apigatewayv2_integration" "graph_api_integration" {
  api_id                 = aws_apigatewayv2_api.carrick_api.id
  integration_type       = "AWS_PROXY"
  integration_uri        = aws_lambda_function.graph_api.invoke_arn
  integration_method     = "POST"
  payload_format_version = "2.0"
}

resource "aws_apigatewayv2_route" "graph_live" {
  api_id    = aws_apigatewayv2_api.carrick_api.id
  route_key = "GET /graph/{org}"
  target    = "integrations/${aws_apigatewayv2_integration.graph_api_integration.id}"
}

resource "aws_apigatewayv2_route" "graph_snapshot_create" {
  api_id    = aws_apigatewayv2_api.carrick_api.id
  route_key = "POST /graph/{org}/snapshot"
  target    = "integrations/${aws_apigatewayv2_integration.graph_api_integration.id}"
}

resource "aws_apigatewayv2_route" "graph_snapshot_get" {
  api_id    = aws_apigatewayv2_api.carrick_api.id
  route_key = "GET /graph/{org}/snapshot/{snapshotId}"
  target    = "integrations/${aws_apigatewayv2_integration.graph_api_integration.id}"
}

resource "aws_apigatewayv2_route" "graph_options" {
  api_id    = aws_apigatewayv2_api.carrick_api.id
  route_key = "OPTIONS /graph/{org}"
  target    = "integrations/${aws_apigatewayv2_integration.graph_api_integration.id}"
}

resource "aws_lambda_permission" "graph_api" {
  statement_id  = "AllowAPIGatewayInvokeGraph"
  action        = "lambda:InvokeFunction"
  function_name = aws_lambda_function.graph_api.function_name
  principal     = "apigateway.amazonaws.com"
  source_arn    = "${aws_apigatewayv2_api.carrick_api.execution_arn}/*/*"
}

# ─── Agent Proxy ──────────────────────────────────────────────────────────────

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
