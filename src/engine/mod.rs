use crate::agent_service::AgentService;
use crate::agents::file_orchestrator::FileOrchestrator;
use crate::agents::framework_guidance_agent::{FrameworkGuidanceAgent, ProtocolGuidance};
use crate::analyzer::{Analyzer, ApiEndpointDetails, builder::AnalyzerBuilder};
use crate::cloud_storage::{
    CloudRepoData, CloudStorage, ManifestRole, ManifestTypeKind, ManifestTypeState,
    TypeManifestEntry, get_current_commit_hash,
};
use crate::config::{Config, create_dynamic_tsconfig};
use crate::file_finder::find_service_files;
use crate::framework_detector::{DetectionResult, FrameworkDetector};
use crate::intent_generator::{generate_function_intents, intents_by_hash};
use crate::logging;
use crate::mount_graph::MountGraph;
use crate::multi_agent_orchestrator::MultiAgentOrchestrator;
use crate::operation::OperationKey;
use crate::packages::Packages;
use crate::parser::parse_file;
use crate::services::{
    TypeSidecar,
    type_sidecar::{InferKind, TypeResolutionResult},
};
use crate::signature_pass::populate_function_signatures;
use crate::type_manifest::{
    build_call_site_id, build_manifest_type_alias_with_call_id, is_http_method,
    normalize_manifest_method, parse_file_location,
};
use crate::url_normalizer::UrlNormalizer;
use crate::utils::get_repository_name;
use crate::visitor::{FunctionDefinition, FunctionDefinitionExtractor, ImportSymbolExtractor};
use sha2::{Digest, Sha256};
use std::collections::{HashMap, HashSet};
use std::env;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};
use tracing::{debug, info, warn};

use serde::Serialize;
use swc_common::{
    SourceMap,
    errors::{ColorConfig, Handler},
    sync::Lrc,
};
use swc_ecma_visit::VisitWith;

/// Current cache format version. Increment when FileAnalysisResult schema changes.
/// 4: EndpointResult gained `emission_style` — pre-4 cached results would
/// replay with `None` forever and never take the return-value/no-payload
/// inference paths for unchanged files.
const CACHE_VERSION: u32 = 4;

// Type aliases to reduce complexity
type FileDiscoveryResult = Result<
    (
        Vec<PathBuf>,
        HashMap<String, crate::visitor::ImportedSymbol>,
        HashMap<String, FunctionDefinition>,
        String,
    ),
    Box<dyn std::error::Error>,
>;

/// Determine if we should upload data based on GitHub context
/// Only upload on main/master branch, not on PRs
fn should_upload_data() -> bool {
    // Eval runs (`CARRICK_OUTPUT_JSON`) are read-only benchmarks against throwaway
    // fixtures. Never upload, or a dispatch on main would pollute the real cloud
    // index with fixture "services". This is the upstream half of eval mode's
    // no-side-effects guarantee; the JSON output branch skips the markdown
    // report + PR comment downstream.
    if env::var("CARRICK_OUTPUT_JSON").is_ok() {
        return false;
    }

    // LocalDirStorage (the offline cross-repo eval harness, Phase A) writes
    // CloudRepoData to a local cache dir, never the real cloud — so the
    // PR/branch anti-pollution guards below do not apply. Without this, a CI
    // run (GITHUB_EVENT_NAME=pull_request) skips the upload and Phase A
    // persists nothing. Phase B sets CARRICK_OUTPUT_JSON and returns above.
    if env::var("CARRICK_LOCAL_STORAGE_DIR").is_ok() {
        return true;
    }

    // Check if we're in a pull request
    if let Ok(event_name) = env::var("GITHUB_EVENT_NAME")
        && event_name == "pull_request"
    {
        return false;
    }

    // Check if we're on a feature branch (not main/master)
    if let Ok(ref_name) = env::var("GITHUB_REF") {
        // GITHUB_REF format: refs/heads/branch-name or refs/pull/123/merge
        if ref_name.starts_with("refs/pull/") {
            return false;
        }

        if let Some(branch) = ref_name.strip_prefix("refs/heads/") {
            // Only upload for main/master branches
            return branch == "main" || branch == "master";
        }
    }

    // If we can't determine the context, default to upload (for local testing)
    // You might want to change this to false for stricter behavior
    true
}

/// The PR number for a `pull_request` run, or None on push/dispatch/local
/// runs. GitHub sets GITHUB_REF to `refs/pull/<n>/merge` (or `/head`) on PRs;
/// returning None on any other ref is exactly the "only post on PR runs" gate.
fn pr_number_from_env() -> Option<u64> {
    let ref_name = env::var("GITHUB_REF").ok()?;
    let rest = ref_name.strip_prefix("refs/pull/")?;
    rest.split('/').next()?.parse::<u64>().ok()
}

/// This run's GitHub Actions run id (`GITHUB_RUN_ID`), or empty if unset. The
/// cloud records it against the PR so a later sibling main change can re-run
/// this exact workflow run and refresh the comment.
fn run_id_from_env() -> String {
    env::var("GITHUB_RUN_ID").unwrap_or_default()
}

#[allow(dead_code)]
pub async fn run_analysis_engine<T: CloudStorage>(
    storage: T,
    repo_path: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    run_analysis_engine_with_sidecar(storage, repo_path, None, false, None).await
}

/// Run analysis engine with optional sidecar for type extraction.
///
/// Always attempts log upload before returning, including on the error path —
/// the failing runs are exactly the ones whose logs we need. The inner
/// pipeline lives in `run_analysis_engine_inner` so `?`-propagated errors
/// don't bypass the upload.
pub async fn run_analysis_engine_with_sidecar<T: CloudStorage>(
    storage: T,
    repo_path: &str,
    sidecar: Option<&TypeSidecar>,
    no_cache: bool,
    ts_check_dir: Option<&std::path::Path>,
) -> Result<(), Box<dyn std::error::Error>> {
    let result =
        run_analysis_engine_inner(&storage, repo_path, sidecar, no_cache, ts_check_dir).await;
    upload_run_logs(&storage, repo_path).await;
    result
}

async fn run_analysis_engine_inner<T: CloudStorage>(
    storage: &T,
    repo_path: &str,
    sidecar: Option<&TypeSidecar>,
    no_cache: bool,
    ts_check_dir: Option<&std::path::Path>,
) -> Result<(), Box<dyn std::error::Error>> {
    let should_upload = should_upload_data();
    debug!(upload = should_upload, "Running Carrick in CI mode");

    // 1. Health check
    let sp = logging::spinner("Connecting to Carrick Cloud...");
    storage
        .health_check()
        .await
        .map_err(|e| format!("Failed to connect to Carrick Cloud: {}", e))?;
    logging::finish_spinner(&sp, "Connected to Carrick Cloud");

    // 2. Download all repos (moved earlier for incremental cache lookup)
    let sp = logging::spinner("Downloading cross-repo data...");
    let (mut all_repo_data, _repo_s3_urls) = storage
        .download_all_repo_data()
        .await
        .map_err(|e| format!("Failed to download cross-repo data: {}", e))?;

    // 3. Resolve the services declared for this repo (one for the common
    //    single-service case; one per directory for a monorepo carrick.json).
    let repo_name = get_repository_name(repo_path);
    let services = resolve_services(repo_path)?;
    let multi_service = services.len() > 1;
    if multi_service {
        info!(
            "carrick.json declares {} services in {}",
            services.len(),
            repo_name
        );
    }
    logging::finish_spinner(
        &sp,
        &format!("Downloaded data from {} repos", all_repo_data.len()),
    );

    // 4. Analyze each service (incremental per service where possible).
    let sp = logging::spinner("Analyzing repository...");
    let mut current_services_data = Vec::with_capacity(services.len());
    for service in &services {
        let packages = load_packages_for_service(repo_path, service)?;

        // Scope the sidecar's type extraction to this service's directory/tsconfig.
        scope_sidecar_to_service(sidecar, repo_path, service);

        // Incremental cache is per service: match on repo + service name so
        // editing one service does not invalidate the others.
        let previous_data = if no_cache {
            None
        } else {
            all_repo_data
                .iter()
                .find(|r| r.repo_name == repo_name && r.service_name == service.service_name)
                .cloned()
        };

        let data = analyze_current_repo_incremental(
            repo_path,
            service,
            &packages,
            sidecar,
            previous_data.as_ref(),
        )
        .await?;

        if data.bundled_types.is_some() {
            debug!(
                "Type resolution ({}): {} bundled types, {} manifest entries",
                data.service_name.as_deref().unwrap_or(&repo_name),
                data.bundled_types
                    .as_ref()
                    .map(|s| s.lines().count())
                    .unwrap_or(0),
                data.type_manifest.as_ref().map(|v| v.len()).unwrap_or(0)
            );
        }

        current_services_data.push(data);
    }
    logging::finish_spinner(
        &sp,
        &format!("Analyzed {} ({} service(s))", repo_name, services.len()),
    );

    // If the LLM quota was exhausted at any point during analysis, the per-call
    // circuit breaker tripped and the remaining files/functions failed fast — so
    // the results above are partial. Abort before uploading (or producing any
    // cross-repo/PR output) so a quota-degraded scan can't overwrite the existing
    // index with a half-empty one. (Run-log upload still happens in the caller.)
    if crate::agent_service::rate_limit_tripped() {
        return Err(
            "Carrick Cloud LLM quota was exhausted mid-scan; the analysis is \
                    incomplete, so aborting before upload to avoid overwriting the existing \
                    index with partial results. Re-run after the quota resets."
                .into(),
        );
    }

    // 5. Conditionally upload each service's data to cloud storage.
    //    The production index keys on (workspace, project, repo) only, so it
    //    cannot yet hold more than one service per repo — a multi-service
    //    upload would clobber. Gate it on the backend advertising support;
    //    cross-repo analysis below still runs locally regardless.
    if should_upload {
        if multi_service && !storage.supports_multi_service() {
            warn!(
                "Skipping index upload: {} services declared but the cloud key has no \
                 service discriminator yet, so uploads would overwrite each other. \
                 Cross-repo analysis still runs locally.",
                services.len()
            );
        } else {
            let sp = logging::spinner("Uploading results...");
            // Prepare every payload before uploading any, so a serialization
            // problem can't surface halfway through a multi-service upload.
            let payloads: Vec<CloudRepoData> = current_services_data
                .iter()
                .map(|data| strip_ast_nodes(data.clone()))
                .collect();
            for (i, payload) in payloads.iter().enumerate() {
                if let Err(e) = storage.upload_repo_data(payload).await {
                    // Uploads are keyed per (repo, service) and idempotent, so
                    // a re-run restores consistency — but until then the index
                    // holds this run's data for some services and the previous
                    // run's for the rest. Make that state explicit.
                    let uploaded: Vec<&str> = payloads[..i]
                        .iter()
                        .map(|d| d.service_name.as_deref().unwrap_or(&d.repo_name))
                        .collect();
                    let not_uploaded: Vec<&str> = payloads[i..]
                        .iter()
                        .map(|d| d.service_name.as_deref().unwrap_or(&d.repo_name))
                        .collect();
                    return Err(format!(
                        "Failed to upload repo data: {}. Uploaded: [{}]; not uploaded: [{}]. \
                         The index is mixed-generation for this repo until a successful re-run.",
                        e,
                        uploaded.join(", "),
                        not_uploaded.join(", ")
                    )
                    .into());
                }
            }
            logging::finish_spinner(&sp, "Uploaded results to Carrick Cloud");
        }
    } else {
        debug!("Skipping upload (PR/branch mode)");
    }

    // On a PR run, capture this repo's previously-indexed endpoints (its last
    // uploaded state, i.e. main) before they're removed below, so the diff can
    // surface what this change added relative to that baseline. Keyed by
    // (service, key) so that in a
    // monorepo an endpoint newly added to one service still counts as new even
    // if a sibling service already exposes the same route. `had_prior_index` is
    // tracked separately so a prior scan that indexed zero endpoints still
    // counts as a baseline, rather than being conflated with a first-ever scan
    // where "new" is meaningless. On non-PR runs the capture is skipped
    // entirely, since the block is suppressed there anyway.
    type ServiceEndpointKey = (Option<String>, crate::operation::OperationKey);
    let is_pr_run = pr_number_from_env().is_some();
    let (had_prior_index, previous_self_keys): (
        bool,
        std::collections::HashSet<ServiceEndpointKey>,
    ) = if is_pr_run {
        let had = all_repo_data.iter().any(|repo| repo.repo_name == repo_name);
        let keys = all_repo_data
            .iter()
            .filter(|repo| repo.repo_name == repo_name)
            .flat_map(|repo| {
                repo.endpoints
                    .iter()
                    .map(|e| (repo.service_name.clone(), e.key.clone()))
            })
            .collect();
        (had, keys)
    } else {
        (false, std::collections::HashSet::new())
    };

    // 6. Cross-repo analysis (reuse already-downloaded data).
    // Remove this repo's downloaded copies so the freshly-analyzed services
    // are the ones used.
    all_repo_data.retain(|repo| repo.repo_name != repo_name);

    // Peer repos and local service count describe the project topology, which
    // the formatter uses to frame findings (single repo / monorepo / poly-repo)
    // and to decide whether connectivity findings are conclusive. Captured
    // before `all_repo_data` is moved into the analyzer below.
    let peer_repo_count = all_repo_data.len();
    let local_service_count = services.len();

    debug!(
        "Cross-repo analysis with {} other repos + {} local service(s)",
        peer_repo_count, local_service_count
    );

    // On a PR run with a prior index, surface what this change added: operations
    // in the freshly-analyzed services that the previous (last-uploaded) index
    // didn't have. Because the baseline is the last uploaded index, this can
    // include an operation that landed on main since its last scan rather than
    // in this PR. Computed before `current_services_data` is moved into the
    // analyzer.
    let pr_delta = if is_pr_run && had_prior_index {
        let mut new_endpoints = Vec::new();
        let mut seen = std::collections::HashSet::new();
        for service_data in &current_services_data {
            for endpoint in &service_data.endpoints {
                let id = (service_data.service_name.clone(), endpoint.key.clone());
                if !previous_self_keys.contains(&id) && seen.insert(id) {
                    let (label, name) = endpoint.key.display_labels();
                    new_endpoints.push(crate::formatter::NewEndpoint {
                        label,
                        name,
                        service: service_data.service_name.clone(),
                    });
                }
            }
        }
        // Sort by (label, name, service) for deterministic output even when two
        // services add the same operation.
        new_endpoints
            .sort_by(|a, b| (&a.label, &a.name, &a.service).cmp(&(&b.label, &b.name, &b.service)));
        Some(crate::formatter::PrDelta { new_endpoints })
    } else {
        None
    };

    // Collect the merged type manifest before `all_repo_data` /
    // `current_services_data` are moved into the analyzer. The eval projection
    // joins these to each op (keyed by OperationKey) for the type-resolution
    // metrics; nothing else reads it. Cheap clone, only materialised for the
    // eval path (it's just a flatten of each repo's already-built manifest).
    let eval_type_manifest: Vec<TypeManifestEntry> = if std::env::var("CARRICK_OUTPUT_JSON").is_ok()
    {
        all_repo_data
            .iter()
            .chain(current_services_data.iter())
            .filter_map(|repo| repo.type_manifest.as_ref())
            .flat_map(|entries| entries.iter().cloned())
            .collect()
    } else {
        Vec::new()
    };

    let sp = logging::spinner("Running cross-repo analysis...");
    let analyzer =
        build_cross_repo_analyzer(all_repo_data, current_services_data, ts_check_dir).await?;
    logging::finish_spinner(&sp, "Cross-repo analysis complete");

    let results = analyzer.get_results();

    // Eval harness output mode: emit a machine-readable projection of the
    // results and skip the human Markdown report + PR-comment relay. Consumed
    // by the offline scorer (Slice 1 of the evals plan). Deliberately terminal —
    // an eval run wants only the JSON, no upload or comment side effects.
    if std::env::var("CARRICK_OUTPUT_JSON").is_ok() {
        let projection =
            crate::eval_output::EvalProjection::from_results(&results, &eval_type_manifest);
        println!("{}", serde_json::to_string_pretty(&projection)?);
        return Ok(());
    }

    let topology = crate::formatter::Topology {
        repo_name: repo_name.clone(),
        local_service_count,
        peer_repo_count,
    };
    let formatted = crate::formatter::FormattedOutput::new(results, topology, pr_delta);
    formatted.print();

    // On pull_request runs we deliberately skip the index upload (see
    // should_upload_data — PR-branch data must not pollute the cross-repo
    // index), but we still relay the rendered findings to the cloud, which
    // posts (and updates in place on later pushes) a single PR comment via
    // the GitHub App, gated on the project's pr_comments_enabled toggle.
    // Best-effort: a comment failure is logged, never fatal.
    if let Some(pr_number) = pr_number_from_env() {
        let body = formatted.pr_comment_body();
        let run_id = run_id_from_env();
        if let Err(e) = storage
            .post_pr_comment(&repo_name, pr_number, &run_id, &body)
            .await
        {
            warn!("Failed to post PR comment: {}", e);
        }
    }

    Ok(())
}

