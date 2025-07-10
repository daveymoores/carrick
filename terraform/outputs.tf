output "api_endpoint" {
  value       = aws_apigatewayv2_api.carrick_api.api_endpoint
  description = "Base API endpoint URL - append /types/check-or-upload for the lambda function"
}

output "aws_api_endpoint" {
  value       = aws_apigatewayv2_api.carrick_api.api_endpoint
  description = "AWS-generated API endpoint (fallback)"
}

output "dns_instructions" {
  value       = "Create CNAME record: api.${var.domain_name} -> ${aws_apigatewayv2_domain_name.api_domain.domain_name_configuration[0].target_domain_name}"
  description = "DNS record to create at your domain provider"
}

output "acm_certificate_validation_options" {
  description = "The DNS validation options for the ACM certificate"
  value       = aws_acm_certificate.api_cert.domain_validation_options
}
