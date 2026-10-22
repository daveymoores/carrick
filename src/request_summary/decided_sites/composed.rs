//! The full path a route site is served at, per instance end (carrick#2474
//! R2b). Record-only, with the receiver trace on.
//!
//! A site's instance ends ([`super::instances`]) say what its labelled
//! call is made on: a maker's result, a scope a labelled member opens, or a
//! hole. A maker's result is served wherever a member labelled with
//! `mounts` is handed it ([`mount_edges`], read once per service), under
//! that member's prefix, below that member's own receiver; one handed to
//! no mount is served at the top. The composed path joins every prefix
//! from the top down with the path the route's own call writes. Every
//! stated value is recorded, one path each (#2474 Q2). A walk that ends at
//! a hole still composes the prefixes below it, and says the path is
//! relative to that hole (`end`); a prefix that is not text composes no
//! path.

use std::path::Path;

use serde::Serialize;

use super::super::RequestSummaryInputs;
use super::instances::{
    InstanceAt, InstanceEnd, InstanceWalk, MakerLabel, Prefix, ScopeBinds, ScopeLabel, prefix_of,
};
use super::{Calls, Placed, relative};

/// One path a route site is served at.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize)]
pub struct Composed {
    /// The path: every prefix from where the walk up ends down, joined
    /// with the route's own; `None` where a prefix or the route's own path
    /// is not text. Full only where `end` is `top`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    /// Each scope or mount edge, from the top down: `file:line member
    /// prefix`.
    pub edges: Vec<String>,
    /// Where the walk up ends: `top` (an instance served at no mount), a
    /// hole's reason (the path is relative to what serves the instance
    /// there), or `cycle`.
    pub end: String,
}

/// An instance served under a prefix: a call of a member labelled with
/// `mounts`, handed a labelled maker's result.
#[derive(Debug, Clone)]
pub(super) struct MountEdge {
    /// The maker call of the instance handed in (`file`, `line`).
    child: (String, u32),
    /// The mount call, `file:line`.
    at: String,
    member: String,
    prefix: Prefix,
    /// The ends of the mount call's own receiver the label applies to.
    parents: Vec<InstanceEnd>,
}

/// Every mount edge of the service: each call of a member a `mounts` label
/// names, made on an instance the label applies to, with each maker's
/// result it is handed.
pub(super) fn mount_edges<'a>(
    calls: &Calls<'a>,
    inputs: &RequestSummaryInputs,
    root: &Path,
    makers: &[MakerLabel],
    scopes: &[ScopeLabel],
) -> Vec<MountEdge> {
    let mounts: Vec<&ScopeLabel> = scopes
        .iter()
        .filter(|label| matches!(label.binds, ScopeBinds::Mounts { .. }))
        .collect();
    if mounts.is_empty() {
        return Vec::new();
    }
    let mut placed: Vec<Placed<'a>> = calls
        .by_span
        .values()
        .filter(|placed| {
            mounts
                .iter()
                .any(|label| label.member == placed.call.callee)
        })
        .copied()
        .collect();
    placed.sort_by(|a, b| {
        (a.file, a.call.site.span_start, a.call.site.span_end).cmp(&(
            b.file,
            b.call.site.span_start,
            b.call.site.span_end,
        ))
    });
    let walk = || InstanceWalk::new(calls, inputs, root, makers, scopes);
    let mut edges = Vec::new();
    for placed in placed {
        if !walk().library_call(placed.file, placed.call) {
            continue;
        }
        let receivers = walk().of(&[placed]);
        for label in mounts
            .iter()
            .filter(|label| label.member == placed.call.callee)
        {
            let ScopeBinds::Mounts { arg, key } = &label.binds else {
                continue;
            };
            let parents: Vec<InstanceEnd> = receivers
                .iter()
                .filter(|end| label.applies_to(end))
                .cloned()
                .collect();
            if parents.is_empty() {
                continue;
            }
            for child in walk().of_arg(placed, *arg, key.as_deref()) {
                let InstanceAt::Root { file, line, .. } = &child.end else {
                    continue;
                };
                edges.push(MountEdge {
                    child: (file.clone(), *line),
                    at: format!("{}:{}", relative(root, placed.file), placed.call.site.line),
                    member: label.member.clone(),
                    prefix: prefix_of(&placed, &label.name),
                    parents: parents.clone(),
                });
            }
        }
    }
    edges
}

