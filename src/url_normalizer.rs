//! URL Normalization Module
//!
//! This module handles the critical task of normalizing consumer URLs so they can be
//! matched against endpoint paths. Without this normalization, Carrick cannot match
//! cross-service API calls to endpoints.
//!
//! ## Problem
//!
//! In real microservices deployments:
//! - Service A defines: `GET /users/:id`
//! - Service B calls: `fetch(\`http://user-service.internal/users/${id}\`)`
//!
//! The call URL is `http://user-service.internal/users/123`, but the endpoint path is `/users/:id`.
//! Without normalization, no calls match any endpoints.
//!
//! ## URL Patterns Handled
//!
//! 1. Full URLs: `https://user-service.internal/api/users` → `/api/users`
//! 2. Env var patterns: `ENV_VAR:SERVICE_URL:/users` → `/users`
//! 3. Template literals: `${API_URL}/users/${id}` → `/users/:id`
//! 4. Query strings: `/users?page=1` → `/users`
//! 5. Trailing slashes: `/users/` → `/users`

use crate::config::Config;
use std::collections::HashSet;

/// Result of URL normalization
#[derive(Debug, Clone, PartialEq)]
pub struct NormalizedUrl {
    /// The normalized path (e.g., `/users/:id`)
    pub path: String,
    /// Whether the URL was recognized as internal
    pub is_internal: bool,
    /// Whether the URL was recognized as external
    pub is_external: bool,
    /// The original URL before normalization
    pub original: String,
    /// Any host/domain that was stripped
    pub stripped_host: Option<String>,
}

/// URL normalizer that uses configuration to identify internal/external domains
#[derive(Debug, Clone)]
pub struct UrlNormalizer {
    internal_domains: HashSet<String>,
    external_domains: HashSet<String>,
    internal_env_vars: HashSet<String>,
    external_env_vars: HashSet<String>,
}

impl UrlNormalizer {
    /// Create a new URL normalizer from config
    pub fn new(config: &Config) -> Self {
        Self {
            internal_domains: config.internal_domains.clone(),
            external_domains: config.external_domains.clone(),
            internal_env_vars: config.internal_env_vars.clone(),
            external_env_vars: config.external_env_vars.clone(),
        }
    }

    /// Create a URL normalizer with no configured domains (normalizes all URLs)
    pub fn default_permissive() -> Self {
        Self {
            internal_domains: HashSet::new(),
            external_domains: HashSet::new(),
            internal_env_vars: HashSet::new(),
            external_env_vars: HashSet::new(),
        }
    }

    /// Normalize a URL for matching against endpoint paths
    ///
    /// This handles:
    /// - Full URLs with protocol and host
    /// - Environment variable patterns (ENV_VAR:NAME:/path)
    /// - Template literals with interpolation
    /// - Query strings
    /// - Trailing slashes
    pub fn normalize(&self, url: &str) -> NormalizedUrl {
        let original = url.to_string();

        // Handle ENV_VAR: pattern first
        if url.starts_with("ENV_VAR:") {
            return self.normalize_env_var_pattern(url, original);
        }

        // Handle process.env patterns (e.g., process.env.API_URL + "/users")
        if url.contains("process.env.") {
            return self.normalize_process_env_pattern(url, original);
        }

        // Handle template literal interpolations (e.g., ${API_URL}/users/${id})
        if url.contains("${") {
            return self.normalize_template_literal(url, original);
        }

        // Handle full URLs with protocol
        if url.starts_with("http://") || url.starts_with("https://") {
            return self.normalize_full_url(url, original);
        }

        // Handle protocol-relative URLs (//domain.com/path)
        if url.starts_with("//") {
            return self.normalize_protocol_relative_url(url, original);
        }

        // Already a path - just clean it up
        let path = self.clean_path(url);
        NormalizedUrl {
            path,
            is_internal: false,
            is_external: false,
            original,
            stripped_host: None,
        }
    }

