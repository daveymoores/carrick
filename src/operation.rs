//! Protocol-tagged operation identity.
//!
//! Every producer (endpoint) and consumer (outbound call) is keyed by an
//! [`OperationKey`]. The protocol tag travels with the key so operations from
//! different protocols can never collide in matching, type-manifest aliases,
//! or the cloud index. HTTP-only code paths (mount-graph matching, REST
//! manifest building) dispatch through [`OperationKey::as_http`] and skip
//! other protocols; each protocol gets its own matcher.

use serde::{Deserialize, Serialize};
use std::fmt;

/// Protocol families Carrick recognizes. Used to tag operation keys and to
/// route the LLM pipeline: SWC candidates carry a protocol, and each
/// protocol with non-deterministic evidence gets its own guidance and
/// analyze-file prompt instead of diluting one prompt across all of them.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Protocol {
    Http,
    /// Extracted deterministically (SDL + documents); never LLM-routed.
    Graphql,
    /// `new WebSocket(...)` / `new EventSource(...)` call sites. Tagged by
    /// the scanner today so they stop reaching the HTTP prompt; the
    /// socket-specific prompt arrives with the Phase 2 work.
    Websocket,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GraphqlOperationKind {
    Query,
    Mutation,
    Subscription,
}

impl GraphqlOperationKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            GraphqlOperationKind::Query => "query",
            GraphqlOperationKind::Mutation => "mutation",
            GraphqlOperationKind::Subscription => "subscription",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "protocol", rename_all = "snake_case")]
pub enum OperationKey {
    /// An HTTP operation. For producers, `path` is the mount-resolved route
    /// (e.g. `/api/users/:id`). For consumers, `path` carries the raw call
    /// target (full URL, env-var template, unresolved expression) until URL
    /// normalization runs during matching.
    Http { method: String, path: String },
    /// A GraphQL operation, identified by root operation kind plus top-level
    /// field name (e.g. `query user`). Producers are schema root fields;
    /// consumers are the top-level fields of executable documents.
    Graphql {
        kind: GraphqlOperationKind,
        field: String,
    },
}

impl OperationKey {
    pub fn protocol(&self) -> Protocol {
        match self {
            OperationKey::Http { .. } => Protocol::Http,
            OperationKey::Graphql { .. } => Protocol::Graphql,
        }
    }

    /// Build an HTTP key. The method is stored trimmed and uppercased so one
    /// canonical form flows into aliases, matching, and the index; an empty
    /// method becomes `UNKNOWN` (mirroring `normalize_manifest_method`).
    pub fn http(method: &str, path: impl Into<String>) -> Self {
        let trimmed = method.trim();
        let method = if trimmed.is_empty() {
            "UNKNOWN".to_string()
        } else {
            trimmed.to_uppercase()
        };
        OperationKey::Http {
            method,
            path: path.into(),
        }
    }

    pub fn graphql(kind: GraphqlOperationKind, field: impl Into<String>) -> Self {
        OperationKey::Graphql {
            kind,
            field: field.into(),
        }
    }

    /// `(method, path)` when this is an HTTP operation. HTTP-only code paths
    /// (mount-graph matching, REST manifest building, alias generation)
    /// filter through this — it is the protocol dispatch point.
    pub fn as_http(&self) -> Option<(&str, &str)> {
        match self {
            OperationKey::Http { method, path } => Some((method, path)),
            OperationKey::Graphql { .. } => None,
        }
    }

    pub fn as_graphql(&self) -> Option<(GraphqlOperationKind, &str)> {
        match self {
            OperationKey::Http { .. } => None,
            OperationKey::Graphql { kind, field } => Some((*kind, field)),
        }
    }

    /// Stable identity string for hashing and dedup keys.
    pub fn canonical(&self) -> String {
        match self {
            OperationKey::Http { method, path } => format!("http|{}|{}", method, path),
            OperationKey::Graphql { kind, field } => {
                format!("graphql|{}|{}", kind.as_str(), field)
            }
        }
    }
}

impl fmt::Display for OperationKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            OperationKey::Http { method, path } => write!(f, "{} {}", method, path),
            OperationKey::Graphql { kind, field } => write!(f, "{} {}", kind.as_str(), field),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn http_key_canonicalizes_method() {
        let key = OperationKey::http(" get ", "/users");
        assert_eq!(key.as_http(), Some(("GET", "/users")));
        assert_eq!(key.canonical(), "http|GET|/users");
        assert_eq!(key.to_string(), "GET /users");
    }

    #[test]
    fn empty_method_becomes_unknown() {
        let key = OperationKey::http("", "/users");
        assert_eq!(key.as_http(), Some(("UNKNOWN", "/users")));
    }

    #[test]
    fn serde_round_trip_carries_protocol_tag() {
        let key = OperationKey::http("POST", "/api/orders");
        let json = serde_json::to_string(&key).unwrap();
        assert!(json.contains("\"protocol\":\"http\""), "got {}", json);
        let back: OperationKey = serde_json::from_str(&json).unwrap();
        assert_eq!(back, key);
    }

    #[test]
    fn graphql_key_identity_and_dispatch() {
        let key = OperationKey::graphql(GraphqlOperationKind::Query, "user");
        assert_eq!(key.as_http(), None);
        assert_eq!(
            key.as_graphql(),
            Some((GraphqlOperationKind::Query, "user"))
        );
        assert_eq!(key.canonical(), "graphql|query|user");
        assert_eq!(key.to_string(), "query user");

        let json = serde_json::to_string(&key).unwrap();
        assert!(json.contains("\"protocol\":\"graphql\""), "got {}", json);
        let back: OperationKey = serde_json::from_str(&json).unwrap();
        assert_eq!(back, key);
    }

    #[test]
    fn http_and_graphql_keys_never_collide() {
        let http = OperationKey::http("QUERY", "user");
        let gql = OperationKey::graphql(GraphqlOperationKind::Query, "user");
        assert_ne!(http, gql);
        assert_ne!(http.canonical(), gql.canonical());
    }
}
