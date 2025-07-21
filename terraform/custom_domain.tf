# Request SSL certificate for your API subdomain
resource "aws_acm_certificate" "api_cert" {
  domain_name       = "api.${var.domain_name}"
  validation_method = "DNS"

  lifecycle {
    create_before_destroy = true
  }
}

# Create custom domain for API Gateway
resource "aws_apigatewayv2_domain_name" "api_domain" {
  domain_name = "api.${var.domain_name}"

  domain_name_configuration {
    certificate_arn = aws_acm_certificate.api_cert.arn
    endpoint_type   = "REGIONAL"
    security_policy = "TLS_1_2"
  }

  depends_on = [aws_acm_certificate.api_cert]
}

# Map the custom domain to your API Gateway
resource "aws_apigatewayv2_api_mapping" "api_mapping" {
  api_id      = aws_apigatewayv2_api.carrick_api.id
  domain_name = aws_apigatewayv2_domain_name.api_domain.id
  stage       = aws_apigatewayv2_stage.default_stage.id
}
#
# Output the DNS target for your domain provider
output "api_domain_dns_target" {
  value       = aws_apigatewayv2_domain_name.api_domain.domain_name_configuration[0].target_domain_name
  description = "DNS target for CNAME record at your domain provider"
}