    /// Normalize an ENV_VAR: pattern
    ///
    /// Format: `ENV_VAR:VARIABLE_NAME:/path/here`
    fn normalize_env_var_pattern(&self, url: &str, original: String) -> NormalizedUrl {
        let parts: Vec<&str> = url.splitn(3, ':').collect();

        if parts.len() >= 2 {
            let env_var_name = parts[1];
            let path = if parts.len() >= 3 {
                let path_with_params = self.convert_interpolations_to_params(parts[2]);
                self.clean_path(&path_with_params)
            } else {
                "/".to_string()
            };

            let is_internal = self.internal_env_vars.contains(env_var_name);
            let is_external = self.external_env_vars.contains(env_var_name);

            NormalizedUrl {
                path,
                is_internal,
                is_external,
                original,
                stripped_host: Some(format!("ENV_VAR:{}", env_var_name)),
            }
        } else {
            NormalizedUrl {
                path: self.clean_path(url),
                is_internal: false,
                is_external: false,
                original,
                stripped_host: None,
            }
        }
    }

    /// Normalize a process.env pattern
    ///
    /// Examples:
    /// - `process.env.API_URL + "/users"` → `/users`
    /// - `process.env.SERVICE_URL/users` → `/users`
    fn normalize_process_env_pattern(&self, url: &str, original: String) -> NormalizedUrl {
        let env_var_name = self.extract_process_env_var(url);

        let path = self.extract_path_from_process_env(url);
        let path_with_params = self.convert_interpolations_to_params(&path);

        let is_internal = env_var_name
            .as_ref()
            .map(|v| self.internal_env_vars.contains(v))
            .unwrap_or(false);
        let is_external = env_var_name
            .as_ref()
            .map(|v| self.external_env_vars.contains(v))
            .unwrap_or(false);

        NormalizedUrl {
            path: self.clean_path(&path_with_params),
            is_internal,
            is_external,
            original,
            stripped_host: env_var_name.map(|v| format!("process.env.{}", v)),
        }
    }

    /// Extract env var name from process.env pattern
    fn extract_process_env_var(&self, url: &str) -> Option<String> {
        if let Some(start) = url.find("process.env.") {
            let after_prefix = &url[start + 12..];
            // Env var name ends at non-alphanumeric/underscore
            let end = after_prefix
                .find(|c: char| !c.is_alphanumeric() && c != '_')
                .unwrap_or(after_prefix.len());
            if end > 0 {
                return Some(after_prefix[..end].to_string());
            }
        }
        None
    }

    /// Extract path from process.env pattern
    fn extract_path_from_process_env(&self, url: &str) -> String {
        // Pattern: process.env.VAR + "/path" or process.env.VAR + '/path' or backticks
        if let Some(plus_idx) = url.find('+') {
            let after_plus = url[plus_idx + 1..].trim();
            let path = after_plus
                .trim_start_matches(['"', '\'', '`'])
                .trim_end_matches(['"', '\'', '`']);
            return path.to_string();
        }

        if let Some(env_idx) = url.find("process.env.") {
            let after_prefix = &url[env_idx + 12..];
            let var_end = after_prefix
                .find(|c: char| !c.is_alphanumeric() && c != '_')
                .unwrap_or(after_prefix.len());

            let after_var = &after_prefix[var_end..];
            if let Some(slash_idx) = after_var.find('/') {
                return after_var[slash_idx..].to_string();
            }
        }

        "/".to_string()
    }

