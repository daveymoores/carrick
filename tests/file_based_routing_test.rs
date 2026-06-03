//! End-to-end coverage for file-based routing against *real on-disk fixtures*.
//!
//! The unit tests in `src/file_based_router.rs` and `src/agents/file_orchestrator.rs`
//! feed synthetic string paths into the deriver. These tests instead walk the
//! actual fixture trees under `tests/fixtures/{nextjs-app,astro}` and run them
//! through the same deterministic synthesis the orchestrator uses in production
//! (`FileOrchestrator::file_based_endpoints` over `builtin_conventions`), so a
//! regression in route derivation, the SWC handler extractor, or the framework
//! gate is caught with files that look like a user's repository.

use carrick::agents::file_orchestrator::FileOrchestrator;
use carrick::file_based_router::{RoutingConvention, builtin_conventions};
use carrick::swc_scanner::SwcScanner;
use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

fn fixture_root(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name)
}

/// Recursively collect every file under `dir`.
fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
    let entries =
        fs::read_dir(dir).unwrap_or_else(|e| panic!("failed to read {}: {}", dir.display(), e));
    for entry in entries {
        let path = entry.expect("dir entry").path();
        if path.is_dir() {
            walk(&path, out);
        } else {
            out.push(path);
        }
    }
}

/// Derive every `METHOD path` pair the file-based pass would synthesize for a
/// fixture, exactly as the orchestrator does: relativize against the repo root,
/// run the SWC gatekeeper's handler extractor, gate on `builtin_conventions`.
fn synthesized_routes(fixture: &str, conventions: &[RoutingConvention]) -> BTreeSet<String> {
    let root = fixture_root(fixture);
    let scanner = SwcScanner::new();
    let mut files = Vec::new();
    walk(&root, &mut files);

    let mut routes = BTreeSet::new();
    for file in &files {
        let rel = file.strip_prefix(&root).expect("file under fixture root");
        let content = fs::read_to_string(file).expect("read fixture file");
        let endpoints =
            FileOrchestrator::file_based_endpoints(&scanner, rel, file, &content, conventions);
        for ep in endpoints {
            // Every synthesized file-based endpoint must carry the metadata the
            // downstream sidecar type-resolution relies on: a convention label,
            // the file-based owner marker, and a declaration span. Asserting it
            // here means a regression that produced the right method+path but
            // dropped this metadata is caught, not silently projected away.
            assert!(
                !ep.pattern_matched.is_empty(),
                "{rel:?}: endpoint missing convention label"
            );
            assert_eq!(
                ep.owner_node, "__file_based_route__",
                "{rel:?}: endpoint not tagged as file-based"
            );
            assert!(
                ep.call_expression_span_start.is_some(),
                "{rel:?}: endpoint missing handler declaration span"
            );
            routes.insert(format!("{} {}", ep.method, ep.path));
        }
    }
    routes
}

#[test]
fn nextjs_app_router_fixture_derives_expected_routes() {
    let routes = synthesized_routes("nextjs-app", &builtin_conventions(&["Next.js".to_string()]));

    let expected: BTreeSet<String> = [
        "GET /users",
        "POST /users",
        "GET /users/:id",
        "DELETE /users/:id",
        "GET /health",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect();

    assert_eq!(
        routes, expected,
        "app-router fixture should yield exactly the route handlers, \
         and skip page.tsx / non-HTTP exports like `runtime`"
    );
}

#[test]
fn astro_fixture_derives_expected_routes() {
    let routes = synthesized_routes("astro", &builtin_conventions(&["Astro".to_string()]));

    let expected: BTreeSet<String> = [
        "GET /api/users",
        "POST /api/users",
        "GET /posts/:id",
        "GET /health",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect();

    assert_eq!(
        routes, expected,
        "astro fixture should yield endpoints from .ts/.js files only — \
         skipping index.astro, _helpers.ts, and the `prerender` export"
    );
}

#[test]
fn file_based_pass_is_noop_without_matching_framework() {
    // No convention-bearing framework detected → empty conventions → no routes,
    // regardless of what's on disk.
    let routes = synthesized_routes("astro", &builtin_conventions(&["express".to_string()]));
    assert!(
        routes.is_empty(),
        "no endpoints expected when no file-based framework is detected, got {routes:?}"
    );
}

#[test]
fn astro_convention_rejects_a_non_astro_layout() {
    // Stronger than the empty-conventions gate: run a *non-empty* Astro
    // convention set over the Next.js app-router tree (which has no `src/pages`).
    // The matcher must reject every file via strip_root/raw_segments and yield
    // nothing — proving the convention is correctly scoped, not just that an
    // empty slice produces nothing.
    let routes = synthesized_routes("nextjs-app", &builtin_conventions(&["Astro".to_string()]));
    assert!(
        routes.is_empty(),
        "astro conventions must not match app-router files, got {routes:?}"
    );
}