/// Best-effort upload of the current run's log tail to S3.
///
/// Reads from the byte offset captured at `logging::init` time so the upload
/// only contains *this run's* output — not the day's accumulated tail, which
/// on a developer machine could include unrelated repos analyzed earlier.
/// Capped at 5 MB in case a single run is unusually verbose.
///
/// Runs on both success and failure paths — failing runs are exactly the
/// ones whose logs we need. Errors here are non-fatal: a failed upload is
/// logged at warn but never propagated.
async fn upload_run_logs<T: CloudStorage>(storage: &T, repo_path: &str) {
    const MAX_LOG_BYTES: u64 = 5 * 1024 * 1024;

    let Some(log_path) = logging::get_log_file_path() else {
        return;
    };
    let Ok(mut file) = std::fs::File::open(&log_path) else {
        return;
    };
    let Ok(metadata) = file.metadata() else {
        return;
    };

    use std::io::{Read, Seek};

    let file_len = metadata.len();
    // Start from this run's offset, but cap at 5 MB worth from the end so a
    // pathologically chatty run doesn't ship hundreds of megabytes.
    let run_start = logging::get_run_log_offset().unwrap_or(0);
    let cap_start = file_len.saturating_sub(MAX_LOG_BYTES);
    let start = run_start.max(cap_start);

    if file.seek(std::io::SeekFrom::Start(start)).is_err() {
        return;
    }

    let mut buf = Vec::with_capacity((file_len - start) as usize);
    if file.read_to_end(&mut buf).is_err() {
        return;
    }

    // Note: `from_utf8_lossy` replaces invalid sequences with U+FFFD (3 bytes
    // in UTF-8), so the resulting `log_content.len()` may exceed the original
    // raw byte count when the file contains non-UTF-8 noise. Close enough for
    // a logged size hint.
    let log_content = String::from_utf8_lossy(&buf);
    let repo_name = get_repository_name(repo_path);
    match storage.upload_logs(&repo_name, &log_content).await {
        Ok(()) => {
            debug!(
                bytes = log_content.len(),
                repo = %repo_name,
                "Uploaded run logs to S3"
            );
        }
        Err(e) => {
            warn!("Failed to upload logs: {}", e);
        }
    }
}

/// Serialize CloudRepoData without AST nodes in ApiEndpointDetails
/// Generic function to merge serialized data from repo configs
fn merge_serialized_data<T>(
    all_repo_data: &[CloudRepoData],
    extractor: fn(&CloudRepoData) -> Option<&String>,
) -> Result<T, Box<dyn std::error::Error>>
where
    T: Default + serde::de::DeserializeOwned,
{
    // Special handling for Config to properly merge all configs
    if std::any::type_name::<T>() == std::any::type_name::<crate::config::Config>() {
        let mut temp_files = Vec::new();

        // Write each config to a temporary file
        for (i, repo_data) in all_repo_data.iter().enumerate() {
            if let Some(json_str) = extractor(repo_data) {
                let temp_path = std::env::temp_dir().join(format!("carrick_config_{}.json", i));
                if std::fs::write(&temp_path, json_str).is_err() {
                    continue;
                }
                temp_files.push(temp_path);
            }
        }

        // Use Config::new to properly merge all configs
        if !temp_files.is_empty() {
            let merged_config = crate::config::Config::new(temp_files.clone()).unwrap_or_default();

            // Clean up temp files
            for temp_file in temp_files {
                let _ = std::fs::remove_file(temp_file);
            }

            // This is a bit of a hack to return the merged config as T
            // Since we know T is Config when we get here
            let config_any = Box::new(merged_config) as Box<dyn std::any::Any>;
            if let Ok(config) = config_any.downcast::<crate::config::Config>() {
                let config_json = serde_json::to_string(&*config)?;
                return Ok(serde_json::from_str(&config_json)?);
            }
        }

        return Ok(T::default());
    }

    // For non-Config types, use the first found (original behavior)
    for repo_data in all_repo_data {
        if let Some(json_str) = extractor(repo_data)
            && let Ok(data) = serde_json::from_str::<T>(json_str)
        {
            return Ok(data);
        }
    }
    Ok(T::default())
}

/// Remove AST nodes from CloudRepoData for serialization.
/// Also enforces payload size limit for Lambda (6MB) — drops file_results if too large.
fn strip_ast_nodes(mut data: CloudRepoData) -> CloudRepoData {
    fn strip_endpoint_ast(endpoint: &mut ApiEndpointDetails) {
        endpoint.request_type = None;
        endpoint.response_type = None;
    }

    data.endpoints.iter_mut().for_each(strip_endpoint_ast);
    data.calls.iter_mut().for_each(strip_endpoint_ast);

    // Payload size guard: Lambda function URLs have a 6MB request payload limit.
    // If serialized data exceeds ~5MB, drop file_results to stay under the limit.
    const MAX_PAYLOAD_BYTES: usize = 5 * 1024 * 1024; // 5MB safety margin
    const LAMBDA_HARD_LIMIT_BYTES: usize = 6 * 1024 * 1024;
    if let Ok(serialized) = serde_json::to_string(&data)
        && serialized.len() > MAX_PAYLOAD_BYTES
    {
        warn!(
            "Payload size {}KB exceeds {}KB limit, dropping file_results cache for this \
             upload — the next scan cannot run incrementally and will re-analyze every file",
            serialized.len() / 1024,
            MAX_PAYLOAD_BYTES / 1024
        );
        data.file_results = None;
        data.cached_detection = None;
        data.cached_guidance = None;

        // Re-check: if the payload is still over Lambda's hard request limit
        // even without the cache, say so up front — the upload will be
        // rejected with an otherwise cryptic 413.
        if let Ok(reserialized) = serde_json::to_string(&data)
            && reserialized.len() > LAMBDA_HARD_LIMIT_BYTES
        {
            warn!(
                "Payload is still {}KB after dropping caches, which exceeds the cloud's \
                 {}KB request limit — the upload will likely be rejected. Consider splitting \
                 the repo into services via carrick.json.",
                reserialized.len() / 1024,
                LAMBDA_HARD_LIMIT_BYTES / 1024
            );
        }
    }

    data
}

/// Get files changed between a base commit and HEAD.
/// Returns relative paths matching the file discovery format.
fn get_changed_files(repo_path: &str, base_commit: &str) -> Option<Vec<String>> {
    let output = std::process::Command::new("git")
        .args(["diff", "--name-only", base_commit, "HEAD"])
        .current_dir(repo_path)
        // Clear git env vars so git uses repo_path for repo discovery, not an
        // ambient GIT_DIR / GIT_WORK_TREE inherited from a parent process (e.g.
        // when invoked from a pre-commit hook inside a git worktree).
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .env_remove("GIT_INDEX_FILE")
        .output()
        .ok()?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        // Surface this at warn level with the cause: a shallow clone
        // (actions/checkout defaults to fetch-depth: 1) silently forces a
        // full re-analysis — including its full LLM cost — on every run.
        let is_shallow = std::process::Command::new("git")
            .args(["rev-parse", "--is-shallow-repository"])
            .current_dir(repo_path)
            .env_remove("GIT_DIR")
            .env_remove("GIT_WORK_TREE")
            .env_remove("GIT_INDEX_FILE")
            .output()
            .ok()
            .is_some_and(|o| String::from_utf8_lossy(&o.stdout).trim() == "true");
        if is_shallow {
            warn!(
                "Incremental mode unavailable: this is a shallow clone, so the previous \
                 scan's commit isn't reachable for diffing. Set `fetch-depth: 0` on \
                 actions/checkout to avoid re-analyzing every file on each run."
            );
        } else {
            warn!(
                "Incremental mode unavailable: git diff against {} failed ({}). \
                 Falling back to full analysis.",
                base_commit,
                stderr.trim()
            );
        }
        return None;
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let changed: Vec<String> = stdout
        .lines()
        .filter(|line| !line.is_empty())
        .filter(|line| {
            let ext = Path::new(line)
                .extension()
                .and_then(|e| e.to_str())
                .unwrap_or("");
            matches!(ext, "ts" | "tsx" | "js" | "jsx")
        })
        .map(|line| line.to_string())
        .collect();

    Some(changed)
}

/// Hash file content for cache invalidation (package.json).
fn hash_file_content(content: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(content.as_bytes());
    format!("{:x}", hasher.finalize())
}

/// Hash every discovered package.json (sorted, keyed by repo-relative path)
/// for the detection/guidance/extraction-config cache gate. The artifacts
/// behind the gate are generated from the MERGED dependency set, so the gate
/// must cover workspace manifests too — hashing only the root package.json
/// would let a dependency added in `packages/api/package.json` reuse a stale
/// extraction config (and stale detection) indefinitely.
fn hash_workspace_package_jsons(packages: &Packages, repo_path: &str) -> String {
    let repo_root = Path::new(repo_path);
    let mut keyed: Vec<(String, &PathBuf)> = packages
        .source_paths
        .iter()
        .map(|path| {
            let relative = path
                .strip_prefix(repo_root)
                .unwrap_or(path)
                .to_string_lossy()
                .to_string();
            (relative, path)
        })
        .collect();
    keyed.sort();

    let mut combined = String::new();
    for (relative, path) in keyed {
        combined.push_str(&relative);
        combined.push('\0');
        if let Ok(content) = std::fs::read_to_string(path) {
            combined.push_str(&content);
        }
        combined.push('\0');
    }
    hash_file_content(&combined)
}

/// Normalize file_results keys to be relative to repo root.
/// This ensures cache key consistency between runs.
fn normalize_file_results_keys(
    file_results: &HashMap<String, crate::agents::file_analyzer_agent::FileAnalysisResult>,
    repo_path: &str,
) -> HashMap<String, crate::agents::file_analyzer_agent::FileAnalysisResult> {
    let repo_prefix = if repo_path.ends_with('/') {
        repo_path.to_string()
    } else {
        format!("{}/", repo_path)
    };

    file_results
        .iter()
        .map(|(key, value)| {
            let normalized_key = key
                .strip_prefix(&repo_prefix)
                .or_else(|| key.strip_prefix("./"))
                .unwrap_or(key)
                .to_string();
            (normalized_key, value.clone())
        })
        .collect()
}

/// Strip diagnostic-only fields from file_results before caching.
/// These fields are not needed by build_mount_graph() or collect_type_requests().
///
/// `response_expression_text` is deliberately KEPT: it is the primary locator
/// for return-value endpoints (`EmissionStyle::ReturnValue`), whose only
/// fallback is an inexact registration-line anchor (the sidecar's
/// `findFunctionByLine` tolerates ±2 lines and can bind an adjacent handler).
/// Stripping it would make cached replays of unchanged files silently degrade
/// to that fallback. Every other stripped text field has an exact span
/// fallback, so replays lose nothing.
fn strip_diagnostic_fields(
    file_results: &mut HashMap<String, crate::agents::file_analyzer_agent::FileAnalysisResult>,
) {
    for result in file_results.values_mut() {
        for endpoint in &mut result.endpoints {
            endpoint.candidate_id = String::new();
            endpoint.pattern_matched = String::new();
            endpoint.payload_expression_text = None;
        }
        for data_call in &mut result.data_calls {
            data_call.candidate_id = String::new();
            data_call.pattern_matched = String::new();
            data_call.call_expression_text = None;
            data_call.payload_expression_text = None;
        }
        for mount in &mut result.mounts {
            mount.pattern_matched = String::new();
        }
    }
}

/// Incremental analysis: reuse cached per-file LLM results for unchanged files.
async fn analyze_current_repo_incremental(
    repo_path: &str,
    service: &Config,
    packages: &Packages,
    sidecar: Option<&TypeSidecar>,
    previous_data: Option<&CloudRepoData>,
) -> Result<CloudRepoData, Box<dyn std::error::Error>> {
    let start = Instant::now();

    // Canonicalize repo_path for consistent path normalization between runs
    let canonical = std::fs::canonicalize(repo_path)
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_else(|_| repo_path.to_string());
    let repo_path = canonical.as_str();

    let config = service;

    // Discover files and symbols (fast SWC pass, always full), scoped to the service
    let cm: Lrc<SourceMap> = Default::default();
    let (files, all_imported_symbols, function_definitions, repo_name) =
        discover_files_and_symbols(repo_path, config, cm.clone())?;

    // 3. Check if we can use incremental mode
    let can_use_incremental = previous_data.and_then(|prev| {
        // Must have file_results and matching cache_version
        let has_cache = prev.file_results.is_some();
        let version_matches = prev.cache_version == Some(CACHE_VERSION);
        if has_cache && version_matches {
            Some(prev)
        } else {
            if !has_cache {
                debug!("No cached file_results found, running full analysis");
            }
            if !version_matches {
                debug!(
                    "Cache version mismatch (expected {}, got {:?}), running full analysis",
                    CACHE_VERSION, prev.cache_version
                );
            }
            None
        }
    });

    if let Some(prev) = can_use_incremental {
        let prev_commit = &prev.commit_hash;
        debug!(
            "Found previous analysis (commit {})",
            &prev_commit[..std::cmp::min(7, prev_commit.len())]
        );

        // Get changed files via git diff
        if let Some(changed_files) = get_changed_files(repo_path, prev_commit) {
            let prev_file_results = prev.file_results.as_ref().unwrap();
            let repo_prefix = format!("{}/", repo_path);

            // Helper to normalize a file path to repo-relative
            let normalize_path = |f: &PathBuf| -> String {
                let s = f.to_string_lossy();
                if let Some(stripped) = s.strip_prefix(&repo_prefix) {
                    stripped.to_string()
                } else if let Some(stripped) = s.strip_prefix("./") {
                    stripped.to_string()
                } else {
                    s.to_string()
                }
            };

            // Build set of currently discovered file paths (normalized to relative)
            let current_file_set: HashSet<String> = files.iter().map(&normalize_path).collect();

            // Normalize changed files relative to repo root
            let changed_set: HashSet<String> = changed_files.into_iter().collect();

            // Partition: which files need fresh analysis?
            let files_to_analyze: Vec<PathBuf> = files
                .iter()
                .filter(|f| {
                    let relative = normalize_path(f);
                    changed_set.contains(&relative) || !prev_file_results.contains_key(&relative)
                })
                .cloned()
                .collect();

            let total_files = files.len();
            let changed_count = files_to_analyze.len();
            let reused_count = total_files - changed_count;

            debug!(
                "Detected {} changed file(s) out of {}, reusing {} cached",
                changed_count, total_files, reused_count
            );

            // Check if any package.json changed → need fresh framework
            // detection/guidance/extraction config. Covers workspace
            // manifests, not just the repo root (raw file content, not the
            // serialized struct, for deterministic comparison).
            let current_pkg_hash = hash_workspace_package_jsons(packages, repo_path);

            let pkg_changed = prev.package_json_hash.as_deref() != Some(&current_pkg_hash);

            // Get framework detection, guidance, and extraction config
            // (cached or fresh — all three share the package_json_hash gate)
            let (detection, guidance, extraction_config) = if !pkg_changed {
                if let (Some(det), Some(guid)) = (&prev.cached_detection, &prev.cached_guidance) {
                    debug!("Reusing cached framework detection and guidance");
                    // A missing cached config (older cache entry, or an earlier
                    // failed generation) is regenerated on its own.
                    let extraction = match &prev.cached_extraction_config {
                        Some(config) => Some(config.clone()),
                        None => {
                            let agent = FrameworkGuidanceAgent::new(AgentService::new());
                            generate_extraction_config(&agent, det, packages).await
                        }
                    };
                    (det.clone(), guid.clone(), extraction)
                } else {
                    run_framework_detection_and_guidance(packages, &all_imported_symbols).await?
                }
            } else {
                debug!("package.json changed, re-running framework detection");
                run_framework_detection_and_guidance(packages, &all_imported_symbols).await?
            };

            // Run Gemini file analysis ONLY on changed files
            if changed_count > 0 {
                debug!("Running LLM analysis on {} changed file(s)", changed_count);
            }

            let agent_service = AgentService::new();
            let file_orchestrator = FileOrchestrator::new(agent_service.clone());

            let new_file_results = if !files_to_analyze.is_empty() {
                let result = file_orchestrator
                    .analyze_files(
                        &files_to_analyze,
                        &guidance,
                        &detection,
                        Path::new(repo_path),
                    )
                    .await?;
                result.file_results
            } else {
                HashMap::new()
            };

            // Merge: start with previous, remove deleted files, update changed
            let mut merged_results: HashMap<
                String,
                crate::agents::file_analyzer_agent::FileAnalysisResult,
            > = HashMap::new();

            // Copy cached results for unchanged files that still exist
            for (path, result) in prev_file_results {
                if current_file_set.contains(path) && !changed_set.contains(path) {
                    merged_results.insert(path.clone(), result.clone());
                }
            }

            // Normalize and insert new results
            let normalized_new = normalize_file_results_keys(&new_file_results, repo_path);
            for (path, result) in normalized_new {
                merged_results.insert(path, result);
            }

            // Rebuild mount graph from full merged results
            let graph_orchestrator = FileOrchestrator::new(agent_service.clone());
            let mount_graph = graph_orchestrator.build_mount_graph(&merged_results);

            // Generate function intents (also strips body_source before upload).
            // Run on the same path as the full analysis so incremental scans
            // populate FunctionDefinition.intent in DDB (issue #110).
            //
            // Caching is content-addressed: a `content_hash -> intent` map from
            // the previous scan lets the generator reuse an intent whenever a
            // function's body and its callees' intents are unchanged — without
            // re-calling /generate-intent. Unlike the old name+file seeding,
            // this also refreshes a caller in an unchanged file when one of its
            // callees changed (the caller's hash includes its callee intents),
            // so cross-file staleness no longer slips through.
            let mut function_definitions = function_definitions;
            let prev_intents = intents_by_hash(&prev.function_definitions);
            generate_function_intents(
                &agent_service,
                &mut function_definitions,
                &all_imported_symbols,
                &prev_intents,
            )
            .await;

            // Compose function signatures, inferring unannotated slots via sidecar.
            populate_function_signatures(sidecar, &mut function_definitions, repo_path);

            let elapsed = start.elapsed();
            debug!(
                "Incremental analysis complete in {:.1}s",
                elapsed.as_secs_f64()
            );

            // Build CloudRepoData with merged results
            let mut cloud_data = build_cloud_data_from_mount_graph(
                &repo_name,
                repo_path,
                &mount_graph,
                config,
                packages,
                function_definitions,
            );
            let protocol_extractions = append_deterministic_protocol_operations(
                &mut cloud_data,
                repo_path,
                service,
                &files,
            );

            // Populate cache fields
            let mut cached_file_results = merged_results.clone();
            strip_diagnostic_fields(&mut cached_file_results);
            cloud_data.file_results = Some(cached_file_results);
            cloud_data.cached_detection = Some(detection.clone());
            cloud_data.cached_guidance = Some(guidance);
            cloud_data.cached_extraction_config = extraction_config.clone();
            cloud_data.package_json_hash = Some(current_pkg_hash);
            cloud_data.cache_version = Some(CACHE_VERSION);

            // Build type manifest
            let mut manifest_entries = build_type_manifest_entries(&mount_graph, config);
            stamp_manifest_anchor_symbols(&mut manifest_entries, &merged_results);
            append_protocol_manifest_entries(&mut manifest_entries, &protocol_extractions);
            if !manifest_entries.is_empty() {
                cloud_data.type_manifest = Some(manifest_entries);
            }

            // Socket payload anchors resolve through the same sidecar bundle
            // path as HTTP explicit symbols (#245); GraphQL anchors are deferred
            // to #248.
            let socket_requests = file_orchestrator
                .collect_socket_type_requests(&protocol_extractions.sockets, repo_path);

            // Type resolution via sidecar
            resolve_types_if_available(
                sidecar,
                &file_orchestrator,
                &merged_results,
                repo_path,
                extraction_config.as_ref(),
                &mount_graph,
                config,
                &socket_requests,
                &mut cloud_data,
            );

            if let Some(bundled_types) = cloud_data.bundled_types.take() {
                let updated =
                    append_missing_aliases(bundled_types, cloud_data.type_manifest.as_ref());
                cloud_data.bundled_types = Some(updated);
            }

            // Resolve per-endpoint definitions via compiler
            if let Some(sidecar) = sidecar {
                resolve_per_endpoint_definitions(sidecar, &mut cloud_data);
            }

            return Ok(cloud_data);
        } else {
            debug!("git diff failed, falling back to full analysis");
        }
    }

    // Fallback: full analysis (analyze_current_repo now populates cache fields)
    debug!("Running full analysis...");
    let cloud_data = analyze_current_repo(repo_path, config, packages, sidecar).await?;

    let elapsed = start.elapsed();
    debug!("Full analysis complete in {:.1}s", elapsed.as_secs_f64());

    Ok(cloud_data)
}