    /// Normalize a template literal with ${} interpolations
    ///
    /// Examples:
    /// - `${API_URL}/users/${id}` → `/users/:id`
    /// - `${BASE_URL}/orders/${orderId}/items` → `/orders/:orderId/items`
    fn normalize_template_literal(&self, url: &str, original: String) -> NormalizedUrl {
        let mut result = url.to_string();
        let mut stripped_host = None;
        let mut is_internal = false;
        let mut is_external = false;

        // Check if starts with a variable that might be a base URL
        if url.starts_with("${")
            && let Some(end) = url.find('}')
        {
            let var_name = &url[2..end];
            // Check if this is a known env var
            if self.internal_env_vars.contains(var_name) {
                is_internal = true;
                stripped_host = Some(format!("${{{}}}", var_name));
            } else if self.external_env_vars.contains(var_name) {
                is_external = true;
                stripped_host = Some(format!("${{{}}}", var_name));
            }
            // Remove the base URL variable
            result = url[end + 1..].to_string();
        }

        // Convert remaining ${varName} to :varName for path parameter matching
        let path = self.convert_interpolations_to_params(&result);

        NormalizedUrl {
            path: self.clean_path(&path),
            is_internal,
            is_external,
            original,
            stripped_host,
        }
    }

    /// Convert ${varName} interpolations to :varName path parameters
    fn convert_interpolations_to_params(&self, path: &str) -> String {
        let mut result = String::new();
        let mut chars = path.chars().peekable();

        while let Some(c) = chars.next() {
            if c == '$' && chars.peek() == Some(&'{') {
                chars.next(); // consume '{'
                let mut var_name = String::new();
                for inner_c in chars.by_ref() {
                    if inner_c == '}' {
                        break;
                    }
                    var_name.push(inner_c);
                }
                // Convert to path parameter format
                result.push(':');
                result.push_str(&var_name);
            } else {
                result.push(c);
            }
        }

        result
    }

    /// Normalize a full URL with protocol and host
    fn normalize_full_url(&self, url: &str, original: String) -> NormalizedUrl {
        // Parse the URL to extract host and path
        let without_protocol = url
            .strip_prefix("https://")
            .or_else(|| url.strip_prefix("http://"))
            .unwrap_or(url);

        // Find the path start (first / after host)
        let (host, path) = if let Some(slash_idx) = without_protocol.find('/') {
            let host = &without_protocol[..slash_idx];
            let path = &without_protocol[slash_idx..];
            (host.to_string(), path.to_string())
        } else {
            (without_protocol.to_string(), "/".to_string())
        };

        // Check if host is internal or external
        let is_internal = self.is_internal_host(&host);
        let is_external = self.is_external_host(&host);

        NormalizedUrl {
            path: self.clean_path(&path),
            is_internal,
            is_external,
            original,
            stripped_host: Some(host),
        }
    }

    /// Normalize a protocol-relative URL (//domain.com/path)
    fn normalize_protocol_relative_url(&self, url: &str, original: String) -> NormalizedUrl {
        let without_slashes = url.strip_prefix("//").unwrap_or(url);
        // Reuse full URL logic
        self.normalize_full_url(&format!("https://{}", without_slashes), original)
    }

    /// Check if a host is configured as internal
    fn is_internal_host(&self, host: &str) -> bool {
        // Strip port if present
        let host_without_port = host.split(':').next().unwrap_or(host);

        self.internal_domains.iter().any(|domain| {
            host_without_port == domain
                || host_without_port.ends_with(&format!(".{}", domain))
                || domain.contains(host_without_port)
        })
    }

    /// Check if a host is configured as external
    fn is_external_host(&self, host: &str) -> bool {
        // Strip port if present
        let host_without_port = host.split(':').next().unwrap_or(host);

        self.external_domains.iter().any(|domain| {
            host_without_port == domain
                || host_without_port.ends_with(&format!(".{}", domain))
                || domain.contains(host_without_port)
        })
    }

