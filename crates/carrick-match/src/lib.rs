//! Path-matching primitives shared by every Carrick surface that compares
//! HTTP route paths.
//!
//! Extracted verbatim from the scanner's `MountGraph` (whose methods now
//! delegate here) so the scanner and, via the optional `wasm` feature,
//! Node.js consumers run the exact same matching semantics. Everything is a
//! pure function over `&str`; the native build has zero dependencies.
//!
//! Two questions, two functions (#378/#381):
//!
//! - [`paths_match`] answers ROUTING truth: "does this route pattern cover
//!   this call path?" A catch-all `/*` covers everything; that is what a
//!   router would do with the request.
//! - [`match_agreement`] answers PAIRING strength: how many literal segments
//!   the two sides actually agree on. Params and wildcards cover segments
//!   without vouching for them, so they contribute nothing. A pairing with
//!   zero agreement (a wildcard-only producer absorbing an arbitrary call)
//!   carries no signal and must not be reported as a cross-repo match; among
//!   several matching producers, the ones with maximal agreement are the
//!   real candidates (a catch-all never beats a concrete route).
//!
//! `paths_match` is defined as `match_agreement(..).is_some()`, so the two
//! can never disagree about WHETHER a pair matches.
//!
//! The `wasm` feature (default off) adds wasm-bindgen exports for
//! [`paths_match`], [`is_param_segment`], [`match_agreement`],
//! [`path_literal_specificity`], and [`is_catch_all_path`]:
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
/// consumer call path. Matches on exact equality, on parameter segments
/// (symmetric: a placeholder on either side matches the other side), or on
/// wildcard patterns (trailing wildcards are prefix matches; a mid-path `*`
/// matches exactly one segment).
///
/// This is ROUTING truth only. Whether the pair is worth reporting as a
/// cross-repo match is a separate question — see [`match_agreement`].
#[cfg_attr(feature = "wasm", wasm_bindgen::prelude::wasm_bindgen)]
pub fn paths_match(endpoint_path: &str, call_path: &str) -> bool {
    match_agreement(endpoint_path, call_path).is_some()
}

/// How strongly a producer route and a consumer call path agree, or `None`
/// if they do not match at all.
///
/// The agreement score is the number of segment positions where BOTH sides
/// carry the same literal text. Placeholders (`:id`, `{id}`, …) and
/// wildcards (`*`, `**`, `(.*)`) cover segments without vouching for them,
/// so they contribute 0 — a param is a declared one-segment placeholder, and
/// a trailing catch-all swallows an unbounded tail the producer asserts
/// nothing about. For a trailing-wildcard match only the literal segments of
/// the prefix count.
///
/// `Some(0)` therefore means "routes, but with no corroborating signal":
/// `GET /*` vs anything, `/:slug` vs `/about`. Surfaces must not report such
/// pairs as matches (#381); they are routing/absorption only. Relative use:
/// among all producers matching one call, pair the call with the
/// maximal-agreement producers — a catch-all (`/api/**`, agreement 1) never
/// outranks a concrete route (`/api/v1/chat/new`, agreement 4).
#[cfg_attr(feature = "wasm", wasm_bindgen::prelude::wasm_bindgen)]
pub fn match_agreement(endpoint_path: &str, call_path: &str) -> Option<u32> {
    // Exact match: every literal segment is shared by construction.
    if endpoint_path == call_path {
        return Some(path_literal_specificity(endpoint_path));
    }

    // Parameter matching (e.g., /users/:id matches /users/123), then
    // wildcards — same dispatch order the boolean matcher always had.
    agreement_with_params(endpoint_path, call_path)
        .or_else(|| agreement_with_wildcards(endpoint_path, call_path))
}

/// Number of LITERAL segments in a path: segments that are real text rather
/// than a placeholder (`:id`, `{id}`, `<id>`, `[id]`), a wildcard (`*`,
/// `**`, `(.*)`), or empty/whitespace. This is the most agreement any pairing
/// with this path could ever produce — 0 means the path can never corroborate
/// a match (`/*`, `/**`, `/:param`), i.e. it is routing infrastructure, not a
/// concrete contract.
#[cfg_attr(feature = "wasm", wasm_bindgen::prelude::wasm_bindgen)]
pub fn path_literal_specificity(path: &str) -> u32 {
    path.split('/')
        .filter(|seg| is_literal_segment(seg.trim_end_matches('?')))
        .count() as u32
}

