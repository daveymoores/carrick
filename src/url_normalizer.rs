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
    /// True when the entire URL was a single opaque variable the scanner could
    /// not resolve (e.g. `fetch(new Request(url))` → `${url}`). Such a "call"
    /// has no comparable path, so callers should skip it rather than report a
    /// bogus missing endpoint like `GET ${url}`.
    pub is_unresolved: bool,
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

        // The LLM sometimes returns URL targets verbatim from source — including
        // the JS template-literal backticks or string-literal quotes. Strip
        // those wrapper chars before pattern dispatch so a target like
        // `${USER_SERVICE_URL}/api/users` (with literal backticks) reaches
        // `normalize_template_literal::starts_with("${")`, which strips the
        // base URL host. Without the trim the dispatch *does* still hit the
        // template-literal branch (the URL contains "${"), but the host strip
        // is skipped and `convert_interpolations_to_params` rewrites every
        // `${VAR}` to `:VAR`; `clean_path` then prefixes a leading "/", and
        // the final path becomes `/:USER_SERVICE_URL/api/users` — never
        // matching a producer's `/api/users`.
        let url = url.trim_matches(|c| c == '`' || c == '"' || c == '\'');

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
            is_unresolved: false,
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
                is_unresolved: false,
            }
        } else {
            NormalizedUrl {
                path: self.clean_path(url),
                is_internal: false,
                is_external: false,
                original,
                stripped_host: None,
                is_unresolved: false,
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
            is_unresolved: false,
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

        let mut leading_var_stripped = false;

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
            leading_var_stripped = true;
        }

        // The whole URL was a single opaque variable (e.g. `${url}` from
        // `fetch(new Request(url))`) when stripping the leading `${...}` left no
        // path behind and the variable wasn't a configured internal/external
        // host. There's nothing to match, so flag it for callers to skip. A
        // query string or fragment alone (`${url}?x=1`, `${url}#h`) is not a
        // path — strip it before the emptiness check, or it would slip through
        // and `clean_path` would reduce it to `/` and falsely match root.
        let result_path = result.split(['?', '#']).next().unwrap_or("");
        let is_unresolved = leading_var_stripped
            && !is_internal
            && !is_external
            && result_path.trim_matches('/').is_empty();

        // Convert remaining ${varName} to :varName for path parameter matching
        let path = self.convert_interpolations_to_params(&result);

        NormalizedUrl {
            path: self.clean_path(&path),
            is_internal,
            is_external,
            original,
            stripped_host,
            is_unresolved,
        }
    }

    /// Convert ${varName} interpolations to :varName path parameters.
    ///
    /// Member/call/complex inner expressions are reduced to their final
    /// identifier, so `${row.pr_number}` yields the valid segment `:pr_number`
    /// rather than the malformed `:row.pr_number`. The param name is cosmetic for
    /// matching (`:x` and `:y` are equivalent param segments; see
    /// `is_param_segment` in mount_graph.rs), so collapsing to one clean token is
    /// safe and keeps each param a single well-formed segment.
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
                result.push_str(&Self::clean_param_name(&var_name));
            } else {
                result.push(c);
            }
        }

        result
    }

    /// Reduce an interpolation expression to a single clean param identifier.
    /// `row.pr_number` -> `pr_number`, `userId` -> `userId`; an expression with no
    /// usable leading-alpha identifier (empty, numeric, operators) -> `param`.
    fn clean_param_name(expr: &str) -> String {
        // Take the final run of identifier characters: for a member/bracket/call
        // expression that is the accessed key, which is the meaningful name.
        let mut last = String::new();
        let mut cur = String::new();
        for c in expr.chars() {
            if c.is_alphanumeric() || c == '_' {
                cur.push(c);
            } else if !cur.is_empty() {
                last = std::mem::take(&mut cur);
            }
        }
        if !cur.is_empty() {
            last = cur;
        }
        if last.is_empty() || last.starts_with(|c: char| c.is_ascii_digit()) {
            "param".to_string()
        } else {
            last
        }
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
            is_unresolved: false,
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
        Self::host_matches_domains(host, &self.internal_domains)
    }

    /// Check if a host is configured as external
    fn is_external_host(&self, host: &str) -> bool {
        Self::host_matches_domains(host, &self.external_domains)
    }

    /// True if `host` exactly equals, or is a subdomain of, one of `domains`.
    ///
    /// Comparison is case-insensitive because DNS hostnames are. We deliberately do
    /// NOT do substring matching (the old `domain.contains(host)` clause): it was
    /// backwards and could flip internal/external classification — e.g. host `company`
    /// would spuriously match a configured `api.company.com`, and `api.com` would fail
    /// to match `api.company.com`.
    fn host_matches_domains(host: &str, domains: &HashSet<String>) -> bool {
        // Strip port if present, then lowercase.
        let host_without_port = host.split(':').next().unwrap_or(host).to_ascii_lowercase();

        domains.iter().any(|domain| {
            let domain = Self::domain_host(domain);
            host_without_port == domain || host_without_port.ends_with(&format!(".{}", domain))
        })
    }

    /// Reduce a configured domain entry to a bare, comparable hostname.
    ///
    /// carrick.json lets users write `externalDomains`/`internalDomains` either
    /// as bare hosts (`api.resend.com`) or full URLs (`https://api.resend.com`).
    /// Incoming call hosts are always bare, so we normalize config entries the
    /// same way — strip the scheme, any path/query, and the port — before
    /// comparing. Without this, a `https://`-prefixed entry never matches and
    /// the call is misclassified as internal (reported as a missing endpoint).
    fn domain_host(domain: &str) -> String {
        // Lowercase first so a mixed-case scheme (`HTTPS://`) still strips —
        // URL schemes are case-insensitive.
        let lower = domain.to_ascii_lowercase();
        let without_scheme = lower
            .strip_prefix("https://")
            .or_else(|| lower.strip_prefix("http://"))
            .unwrap_or(&lower);
        // Drop anything from the first '/' (path) onward, then the port.
        let host = without_scheme.split('/').next().unwrap_or(without_scheme);
        host.split(':').next().unwrap_or(host).to_string()
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

    /// Path to key a CONSUMER data call on in the cloud_data projection.
    ///
    /// Like `extract_path`, but conservative about WHICH targets get their host
    /// stripped: only a declared-internal env-var base (e.g. `internalEnvVars`
    /// in `carrick.json`) or a plain relative path (no host at all) is reduced
    /// to its bare route. A target whose host was stripped but is NOT internal —
    /// an external or unknown base like `${process.env.STRIPE_URL}/charges` — is
    /// returned verbatim, so a third-party call can never collide with an
    /// internal producer's `/charges`.
    ///
    /// This exists because the LLM `data_calls` path stores the raw target on
    /// the `DataFetchingCall` (`file_orchestrator.rs` fifth pass), and only the
    /// cross-repo *matcher* normalized it; the per-op cloud_data CALL key stayed
    /// un-normalized, so a consumer call like `${process.env.ANALYTICS_URL}/track`
    /// never keyed as `/track` and so never matched its producer or got a compat
    /// verdict.
    pub fn consumer_call_path(&self, url: &str) -> String {
        // A plain relative path (`/track`, `/users/${id}`) has no host to strip —
        // always safe to clean. NOT protocol-relative (`//host/...`), which has a
        // host. Checked on the raw target (minus literal quote/backtick wrappers).
        let trimmed = url.trim_matches(|c| c == '`' || c == '"' || c == '\'');
        let is_relative_path = trimmed.starts_with('/') && !trimmed.starts_with("//");

        let normalized = self.normalize(url);
        if normalized.is_internal || is_relative_path {
            normalized.path
        } else {
            // External / unknown / full-URL targets are kept VERBATIM. A
            // declared-external or unknown env-var base must not collide with an
            // internal producer's path; and a full URL with an interpolation in
            // its query (`https://h/p?u=${id}`) dispatches to the template-literal
            // branch, which would mangle the scheme — keep it raw rather than
            // emit `/https:/h/p`.
            url.to_string()
        }
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
            ..Default::default()
        }
    }

    /// The corpus-2 HTTP regression: a consumer call whose base is a declared-
    /// internal env var must key on the BARE route (so it matches its producer),
    /// while an unknown/external base must be kept VERBATIM (so a third-party
    /// call can't collide with an internal producer's path). `consumer_call_path`
    /// is what the cloud_data CALL projection uses; before it, the projection
    /// keyed on the raw LLM `target_url` and these edges never matched.
    #[test]
    fn consumer_call_path_strips_internal_base_keeps_external_raw() {
        let n = UrlNormalizer::new(&create_test_config());

        // Internal env-var base (process.env form, and the const-resolved form) →
        // bare route. This is the exact corpus-2 miss.
        assert_eq!(
            n.consumer_call_path("${process.env.API_URL}/users"),
            "/users"
        );
        assert_eq!(n.consumer_call_path("${API_URL}/users"), "/users");

        // Internal base + a path param: base stripped, param preserved.
        let with_param = n.consumer_call_path("${process.env.API_URL}/users/${id}");
        assert!(
            with_param.starts_with("/users/:") && !with_param.contains("process.env"),
            "expected /users/:param, got {with_param}"
        );

        // A relative path (no host) is returned cleaned, shape unchanged.
        assert_eq!(n.consumer_call_path("/track"), "/track");

        // An UNKNOWN base (NOT in internalEnvVars) is returned VERBATIM — never
        // normalized into something that could match an internal producer.
        assert_eq!(
            n.consumer_call_path("${process.env.STRIPE_URL}/charges"),
            "${process.env.STRIPE_URL}/charges"
        );

        // A full URL — even with an interpolation in its query string, which
        // dispatches to the template-literal branch — is kept VERBATIM, never
        // mangled to `/https:/orders.internal/...` by `clean_path`.
        assert_eq!(
            n.consumer_call_path("https://orders.internal/api/orders?user=${userId}"),
            "https://orders.internal/api/orders?user=${userId}"
        );
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

    /// Regression: carrick.json domain lists are often written as full URLs
    /// (`https://api.resend.com`) rather than bare hosts. The incoming call host
    /// is always bare, so config entries must be scheme/path/port-stripped
    /// before comparison — otherwise the external call is misclassified as
    /// internal and reported as a bogus missing endpoint.
    #[test]
    fn test_external_domain_with_scheme_prefix_matches() {
        let config = Config {
            external_domains: ["https://api.resend.com", "https://eu.posthog.com/"]
                .iter()
                .map(|s| s.to_string())
                .collect(),
            internal_domains: ["https://api.company.com"]
                .iter()
                .map(|s| s.to_string())
                .collect(),
            ..Default::default()
        };
        let normalizer = UrlNormalizer::new(&config);

        let external = normalizer.normalize("https://api.resend.com/contacts");
        assert!(
            external.is_external,
            "scheme-prefixed external domain should match"
        );
        assert!(!external.is_internal);

        let internal = normalizer.normalize("https://api.company.com/v1/users");
        assert!(
            internal.is_internal,
            "scheme-prefixed internal domain should match"
        );
        assert!(!internal.is_external);

        // Schemes are case-insensitive — a mixed-case config entry must still match.
        let mixed = UrlNormalizer::new(&Config {
            external_domains: ["HTTPS://api.resend.com"]
                .iter()
                .map(|s| s.to_string())
                .collect(),
            ..Default::default()
        });
        assert!(
            mixed.normalize("https://api.resend.com/emails").is_external,
            "mixed-case scheme in config should still match"
        );
    }

    /// Regression: a call whose URL is a single opaque variable (e.g.
    /// `fetch(new Request(url))` surfacing as `${url}`) has no resolvable path.
    /// It must be flagged unresolved so callers skip it instead of emitting a
    /// bogus `GET ${url}` missing endpoint. A variable that only supplies the
    /// host but is followed by a real path stays resolvable.
    #[test]
    fn test_whole_url_variable_is_unresolved() {
        let config = create_test_config();
        let normalizer = UrlNormalizer::new(&config);

        assert!(normalizer.normalize("${url}").is_unresolved);
        assert!(normalizer.normalize("${url}/").is_unresolved);
        // A query string or fragment alone is not a path.
        assert!(normalizer.normalize("${url}?x=1").is_unresolved);
        assert!(normalizer.normalize("${url}#section").is_unresolved);
        // An unknown base var followed by a real path is NOT unresolved — the
        // path segment is still comparable.
        assert!(!normalizer.normalize("${base}/users").is_unresolved);
        // A configured internal base var is resolved, not unresolved.
        assert!(!normalizer.normalize("${API_URL}/users").is_unresolved);
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

    /// Regression (F3c): a dotted/member interpolation must collapse to its final
    /// identifier so the segment is a valid `:pr_number`, not the malformed
    /// `:row.pr_number` that the verbatim copy used to produce.
    #[test]
    fn test_normalize_dotted_interpolation_param() {
        let config = create_test_config();
        let normalizer = UrlNormalizer::new(&config);

        let result = normalizer.normalize("${API_URL}/pulls/${row.pr_number}/comments");

        assert_eq!(result.path, "/pulls/:pr_number/comments");
        assert!(result.is_internal);
    }

    /// Regression: the file-analyzer LLM intermittently emits URL targets
    /// wrapped in JS template-literal backticks (e.g. `` `${API_URL}/users` ``),
    /// copying the source verbatim. Pre-trim, the leading backtick made the
    /// inner `starts_with("${")` host-strip check fail, so only the inner
    /// `${VAR}` → `:VAR` conversion ran; with `clean_path`'s leading-slash
    /// guarantee the path came out as `/:API_URL/users` — never matching
    /// a producer's `/users`.
    #[test]
    fn test_normalize_strips_template_literal_backticks() {
        let config = create_test_config();
        let normalizer = UrlNormalizer::new(&config);

        let result = normalizer.normalize("`${API_URL}/users/${userId}`");

        assert_eq!(result.path, "/users/:userId");
        assert!(result.is_internal);
        assert_eq!(result.stripped_host, Some("${API_URL}".to_string()));
    }

    /// Same defence applies to single- and double-quoted string literals if
    /// the LLM ever emits those — the wrapper chars must not affect dispatch.
    #[test]
    fn test_normalize_strips_string_literal_quotes() {
        let config = create_test_config();
        let normalizer = UrlNormalizer::new(&config);

        let dq = normalizer.normalize("\"ENV_VAR:API_URL:/users\"");
        assert_eq!(dq.path, "/users");
        assert!(dq.is_internal);

        let sq = normalizer.normalize("'/users/:id'");
        assert_eq!(sq.path, "/users/:id");
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
            internal_env_vars: ["ORDER_SERVICE_URL"]
                .iter()
                .map(|s| s.to_string())
                .collect(),
            ..Default::default()
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
