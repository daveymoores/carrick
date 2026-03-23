variable "carrick_api_keys" {
  description = "Comma-separated list of valid API keys for Carrick authentication"
  type        = string
  sensitive   = true
}

variable "aws_region" {
  description = "AWS region for resources"
  type        = string
  default     = "eu-west-1"
}

variable "environment" {
  description = "Environment name (dev, staging, prod)"
  type        = string
  default     = "dev"
}

variable "domain_name" {
  description = "Your domain name (e.g., yoursite.com)"
  type        = string
  default     = "carrick.tools"
}

variable "agent_api_key" {
  description = "Anthropic API key for AI-powered code analysis"
  type        = string
  sensitive   = true
}

variable "carrick_org" {
  description = "Default org name for the MCP server"
  type        = string
}