    /// Clean up a path by removing query strings, fragments, and normalizing slashes
    fn clean_path(&self, path: &str) -> String {
        let mut result = path.to_string();

        // Remove query string
        if let Some(query_idx) = result.find('?') {
            result = result[..query_idx].to_string();
        }

        // Remove fragment
        if let Some(fragment_idx) = result.find('#') {
            result = result[..fragment_idx].to_string();
        }

        // Strip any surrounding quotes or backticks (template literal artifacts)
        result = result
            .trim_start_matches(['`', '"', '\''])
            .trim_end_matches(['`', '"', '\''])
            .to_string();

        // Ensure path starts with /
        if !result.starts_with('/') {
            result = format!("/{}", result);
        }

        // Remove trailing slash (unless it's just "/")
        if result.len() > 1 && result.ends_with('/') {
            result.pop();
        }

        // Normalize multiple slashes
        while result.contains("//") {
            result = result.replace("//", "/");
        }

        result
    }

    /// Extract just the path from a URL, suitable for endpoint matching
    ///
    /// This is a convenience method that returns just the normalized path string.
    pub fn extract_path(&self, url: &str) -> String {
        self.normalize(url).path
    }

    /// Heuristic check for URL-like inputs to avoid matching variable names as paths.
    pub fn is_probable_url(&self, url: &str) -> bool {
        let trimmed = url.trim();
        if trimmed.is_empty() {
            return false;
        }

        if trimmed.starts_with("ENV_VAR:") {
            return true;
        }

        if trimmed.starts_with("http://")
            || trimmed.starts_with("https://")
            || trimmed.starts_with("//")
        {
            return true;
        }

        if trimmed.contains("process.env.") || trimmed.contains("${") {
            return true;
        }

        if trimmed.starts_with('/') || trimmed.contains('/') {
            return true;
        }

        false
    }
}

