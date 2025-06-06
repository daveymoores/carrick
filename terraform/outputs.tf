output "api_endpoint" {
  value = aws_apigatewayv2_api.carrick_api.api_endpoint
}

output "check_upload_url" {
  value = "${aws_apigatewayv2_api.carrick_api.api_endpoint}/types/check-or-upload"
  description = "Full URL for the check-or-upload endpoint"
}
