//! Path-matching primitives shared by every Carrick surface that compares
//! HTTP route paths.
//!
//! Extracted verbatim from the scanner's `MountGraph` (whose methods now
//! delegate here) so the scanner and, via the optional `wasm` feature,
//! Node.js consumers run the exact same matching semantics. Everything is a
//! pure function over `&str`; the native build has zero dependencies.
//!
//! The `wasm` feature (default off) adds wasm-bindgen exports for
//! [`paths_match`] and [`is_param_segment`]:
//!
//! ```text
//! cargo build --target wasm32-unknown-unknown -p carrick-match --features wasm
//! wasm-bindgen --target nodejs --out-dir <dir> \
//!     target/wasm32-unknown-unknown/<profile>/carrick_match.wasm
//! ```

/// Whether a producer route path matches a consumer call path.
///
/// `endpoint_path` is the declared route on the producer side (it may carry
/// `:param`/`{param}`/`<param>`/`[param]` placeholders, trailing `?` optional
/// markers, and `*`/`**`/`(.*)` wildcards). `call_path` is the canonical
/// consumer call path. Matches on exact equality, parameter segments
/// (symmetric, see [`path_matches_with_params`]), or wildcards (see
/// [`path_matches_with_wildcards`]).
#[cfg_attr(feature = "wasm", wasm_bindgen::prelude::wasm_bindgen)]
pub fn paths_match(endpoint_path: &str, call_path: &str) -> bool {
    // Exact match
    if endpoint_path == call_path {
        return true;
    }

    // Parameter matching (e.g., /users/:id matches /users/123)
    if path_matches_with_params(endpoint_path, call_path) {
        return true;
    }

    // Try matching with wildcards
    if path_matches_with_wildcards(endpoint_path, call_path) {
        return true;
    }

    false
}

/// Segment-by-segment matching with route parameters (on either side) and
/// trailing `?` optional segments on the endpoint side.
pub fn path_matches_with_params(endpoint_path: &str, call_path: &str) -> bool {
    let endpoint_segments: Vec<&str> = endpoint_path.split('/').collect();
    let call_segments: Vec<&str> = call_path.split('/').collect();

    // Handle optional segments (e.g., /users/:id?)
    let endpoint_required_count = endpoint_segments
        .iter()
        .filter(|s| !s.ends_with('?'))
        .count();

    // Call must have at least required segments and at most all segments
    if call_segments.len() < endpoint_required_count
        || call_segments.len() > endpoint_segments.len()
    {
        return false;
    }

    for (i, endpoint_seg) in endpoint_segments.iter().enumerate() {
        // Check if this is an optional segment
        let is_optional = endpoint_seg.ends_with('?');
        let seg = endpoint_seg.trim_end_matches('?');

        // If we're past the call segments, remaining endpoint segments must be optional
        if i >= call_segments.len() {
            if !is_optional {
                return false;
            }
            continue;
        }

        let call_seg = call_segments[i];

        // A path parameter on EITHER side matches the other side. This must be
        // symmetric: a caller URL with a variable normalizes to a `:param` segment
        // and must match a concrete provider segment, just as a caller's concrete
        // value must match a provider's `:param`. We also accept the other param
        // syntaxes (`{id}`, `<id>`, `[id]`) so that routes captured verbatim from
        // frameworks that don't use Express-style colons still match cross-repo.
        if is_param_segment(seg) || is_param_segment(call_seg) {
            continue;
        }

        // Exact match required for non-parameter segments
        if seg != call_seg {
            return false;
        }
    }

    true
}

/// Returns true if a single path segment is a route parameter placeholder in any
/// of the common syntaxes: `:id` (Express/path-to-regexp), `{id}` (OpenAPI/Fastify),
/// `<id>` (Flask-style), or `[id]`/`[...id]` (Next.js dynamic segments).
/// Also the param definition `canonical_path_has_literal_segment` (#307)
/// reuses, so the noise gate and the matcher agree on what a placeholder is.
#[cfg_attr(feature = "wasm", wasm_bindgen::prelude::wasm_bindgen)]
pub fn is_param_segment(seg: &str) -> bool {
    seg.starts_with(':')
        || (seg.starts_with('{') && seg.ends_with('}'))
        || (seg.starts_with('<') && seg.ends_with('>'))
        || (seg.starts_with('[') && seg.ends_with(']'))
}