/// Run framework detection, per-protocol guidance generation, and
/// extraction-config generation (machinery-unwrap rules). All three are
/// cached together under the package_json_hash gate.
async fn run_framework_detection_and_guidance(
    packages: &Packages,
    imported_symbols: &HashMap<String, crate::visitor::ImportedSymbol>,
) -> Result<
    (
        DetectionResult,
        ProtocolGuidance,
        Option<crate::services::type_sidecar::ExtractionConfig>,
    ),
    Box<dyn std::error::Error>,
> {
    let agent_service = AgentService::new();
    let framework_detector = FrameworkDetector::new(agent_service.clone());
    let detection = framework_detector
        .detect_frameworks_and_libraries(packages, imported_symbols)
        .await?;

    let guidance_agent = FrameworkGuidanceAgent::new(agent_service);
    // Guidance and extraction config both depend only on detection — run
    // them concurrently instead of paying a lone extra lambda round-trip.
    let (guidance, extraction_config) = tokio::join!(
        guidance_agent.generate_for_active_protocols(&detection),
        generate_extraction_config(&guidance_agent, &detection, packages),
    );

    Ok((detection, guidance?, extraction_config))
}

/// Generate machinery-unwrap rules via the cloud's extraction_config task.
/// Non-fatal: on failure the scan proceeds without unwrapping (machinery
/// types like `AxiosResponse<T>` stay wrapped in the manifest).
async fn generate_extraction_config(
    agent: &FrameworkGuidanceAgent,
    detection: &DetectionResult,
    packages: &Packages,
) -> Option<crate::services::type_sidecar::ExtractionConfig> {
    let dependencies = packages.cleaned_dependency_names();
    match agent
        .fetch_extraction_config(detection, &dependencies)
        .await
    {
        Ok(config) => {
            debug!(
                "Extraction config generated: {} unwrap rule(s)",
                config.rules.len()
            );
            Some(config)
        }
        Err(e) => {
            warn!(
                "Extraction-config generation failed: {} — machinery wrapper types will not \
                 be unwrapped this run",
                e
            );
            None
        }
    }
}

/// Append deterministically extracted protocol operations to the repo's
/// index data: GraphQL (SDL root fields as endpoints, document top-level
/// fields as calls) and Socket.IO (listeners as endpoints, emitters as
/// calls). These protocols never go through the LLM pipeline.
/// Deterministically-extracted non-HTTP operations (GraphQL + Socket.IO).
/// Returned by `append_deterministic_protocol_operations` so the same scan
/// feeds both `cloud_data.endpoints/calls` and the type manifest
/// (`append_protocol_manifest_entries`) without scanning the files twice.
struct ProtocolExtractions {
    graphql: crate::graphql::GraphqlExtraction,
    sockets: crate::socket_io::SocketExtraction,
}

/// The directories to walk for a service's own GraphQL SDL files: its
/// `directory` (or the repo root for a flat single-service config) plus any
/// `include` roots. Mirrors `find_service_files`' scoping so a monorepo
/// package's schema is attributed only to the package that declares it, not its
/// siblings (#242).
fn service_graphql_roots(repo_path: &str, service: &Config) -> Vec<PathBuf> {
    let root = Path::new(repo_path);
    let mut roots = vec![match &service.directory {
        Some(dir) => root.join(dir),
        None => root.to_path_buf(),
    }];
    for inc in &service.include {
        roots.push(root.join(inc));
    }
    roots
}

fn append_deterministic_protocol_operations(
    cloud_data: &mut CloudRepoData,
    repo_path: &str,
    service: &Config,
    files: &[PathBuf],
) -> ProtocolExtractions {
    // Same "{file}:{line}" convention the mount-graph conversions use
    let to_details = |key: OperationKey, file_path: &Path, line: u32| ApiEndpointDetails {
        owner: None,
        key,
        params: vec![],
        request_body: None,
        response_body: None,
        handler_name: None,
        request_type: None,
        response_type: None,
        file_path: PathBuf::from(format!("{}:{}", file_path.display(), line)),
        repo_name: None,
        service_name: None,
    };

    let scan_roots = service_graphql_roots(repo_path, service);
    let graphql = crate::graphql::scan_repo(&scan_roots, files);
    if !graphql.is_empty() {
        debug!(
            producers = graphql.producers.len(),
            consumers = graphql.consumers.len(),
            "Indexing GraphQL operations"
        );
        cloud_data.endpoints.extend(
            graphql
                .producers
                .iter()
                .map(|op| to_details(op.key.clone(), &op.file_path, op.line)),
        );
        cloud_data.calls.extend(
            graphql
                .consumers
                .iter()
                .map(|op| to_details(op.key.clone(), &op.file_path, op.line)),
        );
    }

    let sockets = crate::socket_io::scan_files(files);
    if !sockets.is_empty() {
        debug!(
            listeners = sockets.listeners.len(),
            emitters = sockets.emitters.len(),
            "Indexing Socket.IO operations"
        );
        cloud_data.endpoints.extend(
            sockets
                .listeners
                .iter()
                .map(|op| to_details(op.key.clone(), &op.file_path, op.line)),
        );
        cloud_data.calls.extend(
            sockets
                .emitters
                .iter()
                .map(|op| to_details(op.key.clone(), &op.file_path, op.line)),
        );
    }

    ProtocolExtractions { graphql, sockets }
}

/// Emit type-manifest entries for the deterministically-extracted GraphQL and
/// Socket.IO operations (#245 Phase 1). Without this, only the HTTP mount-graph
/// produces manifest entries, so every non-HTTP op reported `type_state=(none)`
/// with no `type_alias`/anchor.
///
/// Each op gets a single Response-kind entry keyed by its real `OperationKey`
/// (so `OperationKey::canonical()` joins it in the cloud index and eval
/// projection). Listeners / SDL producers are `Producer`; emitters / document
/// consumers are `Consumer`. Only the Response kind is emitted: a phantom
/// Request alias would never resolve and would drag a second `Unknown` entry
/// into the manifest for ops that have no request body concept.
///
/// Socket entries carry `primary_type_symbol` directly (the payload type the
/// extractor captured), which the sidecar then resolves through the existing
/// SymbolRequest path. GraphQL entries deliberately leave `primary_type_symbol`
/// as `None`: mapping an SDL field to its TS resolver return type (or the raw
/// SDL type expression) is deferred to #248, so Phase 1 only gives GraphQL ops
/// a stable `type_alias` + projection entry (an honest `Unknown` instead of
/// `(none)`).
fn append_protocol_manifest_entries(
    entries: &mut Vec<TypeManifestEntry>,
    extractions: &ProtocolExtractions,
) {
    for op in &extractions.graphql.producers {
        add_protocol_manifest_entry(
            entries,
            &op.key,
            ManifestRole::Producer,
            &op.file_path.to_string_lossy(),
            op.line,
            None,
        );
    }
    for op in &extractions.graphql.consumers {
        add_protocol_manifest_entry(
            entries,
            &op.key,
            ManifestRole::Consumer,
            &op.file_path.to_string_lossy(),
            op.line,
            None,
        );
    }
    for op in &extractions.sockets.listeners {
        add_protocol_manifest_entry(
            entries,
            &op.key,
            ManifestRole::Producer,
            &op.file_path.to_string_lossy(),
            op.line,
            op.payload_type_symbol.clone(),
        );
    }
    for op in &extractions.sockets.emitters {
        add_protocol_manifest_entry(
            entries,
            &op.key,
            ManifestRole::Consumer,
            &op.file_path.to_string_lossy(),
            op.line,
            op.payload_type_symbol.clone(),
        );
    }
}

/// Add a single Response-kind manifest entry for a non-HTTP operation. Shared
/// by `append_protocol_manifest_entries`; the HTTP path uses
/// `add_manifest_pair` instead (it emits both Request and Response and dispatches
/// on the HTTP method).
///
/// `primary_type_symbol` is threaded straight onto the entry at creation — the
/// op carries its anchor deterministically, unlike HTTP where it is stamped
/// later from the LLM result. The `type_alias` MUST be computed with the same
/// `build_manifest_type_alias(key, role, Response)` the SymbolRequest side uses,
/// or the enrich-join silently fails to flip `Unknown` → resolved.
fn add_protocol_manifest_entry(
    entries: &mut Vec<TypeManifestEntry>,
    key: &OperationKey,
    role: ManifestRole,
    file_path: &str,
    line_number: u32,
    primary_type_symbol: Option<String>,
) {
    let type_kind = ManifestTypeKind::Response;
    let type_alias = crate::type_manifest::build_manifest_type_alias(key, role, type_kind);
    let infer_kind = infer_kind_for_manifest(role, type_kind);
    let evidence = crate::cloud_storage::TypeEvidence {
        file_path: file_path.to_string(),
        span_start: None,
        span_end: None,
        line_number,
        infer_kind,
        is_explicit: false,
        type_state: ManifestTypeState::Unknown,
    };
    entries.push(TypeManifestEntry {
        key: key.clone(),
        role,
        type_kind,
        type_alias,
        file_path: file_path.to_string(),
        line_number,
        is_explicit: false,
        type_state: ManifestTypeState::Unknown,
        evidence,
        resolved_definition: None,
        expanded_definition: None,
        primary_type_symbol,
    });
}

/// Build CloudRepoData from a mount graph (used by incremental path).
fn build_cloud_data_from_mount_graph(
    repo_name: &str,
    repo_path: &str,
    mount_graph: &MountGraph,
    config: &Config,
    packages: &Packages,
    function_definitions: HashMap<String, FunctionDefinition>,
) -> CloudRepoData {
    let config_json = serde_json::to_string(config).ok();
    let service_name = config_json.as_ref().and_then(|json| {
        serde_json::from_str::<serde_json::Value>(json)
            .ok()
            .and_then(|v| {
                v.get("serviceName")
                    .and_then(|s| s.as_str())
                    .map(String::from)
            })
    });

    let endpoints: Vec<ApiEndpointDetails> = mount_graph
        .get_resolved_endpoints()
        .iter()
        .map(|endpoint| ApiEndpointDetails {
            owner: Some(crate::visitor::OwnerType::App(endpoint.owner.clone())),
            key: OperationKey::http(&endpoint.method, endpoint.full_path.clone()),
            params: vec![],
            request_body: None,
            response_body: None,
            handler_name: endpoint.handler.clone(),
            request_type: None,
            response_type: None,
            file_path: PathBuf::from(&endpoint.file_location),
            repo_name: None,
            service_name: None,
        })
        .collect();

    let calls: Vec<ApiEndpointDetails> = mount_graph
        .get_data_calls()
        .iter()
        .map(|call| ApiEndpointDetails {
            owner: None,
            key: OperationKey::http(&call.method, call.target_url.clone()),
            params: vec![],
            request_body: None,
            response_body: None,
            handler_name: Some(call.client.clone()),
            request_type: None,
            response_type: None,
            file_path: PathBuf::from(&call.file_location),
            repo_name: None,
            service_name: None,
        })
        .collect();

    let mounts: Vec<crate::visitor::Mount> = mount_graph
        .get_mounts()
        .iter()
        .map(|mount| crate::visitor::Mount {
            parent: crate::visitor::OwnerType::App(mount.parent.clone()),
            child: crate::visitor::OwnerType::Router(mount.child.clone()),
            prefix: mount.path_prefix.clone(),
        })
        .collect();

    debug!(
        "CloudRepoData from incremental: {} endpoints, {} calls, {} mounts",
        endpoints.len(),
        calls.len(),
        mounts.len()
    );

    CloudRepoData {
        repo_name: repo_name.to_string(),
        service_name,
        endpoints,
        calls,
        mounts,
        apps: HashMap::new(),
        imported_handlers: vec![],
        function_definitions,
        config_json,
        package_json: serde_json::to_string(packages).ok(),
        packages: Some(packages.clone()),
        last_updated: chrono::Utc::now(),
        commit_hash: get_current_commit_hash(repo_path),
        mount_graph: Some(mount_graph.clone()),
        bundled_types: None,
        type_manifest: None,
        file_results: None,
        cached_detection: None,
        cached_guidance: None,
        cached_extraction_config: None,
        package_json_hash: None,
        cache_version: None,
        type_extraction_status: None,
    }
}

/// Re-scope the already-spawned sidecar to a single service's project so type
/// extraction uses that service's directory and tsconfig instead of the whole
/// repo. No-op for a whole-repo service (no `directory` and no `tsconfig`),
/// which keeps the single-service path on the warm init done in `main`.
///
/// Re-init rebuilds the sidecar's ts-morph project; for a large monorepo this
/// runs once per scoped service. A future optimization could load all service
/// projects in a single init via the sidecar's monorepo builder.
fn scope_sidecar_to_service(sidecar: Option<&TypeSidecar>, repo_path: &str, service: &Config) {
    let Some(sidecar) = sidecar else { return };
    if service.directory.is_none() && service.tsconfig.is_none() {
        return; // whole-repo service: already initialized at the repo root
    }

    // Match main()'s absolute-path init so the sidecar resolves files the same way.
    let canonical =
        std::fs::canonicalize(repo_path).unwrap_or_else(|_| std::path::PathBuf::from(repo_path));
    let service_root = match &service.directory {
        Some(dir) => canonical.join(dir),
        None => canonical,
    };
    let label = service.service_name.as_deref().unwrap_or("(root)");

    debug!(
        "Re-initializing sidecar for service '{}' at {}",
        label,
        service_root.display()
    );
    sidecar.start_init(&service_root, service.tsconfig.as_deref());
    if let Err(e) = sidecar.wait_ready(Duration::from_secs(30)) {
        warn!(
            "Sidecar re-init for service '{}' failed: {} — type extraction may be skipped \
             for this service",
            label, e
        );
    }
}