/// One walk from an instance end up to the top: the prefixes and edges
/// from the top down, and where it ends.
#[derive(Clone)]
struct Chain {
    prefixes: Vec<Prefix>,
    edges: Vec<String>,
    end: String,
}

fn edge_text(at: &str, member: &str, prefix: &Prefix) -> String {
    let prefix = match prefix {
        Prefix::Unstated => "-".to_string(),
        Prefix::Stated { values } => values.join("|"),
        Prefix::Unread { text } => format!("?{}", text.as_deref().unwrap_or_default()),
    };
    format!("{at} {member} {prefix}")
}

fn chains(end: &InstanceEnd, mounts: &[MountEdge], seen: &mut Vec<(String, u32)>) -> Vec<Chain> {
    match &end.end {
        InstanceAt::Hole { reason, .. } => vec![Chain {
            prefixes: Vec::new(),
            edges: Vec::new(),
            end: serde_json::to_value(reason)
                .ok()
                .and_then(|reason| reason.as_str().map(str::to_string))
                .unwrap_or_default(),
        }],
        InstanceAt::Root { file, line, .. } => {
            let here = (file.clone(), *line);
            if seen.contains(&here) {
                return vec![Chain {
                    prefixes: Vec::new(),
                    edges: Vec::new(),
                    end: "cycle".to_string(),
                }];
            }
            let served: Vec<&MountEdge> = mounts.iter().filter(|edge| edge.child == here).collect();
            if served.is_empty() {
                return vec![Chain {
                    prefixes: Vec::new(),
                    edges: Vec::new(),
                    end: "top".to_string(),
                }];
            }
            seen.push(here);
            let mut out = Vec::new();
            for edge in served {
                for parent in &edge.parents {
                    for mut chain in chains(parent, mounts, seen) {
                        chain.prefixes.push(edge.prefix.clone());
                        chain
                            .edges
                            .push(edge_text(&edge.at, &edge.member, &edge.prefix));
                        out.push(chain);
                    }
                }
            }
            seen.pop();
            out
        }
        InstanceAt::Scope {
            file,
            line,
            member,
            prefix,
            parents,
            ..
        } => {
            let at = format!("{file}:{line}");
            let mut out = Vec::new();
            for parent in parents {
                for mut chain in chains(parent, mounts, seen) {
                    chain.prefixes.push(prefix.clone());
                    chain.edges.push(edge_text(&at, member, prefix));
                    out.push(chain);
                }
            }
            out
        }
    }
}

/// `parts` joined as one path, each prefix before the next
/// ([`crate::utils::join_prefix_and_path`]).
fn join_path(parts: &[&str]) -> String {
    parts.iter().fold(String::new(), |path, part| {
        crate::utils::join_prefix_and_path(&path, part)
    })
}

/// The paths a route site whose call writes `route`, made on `ends`, is
/// served at: one per chain up each end, and per stated prefix value.
pub(super) fn compose(
    ends: &[InstanceEnd],
    route: Option<&str>,
    mounts: &[MountEdge],
) -> Vec<Composed> {
    let mut out: Vec<Composed> = Vec::new();
    for end in ends {
        for chain in chains(end, mounts, &mut Vec::new()) {
            // Every stated value, one path each.
            let mut paths: Vec<Option<Vec<String>>> = vec![Some(Vec::new())];
            let mut unread = false;
            for prefix in &chain.prefixes {
                match prefix {
                    Prefix::Unstated => {}
                    Prefix::Stated { values } => {
                        paths = paths
                            .into_iter()
                            .flat_map(|path| {
                                values.iter().map(move |value| {
                                    path.clone().map(|mut parts| {
                                        parts.push(value.clone());
                                        parts
                                    })
                                })
                            })
                            .collect();
                    }
                    Prefix::Unread { .. } => unread = true,
                }
            }
            for parts in paths {
                let path = match (!unread, route, parts) {
                    (true, Some(route), Some(mut parts)) => {
                        parts.push(route.to_string());
                        let parts: Vec<&str> = parts.iter().map(String::as_str).collect();
                        Some(join_path(&parts))
                    }
                    _ => None,
                };
                let composed = Composed {
                    path,
                    edges: chain.edges.clone(),
                    end: chain.end.clone(),
                };
                if !out.contains(&composed) {
                    out.push(composed);
                }
            }
        }
    }
    out
}
