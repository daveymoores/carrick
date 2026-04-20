//! Framework-coverage regression tests.
//!
//! These tests exercise the deterministic first stage of the pipeline — the
//! SWC candidate scanner — against each orphan fixture named in
//! `.thoughts/framework-coverage.md` §10.5. They assert the shape of the
//! candidates the scanner produces (method calls, fetch calls, decorator
//! calls). They do NOT exercise the LLM-dependent path; that's tracked in
//! the §10.3 harness note and runs behind `CARRICK_API_KEY`.
//!
//! Acceptance for §7 Step 2 is "a Rust test exercises the pipeline against
//! `tests/fixtures/fastify-api/` and `tests/fixtures/koa-api/` and asserts
//! endpoint counts and types." The candidate-count layer below is the MVP
//! part of that acceptance. Full end-to-end (types included) lives with
//! Step 2's CI harness; see the comment near the bottom of this file.

use carrick::swc_scanner::{CandidateTarget, ScanResult, SwcScanner};
use std::fs;
use std::path::{Path, PathBuf};

fn fixture_path(subpath: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(subpath)
}

fn scan(path: &Path) -> ScanResult {
    let content = fs::read_to_string(path)
        .unwrap_or_else(|e| panic!("failed to read {}: {}", path.display(), e));
    SwcScanner::new().scan_content(path, &content)
}

fn methods(candidates: &[CandidateTarget]) -> Vec<String> {
    candidates
        .iter()
        .filter_map(|c| c.callee_property.clone())
        .collect()
}

fn objects(candidates: &[CandidateTarget]) -> Vec<String> {
    candidates.iter().map(|c| c.callee_object.clone()).collect()
}

// ---------------------------------------------------------------------------
// Koa — orphan fixture now exercised
// ---------------------------------------------------------------------------

#[test]
fn koa_api_fixture_produces_expected_candidates() {
    let file = fixture_path("koa-api/server.ts");
    let result = scan(&file);

    assert!(
        result.should_analyze,
        "koa-api/server.ts should produce at least one candidate"
    );

    let methods = methods(&result.candidates);
    // Three endpoint methods on the router plus one on the prefixed apiRouter.
    assert!(methods.contains(&"get".to_string()), "expected router.get");
    assert!(
        methods.contains(&"post".to_string()),
        "expected router.post"
    );
    assert!(
        methods.contains(&"use".to_string()),
        "expected app.use mount"
    );

    let has_fetch = result
        .candidates
        .iter()
        .any(|c| c.callee_object == "fetch" && c.callee_property.is_none());
    assert!(has_fetch, "expected global fetch() call to be captured");
}

// ---------------------------------------------------------------------------
// Fastify — orphan fixture now exercised
// ---------------------------------------------------------------------------

#[test]
fn fastify_api_fixture_produces_expected_candidates() {
    let file = fixture_path("fastify-api/server.ts");
    let result = scan(&file);

    assert!(
        result.should_analyze,
        "fastify-api/server.ts should produce at least one candidate"
    );

    let methods = methods(&result.candidates);
    assert!(methods.contains(&"get".to_string()));
    assert!(methods.contains(&"post".to_string()));
    assert!(
        methods.contains(&"register".to_string()),
        "expected app.register() plugin mount"
    );
}

// ---------------------------------------------------------------------------
// Hapi — fixture added as part of §7 Step 1
// ---------------------------------------------------------------------------

#[test]
fn hapi_api_fixture_produces_expected_candidates() {
    let file = fixture_path("hapi-api/server.ts");
    let result = scan(&file);

    assert!(
        result.should_analyze,
        "hapi-api/server.ts should produce at least one candidate"
    );

    // Hapi uses `server.route({...})` — the object literal is the first arg;
    // extraction of method/path happens inside the LLM. What we verify here
    // is that the scanner emits candidates for each route() call plus the
    // register() mount and the downstream fetch().
    let methods = methods(&result.candidates);
    let route_count = methods.iter().filter(|m| *m == "route").count();
    assert!(
        route_count >= 3,
        "expected >=3 server.route(...) candidates, got {}: {:?}",
        route_count,
        methods
    );
    assert!(
        methods.contains(&"register".to_string()),
        "expected server.register() plugin mount"
    );
    assert!(
        result
            .candidates
            .iter()
            .any(|c| c.callee_object == "fetch" && c.callee_property.is_none()),
        "expected global fetch() call"
    );
}

// ---------------------------------------------------------------------------
// NestJS — verifies Move 2 (§9) shipped: decorators emit candidates
// ---------------------------------------------------------------------------

#[test]
fn nestjs_controller_fixture_emits_decorator_candidates() {
    let file = fixture_path("nestjs-api/users.controller.ts");
    let result = scan(&file);

    assert!(
        result.should_analyze,
        "nestjs-api controller should produce candidates after Move 2"
    );

    // The fixture has @Controller + @Get + @Get(':id') + @Post + @Param + @Body.
    // We require at minimum: the Controller class decorator and the three
    // routing-method decorators. The Param/Body parameter decorators may or
    // may not surface depending on the parser's traversal order; don't gate on
    // those to keep the assertion tight on what Step 3 actually promises.
    let objects = objects(&result.candidates);
    assert!(
        objects.contains(&"Controller".to_string()),
        "expected @Controller() decorator candidate"
    );
    assert!(
        objects.contains(&"Get".to_string()),
        "expected @Get() decorator candidate"
    );
    assert!(
        objects.contains(&"Post".to_string()),
        "expected @Post() decorator candidate"
    );

    // At least 4 decorator candidates (Controller + Get + Get(':id') + Post).
    let decorator_count = result
        .candidates
        .iter()
        .filter(|c| {
            matches!(
                c.callee_object.as_str(),
                "Controller" | "Get" | "Post" | "Put" | "Patch" | "Delete"
            )
        })
        .count();
    assert!(
        decorator_count >= 4,
        "expected >=4 routing-decorator candidates, got {}",
        decorator_count
    );
}

// ---------------------------------------------------------------------------
// End-to-end (LLM-dependent) acceptance
// ---------------------------------------------------------------------------
//
// The original §7 Step 2 acceptance bullet — "asserts endpoint counts AND types"
// — requires the full pipeline, which means `CARRICK_API_KEY` and a live
// Gemini call per file. Per §10.3 and §10.4 that belongs in a CI harness
// running the published binary with response caching, not in `cargo test`.
// When that harness lands, it should:
//   1. Run `carrick scan .` in each `tests/fixtures/{framework}-api/` dir.
//   2. Diff against a per-fixture `expected.json` (predicates, not literals).
//   3. Fail on drift.
// Until then, the deterministic candidate assertions above are the regression
// net: they catch any regression in the scanner that would remove coverage
// from an LLM we can no longer verify in-sandbox.