/// Match paths with wildcard patterns
///
/// Supports:
/// - `*` matches a single path segment
/// - `**` or `(.*)` matches zero or more path segments
pub fn path_matches_with_wildcards(endpoint_path: &str, call_path: &str) -> bool {
    // Check for catch-all patterns
    if endpoint_path.ends_with("/*") || endpoint_path.ends_with("/**") {
        let prefix = endpoint_path.trim_end_matches("/**").trim_end_matches("/*");
        return call_path.starts_with(prefix);
    }

    // Check for regex-style catch-all
    if endpoint_path.ends_with("/(.*)") {
        let prefix = endpoint_path.trim_end_matches("/(.*)");
        return call_path.starts_with(prefix);
    }

    // Check for single-segment wildcards in the middle
    let endpoint_segments: Vec<&str> = endpoint_path.split('/').collect();
    let call_segments: Vec<&str> = call_path.split('/').collect();

    if endpoint_segments.len() != call_segments.len() {
        return false;
    }

    for (endpoint_seg, call_seg) in endpoint_segments.iter().zip(call_segments.iter()) {
        if *endpoint_seg == "*" {
            continue; // Single-segment wildcard matches anything
        }
        if endpoint_seg != call_seg {
            return false;
        }
    }

    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_paths_match_dispatch() {
        // Exact match
        assert!(paths_match("/users", "/users"));
        // Parameter matching (e.g., /users/:id matches /users/123)
        assert!(paths_match("/users/:id", "/users/123"));
        // Wildcard matching
        assert!(paths_match("/api/**", "/api/users/123/orders"));
        // No match
        assert!(!paths_match("/users", "/orders"));
        assert!(!paths_match("/users/:id", "/users/123/orders"));
    }

    #[test]
    fn test_path_matches_with_optional_segments() {
        // Test optional segment matching
        assert!(path_matches_with_params("/users/:id?", "/users"));
        assert!(path_matches_with_params("/users/:id?", "/users/123"));
        assert!(!path_matches_with_params("/users/:id", "/users")); // Not optional
    }

    #[test]
    fn test_path_matches_with_params_is_symmetric() {
        // A param on the endpoint side matches a concrete caller segment.
        assert!(path_matches_with_params("/users/:id", "/users/123"));
        // ...and a param on the caller side (a normalized `${id}` interpolation) matches
        // a concrete provider segment. This direction previously failed.
        assert!(path_matches_with_params("/users/123", "/users/:id"));
        // Params on both sides match.
        assert!(path_matches_with_params("/users/:id", "/users/:userId"));
    }

    #[test]
    fn test_path_matches_with_params_across_syntaxes() {
        // A caller normalized to `:id` must match a provider route captured verbatim in
        // OpenAPI/Fastify (`{id}`), Flask (`<id>`), or Next.js (`[id]`) syntax.
        assert!(path_matches_with_params("/users/{id}", "/users/:id"));
        assert!(path_matches_with_params("/users/<id>", "/users/:id"));
        assert!(path_matches_with_params("/users/[id]", "/users/:id"));
        // ...and against a concrete value.
        assert!(path_matches_with_params("/users/{id}", "/users/123"));
        // Non-param segments must still match exactly.
        assert!(!path_matches_with_params("/users/{id}", "/orders/123"));
    }

    #[test]
    fn test_path_matches_with_wildcards() {
        // Test wildcard matching
        assert!(path_matches_with_wildcards("/api/*", "/api/users"));
        assert!(path_matches_with_wildcards(
            "/api/**",
            "/api/users/123/orders"
        ));
        assert!(path_matches_with_wildcards(
            "/files/(.*)",
            "/files/path/to/file.txt"
        ));

        // Regression: file-based routing synthesizes Next.js catch-all routes
        // (`app/files/[...slug]/route.ts`) as `/files/**`, which must match
        // single- and multi-segment caller URLs. A named catch-all (the old
        // `*slug` emission) would be treated as a literal and NOT match —
        // guards against regressing the router.
        assert!(path_matches_with_wildcards("/files/**", "/files/foo"));
        assert!(path_matches_with_wildcards("/files/**", "/files/foo/bar"));
        assert!(!path_matches_with_wildcards("/files/*slug", "/files/foo"));
    }

    #[test]
    fn test_is_param_segment_syntaxes() {
        assert!(is_param_segment(":id"));
        assert!(is_param_segment("{id}"));
        assert!(is_param_segment("<id>"));
        assert!(is_param_segment("[id]"));
        assert!(is_param_segment("[...slug]"));
        assert!(!is_param_segment("users"));
        assert!(!is_param_segment("*"));
        assert!(!is_param_segment("v${n}"));
    }
}