/// Resolve types via sidecar if available (shared logic for full and incremental paths).
#[allow(clippy::too_many_arguments)]
fn resolve_types_if_available(
    sidecar: Option<&TypeSidecar>,
    file_orchestrator: &FileOrchestrator,
    file_results: &HashMap<String, crate::agents::file_analyzer_agent::FileAnalysisResult>,
    repo_path: &str,
    extraction_config: Option<&crate::services::type_sidecar::ExtractionConfig>,
    mount_graph: &MountGraph,
    config: &Config,
    extra_explicit: &[crate::services::type_sidecar::SymbolRequest],
    cloud_data: &mut CloudRepoData,
) {
    let Some(sidecar) = sidecar else {
        cloud_data.type_extraction_status = Some(
            "type extraction skipped: sidecar unavailable (not found, failed to start, \
             or failed to initialize)"
                .to_string(),
        );
        return;
    };

    debug!("Starting sidecar type resolution");
    match sidecar.wait_ready(Duration::from_secs(10)) {
        Ok(()) => {
            match file_orchestrator.resolve_types_with_sidecar(
                sidecar,
                file_results,
                repo_path,
                extraction_config,
                mount_graph,
                config,
                extra_explicit,
            ) {
                Ok(type_resolution) => {
                    debug!(
                        "Type resolution: {} explicit, {} inferred, {} failures",
                        type_resolution.explicit_manifest.len(),
                        type_resolution.inferred_types.len(),
                        type_resolution.symbol_failures.len()
                    );
                    cloud_data.bundled_types = type_resolution.dts_content.clone();
                    if let Some(ref mut manifest) = cloud_data.type_manifest {
                        enrich_manifest_with_type_resolution(
                            manifest,
                            &type_resolution,
                            type_resolution.dts_content.as_deref(),
                        );
                    }
                    // Per-symbol failures are logged in FileOrchestrator at the
                    // resolution call site (with capped warn + spillover to debug).

                    // A missing extraction config means the cloud's rule
                    // generation failed (an empty rule set arrives as
                    // Some(config) with zero rules). Types still resolve, but
                    // machinery wrappers stay wrapped — surface the
                    // degradation instead of letting the run look healthy.
                    if extraction_config.is_none() {
                        cloud_data.type_extraction_status = Some(
                            "machinery unwrapping disabled this run: extraction-config \
                             generation failed; wrapper types (e.g. AxiosResponse<T>) may \
                             surface in the type manifest"
                                .to_string(),
                        );
                    }
                }
                Err(e) => {
                    warn!("Type resolution failed: {}", e);
                    debug!("Continuing without bundled types");
                    cloud_data.type_extraction_status =
                        Some(format!("type resolution failed: {}", e));
                }
            }
        }
        Err(e) => {
            warn!("Sidecar not ready: {}", e);
            debug!("Skipping type resolution");
            cloud_data.type_extraction_status =
                Some(format!("type extraction skipped: sidecar not ready: {}", e));
        }
    }
}

/// Discover files and extract symbols for MultiAgentOrchestrator
fn discover_files_and_symbols(
    repo_path: &str,
    service: &Config,
    cm: Lrc<SourceMap>,
) -> FileDiscoveryResult {
    let handler = Handler::with_tty_emitter(ColorConfig::Auto, true, false, Some(cm.clone()));
    let repo_name = get_repository_name(repo_path);

    // Find files scoped to this service's directory (+ include roots).
    let ignore_patterns = service_ignore_patterns(service);
    let (files, _) = find_service_files(repo_path, service, &ignore_patterns);

    // Zero files means the scan target is wrong (typo'd path, empty checkout):
    // proceeding would upload an empty service and silently erase its
    // coverage from the index.
    if files.is_empty() {
        let scope = match &service.directory {
            Some(dir) => format!("service directory '{}'", dir),
            None => "repository root".to_string(),
        };
        return Err(format!(
            "No JS/TS source files found under {} in '{}'. Check the scan path \
             and the directory/include entries in carrick.json.",
            scope, repo_path
        )
        .into());
    }

    debug!("Found {} files to analyze in {}", files.len(), repo_path);

    // Extract imported symbols and function definitions by parsing files
    let mut all_imported_symbols = HashMap::new();
    let mut all_function_definitions = HashMap::new();

    for file_path in &files {
        if let Some(module) = parse_file(file_path, &cm, &handler) {
            // Extract import symbols
            let mut import_extractor = ImportSymbolExtractor::new();
            module.visit_with(&mut import_extractor);
            all_imported_symbols.extend(import_extractor.imported_symbols);

            // Extract function definitions with type annotations and source text
            let mut func_extractor =
                FunctionDefinitionExtractor::new(file_path.clone(), cm.clone());
            module.visit_with(&mut func_extractor);
            func_extractor.finalize_exports();
            all_function_definitions.extend(func_extractor.function_definitions);
        }
    }

    debug!(
        "Extracted {} imported symbols and {} function definitions from {} files",
        all_imported_symbols.len(),
        all_function_definitions.len(),
        files.len()
    );

    Ok((
        files,
        all_imported_symbols,
        all_function_definitions,
        repo_name,
    ))
}

/// Resolve a repo's `carrick.json` into one service config per service.
///
/// No config, or a flat config, yields a single service rooted at the repo
/// root (zero-config single-service mode). A `services` array yields one entry
/// per declared service. Always returns at least one service.
///
/// A config that exists but cannot be parsed, or that declares paths that
/// don't exist, is a hard error: silently falling back to defaults would
/// ignore the user's declared service layout and upload a wrong index.
fn resolve_services(repo_path: &str) -> Result<Vec<Config>, Box<dyn std::error::Error>> {
    // carrick.json belongs at the scan root. Read it there directly rather than
    // walking the tree (a tree walk would pick up nested example/fixture
    // configs in a repo that contains them).
    let config_path = std::path::Path::new(repo_path).join("carrick.json");

    let services = if config_path.is_file() {
        debug!("Found carrick.json: {}", config_path.display());
        Config::load_services(vec![config_path.clone()]).map_err(|e| {
            format!(
                "Failed to parse {}: {}. Fix the config or delete it to scan \
                 the repo as a single zero-config service.",
                config_path.display(),
                e
            )
        })?
    } else {
        Vec::new()
    };

    // A typo'd directory would otherwise walk nothing and upload an empty
    // service, silently erasing its coverage from the index.
    let root = std::path::Path::new(repo_path);
    for service in &services {
        let label = service
            .service_name
            .as_deref()
            .or(service.directory.as_deref())
            .unwrap_or("<unnamed>");
        if let Some(dir) = &service.directory
            && !root.join(dir).is_dir()
        {
            return Err(format!(
                "Service '{}' in {} declares directory '{}', which does not exist under '{}'",
                label,
                config_path.display(),
                dir,
                repo_path
            )
            .into());
        }
        for inc in &service.include {
            if !root.join(inc).exists() {
                return Err(format!(
                    "Service '{}' in {} declares include path '{}', which does not exist under '{}'",
                    label,
                    config_path.display(),
                    inc,
                    repo_path
                )
                .into());
            }
        }
    }

    if services.is_empty() {
        Ok(vec![Config::default()])
    } else {
        Ok(services)
    }
}

/// Build-artifact directories to skip everywhere.
///
/// `ts_check` is the scanner's own bundled type-checker, which sits at the scan
/// root when the GitHub Action runs `./carrick .`. It's only ignored for the
/// implicit whole-repo scan (`directory: None`); an explicitly declared service
/// directory is honoured even if it is named `ts_check`, so a repo can index
/// such a directory on purpose.
fn service_ignore_patterns(service: &Config) -> Vec<&'static str> {
    let mut patterns = vec!["node_modules", "dist", "build", ".next"];
    if service.directory.is_none() {
        patterns.push("ts_check");
    }
    patterns
}

/// Load the package data for a single service, scoped to its own
/// `package.json` (within its directory), not an arbitrary one from the repo.
///
/// A missing `package.json` is fine (empty dependency set); one that exists
/// but cannot be parsed is a hard error — defaulting to zero dependencies
/// would silently gut framework detection and endpoint extraction.
fn load_packages_for_service(
    repo_path: &str,
    service: &Config,
) -> Result<Packages, Box<dyn std::error::Error>> {
    let ignore_patterns = service_ignore_patterns(service);
    let (_, package_json_path) = find_service_files(repo_path, service, &ignore_patterns);
    if let Some(package_path) = package_json_path {
        debug!("Found package.json: {}", package_path.display());
        Packages::new(vec![package_path.clone()])
            .map_err(|e| format!("Failed to parse {}: {}", package_path.display(), e).into())
    } else {
        Ok(Packages::default())
    }
}

/// Resolve per-endpoint type definitions using the sidecar's compiler.
/// Populates `resolved_definition` and `expanded_definition` on each manifest entry.
/// Non-fatal: if resolution fails, entries keep their None values and the MCP falls back to regex.
fn resolve_per_endpoint_definitions(sidecar: &TypeSidecar, cloud_data: &mut CloudRepoData) {
    let Some(ref bundled_types) = cloud_data.bundled_types else {
        return;
    };
    let Some(ref mut manifest) = cloud_data.type_manifest else {
        return;
    };

    // Collect unique aliases that have actual types (not Unknown)
    let aliases: Vec<String> = manifest
        .iter()
        .filter(|e| e.type_state != ManifestTypeState::Unknown)
        .map(|e| e.type_alias.clone())
        .collect::<std::collections::HashSet<_>>()
        .into_iter()
        .collect();

    if aliases.is_empty() {
        return;
    }

    debug!(
        "Resolving {} type definition(s) via compiler",
        aliases.len()
    );

    match sidecar.resolve_definitions(bundled_types, &aliases) {
        Ok(resolved) => {
            let lookup: std::collections::HashMap<String, _> = resolved
                .into_iter()
                .map(|r| (r.type_alias.clone(), r))
                .collect();
            for entry in manifest.iter_mut() {
                if let Some(r) = lookup.get(&entry.type_alias) {
                    entry.resolved_definition = Some(r.definition.clone());
                    entry.expanded_definition = Some(r.expanded.clone());
                }
            }
            debug!("Resolved {} type definition(s)", lookup.len());
        }
        Err(e) => {
            warn!("Per-endpoint definition resolution failed: {}", e);
            debug!("Continuing without resolved definitions (MCP will use regex fallback)");
        }
    }
}

fn build_type_manifest_entries(
    mount_graph: &MountGraph,
    config: &Config,
) -> Vec<TypeManifestEntry> {
    let normalizer = UrlNormalizer::new(config);
    let mut entries = Vec::new();

    for endpoint in mount_graph.get_resolved_endpoints() {
        let method = normalize_manifest_method(&endpoint.method);
        if !is_http_method(&method) {
            continue;
        }
        let path = endpoint.full_path.clone();
        if !path.starts_with('/') {
            continue;
        }
        let (file_path, line_number) = parse_file_location(&endpoint.file_location);

        add_manifest_pair(
            &mut entries,
            OperationKey::http(&method, path),
            ManifestRole::Producer,
            &file_path,
            line_number,
            None,
        );
    }

    for call in mount_graph.get_data_calls() {
        if !normalizer.is_probable_url(&call.target_url) {
            continue;
        }
        let (file_path, line_number) = parse_file_location(&call.file_location);
        let method = normalize_manifest_method(&call.method);
        if !is_http_method(&method) {
            continue;
        }
        let path = normalizer.extract_path(&call.target_url);
        let key = OperationKey::http(&method, path);
        let call_id = build_call_site_id(&file_path, line_number, &key);

        add_manifest_pair(
            &mut entries,
            key,
            ManifestRole::Consumer,
            &file_path,
            line_number,
            Some(&call_id),
        );
    }

    entries
}

/// Thread the LLM's real type-anchor symbol onto the manifest entries (#233).
///
/// The manifest's `type_alias` is a synthetic hashed name (`Endpoint_<hash>_…`);
/// the real source symbol (`StatusResponse`) lives on the file-analyzer result.
/// Join by `(file_path, line_number)`: the mount-graph `file_location` the
/// manifest is built from is `"{file_path}:{line}"`, where `file_path` is the
/// same key `file_results` is keyed by and `line` is the same LLM-emitted
/// `line_number`. Stamp the symbol onto every manifest entry for that op
/// (request + response) so the eval projection surfaces the real anchor instead
/// of the hash. The first non-None symbol per `(file, line)` wins.
///
/// The symbol-side line is normalized exactly as the manifest side
/// (`parse_file_location`): a non-positive/missing line collapses to `1`. Keying
/// a line-0 anchor at `0` would never join a manifest entry (always `>= 1`), so
/// the anchor would silently fall back to the hashed `type_alias`.
fn stamp_manifest_anchor_symbols(
    manifest: &mut [TypeManifestEntry],
    file_results: &HashMap<String, crate::agents::file_analyzer_agent::FileAnalysisResult>,
) {
    // Mirror `parse_file_location`'s `Some(0) | None => 1` normalization so the
    // join keys line up on both sides.
    let normalize_line = |line: i32| -> u32 { if line <= 0 { 1 } else { line as u32 } };
    // (file_path, line_number) -> primary_type_symbol, from endpoints and calls.
    let mut symbols: HashMap<(String, u32), String> = HashMap::new();
    for (file_path, result) in file_results {
        for endpoint in &result.endpoints {
            if let Some(symbol) = endpoint.primary_type_symbol.as_ref() {
                symbols
                    .entry((file_path.clone(), normalize_line(endpoint.line_number)))
                    .or_insert_with(|| symbol.clone());
            }
        }
        for call in &result.data_calls {
            if let Some(symbol) = call.primary_type_symbol.as_ref() {
                symbols
                    .entry((file_path.clone(), normalize_line(call.line_number)))
                    .or_insert_with(|| symbol.clone());
            }
        }
    }
    if symbols.is_empty() {
        return;
    }
    for entry in manifest.iter_mut() {
        if let Some(symbol) = symbols.get(&(entry.file_path.clone(), entry.line_number)) {
            entry.primary_type_symbol = Some(symbol.clone());
        }
    }
}

fn add_manifest_pair(
    entries: &mut Vec<TypeManifestEntry>,
    key: OperationKey,
    role: ManifestRole,
    file_path: &str,
    line_number: u32,
    call_id: Option<&str>,
) {
    // Producers for GET/HEAD/OPTIONS never have request bodies
    let skip_request = role == ManifestRole::Producer
        && matches!(
            key.as_http().map(|(method, _)| method),
            Some("GET" | "HEAD" | "OPTIONS")
        );

    for type_kind in [ManifestTypeKind::Request, ManifestTypeKind::Response] {
        if skip_request && type_kind == ManifestTypeKind::Request {
            continue;
        }
        let type_alias = build_manifest_type_alias_with_call_id(&key, role, type_kind, call_id);
        let infer_kind = infer_kind_for_manifest(role, type_kind);
        let evidence = crate::cloud_storage::TypeEvidence {
            file_path: file_path.to_string(),
            span_start: None,
            span_end: None,
            line_number,
            infer_kind,
            is_explicit: false,
            type_state: ManifestTypeState::Unknown,
        };
        entries.push(TypeManifestEntry {
            key: key.clone(),
            role,
            type_kind,
            type_alias,
            file_path: file_path.to_string(),
            line_number,
            is_explicit: false,
            type_state: ManifestTypeState::Unknown,
            evidence,
            resolved_definition: None,
            expanded_definition: None,
            // Threaded on after the fact by `stamp_manifest_anchor_symbols`,
            // which joins the LLM's real anchor symbol by `(file_path, line)`.
            primary_type_symbol: None,
        });
    }
}

fn infer_kind_for_manifest(role: ManifestRole, type_kind: ManifestTypeKind) -> InferKind {
    match (role, type_kind) {
        (ManifestRole::Consumer, ManifestTypeKind::Response) => InferKind::CallResult,
        (_, ManifestTypeKind::Response) => InferKind::ResponseBody,
        (_, ManifestTypeKind::Request) => InferKind::RequestBody,
    }
}

