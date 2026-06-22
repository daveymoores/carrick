//! Machine-readable scanner output for the eval harness.
//!
//! When `CARRICK_OUTPUT_JSON` is set, the engine emits an [`EvalProjection`]
//! instead of the human-readable Markdown report. This is a *dedicated* shape,
//! deliberately decoupled from the internal analyzer types, so the eval scoring
//! contract stays stable across refactors of `ApiEndpointDetails` and friends.
//! Slice 1 of the evals plan scores endpoint/call set accuracy from this.

use serde::{Deserialize, Serialize};
use std::path::Path;

use crate::analyzer::{ApiAnalysisResult, ApiEndpointDetails};
use crate::operation::OperationKey;

/// The full eval projection of a single scan: the producer endpoints and the
/// consumer calls the scanner extracted.
#[derive(Debug, Serialize, Deserialize)]
pub struct EvalProjection {
    pub endpoints: Vec<EvalOp>,
    pub calls: Vec<EvalOp>,
}

/// One extracted operation (endpoint or call), flattened for scoring.
#[derive(Debug, Serialize, Deserialize)]
pub struct EvalOp {
    /// Stable identity from `OperationKey::canonical()`, e.g. `http|GET|/users`.
    pub key: String,
    /// `"http" | "graphql" | "socket"`.
    pub protocol: String,
    /// HTTP method, when this is an HTTP operation.
    pub method: Option<String>,
    /// HTTP path, GraphQL field, or socket event.
    pub path: Option<String>,
    pub handler: Option<String>,
    pub request_type: Option<String>,
    pub response_type: Option<String>,
    pub file: String,
    pub line: u32,
}

impl EvalProjection {
    pub fn from_results(result: &ApiAnalysisResult) -> Self {
        Self {
            endpoints: result.endpoints.iter().map(EvalOp::from_details).collect(),
            calls: result.calls.iter().map(EvalOp::from_details).collect(),
        }
    }
}

impl EvalOp {
    fn from_details(d: &ApiEndpointDetails) -> Self {
        let (protocol, method, path) = project_key(&d.key);
        let (file, line) = split_location(&d.file_path);
        EvalOp {
            key: d.key.canonical(),
            protocol,
            method,
            path,
            handler: d.handler_name.clone(),
            request_type: d
                .request_type
                .as_ref()
                .map(|t| t.composite_type_string.clone()),
            response_type: d
                .response_type
                .as_ref()
                .map(|t| t.composite_type_string.clone()),
            file,
            line,
        }
    }
}

/// `(protocol, method, path)` projected from the operation key. For non-HTTP
/// protocols `method` is `None` and `path` carries the field / event name.
fn project_key(key: &OperationKey) -> (String, Option<String>, Option<String>) {
    match key {
        OperationKey::Http { method, path } => {
            ("http".to_string(), Some(method.clone()), Some(path.clone()))
        }
        OperationKey::Graphql { field, .. } => ("graphql".to_string(), None, Some(field.clone())),
        OperationKey::Socket { event, .. } => ("socket".to_string(), None, Some(event.clone())),
    }
}

/// `file_path` is stored as `"<file>:<line>"` for deterministic output. Split it
/// back into a path and a best-effort line number (0 when absent/unparseable).
/// `file`/`line` are informational only — scoring keys off method + path.
fn split_location(p: &Path) -> (String, u32) {
    let s = p.to_string_lossy();
    match s.rsplit_once(':') {
        Some((file, line)) if !line.is_empty() && line.bytes().all(|b| b.is_ascii_digit()) => {
            (file.to_string(), line.parse().unwrap_or(0))
        }
        _ => (s.into_owned(), 0),
    }
}