impl Default for UrlNormalizer {
    fn default() -> Self {
        Self::default_permissive()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn create_test_config() -> Config {
        Config {
            service_name: None,
            internal_domains: [
                "user-service.internal",
                "api.company.com",
                "core-api.company.com",
            ]
            .iter()
            .map(|s| s.to_string())
            .collect(),
            external_domains: ["api.stripe.com", "api.github.com"]
                .iter()
                .map(|s| s.to_string())
                .collect(),
            internal_env_vars: ["API_URL", "SERVICE_URL", "CORE_API"]
                .iter()
                .map(|s| s.to_string())
                .collect(),
            external_env_vars: ["STRIPE_API", "GITHUB_API"]
                .iter()
                .map(|s| s.to_string())
                .collect(),
        }
    }

    #[test]
    fn test_normalize_full_url_internal() {
        let config = create_test_config();
        let normalizer = UrlNormalizer::new(&config);

        let result = normalizer.normalize("https://user-service.internal/users/123");

        assert_eq!(result.path, "/users/123");
        assert!(result.is_internal);
        assert!(!result.is_external);
        assert_eq!(
            result.stripped_host,
            Some("user-service.internal".to_string())
        );
    }

    #[test]
    fn test_normalize_full_url_external() {
        let config = create_test_config();
        let normalizer = UrlNormalizer::new(&config);

        let result = normalizer.normalize("https://api.stripe.com/v1/charges");

        assert_eq!(result.path, "/v1/charges");
        assert!(!result.is_internal);
        assert!(result.is_external);
    }

    #[test]
    fn test_normalize_env_var_pattern_internal() {
        let config = create_test_config();
        let normalizer = UrlNormalizer::new(&config);

        let result = normalizer.normalize("ENV_VAR:API_URL:/users/123");

        assert_eq!(result.path, "/users/123");
        assert!(result.is_internal);
        assert!(!result.is_external);
        assert_eq!(result.stripped_host, Some("ENV_VAR:API_URL".to_string()));
    }

    #[test]
    fn test_normalize_env_var_pattern_external() {
        let config = create_test_config();
        let normalizer = UrlNormalizer::new(&config);

        let result = normalizer.normalize("ENV_VAR:STRIPE_API:/v1/charges");

        assert_eq!(result.path, "/v1/charges");
        assert!(!result.is_internal);
        assert!(result.is_external);
    }

    #[test]
    fn test_normalize_template_literal() {
        let config = create_test_config();
        let normalizer = UrlNormalizer::new(&config);

        let result = normalizer.normalize("${API_URL}/users/${userId}");

        assert_eq!(result.path, "/users/:userId");
        assert!(result.is_internal);
        assert_eq!(result.stripped_host, Some("${API_URL}".to_string()));
    }

    #[test]
    fn test_normalize_template_literal_complex() {
        let config = create_test_config();
        let normalizer = UrlNormalizer::new(&config);

        let result = normalizer.normalize("${SERVICE_URL}/orders/${orderId}/items/${itemId}");

        assert_eq!(result.path, "/orders/:orderId/items/:itemId");
        assert!(result.is_internal);
    }

    #[test]
    fn test_normalize_query_string_removal() {
        let normalizer = UrlNormalizer::default_permissive();

        let result = normalizer.normalize("/users?page=1&limit=10");

        assert_eq!(result.path, "/users");
    }

    #[test]
    fn test_normalize_trailing_slash() {
        let normalizer = UrlNormalizer::default_permissive();

        let result = normalizer.normalize("/users/");

        assert_eq!(result.path, "/users");
    }

    #[test]
    fn test_normalize_root_path() {
        let normalizer = UrlNormalizer::default_permissive();

        let result = normalizer.normalize("/");

        assert_eq!(result.path, "/");
    }

    #[test]
    fn test_normalize_plain_path() {
        let normalizer = UrlNormalizer::default_permissive();

        let result = normalizer.normalize("/api/v1/users");

        assert_eq!(result.path, "/api/v1/users");
        assert!(!result.is_internal);
        assert!(!result.is_external);
        assert!(result.stripped_host.is_none());
    }

    #[test]
    fn test_normalize_url_with_port() {
        let config = create_test_config();
        let normalizer = UrlNormalizer::new(&config);

        let result = normalizer.normalize("http://user-service.internal:3000/users");

        assert_eq!(result.path, "/users");
        assert!(result.is_internal);
    }

    #[test]
    fn test_normalize_protocol_relative() {
        let config = create_test_config();
        let normalizer = UrlNormalizer::new(&config);

        let result = normalizer.normalize("//api.stripe.com/v1/charges");

        assert_eq!(result.path, "/v1/charges");
        assert!(result.is_external);
    }

    #[test]
    fn test_normalize_fragment_removal() {
        let normalizer = UrlNormalizer::default_permissive();

        let result = normalizer.normalize("/users#section");

        assert_eq!(result.path, "/users");
    }

    #[test]
    fn test_normalize_multiple_slashes() {
        let normalizer = UrlNormalizer::default_permissive();

        let result = normalizer.normalize("/api//v1///users");

        assert_eq!(result.path, "/api/v1/users");
    }

    #[test]
    fn test_extract_path_convenience() {
        let normalizer = UrlNormalizer::default_permissive();

        let path = normalizer.extract_path("https://example.com/api/users?page=1");

        assert_eq!(path, "/api/users");
    }

    #[test]
    fn test_is_probable_url() {
        let normalizer = UrlNormalizer::default_permissive();

        assert!(normalizer.is_probable_url("/users/123"));
        assert!(normalizer.is_probable_url("https://example.com/api/users"));
        assert!(normalizer.is_probable_url("ENV_VAR:API_URL:/users"));
        assert!(normalizer.is_probable_url("process.env.API_URL + \"/users\""));
        assert!(normalizer.is_probable_url("${API_URL}/users/${userId}"));
        assert!(normalizer.is_probable_url("api/users"));

        assert!(!normalizer.is_probable_url("ordersResp"));
        assert!(!normalizer.is_probable_url("resp.json()"));
        assert!(!normalizer.is_probable_url(""));
    }

    #[test]
    fn test_is_external_via_normalize() {
        let config = create_test_config();
        let normalizer = UrlNormalizer::new(&config);

        assert!(
            normalizer
                .normalize("https://api.stripe.com/v1/charges")
                .is_external
        );
        assert!(
            !normalizer
                .normalize("https://user-service.internal/users")
                .is_external
        );
        assert!(
            normalizer
                .normalize("ENV_VAR:GITHUB_API:/repos")
                .is_external
        );
    }

    #[test]
    fn test_is_internal_via_normalize() {
        let config = create_test_config();
        let normalizer = UrlNormalizer::new(&config);

        assert!(
            normalizer
                .normalize("https://user-service.internal/users")
                .is_internal
        );
        assert!(normalizer.normalize("ENV_VAR:API_URL:/users").is_internal);
        assert!(
            !normalizer
                .normalize("https://api.stripe.com/v1/charges")
                .is_internal
        );
    }

    #[test]
    fn test_process_env_pattern() {
        let config = create_test_config();
        let normalizer = UrlNormalizer::new(&config);

        let result = normalizer.normalize("process.env.API_URL + \"/users\"");

        assert_eq!(result.path, "/users");
        assert!(result.is_internal);
    }

    #[test]
    fn test_process_env_pattern_with_multiple_segments_and_template_params() {
        let config = create_test_config();
        let normalizer = UrlNormalizer::new(&config);

        let result = normalizer.normalize("${process.env.API_URL}/users/${userId}");

        assert_eq!(result.path, "/users/:userId");
        assert!(result.is_internal);
    }

    #[test]
    fn test_process_env_pattern_with_backticks_and_template_params() {
        let config = create_test_config();
        let normalizer = UrlNormalizer::new(&config);

        let result = normalizer.normalize("process.env.API_URL + `/users/${userId}`");

        assert_eq!(result.path, "/users/:userId");
        assert!(result.is_internal);
    }

    #[test]
    fn test_process_env_pattern_without_plus_multiple_segments() {
        let config = create_test_config();
        let normalizer = UrlNormalizer::new(&config);

        let result = normalizer.normalize("process.env.API_URL/users/123/orders");

        assert_eq!(result.path, "/users/123/orders");
        assert!(result.is_internal);
    }

    #[test]
    fn test_env_var_pattern_with_template_params() {
        let config = create_test_config();
        let normalizer = UrlNormalizer::new(&config);

        let result = normalizer.normalize("ENV_VAR:API_URL:/users/${userId}");

        assert_eq!(result.path, "/users/:userId");
        assert!(result.is_internal);
    }

    #[test]
    fn test_normalize_template_literal_strips_trailing_backtick() {
        let config = Config {
            service_name: None,
            internal_domains: HashSet::new(),
            external_domains: HashSet::new(),
            internal_env_vars: ["ORDER_SERVICE_URL"]
                .iter()
                .map(|s| s.to_string())
                .collect(),
            external_env_vars: HashSet::new(),
        };
        let normalizer = UrlNormalizer::new(&config);

        let result = normalizer.normalize("`${process.env.ORDER_SERVICE_URL}/api/orders/101`");
        assert_eq!(result.path, "/api/orders/101");
        assert!(result.is_internal);
    }

    #[test]
    fn test_unknown_domain_treated_as_potentially_internal() {
        let config = create_test_config();
        let normalizer = UrlNormalizer::new(&config);

        // Unknown domain - not explicitly internal or external
        let result = normalizer.normalize("https://unknown-service.local/api/data");

        assert_eq!(result.path, "/api/data");
        assert!(!result.is_internal);
        assert!(!result.is_external);
        // Unknown domains are not marked as internal, but also not external
        // This allows them to be matched against endpoints
        let unknown_result = normalizer.normalize("https://unknown-service.local/api/data");
        assert!(!unknown_result.is_internal);
        assert!(!unknown_result.is_external);
    }
}