/// Enrich manifest entries with type resolution results.
///
/// This function updates the `type_state` and `is_explicit` fields of manifest entries
/// based on the results from the TypeSidecar. Types that were successfully resolved
/// (either explicitly or through inference) will have their state updated from `Unknown`
/// to `Explicit` or `Implicit`.
fn enrich_manifest_with_type_resolution(
    manifest: &mut [TypeManifestEntry],
    type_resolution: &TypeResolutionResult,
    bundled_dts: Option<&str>,
) {
    // Build a lookup of resolved type aliases
    // Key: type_alias, Value: (type_string, is_explicit)
    let mut resolved_types: HashMap<String, (String, bool)> = HashMap::new();

    // Add explicit types from the manifest
    for entry in &type_resolution.explicit_manifest {
        resolved_types.insert(entry.alias.clone(), (entry.type_string.clone(), true));
    }

    // Add inferred types
    for inferred in &type_resolution.inferred_types {
        // Don't overwrite explicit types with inferred ones
        if !resolved_types.contains_key(&inferred.alias) {
            resolved_types.insert(
                inferred.alias.clone(),
                (inferred.type_string.clone(), inferred.is_explicit),
            );
        }
    }

    // Also check the bundled .d.ts content for defined types
    // This catches types that were successfully bundled but not in the manifest.
    // Exclude aliases defined as `= unknown` — those are placeholders for failed
    // inferences and should not be promoted to Implicit.
    let dts_defined_aliases: HashSet<String> = if let Some(dts) = bundled_dts {
        manifest
            .iter()
            .filter(|e| {
                dts_defines_alias(dts, &e.type_alias)
                    && !dts_alias_is_trivially_unknown(dts, &e.type_alias)
            })
            .map(|e| e.type_alias.clone())
            .collect()
    } else {
        HashSet::new()
    };

    // A type is genuinely unresolved when it resolves to `unknown`/`any`/empty,
    // or when the bundled .d.ts only carries the trivial `= unknown` placeholder
    // that append_missing_aliases injects for a missing alias. Either way the
    // shape never reached the bundle, so the entry must read `Unknown` — never a
    // promoted state that asserts a shape we don't actually have.
    let dts_trivially_unknown = |alias: &str| {
        bundled_dts
            .map(|dts| dts_alias_is_trivially_unknown(dts, alias))
            .unwrap_or(false)
    };

    // Update manifest entries
    for entry in manifest.iter_mut() {
        if let Some((type_string, is_explicit)) = resolved_types.get(&entry.type_alias) {
            // Check if the type is actually resolved (not "unknown")
            let is_unknown_type = type_string.trim() == "unknown"
                || type_string.trim() == "any"
                || type_string.is_empty()
                || dts_trivially_unknown(&entry.type_alias);

            if is_unknown_type {
                // Downgrade to Unknown so the `= unknown` placeholder gate
                // (resolve_per_endpoint_definitions, ts_check) stays shut and the
                // edge is reported unverifiable rather than falsely compatible.
                entry.is_explicit = false;
                entry.type_state = ManifestTypeState::Unknown;
                entry.evidence.is_explicit = false;
                entry.evidence.type_state = ManifestTypeState::Unknown;
            } else {
                entry.is_explicit = *is_explicit;
                entry.type_state = if *is_explicit {
                    ManifestTypeState::Explicit
                } else {
                    ManifestTypeState::Implicit
                };
                entry.evidence.is_explicit = *is_explicit;
                entry.evidence.type_state = entry.type_state;
            }
        } else if dts_defined_aliases.contains(&entry.type_alias) {
            // Type is defined in the .d.ts but wasn't in our resolution results
            // This can happen for inline aliases or other edge cases
            entry.type_state = ManifestTypeState::Implicit;
            entry.evidence.type_state = ManifestTypeState::Implicit;
        } else if dts_trivially_unknown(&entry.type_alias) {
            // Only a `= unknown` placeholder reached the bundle — keep Unknown.
            entry.is_explicit = false;
            entry.type_state = ManifestTypeState::Unknown;
            entry.evidence.is_explicit = false;
            entry.evidence.type_state = ManifestTypeState::Unknown;
        }
    }

    // Log enrichment stats
    let explicit_count = manifest
        .iter()
        .filter(|e| e.type_state == ManifestTypeState::Explicit)
        .count();
    let implicit_count = manifest
        .iter()
        .filter(|e| e.type_state == ManifestTypeState::Implicit)
        .count();
    let unknown_count = manifest
        .iter()
        .filter(|e| e.type_state == ManifestTypeState::Unknown)
        .count();
    debug!(
        "Manifest enrichment: {} explicit, {} implicit, {} unknown",
        explicit_count, implicit_count, unknown_count
    );
}

#[derive(Serialize)]
struct TypeManifestFile {
    repo_name: String,
    commit_hash: String,
    entries: Vec<TypeManifestEntry>,
}

async fn analyze_current_repo(
    repo_path: &str,
    service: &Config,
    packages: &Packages,
    sidecar: Option<&TypeSidecar>,
) -> Result<CloudRepoData, Box<dyn std::error::Error>> {
    // Canonicalize repo_path for consistent path normalization between runs
    let canonical = std::fs::canonicalize(repo_path)
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_else(|_| repo_path.to_string());
    let repo_path = canonical.as_str();

    debug!("Running multi-agent analysis on: {}", repo_path);

    let config = service;

    // Create shared SourceMap and discover files and symbols, scoped to the service
    let cm: Lrc<SourceMap> = Default::default();
    let (files, all_imported_symbols, function_definitions, repo_name) =
        discover_files_and_symbols(repo_path, config, cm.clone())?;
    debug!(
        "Repository '{}': {} files, {} function definitions",
        repo_name,
        files.len(),
        function_definitions.len()
    );

    // 3. Create MultiAgentOrchestrator (auth is via GitHub Actions OIDC)
    let orchestrator = MultiAgentOrchestrator::new(cm.clone());

    // 4. Run the complete multi-agent analysis
    let analysis_result = orchestrator
        .run_complete_analysis(files.clone(), packages, &all_imported_symbols, repo_path)
        .await?;

    // 4b. Generate function intents using LLM
    let mut function_definitions = function_definitions;
    {
        let intent_agent = AgentService::new();
        // Full scan: no previous data, so nothing to reuse — every intent is
        // generated fresh and its content hash recorded for the next scan.
        generate_function_intents(
            &intent_agent,
            &mut function_definitions,
            &all_imported_symbols,
            &HashMap::new(),
        )
        .await;
    }

    // 4c. Compose function signatures, inferring unannotated slots via sidecar.
    populate_function_signatures(sidecar, &mut function_definitions, repo_path);

    // 5. Build CloudRepoData directly from multi-agent results (bypassing Analyzer adapter layer)
    let mut cloud_data = CloudRepoData::from_multi_agent_results(
        repo_name.clone(),
        repo_path,
        &analysis_result,
        serde_json::to_string(config).ok(),
        serde_json::to_string(packages).ok(),
        Some(packages.clone()),
        function_definitions.clone(),
    );
    let protocol_extractions =
        append_deterministic_protocol_operations(&mut cloud_data, repo_path, service, &files);

    let mut manifest_entries = build_type_manifest_entries(&analysis_result.mount_graph, config);
    stamp_manifest_anchor_symbols(&mut manifest_entries, &analysis_result.file_results);
    append_protocol_manifest_entries(&mut manifest_entries, &protocol_extractions);
    if !manifest_entries.is_empty() {
        cloud_data.type_manifest = Some(manifest_entries);
    }

    // 6. Resolve types using sidecar if available
    let agent_service = AgentService::new();
    let file_orchestrator = FileOrchestrator::new(agent_service.clone());

    let guidance_agent = FrameworkGuidanceAgent::new(agent_service);
    let extraction_config = generate_extraction_config(
        &guidance_agent,
        &analysis_result.framework_detection,
        packages,
    )
    .await;
    cloud_data.cached_extraction_config = extraction_config.clone();

    // Socket payload anchors resolve through the same sidecar bundle path as
    // HTTP explicit symbols (#245). GraphQL anchors are deferred to #248, so no
    // GraphQL SymbolRequests are produced here.
    let socket_requests =
        file_orchestrator.collect_socket_type_requests(&protocol_extractions.sockets, repo_path);

    resolve_types_if_available(
        sidecar,
        &file_orchestrator,
        &analysis_result.file_results,
        repo_path,
        extraction_config.as_ref(),
        &analysis_result.mount_graph,
        config,
        &socket_requests,
        &mut cloud_data,
    );

    if let Some(bundled_types) = cloud_data.bundled_types.take() {
        let updated = append_missing_aliases(bundled_types, cloud_data.type_manifest.as_ref());
        cloud_data.bundled_types = Some(updated);
    }

    // 6b. Resolve per-endpoint definitions via compiler
    if let Some(sidecar) = sidecar {
        resolve_per_endpoint_definitions(sidecar, &mut cloud_data);
    }

    // 7. Populate cache fields for future incremental runs
    let mut cached_file_results = analysis_result.file_results.clone();
    let normalized = normalize_file_results_keys(&cached_file_results, repo_path);
    cached_file_results = normalized;
    strip_diagnostic_fields(&mut cached_file_results);
    cloud_data.file_results = Some(cached_file_results);
    cloud_data.cached_detection = Some(analysis_result.framework_detection.clone());
    cloud_data.cached_guidance = Some(analysis_result.framework_guidance.clone());
    cloud_data.cache_version = Some(CACHE_VERSION);
    // Same workspace-wide hash the incremental gate compares against.
    cloud_data.package_json_hash = Some(hash_workspace_package_jsons(packages, repo_path));

    Ok(cloud_data)
}

async fn build_cross_repo_analyzer(
    mut all_repo_data: Vec<CloudRepoData>,
    current_repos: Vec<CloudRepoData>,
    ts_check_dir: Option<&std::path::Path>,
) -> Result<Analyzer, Box<dyn std::error::Error>> {
    // Add the freshly-analyzed local services (one per service) to the mix
    all_repo_data.extend(current_repos);
    // 1. Merge configs and packages using generic function
    let combined_config = merge_serialized_data(&all_repo_data, |data| data.config_json.as_ref())?;
    let combined_packages =
        merge_serialized_data(&all_repo_data, |data| data.package_json.as_ref())?;

    // 2. Build analyzer using shared logic (skip type resolution for cross-repo)
    let cm: Lrc<SourceMap> = Default::default();
    let builder = AnalyzerBuilder::new_for_cross_repo(combined_config, cm);
    let mut analyzer = builder.build_from_repo_data(all_repo_data.clone()).await?;

    // 3. Merge mount graphs from all repos for framework-agnostic analysis
    let merged_mount_graph = MountGraph::merge_from_repos(&all_repo_data);
    analyzer.set_mount_graph(merged_mount_graph);

    // 4. Add packages data from all repos for dependency analysis. Key by
    //    service identity (service_name, falling back to repo_name) so two
    //    services in the same monorepo don't overwrite each other — matching
    //    the cloud's service_name ?? repo_name attribution convention.
    for repo_data in &all_repo_data {
        if let Some(packages) = &repo_data.packages {
            let key = repo_data
                .service_name
                .clone()
                .unwrap_or_else(|| repo_data.repo_name.clone());
            analyzer.add_repo_packages(key, packages.clone());
        }
    }

    // 5. Recreate type files from S3 and run type checking
    if let Some(ts_check_dir) = ts_check_dir {
        analyzer.set_ts_check_dir(ts_check_dir.to_path_buf());
        recreate_type_files_and_check(&all_repo_data, &combined_packages, ts_check_dir)?;

        // 6. Run final type checking
        if let Err(e) = analyzer.run_final_type_checking() {
            warn!("Type checking failed: {}", e);
        }
    } else {
        warn!(
            "Skipping type checking: ts_check/ directory was not found adjacent to the \
             carrick binary. Expected at <exe_dir>/ts_check or <exe_dir>/../lib/ts_check."
        );
    }

    Ok(analyzer)
}

fn recreate_type_files_and_check(
    all_repo_data: &[CloudRepoData],
    packages: &Packages,
    ts_check_dir: &std::path::Path,
) -> Result<(), Box<dyn std::error::Error>> {
    let output_dir_buf = ts_check_dir.join("output");
    let output_dir = output_dir_buf.as_path();
    if output_dir.exists() {
        debug!("Cleaning output directory: {}", output_dir.display());
        if let Err(e) = std::fs::remove_dir_all(output_dir) {
            warn!("Failed to clean output directory: {}", e);
        }
    }

    if let Err(e) = std::fs::create_dir_all(output_dir) {
        warn!("Failed to create output directory: {}", e);
    } else {
        debug!("Created clean output directory: {}", output_dir.display());
    }

    for repo_data in all_repo_data {
        if let Some(bundled_types) = &repo_data.bundled_types {
            let safe_repo_name = repo_data.repo_name.replace("/", "_");
            let file_name = format!("{}_types.d.ts", safe_repo_name);
            let file_path = output_dir.join(&file_name);
            let content =
                append_missing_aliases(bundled_types.clone(), repo_data.type_manifest.as_ref());

            if let Err(e) = std::fs::write(&file_path, content) {
                warn!("Failed to write type file {}: {}", file_name, e);
            } else {
                debug!("Created bundled type file: {}", file_path.display());
            }
        } else {
            debug!(
                "No bundled types available for repo: {}",
                repo_data.repo_name
            );
        }
    }

    write_manifest_files(all_repo_data, output_dir)?;

    // Recreate package.json and tsconfig.json after writing type files
    recreate_package_and_tsconfig(output_dir, packages)?;

    Ok(())
}

fn write_manifest_files(
    all_repo_data: &[CloudRepoData],
    output_dir: &std::path::Path,
) -> Result<(), Box<dyn std::error::Error>> {
    let mut producer_entries = Vec::new();
    let mut consumer_entries = Vec::new();

    for repo_data in all_repo_data {
        if let Some(entries) = &repo_data.type_manifest {
            for entry in entries {
                // ts_check manifests are HTTP-only by contract: `ManifestEntry`
                // (ts_check/lib/manifest-matcher.ts) requires `method`/`path`,
                // which GraphQL/socket OperationKeys never serialise. Those
                // protocols are scored by their own pipelines (#236/#248), so
                // they stay in `cloud_data.type_manifest` (cloud index + eval
                // projection) but must not reach the producer/consumer manifest
                // files. Without this filter a single non-HTTP entry makes
                // `validateEntry` throw, zeroing out every cross-repo verdict (#253).
                if entry.key.protocol() != crate::operation::Protocol::Http {
                    continue;
                }
                match entry.role {
                    ManifestRole::Producer => producer_entries.push(entry.clone()),
                    ManifestRole::Consumer => consumer_entries.push(entry.clone()),
                }
            }
        }
    }

    let producer_manifest = TypeManifestFile {
        repo_name: "cross-repo-producers".to_string(),
        commit_hash: "mixed".to_string(),
        entries: producer_entries,
    };
    let consumer_manifest = TypeManifestFile {
        repo_name: "cross-repo-consumers".to_string(),
        commit_hash: "mixed".to_string(),
        entries: consumer_entries,
    };

    let producer_path = output_dir.join("producer-manifest.json");
    let consumer_path = output_dir.join("consumer-manifest.json");

    std::fs::write(
        &producer_path,
        serde_json::to_string_pretty(&producer_manifest)?,
    )?;
    std::fs::write(
        &consumer_path,
        serde_json::to_string_pretty(&consumer_manifest)?,
    )?;

    debug!(
        "Wrote manifest files: {} producers, {} consumers",
        producer_manifest.entries.len(),
        consumer_manifest.entries.len()
    );

    Ok(())
}

/// Trailing marker stamped onto every `= unknown` alias that
/// `append_missing_aliases` injects for a manifest entry that never reached the
/// bundle. It lets `dts_alias_is_trivially_unknown` recognise *our* placeholder
/// without misclassifying a developer-authored `type X = unknown` in a real API
/// type (which keeps its resolved shape rather than being downgraded).
const MISSING_ALIAS_MARKER: &str = "// carrick:missing-alias";

fn append_missing_aliases(content: String, manifest: Option<&Vec<TypeManifestEntry>>) -> String {
    let Some(entries) = manifest else {
        return content;
    };

    let mut updated = content;
    let mut seen = std::collections::HashSet::new();

    for entry in entries {
        if !seen.insert(entry.type_alias.clone()) {
            continue;
        }

        if dts_defines_alias(&updated, &entry.type_alias) {
            continue;
        }

        if !updated.is_empty() && !updated.ends_with('\n') {
            updated.push('\n');
        }
        updated.push_str("export type ");
        updated.push_str(&entry.type_alias);
        updated.push_str(" = unknown; ");
        updated.push_str(MISSING_ALIAS_MARKER);
        updated.push('\n');
    }

    updated
}

fn dts_defines_alias(content: &str, alias: &str) -> bool {
    let escaped = regex::escape(alias);
    let pattern = format!(r"\b(type|interface|class|enum|namespace)\s+{}\b", escaped);
    match regex::Regex::new(&pattern) {
        Ok(re) => re.is_match(content),
        Err(_) => false,
    }
}

/// Returns true only when the .d.ts carries the *Carrick-injected* `= unknown`
/// placeholder for this alias, identified by the `MISSING_ALIAS_MARKER` comment
/// that `append_missing_aliases` stamps on it. A developer-authored
/// `type X = unknown` in a real API type carries no marker and is therefore not
/// treated as a placeholder, so its cross-repo edge keeps its resolved state
/// instead of being silently downgraded to `Unknown` (#244).
fn dts_alias_is_trivially_unknown(content: &str, alias: &str) -> bool {
    let escaped = regex::escape(alias);
    let marker = regex::escape(MISSING_ALIAS_MARKER);
    // Anchor on the exact form append_missing_aliases emits:
    //   export type <alias> = unknown; // carrick:missing-alias
    // The optional `export`, generics, and modifiers are tolerated, but the
    // trailing marker on the same line is what actually identifies it as ours.
    let pattern = format!(r"\btype\s+{escaped}\b[^\n]*=\s*unknown\s*;[^\n]*{marker}");
    match regex::Regex::new(&pattern) {
        Ok(re) => re.is_match(content),
        Err(_) => false,
    }
}

