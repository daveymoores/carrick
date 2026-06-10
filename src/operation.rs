//! Protocol-tagged operation identity.
//!
//! Every producer (endpoint) and consumer (outbound call) is keyed by an
//! [`OperationKey`]. The protocol tag travels with the key so operations from
//! different protocols can never collide in matching, type-manifest aliases,
//! or the cloud index. HTTP is the only protocol today; a new protocol adds a
//! variant plus its own matcher, and HTTP-only code paths skip it via
//! [`OperationKey::as_http`].

use serde::{Deserialize, Serialize};
use std::fmt;

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "protocol", rename_all = "snake_case")]
pub enum OperationKey {
    /// An HTTP operation. For producers, `path` is the mount-resolved route
    /// (e.g. `/api/users/:id`). For consumers, `path` carries the raw call
    /// target (full URL, env-var template, unresolved expression) until URL
    /// normalization runs during matching.
    Http { method: String, path: String },
}

impl OperationKey {
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

    /// `(method, path)` when this is an HTTP operation. HTTP-only code paths
    /// (mount-graph matching, REST manifest building, alias generation)
    /// filter through this — it is the protocol dispatch point.
    pub fn as_http(&self) -> Option<(&str, &str)> {
        match self {
            OperationKey::Http { method, path } => Some((method, path)),
        }
    }

    /// Stable identity string for hashing and dedup keys.
    pub fn canonical(&self) -> String {
        match self {
            OperationKey::Http { method, path } => format!("http|{}|{}", method, path),
        }
    }
}

impl fmt::Display for OperationKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            OperationKey::Http { method, path } => write!(f, "{} {}", method, path),
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
}