/// Whether a producer route ends in a catch-all wildcard (`/*`, `/**`,
/// `/(.*)`) — the patterns the matcher treats as unbounded prefix matches.
/// Such a route is a mount or fallback: it absorbs calls by design, so
/// "no consumer matched it" (orphaned) is not a meaningful observation.
#[cfg_attr(feature = "wasm", wasm_bindgen::prelude::wasm_bindgen)]
pub fn is_catch_all_path(path: &str) -> bool {
    path.ends_with("/*") || path.ends_with("/**") || path.ends_with("/(.*)")
}

/// A segment that carries literal, comparable text: non-empty after
/// whitespace trim, not a route parameter, not a wildcard token.
fn is_literal_segment(seg: &str) -> bool {
    !seg.trim().is_empty() && !is_param_segment(seg) && !is_wildcard_segment(seg)
}

/// The wildcard tokens the matcher understands as single segments.
fn is_wildcard_segment(seg: &str) -> bool {
    seg == "*" || seg == "**" || seg == "(.*)"
}

/// Segment-by-segment matching with route parameters (on either side) and
/// trailing `?` optional segments on the endpoint side. Returns the literal
/// agreement count, or `None` if the paths don't match.
fn agreement_with_params(endpoint_path: &str, call_path: &str) -> Option<u32> {
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
        return None;
    }

    let mut agreement = 0u32;
    for (i, endpoint_seg) in endpoint_segments.iter().enumerate() {
        // Check if this is an optional segment
        let is_optional = endpoint_seg.ends_with('?');
        let seg = endpoint_seg.trim_end_matches('?');

        // If we're past the call segments, remaining endpoint segments must be optional
        if i >= call_segments.len() {
            if !is_optional {
                return None;
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
            // A param stands in for a real path value; an empty or
            // whitespace-only segment on the other side is not one. Without
            // this guard a route indexed with a stray blank segment (#332,
            // "/api/users/ ") reads as satisfying "/api/users/:userId".
            if seg.trim().is_empty() || call_seg.trim().is_empty() {
                return None;
            }
            // A param covers the segment without vouching for it: no agreement.
            continue;
        }

        // Exact match required for non-parameter segments
        if seg != call_seg {
            return None;
        }

        if is_literal_segment(seg) {
            agreement += 1;
        }
    }

    Some(agreement)
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

/// Match paths with wildcard patterns, returning the literal agreement count.
///
/// Match semantics as implemented (kept byte-identical to the scanner's
/// original boolean matcher):
/// - An endpoint ending in `/*`, `/**`, or `/(.*)` is a prefix match: the
///   call path only has to start with the part before the wildcard. A
///   trailing `/*` therefore matches any number of segments, exactly like
///   `/**`, not just one. Agreement is the literal specificity of the
///   prefix — the wildcard-covered tail vouches for nothing.
/// - Anywhere else, a `*` segment matches exactly one call segment, and both
///   paths must have the same number of segments. Agreement counts the
///   literal positions.
fn agreement_with_wildcards(endpoint_path: &str, call_path: &str) -> Option<u32> {
    // Check for catch-all patterns
    if endpoint_path.ends_with("/*") || endpoint_path.ends_with("/**") {
        let prefix = endpoint_path.trim_end_matches("/**").trim_end_matches("/*");
        return call_path
            .starts_with(prefix)
            .then(|| path_literal_specificity(prefix));
    }

    // Check for regex-style catch-all
    if endpoint_path.ends_with("/(.*)") {
        let prefix = endpoint_path.trim_end_matches("/(.*)");
        return call_path
            .starts_with(prefix)
            .then(|| path_literal_specificity(prefix));
    }

    // Check for single-segment wildcards in the middle
    let endpoint_segments: Vec<&str> = endpoint_path.split('/').collect();
    let call_segments: Vec<&str> = call_path.split('/').collect();

    if endpoint_segments.len() != call_segments.len() {
        return None;
    }

    let mut agreement = 0u32;
    for (endpoint_seg, call_seg) in endpoint_segments.iter().zip(call_segments.iter()) {
        if *endpoint_seg == "*" {
            continue; // Single-segment wildcard matches anything
        }
        if endpoint_seg != call_seg {
            return None;
        }
        if is_literal_segment(endpoint_seg) {
            agreement += 1;
        }
    }

    Some(agreement)
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
        assert!(agreement_with_params("/users/:id?", "/users").is_some());
        assert!(agreement_with_params("/users/:id?", "/users/123").is_some());
        assert!(agreement_with_params("/users/:id", "/users").is_none()); // Not optional
    }

    #[test]
    fn test_path_matches_with_params_is_symmetric() {
        // A param on the endpoint side matches a concrete caller segment.
        assert!(agreement_with_params("/users/:id", "/users/123").is_some());
        // ...and a param on the caller side (a normalized `${id}` interpolation) matches
        // a concrete provider segment. This direction previously failed.
        assert!(agreement_with_params("/users/123", "/users/:id").is_some());
        // Params on both sides match.
        assert!(agreement_with_params("/users/:id", "/users/:userId").is_some());
    }

    #[test]
    fn test_path_matches_with_params_across_syntaxes() {
        // A caller normalized to `:id` must match a provider route captured verbatim in
        // OpenAPI/Fastify (`{id}`), Flask (`<id>`), or Next.js (`[id]`) syntax.
        assert!(agreement_with_params("/users/{id}", "/users/:id").is_some());
        assert!(agreement_with_params("/users/<id>", "/users/:id").is_some());
        assert!(agreement_with_params("/users/[id]", "/users/:id").is_some());
        // ...and against a concrete value.
        assert!(agreement_with_params("/users/{id}", "/users/123").is_some());
        // Non-param segments must still match exactly.
        assert!(agreement_with_params("/users/{id}", "/orders/123").is_none());
    }

    #[test]
    fn test_path_matches_with_wildcards() {
        // Test wildcard matching
        assert!(agreement_with_wildcards("/api/*", "/api/users").is_some());
        assert!(agreement_with_wildcards("/api/**", "/api/users/123/orders").is_some());
        assert!(agreement_with_wildcards("/files/(.*)", "/files/path/to/file.txt").is_some());

        // Regression: file-based routing synthesizes Next.js catch-all routes
        // (`app/files/[...slug]/route.ts`) as `/files/**`, which must match
        // single- and multi-segment caller URLs. A named catch-all (the old
        // `*slug` emission) would be treated as a literal and NOT match —
        // guards against regressing the router.
        assert!(agreement_with_wildcards("/files/**", "/files/foo").is_some());
        assert!(agreement_with_wildcards("/files/**", "/files/foo/bar").is_some());
        assert!(agreement_with_wildcards("/files/*slug", "/files/foo").is_none());
    }

    #[test]
    fn test_param_never_matches_empty_or_whitespace_segment() {
        // Regression (#333): an endpoint whose full_path carried a trailing
        // whitespace segment ("/api/users/ ", see #332) matched the consumer
        // call "/api/users/:userId" because a param on either side skipped the
        // comparison entirely. A param stands in for a real path value, and an
        // empty or whitespace segment is not one.
        assert!(agreement_with_params("/api/users/ ", "/api/users/:userId").is_none());
        assert!(agreement_with_params("/api/users/:userId", "/api/users/ ").is_none());
        assert!(agreement_with_params("/api/users/", "/api/users/:userId").is_none());
        assert!(!paths_match("/api/users/ ", "/api/users/:userId"));
        // Real values still match a param on the other side.
        assert!(agreement_with_params("/api/users/:id", "/api/users/123").is_some());
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

    // ---- #378/#381: agreement scoring ----

    /// `paths_match` is defined over `match_agreement`, so any matching pair
    /// must score, and any non-matching pair must not.
    #[test]
    fn match_agreement_is_some_iff_paths_match() {
        let cases = [
            ("/users", "/users"),
            ("/users/:id", "/users/123"),
            ("/api/**", "/api/users/123/orders"),
            ("/*", "/anything/at/all"),
            ("/users", "/orders"),
            ("/users/:id", "/users/123/orders"),
            ("/files/*slug", "/files/foo"),
        ];
        for (endpoint, call) in cases {
            assert_eq!(
                paths_match(endpoint, call),
                match_agreement(endpoint, call).is_some(),
                "paths_match and match_agreement disagree on ({endpoint}, {call})"
            );
        }
    }

    #[test]
    fn match_agreement_counts_shared_literal_segments() {
        // Exact literal paths: every segment agrees.
        assert_eq!(match_agreement("/oauth/token", "/oauth/token"), Some(2));
        assert_eq!(match_agreement("/nodes", "/nodes"), Some(1));
        assert_eq!(
            match_agreement("/api/v1/chat/new", "/api/v1/chat/new"),
            Some(4)
        );
        // Params cover a segment without vouching for it.
        assert_eq!(match_agreement("/users/:id", "/users/123"), Some(1));
        assert_eq!(match_agreement("/users/123", "/users/:id"), Some(1));
        assert_eq!(match_agreement("/users/:id", "/users/:userId"), Some(1));
        // Mid-path `*` likewise.
        assert_eq!(match_agreement("/api/*/users", "/api/v2/users"), Some(2));
        // Non-matching pairs score nothing.
        assert_eq!(match_agreement("/users", "/orders"), None);
    }

    /// #381: wildcard-only producers carry no agreement signal — the pair
    /// routes (`Some`) but corroborates nothing (`0`).
    #[test]
    fn wildcard_only_producers_have_zero_agreement() {
        assert_eq!(match_agreement("/*", "/repos/acme/widgets"), Some(0));
        assert_eq!(match_agreement("/**", "/internal/metrics/export"), Some(0));
        assert_eq!(match_agreement("/(.*)", "/spa/route"), Some(0));
        // A root param route is the same class: nothing literal to agree on.
        assert_eq!(match_agreement("/:slug", "/about"), Some(0));
        // Root-to-root: nothing literal either.
        assert_eq!(match_agreement("/", "/"), Some(0));
    }

    /// #381: a catch-all with a literal mount prefix DOES carry the prefix's
    /// signal — but only the prefix's. The swallowed tail vouches for nothing,
    /// so a concrete route over the same call always outranks it.
    #[test]
    fn trailing_wildcard_agreement_is_prefix_only() {
        assert_eq!(match_agreement("/api/**", "/api/v1/chat/new"), Some(1));
        assert_eq!(match_agreement("/api/*", "/api/v1/chat/new"), Some(1));
        assert_eq!(match_agreement("/files/(.*)", "/files/a/b/c"), Some(1));
        let concrete = match_agreement("/api/v1/chat/new", "/api/v1/chat/new").unwrap();
        let catch_all = match_agreement("/api/**", "/api/v1/chat/new").unwrap();
        assert!(
            concrete > catch_all,
            "a concrete producer must outrank a catch-all for the same call"
        );
    }

    #[test]
    fn test_path_literal_specificity() {
        assert_eq!(path_literal_specificity("/"), 0);
        assert_eq!(path_literal_specificity("/*"), 0);
        assert_eq!(path_literal_specificity("/**"), 0);
        assert_eq!(path_literal_specificity("/(.*)"), 0);
        assert_eq!(path_literal_specificity("/:slug"), 0);
        assert_eq!(path_literal_specificity("/nodes"), 1);
        assert_eq!(path_literal_specificity("/api/**"), 1);
        assert_eq!(path_literal_specificity("/oauth/token"), 2);
        assert_eq!(path_literal_specificity("/users/:id/orders"), 2);
        assert_eq!(path_literal_specificity("/api/v1/chat/new"), 4);
        // Optional marker doesn't change what's literal.
        assert_eq!(path_literal_specificity("/users/:id?"), 1);
    }

    #[test]
    fn test_is_catch_all_path() {
        assert!(is_catch_all_path("/*"));
        assert!(is_catch_all_path("/**"));
        assert!(is_catch_all_path("/(.*)"));
        assert!(is_catch_all_path("/api/**"));
        assert!(is_catch_all_path("/files/(.*)"));
        assert!(!is_catch_all_path("/api/v1/chat/new"));
        assert!(!is_catch_all_path("/users/:id"));
        assert!(!is_catch_all_path("/files/*slug"));
        // Mid-path single-segment wildcard is not a catch-all.
        assert!(!is_catch_all_path("/api/*/users"));
    }
}