/// Recreate package.json and tsconfig.json in the output directory
fn recreate_package_and_tsconfig(
    output_dir: &std::path::Path,
    packages: &Packages,
) -> Result<(), Box<dyn std::error::Error>> {
    // Create package.json
    let package_json_path = output_dir.join("package.json");
    let package_dependencies = packages.get_dependencies();

    // Convert PackageInfo objects to simple version strings for npm. Drop
    // entries whose version is unusable (empty, the literal "undefined", or a
    // value with no digit) — a merged repo can carry these, and npm turns
    // `typescript@undefined` into a hard ERESOLVE that aborts the whole
    // cross-repo type pass.
    let mut dependencies = std::collections::HashMap::new();
    for (name, package_info) in package_dependencies {
        let version = package_info.version.trim();
        if version.is_empty()
            || version == "undefined"
            || !version.chars().any(|c| c.is_ascii_digit())
        {
            debug!("Skipping dependency {name} with unusable version {version:?}");
            continue;
        }
        dependencies.insert(name.clone(), version.to_string());
    }

    // Pin the TypeScript toolchain we control for this synthetic type-check
    // package. Overwrite (not insert-if-missing): a merged repo may pin a
    // different/older typescript that conflicts with ts-node's peer range, so
    // forcing a known-good pair is what keeps `npm install` resolvable. These
    // match ts_check/package.json's pins.
    dependencies.insert("typescript".to_string(), "5.8.3".to_string());
    dependencies.insert("ts-node".to_string(), "10.9.2".to_string());

    let package_json_content = serde_json::json!({
        "name": "carrick-type-check",
        "version": "1.0.0",
        "dependencies": dependencies
    });

    std::fs::write(
        &package_json_path,
        serde_json::to_string_pretty(&package_json_content)?,
    )?;
    debug!("Recreated package.json at {}", package_json_path.display());

    let skip_npm_install = std::env::var("CARRICK_SKIP_NPM_INSTALL").is_ok()
        || std::env::var("CARRICK_MOCK_ALL").is_ok();

    if skip_npm_install {
        debug!("Skipping npm install (mock mode or CARRICK_SKIP_NPM_INSTALL set)");
    } else {
        // Clean any existing node_modules and package-lock.json to avoid conflicts
        let node_modules_path = output_dir.join("node_modules");
        let package_lock_path = output_dir.join("package-lock.json");

        if node_modules_path.exists() {
            debug!("Removing existing node_modules directory");
            std::fs::remove_dir_all(&node_modules_path).ok();
        }

        if package_lock_path.exists() {
            debug!("Removing existing package-lock.json");
            std::fs::remove_file(&package_lock_path).ok();
        }

        // Install dependencies
        use std::process::Command;
        debug!("Installing dependencies...");

        // `--legacy-peer-deps` so a transitive peer-range disagreement (e.g.
        // ts-node's `typescript@>=2.7` vs a repo's pinned major) can't abort
        // the install. This package only feeds ts-morph type extraction, so a
        // looser peer graph is harmless.
        let install_output = Command::new("npm")
            .arg("install")
            .arg("--legacy-peer-deps")
            .current_dir(output_dir)
            .output()
            .map_err(|e| format!("Failed to run npm install: {}", e))?;

        if !install_output.status.success() {
            // Fail loudly (#149): a swallowed install failure used to let the
            // run print "✓ Cross-repo analysis complete" while type checking
            // silently degraded — masking, e.g., an ERESOLVE conflict.
            let stderr = String::from_utf8_lossy(&install_output.stderr);
            let tail_start = stderr
                .char_indices()
                .rev()
                .nth(1999)
                .map(|(i, _)| i)
                .unwrap_or(0);
            let excerpt = &stderr[tail_start..];
            return Err(format!(
                "npm install failed for the cross-repo type-check package — type checking \
                 cannot run. Set CARRICK_SKIP_NPM_INSTALL=1 to bypass (type checking will \
                 be skipped). npm stderr (tail):\n{}",
                excerpt
            )
            .into());
        }
        debug!("Dependencies installed successfully");
    }

    // Create tsconfig.json with dynamic path mappings based on actual type files
    let tsconfig_path = output_dir.join("tsconfig.json");
    let tsconfig_content = create_dynamic_tsconfig(output_dir);

    std::fs::write(
        &tsconfig_path,
        serde_json::to_string_pretty(&tsconfig_content)?,
    )?;
    debug!("Recreated tsconfig.json at {}", tsconfig_path.display());

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::analyzer::ApiEndpointDetails;
    use crate::cloud_storage::TypeEvidence;
    use crate::services::type_sidecar::{InferredType, SourceLocation};
    use crate::visitor::{OwnerType, TypeReference};
    use std::path::PathBuf;

    #[test]
    fn test_ast_stripping_removes_nodes() {
        // Create test CloudRepoData with AST nodes
        let endpoint = ApiEndpointDetails {
            owner: Some(OwnerType::App("test_app".to_string())),
            key: OperationKey::http("GET", "/test"),
            params: vec![],
            request_body: None,
            response_body: None,
            file_path: PathBuf::from("test.js"),
            request_type: Some(TypeReference {
                file_path: PathBuf::from("test.ts"),
                type_ann: None,
                start_position: 0,
                composite_type_string: "TestType".to_string(),
                alias: "TestType".to_string(),
            }),
            response_type: Some(TypeReference {
                file_path: PathBuf::from("test.ts"),
                type_ann: None,
                start_position: 0,
                composite_type_string: "ResponseType".to_string(),
                alias: "ResponseType".to_string(),
            }),
            handler_name: Some("testHandler".to_string()),
            repo_name: None,
            service_name: None,
        };

        let test_data = CloudRepoData {
            repo_name: "express-single".to_string(),
            service_name: None,
            endpoints: vec![endpoint.clone()],
            calls: vec![endpoint.clone()],
            mounts: vec![],
            apps: std::collections::HashMap::new(),
            imported_handlers: vec![],
            function_definitions: std::collections::HashMap::new(),
            config_json: None,
            package_json: None,
            packages: None,
            last_updated: chrono::Utc::now(),
            commit_hash: "test-hash".to_string(),
            mount_graph: None,
            bundled_types: None,
            type_manifest: None,
            file_results: None,
            cached_detection: None,
            cached_guidance: None,
            cached_extraction_config: None,
            package_json_hash: None,
            cache_version: None,
            type_extraction_status: None,
        };

        // Verify strip_ast_nodes removes AST nodes
        let stripped = strip_ast_nodes(test_data);

        assert!(stripped.endpoints[0].request_type.is_none());
        assert!(stripped.endpoints[0].response_type.is_none());
        assert!(stripped.calls[0].request_type.is_none());
        assert!(stripped.calls[0].response_type.is_none());
    }

    #[test]
    fn test_merge_serialized_data() {
        use crate::config::Config;
        use crate::packages::Packages;

        let test_data = vec![CloudRepoData {
            repo_name: "express-single".to_string(),
            service_name: None,
            endpoints: vec![],
            calls: vec![],
            mounts: vec![],
            apps: std::collections::HashMap::new(),
            imported_handlers: vec![],
            function_definitions: std::collections::HashMap::new(),
            config_json: None,
            package_json: None,
            packages: None,
            last_updated: chrono::Utc::now(),
            commit_hash: "test-hash".to_string(),
            mount_graph: None,
            bundled_types: None,
            type_manifest: None,
            file_results: None,
            cached_detection: None,
            cached_guidance: None,
            cached_extraction_config: None,
            package_json_hash: None,
            cache_version: None,
            type_extraction_status: None,
        }];

        // Test Config merging
        let merged_config: Result<Config, _> =
            merge_serialized_data(&test_data, |data| data.config_json.as_ref());
        assert!(merged_config.is_ok());

        // Test Packages merging
        let merged_packages: Result<Packages, _> =
            merge_serialized_data(&test_data, |data| data.package_json.as_ref());
        assert!(merged_packages.is_ok());

        // Test with empty data returns default
        let empty_data: Vec<CloudRepoData> = vec![];
        let default_config: Result<Config, _> =
            merge_serialized_data(&empty_data, |data| data.config_json.as_ref());
        assert!(default_config.is_ok());
    }

    #[tokio::test]
    async fn test_cross_repo_analyzer_builder_no_sourcemap_issues() {
        use crate::analyzer::builder::AnalyzerBuilder;
        use crate::config::Config;
        use swc_common::{SourceMap, sync::Lrc};

        // Create test data with TypeReferences that would cause SourceMap issues
        let endpoint = ApiEndpointDetails {
            owner: Some(OwnerType::App("test_app".to_string())),
            key: OperationKey::http("GET", "/test"),
            params: vec![],
            request_body: None,
            response_body: None,
            file_path: PathBuf::from("test.js"),
            request_type: Some(TypeReference {
                file_path: PathBuf::from("test.ts"),
                type_ann: None,
                start_position: 999999, // This would cause SourceMap issues
                composite_type_string: "TestType".to_string(),
                alias: "TestType".to_string(),
            }),
            response_type: Some(TypeReference {
                file_path: PathBuf::from("test.ts"),
                type_ann: None,
                start_position: 999999, // This would cause SourceMap issues
                composite_type_string: "ResponseType".to_string(),
                alias: "ResponseType".to_string(),
            }),
            handler_name: Some("testHandler".to_string()),
            repo_name: None,
            service_name: None,
        };

        let test_data = vec![CloudRepoData {
            repo_name: "express-single".to_string(),
            service_name: None,
            endpoints: vec![endpoint.clone()],
            calls: vec![endpoint.clone()],
            mounts: vec![],
            apps: std::collections::HashMap::new(),
            imported_handlers: vec![],
            function_definitions: std::collections::HashMap::new(),
            config_json: Some(r#"{"ignore_patterns": [], "type_check": false}"#.to_string()),
            package_json: None,
            packages: None,
            last_updated: chrono::Utc::now(),
            commit_hash: "test-hash".to_string(),
            mount_graph: None,
            bundled_types: None,
            type_manifest: None,
            file_results: None,
            cached_detection: None,
            cached_guidance: None,
            cached_extraction_config: None,
            package_json_hash: None,
            cache_version: None,
            type_extraction_status: None,
        }];

        // Test that cross-repo builder doesn't fail with SourceMap issues
        let cm: Lrc<SourceMap> = Default::default();
        let config = Config::default();
        let builder = AnalyzerBuilder::new_for_cross_repo(config, cm);

        // This should not panic with SourceMap issues
        let result = builder.build_from_repo_data(test_data).await;
        assert!(result.is_ok(), "build_cross_repo_analyzer should not fail");

        let analyzer = result.unwrap();
        assert_eq!(analyzer.endpoints.len(), 1);
        assert_eq!(analyzer.calls.len(), 1);
    }

    // === Incremental analysis tests ===

    use crate::agents::file_analyzer_agent::{
        DataCallResult, EndpointResult, FileAnalysisResult, MountResult,
    };

    fn make_file_result(endpoints: Vec<&str>, data_calls: Vec<&str>) -> FileAnalysisResult {
        FileAnalysisResult {
            mounts: vec![],
            endpoints: endpoints
                .into_iter()
                .map(|path| EndpointResult {
                    candidate_id: "cand_123".to_string(),
                    line_number: 10,
                    owner_node: "app".to_string(),
                    method: "GET".to_string(),
                    path: path.to_string(),
                    handler_name: "handler".to_string(),
                    pattern_matched: "app.get(...)".to_string(),
                    call_expression_span_start: Some(100),
                    call_expression_span_end: Some(200),
                    payload_expression_text: Some("req.body".to_string()),
                    payload_expression_line: Some(11),
                    response_expression_text: Some("res.json(data)".to_string()),
                    response_expression_line: Some(12),
                    emission_style: None,
                    primary_type_symbol: None,
                    type_import_source: None,
                })
                .collect(),
            data_calls: data_calls
                .into_iter()
                .map(|target| DataCallResult {
                    call_kind: None,
                    candidate_id: "cand_456".to_string(),
                    line_number: 20,
                    target: target.to_string(),
                    method: Some("GET".to_string()),
                    pattern_matched: "fetch(...)".to_string(),
                    call_expression_span_start: Some(300),
                    call_expression_span_end: Some(400),
                    call_expression_text: Some("fetch('/api')".to_string()),
                    call_expression_line: Some(21),
                    payload_expression_text: Some("body".to_string()),
                    payload_expression_line: Some(22),
                    primary_type_symbol: None,
                    type_import_source: None,
                })
                .collect(),
        }
    }

    /// Regression for #102: consumer manifest paths must run through the same
    /// UrlNormalizer::normalize as the live mount-graph matcher. The exact
    /// targets from the bad run (backticked template literals with env-var
    /// base URLs) previously surfaced in the consumer manifest as
    /// `/:USER_SERVICE_URL/api/users/:order.userId`, so ts_check reported
    /// orphans the live matcher had already correlated.
    #[test]
    fn test_consumer_manifest_paths_strip_env_var_base_urls() {
        let mut mount_graph = MountGraph::new();
        mount_graph.data_calls = vec![
            crate::mount_graph::DataFetchingCall {
                method: "GET".to_string(),
                target_url: "`${USER_SERVICE_URL}/api/users/${order.userId}`".to_string(),
                client: "fetch".to_string(),
                file_location: "src/orders.ts:42".to_string(),
                call_kind: None,
                repo_name: None,
            },
            crate::mount_graph::DataFetchingCall {
                method: "GET".to_string(),
                target_url: "`${NOTIFICATION_SERVICE_URL}/api/notifications/status`".to_string(),
                client: "fetch".to_string(),
                file_location: "src/notify.ts:7".to_string(),
                call_kind: None,
                repo_name: None,
            },
        ];

        let config = Config::default();
        let entries = build_type_manifest_entries(&mount_graph, &config);

        let consumer_paths: Vec<&str> = entries
            .iter()
            .filter(|e| e.role == ManifestRole::Consumer)
            .filter_map(|e| e.key.as_http().map(|(_, path)| path))
            .collect();
        assert!(!consumer_paths.is_empty(), "consumer entries expected");
        for path in &consumer_paths {
            assert!(
                !path.contains("_SERVICE_URL"),
                "env-var base URL leaked into consumer manifest path: {}",
                path
            );
        }
        // F3c: the member expression `${order.userId}` now collapses to the clean
        // segment `:userId` rather than the malformed `:order.userId`. Param names
        // are matching-agnostic, so this is purely a well-formedness improvement.
        assert!(
            consumer_paths.contains(&"/api/users/:userId"),
            "expected normalized user-service path, got {:?}",
            consumer_paths
        );
        assert!(
            consumer_paths.contains(&"/api/notifications/status"),
            "expected normalized notification path, got {:?}",
            consumer_paths
        );
    }

    #[test]
    fn test_hash_file_content_deterministic() {
        let hash1 = hash_file_content("hello world");
        let hash2 = hash_file_content("hello world");
        assert_eq!(hash1, hash2);

        let hash3 = hash_file_content("different content");
        assert_ne!(hash1, hash3);
    }

    #[test]
    fn test_normalize_file_results_keys_absolute_path() {
        let mut results = HashMap::new();
        results.insert(
            "/home/user/repo/src/app.ts".to_string(),
            make_file_result(vec!["/api/users"], vec![]),
        );
        results.insert(
            "/home/user/repo/src/routes.ts".to_string(),
            make_file_result(vec!["/api/posts"], vec![]),
        );

        let normalized = normalize_file_results_keys(&results, "/home/user/repo");

        assert!(normalized.contains_key("src/app.ts"));
        assert!(normalized.contains_key("src/routes.ts"));
        assert_eq!(normalized.len(), 2);
    }

    #[test]
    fn test_normalize_file_results_keys_dot_prefix() {
        let mut results = HashMap::new();
        results.insert(
            "./src/app.ts".to_string(),
            make_file_result(vec!["/api/users"], vec![]),
        );

        let normalized = normalize_file_results_keys(&results, ".");

        assert!(normalized.contains_key("src/app.ts"));
    }

    #[test]
    fn test_normalize_file_results_keys_already_relative() {
        let mut results = HashMap::new();
        results.insert(
            "src/app.ts".to_string(),
            make_file_result(vec!["/api/users"], vec![]),
        );

        let normalized = normalize_file_results_keys(&results, "/some/other/path");

        // Key doesn't match prefix, should be kept as-is
        assert!(normalized.contains_key("src/app.ts"));
    }

    #[test]
    fn test_strip_diagnostic_fields() {
        let mut results = HashMap::new();
        results.insert(
            "src/app.ts".to_string(),
            make_file_result(vec!["/api/users"], vec!["/api/posts"]),
        );

        // Add a mount with pattern_matched
        results
            .get_mut("src/app.ts")
            .unwrap()
            .mounts
            .push(MountResult {
                line_number: 5,
                parent_node: "app".to_string(),
                child_node: "router".to_string(),
                mount_path: "/api".to_string(),
                import_source: Some("./routes".to_string()),
                pattern_matched: "app.use('/api', router)".to_string(),
            });

        strip_diagnostic_fields(&mut results);

        let result = &results["src/app.ts"];

        // Endpoint diagnostic fields should be cleared
        assert_eq!(result.endpoints[0].candidate_id, "");
        assert_eq!(result.endpoints[0].pattern_matched, "");
        assert!(result.endpoints[0].payload_expression_text.is_none());
        // The response expression text is load-bearing for ReturnValue
        // endpoints on cached replays (its only fallback locator is an
        // inexact line anchor) — it must survive the strip.
        assert_eq!(
            result.endpoints[0].response_expression_text.as_deref(),
            Some("res.json(data)")
        );
        // Non-diagnostic fields should be preserved
        assert_eq!(result.endpoints[0].path, "/api/users");
        assert_eq!(result.endpoints[0].method, "GET");
        assert_eq!(result.endpoints[0].handler_name, "handler");
        assert_eq!(result.endpoints[0].line_number, 10);

        // Data call diagnostic fields should be cleared
        assert_eq!(result.data_calls[0].candidate_id, "");
        assert_eq!(result.data_calls[0].pattern_matched, "");
        assert!(result.data_calls[0].call_expression_text.is_none());
        assert!(result.data_calls[0].payload_expression_text.is_none());
        // Non-diagnostic fields preserved
        assert_eq!(result.data_calls[0].target, "/api/posts");

        // Mount diagnostic fields should be cleared
        assert_eq!(result.mounts[0].pattern_matched, "");
        // Non-diagnostic fields preserved
        assert_eq!(result.mounts[0].mount_path, "/api");
        assert_eq!(result.mounts[0].parent_node, "app");
    }

    #[test]
    fn test_get_changed_files_with_real_git_repo() {
        use std::process::Command;

        let temp_dir = tempfile::TempDir::new().unwrap();
        let repo_path = temp_dir.path().to_str().unwrap();

        // Init a git repo. Clear git env vars so the commands are scoped to
        // repo_path rather than any ambient GIT_DIR set by a parent process
        // (e.g. a pre-commit hook running inside a git worktree).
        Command::new("git")
            .args(["init"])
            .current_dir(repo_path)
            .env_remove("GIT_DIR")
            .env_remove("GIT_WORK_TREE")
            .env_remove("GIT_INDEX_FILE")
            .output()
            .unwrap();
        Command::new("git")
            .args(["config", "user.email", "test@test.com"])
            .current_dir(repo_path)
            .env_remove("GIT_DIR")
            .env_remove("GIT_WORK_TREE")
            .env_remove("GIT_INDEX_FILE")
            .output()
            .unwrap();
        Command::new("git")
            .args(["config", "user.name", "Test"])
            .current_dir(repo_path)
            .env_remove("GIT_DIR")
            .env_remove("GIT_WORK_TREE")
            .env_remove("GIT_INDEX_FILE")
            .output()
            .unwrap();

        // Create initial commit with a .ts file
        std::fs::write(temp_dir.path().join("app.ts"), "const x = 1;").unwrap();
        std::fs::write(temp_dir.path().join("readme.md"), "# Readme").unwrap();
        Command::new("git")
            .args(["add", "."])
            .current_dir(repo_path)
            .env_remove("GIT_DIR")
            .env_remove("GIT_WORK_TREE")
            .env_remove("GIT_INDEX_FILE")
            .output()
            .unwrap();
        Command::new("git")
            .args(["commit", "-m", "initial"])
            .current_dir(repo_path)
            .env_remove("GIT_DIR")
            .env_remove("GIT_WORK_TREE")
            .env_remove("GIT_INDEX_FILE")
            .output()
            .unwrap();

        // Get the first commit hash
        let base_hash = String::from_utf8(
            Command::new("git")
                .args(["rev-parse", "HEAD"])
                .current_dir(repo_path)
                .env_remove("GIT_DIR")
                .env_remove("GIT_WORK_TREE")
                .env_remove("GIT_INDEX_FILE")
                .output()
                .unwrap()
                .stdout,
        )
        .unwrap()
        .trim()
        .to_string();

        // Make changes: modify .ts, add new .tsx, modify .md (should be filtered)
        std::fs::write(temp_dir.path().join("app.ts"), "const x = 2;").unwrap();
        std::fs::write(
            temp_dir.path().join("new.tsx"),
            "export default () => <div/>;",
        )
        .unwrap();
        std::fs::write(temp_dir.path().join("readme.md"), "# Updated").unwrap();
        Command::new("git")
            .args(["add", "."])
            .current_dir(repo_path)
            .env_remove("GIT_DIR")
            .env_remove("GIT_WORK_TREE")
            .env_remove("GIT_INDEX_FILE")
            .output()
            .unwrap();
        Command::new("git")
            .args(["commit", "-m", "changes"])
            .current_dir(repo_path)
            .env_remove("GIT_DIR")
            .env_remove("GIT_WORK_TREE")
            .env_remove("GIT_INDEX_FILE")
            .output()
            .unwrap();

        // Test get_changed_files
        let changed = get_changed_files(repo_path, &base_hash);
        assert!(changed.is_some());

        let changed = changed.unwrap();
        assert!(changed.contains(&"app.ts".to_string()));
        assert!(changed.contains(&"new.tsx".to_string()));
        // .md file should be filtered out
        assert!(!changed.contains(&"readme.md".to_string()));
    }

    #[test]
    fn test_get_changed_files_returns_none_for_invalid_commit() {
        use std::process::Command;

        let temp_dir = tempfile::TempDir::new().unwrap();
        let repo_path = temp_dir.path().to_str().unwrap();

        Command::new("git")
            .args(["init"])
            .current_dir(repo_path)
            .env_remove("GIT_DIR")
            .env_remove("GIT_WORK_TREE")
            .env_remove("GIT_INDEX_FILE")
            .output()
            .unwrap();
        Command::new("git")
            .args(["config", "user.email", "test@test.com"])
            .current_dir(repo_path)
            .env_remove("GIT_DIR")
            .env_remove("GIT_WORK_TREE")
            .env_remove("GIT_INDEX_FILE")
            .output()
            .unwrap();
        Command::new("git")
            .args(["config", "user.name", "Test"])
            .current_dir(repo_path)
            .env_remove("GIT_DIR")
            .env_remove("GIT_WORK_TREE")
            .env_remove("GIT_INDEX_FILE")
            .output()
            .unwrap();
        std::fs::write(temp_dir.path().join("app.ts"), "x").unwrap();
        Command::new("git")
            .args(["add", "."])
            .current_dir(repo_path)
            .env_remove("GIT_DIR")
            .env_remove("GIT_WORK_TREE")
            .env_remove("GIT_INDEX_FILE")
            .output()
            .unwrap();
        Command::new("git")
            .args(["commit", "-m", "init"])
            .current_dir(repo_path)
            .env_remove("GIT_DIR")
            .env_remove("GIT_WORK_TREE")
            .env_remove("GIT_INDEX_FILE")
            .output()
            .unwrap();

        // Non-existent commit hash → should return None (simulates shallow clone)
        let result = get_changed_files(repo_path, "0000000000000000000000000000000000000000");
        assert!(result.is_none());
    }

    #[test]
    fn test_payload_size_guard_drops_file_results_when_too_large() {
        // Create CloudRepoData with large file_results
        let mut large_results = HashMap::new();
        // Create entries large enough to exceed 5MB
        for i in 0..1000 {
            let large_string = "x".repeat(5000);
            large_results.insert(
                format!("src/file_{}.ts", i),
                FileAnalysisResult {
                    mounts: vec![],
                    endpoints: vec![EndpointResult {
                        candidate_id: large_string.clone(),
                        line_number: 1,
                        owner_node: "app".to_string(),
                        method: "GET".to_string(),
                        path: large_string.clone(),
                        handler_name: large_string.clone(),
                        pattern_matched: large_string.clone(),
                        call_expression_span_start: None,
                        call_expression_span_end: None,
                        payload_expression_text: Some(large_string.clone()),
                        payload_expression_line: None,
                        response_expression_text: Some(large_string),
                        response_expression_line: None,
                        emission_style: None,
                        primary_type_symbol: None,
                        type_import_source: None,
                    }],
                    data_calls: vec![],
                },
            );
        }

        let data = CloudRepoData {
            repo_name: "express-single".to_string(),
            service_name: None,
            endpoints: vec![],
            calls: vec![],
            mounts: vec![],
            apps: HashMap::new(),
            imported_handlers: vec![],
            function_definitions: HashMap::new(),
            config_json: None,
            package_json: None,
            packages: None,
            last_updated: chrono::Utc::now(),
            commit_hash: "test-hash".to_string(),
            mount_graph: None,
            bundled_types: None,
            type_manifest: None,
            file_results: Some(large_results),
            cached_detection: None,
            cached_guidance: None,
            cached_extraction_config: None,
            package_json_hash: None,
            cache_version: Some(CACHE_VERSION),
            type_extraction_status: None,
        };

        let stripped = strip_ast_nodes(data);

        // file_results should be dropped because payload exceeds 5MB
        assert!(
            stripped.file_results.is_none(),
            "file_results should be dropped when payload exceeds 5MB"
        );
    }

    #[test]
    fn test_payload_size_guard_keeps_small_file_results() {
        let mut small_results = HashMap::new();
        small_results.insert(
            "src/app.ts".to_string(),
            make_file_result(vec!["/api/users"], vec![]),
        );

        let data = CloudRepoData {
            repo_name: "express-single".to_string(),
            service_name: None,
            endpoints: vec![],
            calls: vec![],
            mounts: vec![],
            apps: HashMap::new(),
            imported_handlers: vec![],
            function_definitions: HashMap::new(),
            config_json: None,
            package_json: None,
            packages: None,
            last_updated: chrono::Utc::now(),
            commit_hash: "test-hash".to_string(),
            mount_graph: None,
            bundled_types: None,
            type_manifest: None,
            file_results: Some(small_results),
            cached_detection: None,
            cached_guidance: None,
            cached_extraction_config: None,
            package_json_hash: None,
            cache_version: Some(CACHE_VERSION),
            type_extraction_status: None,
        };

        let stripped = strip_ast_nodes(data);

        // file_results should be preserved (small payload)
        assert!(
            stripped.file_results.is_some(),
            "file_results should be preserved when payload is small"
        );
    }

    #[test]
    fn test_file_results_merge_handles_deletes_and_additions() {
        // Simulate: previous run had files A, B, C
        // Current run discovers A, B, D (C deleted, D new)
        // Git diff says B changed
        let mut prev_results = HashMap::new();
        prev_results.insert(
            "src/a.ts".to_string(),
            make_file_result(vec!["/api/a"], vec![]),
        );
        prev_results.insert(
            "src/b.ts".to_string(),
            make_file_result(vec!["/api/b"], vec![]),
        );
        prev_results.insert(
            "src/c.ts".to_string(),
            make_file_result(vec!["/api/c"], vec![]),
        );

        // Current file discovery
        let current_file_set: HashSet<String> = ["src/a.ts", "src/b.ts", "src/d.ts"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        let changed_set: HashSet<String> = ["src/b.ts"].iter().map(|s| s.to_string()).collect();

        // New results from analyzing changed + new files
        let mut new_results = HashMap::new();
        new_results.insert(
            "src/b.ts".to_string(),
            make_file_result(vec!["/api/b_v2"], vec![]),
        );
        new_results.insert(
            "src/d.ts".to_string(),
            make_file_result(vec!["/api/d"], vec![]),
        );

        // Merge logic (mirrors analyze_current_repo_incremental)
        let mut merged: HashMap<String, FileAnalysisResult> = HashMap::new();

        // Copy cached results for unchanged files that still exist
        for (path, result) in &prev_results {
            if current_file_set.contains(path) && !changed_set.contains(path) {
                merged.insert(path.clone(), result.clone());
            }
        }

        // Insert new/changed results
        for (path, result) in new_results {
            merged.insert(path, result);
        }

        // Verify merge result
        assert_eq!(
            merged.len(),
            3,
            "Should have A (cached), B (fresh), D (new)"
        );
        assert!(merged.contains_key("src/a.ts"), "A should be cached");
        assert!(merged.contains_key("src/b.ts"), "B should be fresh");
        assert!(merged.contains_key("src/d.ts"), "D should be new");
        assert!(!merged.contains_key("src/c.ts"), "C should be deleted");

        // A should have old data
        assert_eq!(merged["src/a.ts"].endpoints[0].path, "/api/a");
        // B should have new data
        assert_eq!(merged["src/b.ts"].endpoints[0].path, "/api/b_v2");
        // D should have new data
        assert_eq!(merged["src/d.ts"].endpoints[0].path, "/api/d");
    }

    #[test]
    fn test_file_results_serialization_roundtrip() {
        // Verify FileAnalysisResult survives JSON serialization (critical for AWS cache)
        let result = make_file_result(vec!["/api/users", "/api/posts"], vec!["/external/api"]);

        let json = serde_json::to_string(&result).expect("should serialize");
        let deserialized: FileAnalysisResult =
            serde_json::from_str(&json).expect("should deserialize");

        assert_eq!(deserialized.endpoints.len(), 2);
        assert_eq!(deserialized.data_calls.len(), 1);
        assert_eq!(deserialized.endpoints[0].path, "/api/users");
        assert_eq!(deserialized.data_calls[0].target, "/external/api");
    }

    #[test]
    fn test_cloud_repo_data_with_file_results_roundtrip() {
        // Verify CloudRepoData with file_results survives JSON roundtrip
        let mut file_results = HashMap::new();
        file_results.insert(
            "src/app.ts".to_string(),
            make_file_result(vec!["/api/users"], vec![]),
        );

        let data = CloudRepoData {
            repo_name: "express-single".to_string(),
            service_name: None,
            endpoints: vec![],
            calls: vec![],
            mounts: vec![],
            apps: HashMap::new(),
            imported_handlers: vec![],
            function_definitions: HashMap::new(),
            config_json: None,
            package_json: None,
            packages: None,
            last_updated: chrono::Utc::now(),
            commit_hash: "abc123".to_string(),
            mount_graph: None,
            bundled_types: None,
            type_manifest: None,
            file_results: Some(file_results),
            cached_detection: Some(DetectionResult {
                frameworks: vec!["express".to_string()],
                data_fetchers: vec!["fetch".to_string()],
                notes: "test".to_string(),
            }),
            cached_guidance: None,
            cached_extraction_config: None,
            package_json_hash: Some("abc123hash".to_string()),
            cache_version: Some(CACHE_VERSION),
            type_extraction_status: None,
        };

        let json = serde_json::to_string(&data).expect("should serialize");
        let deserialized: CloudRepoData = serde_json::from_str(&json).expect("should deserialize");

        assert!(deserialized.file_results.is_some());
        let fr = deserialized.file_results.unwrap();
        assert!(fr.contains_key("src/app.ts"));
        assert_eq!(fr["src/app.ts"].endpoints[0].path, "/api/users");

        assert!(deserialized.cached_detection.is_some());
        assert_eq!(
            deserialized.cached_detection.unwrap().frameworks,
            vec!["express"]
        );
        assert_eq!(deserialized.cache_version, Some(CACHE_VERSION));
        assert_eq!(
            deserialized.package_json_hash,
            Some("abc123hash".to_string())
        );
    }

    #[test]
    fn test_function_definition_intent_hash_roundtrips() {
        // The content-hash cache only works if `intent` and `intent_input_hash`
        // survive the upload/download JSON round-trip. A silently-dropped hash
        // would turn every incremental scan into a full cache miss.
        let mut function_definitions = HashMap::new();
        function_definitions.insert(
            "getUser".to_string(),
            crate::visitor::FunctionDefinition {
                name: "getUser".to_string(),
                file_path: "src/users.ts".into(),
                node_type: Default::default(),
                arguments: vec![],
                body_source: None, // stripped before upload
                is_exported: true,
                line_number: 1,
                intent: Some("fetches a user by id".to_string()),
                calls: vec![],
                return_type: None,
                return_is_explicit: false,
                signature: None,
                intent_input_hash: Some("deadbeef".to_string()),
            },
        );

        let data = CloudRepoData {
            repo_name: "svc".to_string(),
            service_name: None,
            endpoints: vec![],
            calls: vec![],
            mounts: vec![],
            apps: HashMap::new(),
            imported_handlers: vec![],
            function_definitions,
            config_json: None,
            package_json: None,
            packages: None,
            last_updated: chrono::Utc::now(),
            commit_hash: "abc123".to_string(),
            mount_graph: None,
            bundled_types: None,
            type_manifest: None,
            file_results: None,
            cached_detection: None,
            cached_guidance: None,
            cached_extraction_config: None,
            package_json_hash: None,
            cache_version: Some(CACHE_VERSION),
            type_extraction_status: None,
        };

        let json = serde_json::to_string(&data).expect("should serialize");
        let deserialized: CloudRepoData = serde_json::from_str(&json).expect("should deserialize");

        let def = &deserialized.function_definitions["getUser"];
        assert_eq!(def.intent.as_deref(), Some("fetches a user by id"));
        assert_eq!(def.intent_input_hash.as_deref(), Some("deadbeef"));

        // And the map feeding the cache is rebuilt correctly from that blob.
        let by_hash = crate::intent_generator::intents_by_hash(&deserialized.function_definitions);
        assert_eq!(
            by_hash.get("deadbeef").map(String::as_str),
            Some("fetches a user by id")
        );
    }

    #[test]
    fn test_cloud_repo_data_without_cache_fields_deserializes() {
        // Old CloudRepoData without cache fields should still deserialize (backwards compat)
        let json = r#"{
            "repo_name": "old-repo",
            "endpoints": [],
            "calls": [],
            "mounts": [],
            "apps": {},
            "imported_handlers": [],
            "function_definitions": {},
            "last_updated": "2025-01-01T00:00:00Z",
            "commit_hash": "old123"
        }"#;

        let data: CloudRepoData =
            serde_json::from_str(json).expect("should deserialize old format");
        assert_eq!(data.repo_name, "old-repo");
        assert!(data.file_results.is_none());
        assert!(data.cached_detection.is_none());
        assert!(data.cached_guidance.is_none());
        assert!(data.package_json_hash.is_none());
        assert!(data.cache_version.is_none());
    }

    #[test]
    fn resolve_services_without_config_defaults_to_single_service() {
        let tmp = tempfile::tempdir().unwrap();
        let services = resolve_services(tmp.path().to_str().unwrap()).unwrap();
        assert_eq!(services.len(), 1);
        assert!(services[0].directory.is_none());
    }

    #[test]
    fn resolve_services_rejects_malformed_config() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("carrick.json"), "{ not valid json").unwrap();

        let err = resolve_services(tmp.path().to_str().unwrap()).unwrap_err();
        assert!(
            err.to_string().contains("Failed to parse"),
            "expected parse error, got: {err}"
        );
    }

    #[test]
    fn resolve_services_rejects_missing_service_directory() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(
            tmp.path().join("carrick.json"),
            r#"{ "services": [{ "name": "api", "directory": "does-not-exist" }] }"#,
        )
        .unwrap();

        let err = resolve_services(tmp.path().to_str().unwrap()).unwrap_err();
        assert!(
            err.to_string().contains("does-not-exist")
                && err.to_string().contains("does not exist"),
            "expected missing-directory error, got: {err}"
        );
    }

    #[test]
    fn resolve_services_rejects_missing_include_path() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir(tmp.path().join("svc")).unwrap();
        std::fs::write(
            tmp.path().join("carrick.json"),
            r#"{ "services": [{ "name": "api", "directory": "svc", "include": ["shared"] }] }"#,
        )
        .unwrap();

        let err = resolve_services(tmp.path().to_str().unwrap()).unwrap_err();
        assert!(
            err.to_string().contains("include path 'shared'"),
            "expected missing-include error, got: {err}"
        );
    }

    #[test]
    fn resolve_services_accepts_valid_service_paths() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir(tmp.path().join("svc")).unwrap();
        std::fs::create_dir(tmp.path().join("shared")).unwrap();
        std::fs::write(
            tmp.path().join("carrick.json"),
            r#"{ "services": [{ "name": "api", "directory": "svc", "include": ["shared"] }] }"#,
        )
        .unwrap();

        let services = resolve_services(tmp.path().to_str().unwrap()).unwrap();
        assert_eq!(services.len(), 1);
        assert_eq!(services[0].directory.as_deref(), Some("svc"));
    }

    #[test]
    fn load_packages_rejects_malformed_package_json() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("package.json"), "{ trailing-comma: ,}").unwrap();

        let err = load_packages_for_service(tmp.path().to_str().unwrap(), &Config::default())
            .unwrap_err();
        assert!(
            err.to_string().contains("Failed to parse"),
            "expected parse error, got: {err}"
        );
    }

    #[test]
    fn load_packages_missing_package_json_defaults_to_empty() {
        let tmp = tempfile::tempdir().unwrap();
        let packages =
            load_packages_for_service(tmp.path().to_str().unwrap(), &Config::default()).unwrap();
        assert!(packages.merged_dependencies.is_empty());
    }

    #[test]
    fn discovery_rejects_service_with_no_source_files() {
        let tmp = tempfile::tempdir().unwrap();
        let cm: Lrc<SourceMap> = Default::default();

        let err = discover_files_and_symbols(tmp.path().to_str().unwrap(), &Config::default(), cm)
            .unwrap_err();
        assert!(
            err.to_string().contains("No JS/TS source files"),
            "expected empty-scan error, got: {err}"
        );
    }

    // -----------------------------------------------------------------------
    // Type-state enrichment / placeholder handling (#235)
    // -----------------------------------------------------------------------

    fn consumer_entry(type_alias: &str) -> TypeManifestEntry {
        let evidence = TypeEvidence {
            file_path: "lib/api.ts".to_string(),
            span_start: None,
            span_end: None,
            line_number: 5,
            infer_kind: InferKind::CallResult,
            is_explicit: false,
            type_state: ManifestTypeState::Unknown,
        };
        TypeManifestEntry {
            key: OperationKey::http("GET", "/orders/:id"),
            role: ManifestRole::Consumer,
            type_kind: ManifestTypeKind::Response,
            type_alias: type_alias.to_string(),
            file_path: "lib/api.ts".to_string(),
            line_number: 5,
            is_explicit: false,
            type_state: ManifestTypeState::Unknown,
            evidence,
            resolved_definition: None,
            expanded_definition: None,
            primary_type_symbol: None,
        }
    }

    fn empty_resolution() -> TypeResolutionResult {
        TypeResolutionResult {
            dts_content: None,
            explicit_manifest: vec![],
            inferred_types: vec![],
            symbol_failures: vec![],
            errors: vec![],
        }
    }

    /// A genuine shape resolved by the sidecar promotes the entry to Implicit.
    #[test]
    fn enrich_promotes_resolved_consumer_shape() {
        let mut manifest = vec![consumer_entry("OrderView")];
        let mut resolution = empty_resolution();
        resolution.inferred_types.push(InferredType {
            alias: "OrderView".to_string(),
            type_string: "{ id: string; currency: string }".to_string(),
            is_explicit: false,
            source_location: SourceLocation {
                file_path: "lib/api.ts".to_string(),
                start_line: 5,
                end_line: 5,
                start_column: None,
                end_column: None,
            },
            infer_kind: InferKind::CallResult,
        });

        enrich_manifest_with_type_resolution(&mut manifest, &resolution, None);

        assert_eq!(manifest[0].type_state, ManifestTypeState::Implicit);
    }

    /// A consumer alias that only resolves to `unknown` must stay `Unknown` so
    /// the placeholder gate stays shut and the edge reads unverifiable, not
    /// compatible (#235).
    #[test]
    fn enrich_keeps_unknown_resolution_unknown() {
        let mut manifest = vec![consumer_entry("OrderView")];
        let mut resolution = empty_resolution();
        resolution.inferred_types.push(InferredType {
            alias: "OrderView".to_string(),
            type_string: "unknown".to_string(),
            is_explicit: false,
            source_location: SourceLocation {
                file_path: "lib/api.ts".to_string(),
                start_line: 5,
                end_line: 5,
                start_column: None,
                end_column: None,
            },
            infer_kind: InferKind::CallResult,
        });

        enrich_manifest_with_type_resolution(&mut manifest, &resolution, None);

        assert_eq!(manifest[0].type_state, ManifestTypeState::Unknown);
    }

    /// The Carrick-injected `= unknown` placeholder (carrying the marker) in the
    /// bundled .d.ts must NOT promote the entry, even if the bundle nominally
    /// "defines" the alias — it is downgraded to `Unknown` (#235).
    #[test]
    fn enrich_downgrades_trivially_unknown_dts_alias() {
        let mut manifest = vec![consumer_entry("OrderView")];
        let resolution = empty_resolution();
        let dts = format!("export type OrderView = unknown; {MISSING_ALIAS_MARKER}\n");

        enrich_manifest_with_type_resolution(&mut manifest, &resolution, Some(&dts));

        assert_eq!(manifest[0].type_state, ManifestTypeState::Unknown);
    }

    /// A *developer-authored* `type X = unknown` in a real API type carries no
    /// Carrick marker and must NOT be mistaken for the injected placeholder, so
    /// the entry keeps its state rather than being downgraded to `Unknown`
    /// (#244). The genuine `unknown` shape is still surfaced as unverifiable
    /// downstream by ts_check's compiler-level `isUnknown()` gate, not by a
    /// silent recall-losing downgrade here.
    #[test]
    fn enrich_does_not_downgrade_developer_authored_unknown() {
        // No resolution entry and no marker: the alias is "defined" in the
        // bundle as a genuine `= unknown`, so dts_defined_aliases promotes it to
        // Implicit and the trivially-unknown gate stays shut.
        let mut manifest = vec![consumer_entry("OrderView")];
        let resolution = empty_resolution();
        let bare = "export type OrderView = unknown;\n";

        enrich_manifest_with_type_resolution(&mut manifest, &resolution, Some(bare));

        assert_ne!(
            manifest[0].type_state,
            ManifestTypeState::Unknown,
            "a developer-authored `type X = unknown` must not be downgraded to Unknown"
        );

        // And other forms a developer might write are equally not the marker.
        for form in [
            "export type OrderView = unknown;\n",
            "type OrderView = unknown;\n",
            "export type OrderView<T> = unknown;\n",
            "export declare type OrderView = unknown;\n",
            "export type OrderView = unknown; // genuinely unknown\n",
        ] {
            assert!(
                !dts_alias_is_trivially_unknown(form, "OrderView"),
                "developer-authored form must not match the placeholder marker: {form:?}"
            );
        }

        // The tagged placeholder, in the form append_missing_aliases emits, does.
        let tagged = format!("export type OrderView = unknown; {MISSING_ALIAS_MARKER}\n");
        assert!(
            dts_alias_is_trivially_unknown(&tagged, "OrderView"),
            "the Carrick-injected marker form must match the placeholder gate"
        );
    }

    /// append_missing_aliases injects a `= unknown` placeholder for a manifest
    /// alias absent from the bundle, and leaves an already-defined alias alone.
    #[test]
    fn append_missing_aliases_injects_unknown_placeholder() {
        let manifest = vec![consumer_entry("OrderView"), consumer_entry("Payment")];
        let dts = "export interface Payment { id: string }\n".to_string();

        let out = append_missing_aliases(dts, Some(&manifest));

        assert!(
            out.contains(&format!(
                "export type OrderView = unknown; {MISSING_ALIAS_MARKER}"
            )),
            "missing alias should be injected as a marked placeholder, got: {out}"
        );
        assert!(
            !out.contains("export type Payment = unknown"),
            "an already-defined alias must not be overwritten, got: {out}"
        );
        // The injected placeholder must be recognised as the Carrick placeholder.
        assert!(
            dts_alias_is_trivially_unknown(&out, "OrderView"),
            "the injected marker must be detected by the placeholder gate, got: {out}"
        );
    }

    // ---- #245 Phase 1: protocol op manifest entries -------------------------

    fn socket_op(
        event: &str,
        direction: crate::operation::SocketDirection,
        symbol: Option<&str>,
        source: Option<&str>,
    ) -> crate::socket_io::SocketOp {
        crate::socket_io::SocketOp {
            key: OperationKey::socket(event, direction),
            file_path: PathBuf::from("src/socket.ts"),
            line: 12,
            payload_type_symbol: symbol.map(String::from),
            payload_type_source: source.map(String::from),
        }
    }

    fn graphql_op(
        kind: crate::operation::GraphqlOperationKind,
        field: &str,
    ) -> crate::graphql::GraphqlOp {
        crate::graphql::GraphqlOp {
            key: OperationKey::graphql(kind, field),
            file_path: PathBuf::from("src/schema.graphql"),
            line: 3,
        }
    }

    /// A typed socket emitter produces a Response-kind manifest entry keyed by
    /// the socket OperationKey, carrying the captured payload symbol as the
    /// anchor. GraphQL ops get an entry but no anchor (deferred to #248).
    #[test]
    fn protocol_manifest_entries_anchor_sockets_not_graphql() {
        use crate::operation::{GraphqlOperationKind, SocketDirection};

        let extractions = ProtocolExtractions {
            graphql: crate::graphql::GraphqlExtraction {
                producers: vec![graphql_op(GraphqlOperationKind::Query, "order")],
                consumers: vec![],
            },
            sockets: crate::socket_io::SocketExtraction {
                listeners: vec![],
                emitters: vec![socket_op(
                    "payment:settled",
                    SocketDirection::ServerToClient,
                    Some("Payment"),
                    Some("./types/payment"),
                )],
            },
        };

        let mut entries = Vec::new();
        append_protocol_manifest_entries(&mut entries, &extractions);

        let socket_entry = entries
            .iter()
            .find(|e| e.key.canonical() == "socket|SERVER->CLIENT|payment:settled")
            .expect("socket manifest entry");
        assert_eq!(socket_entry.role, ManifestRole::Consumer);
        assert_eq!(socket_entry.type_kind, ManifestTypeKind::Response);
        assert_eq!(socket_entry.primary_type_symbol.as_deref(), Some("Payment"));
        // One entry per op — no phantom Request alias.
        assert_eq!(
            entries
                .iter()
                .filter(|e| e.key.canonical() == "socket|SERVER->CLIENT|payment:settled")
                .count(),
            1
        );

        let graphql_entry = entries
            .iter()
            .find(|e| e.key.canonical() == "graphql|query|order")
            .expect("graphql manifest entry");
        assert_eq!(graphql_entry.role, ManifestRole::Producer);
        // Plumbing only: the entry exists (stable alias + projection), but the
        // anchor is deferred to #248.
        assert_eq!(graphql_entry.primary_type_symbol, None);
        assert!(
            !graphql_entry.type_alias.is_empty(),
            "graphql op must get a stable type_alias"
        );
        assert_eq!(graphql_entry.type_state, ManifestTypeState::Unknown);
    }

    /// The fragile contract the whole anchor join hinges on: the alias on the
    /// socket manifest entry MUST equal the alias on the SymbolRequest, both
    /// computed by `build_manifest_type_alias(key, role, Response)`. If they
    /// ever diverge the resolved `.d.ts` never joins back and the entry stays
    /// `Unknown` — silently. (Plan test #3.)
    #[test]
    fn socket_symbol_request_alias_matches_manifest_alias() {
        use crate::operation::SocketDirection;

        let emitter = socket_op(
            "payment:settled",
            SocketDirection::ServerToClient,
            Some("Payment"),
            Some("./types/payment"),
        );
        let extractions = ProtocolExtractions {
            graphql: crate::graphql::GraphqlExtraction::default(),
            sockets: crate::socket_io::SocketExtraction {
                listeners: vec![],
                emitters: vec![emitter.clone()],
            },
        };

        let mut entries = Vec::new();
        append_protocol_manifest_entries(&mut entries, &extractions);
        let manifest_alias = entries
            .iter()
            .find(|e| e.key.canonical() == "socket|SERVER->CLIENT|payment:settled")
            .map(|e| e.type_alias.clone())
            .expect("socket manifest entry");

        let orchestrator = FileOrchestrator::new(AgentService::new());
        let requests = orchestrator.collect_socket_type_requests(&extractions.sockets, ".");
        let request = requests
            .iter()
            .find(|r| r.symbol_name == "Payment")
            .expect("socket SymbolRequest");

        assert_eq!(
            request.alias.as_deref(),
            Some(manifest_alias.as_str()),
            "SymbolRequest.alias must byte-match the manifest entry's alias \
             (both build_manifest_type_alias(key, Consumer, Response)) or the \
             enrich-join silently breaks"
        );
        // Independently confirm both equal the canonical builder output.
        let expected = crate::type_manifest::build_manifest_type_alias(
            &emitter.key,
            ManifestRole::Consumer,
            ManifestTypeKind::Response,
        );
        assert_eq!(manifest_alias, expected);
        assert_eq!(request.alias.as_deref(), Some(expected.as_str()));
    }

    /// #253 regression: `write_manifest_files` feeds the HTTP-only ts_check
    /// matcher, so it must emit ONLY HTTP entries. GraphQL/socket entries are
    /// kept in `cloud_data.type_manifest` (cloud index + eval projection) but
    /// must never reach producer/consumer manifest files — a single non-HTTP
    /// entry there makes ts_check's `validateEntry` throw and zeros out every
    /// cross-repo verdict.
    #[test]
    fn write_manifest_files_emits_only_http_entries() {
        use crate::cloud_storage::TypeEvidence;
        use crate::operation::{GraphqlOperationKind, OperationKey, SocketDirection};
        use crate::services::type_sidecar::InferKind;

        fn entry(key: OperationKey, alias: &str) -> TypeManifestEntry {
            TypeManifestEntry {
                key,
                role: ManifestRole::Producer,
                type_kind: ManifestTypeKind::Response,
                type_alias: alias.to_string(),
                file_path: "src/x.ts".to_string(),
                line_number: 1,
                is_explicit: true,
                type_state: ManifestTypeState::Explicit,
                evidence: TypeEvidence {
                    file_path: "src/x.ts".to_string(),
                    span_start: None,
                    span_end: None,
                    line_number: 1,
                    infer_kind: InferKind::ResponseBody,
                    is_explicit: true,
                    type_state: ManifestTypeState::Explicit,
                },
                resolved_definition: None,
                expanded_definition: None,
                primary_type_symbol: None,
            }
        }

        let manifest = vec![
            entry(OperationKey::http("GET", "/orders/:id"), "OrderResponse"),
            entry(
                OperationKey::graphql(GraphqlOperationKind::Query, "order"),
                "OrderQueryResult",
            ),
            entry(
                OperationKey::socket("order.created", SocketDirection::ServerToClient),
                "OrderCreatedEvent",
            ),
        ];

        let repo_data = CloudRepoData {
            repo_name: "orders-svc".to_string(),
            service_name: None,
            endpoints: vec![],
            calls: vec![],
            mounts: vec![],
            apps: std::collections::HashMap::new(),
            imported_handlers: vec![],
            function_definitions: std::collections::HashMap::new(),
            config_json: None,
            package_json: None,
            packages: None,
            last_updated: chrono::Utc::now(),
            commit_hash: "test-hash".to_string(),
            mount_graph: None,
            bundled_types: None,
            type_manifest: Some(manifest),
            file_results: None,
            cached_detection: None,
            cached_guidance: None,
            cached_extraction_config: None,
            package_json_hash: None,
            cache_version: None,
            type_extraction_status: None,
        };

        // Local mirror of TypeManifestFile for reading back the written JSON
        // (the production struct is serialize-only).
        #[derive(serde::Deserialize)]
        struct ManifestFileForTest {
            entries: Vec<TypeManifestEntry>,
        }

        let dir = tempfile::tempdir().expect("tempdir");
        write_manifest_files(std::slice::from_ref(&repo_data), dir.path())
            .expect("write_manifest_files");

        let producer: ManifestFileForTest = serde_json::from_str(
            &std::fs::read_to_string(dir.path().join("producer-manifest.json")).unwrap(),
        )
        .unwrap();

        // Only the HTTP producer survives into the ts_check manifest.
        assert_eq!(
            producer.entries.len(),
            1,
            "non-HTTP entries must be filtered out of the ts_check manifest"
        );
        assert_eq!(
            producer.entries[0].key.protocol(),
            crate::operation::Protocol::Http
        );
        assert!(
            producer
                .entries
                .iter()
                .all(|e| e.key.protocol() == crate::operation::Protocol::Http),
            "producer manifest must contain only HTTP entries"
        );
    }
}
