use crate::agent_service::AgentService;
use crate::agents::file_orchestrator::FileOrchestrator;
use crate::agents::framework_guidance_agent::{FrameworkGuidanceAgent, ProtocolGuidance};
use crate::analyzer::{Analyzer, ApiEndpointDetails, builder::AnalyzerBuilder};
use crate::cloud_storage::{
    CloudRepoData, CloudStorage, ManifestRole, ManifestTypeKind, ManifestTypeState,
    TypeManifestEntry, get_current_commit_hash, mount_graph_to_api_details,
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

/// This run's GitHub Actions run id (`GITHUB_RUN_ID`), or None if unset. The
/// cloud records it against the PR so a later sibling main change can re-run
/// this exact workflow run and refresh the comment.
fn run_id_from_env() -> Option<String> {
    env::var("GITHUB_RUN_ID").ok().filter(|id| !id.is_empty())
}

/// `pull_request.head.sha` from the GITHUB_EVENT_PATH event payload, or None
/// on any failure (missing env, unreadable file, unexpected JSON). The cloud
/// needs the head SHA to attach a check run; a merge-ref SHA from GITHUB_SHA
/// would pin the check to a commit that isn't on the PR branch.
fn head_sha_from_event() -> Option<String> {
    let path = env::var("GITHUB_EVENT_PATH").ok()?;
    let contents = std::fs::read_to_string(path).ok()?;
    let event: serde_json::Value = serde_json::from_str(&contents).ok()?;
    event
        .get("pull_request")?
        .get("head")?
        .get("sha")?
        .as_str()
        .map(str::to_string)
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

    // 5. Prepare each service's upload payload, but DEFER the actual upload
    //    until after cross-repo analysis (step 6) so every payload can carry the
    //    per-pair ts_check compat verdicts that analysis computes (#351). Every
    //    payload is materialised now (cheap clones), so a serialization problem
    //    can't surface halfway through a multi-service upload.
    //
    //    The production index keys on (workspace, project, repo) only, so it
    //    cannot yet hold more than one service per repo — a multi-service upload
    //    would clobber. Gate it on the backend advertising support; cross-repo
    //    analysis below still runs locally regardless. `None` = do not upload
    //    (PR/branch mode, or an unsupported multi-service repo).
    let upload_payloads: Option<Vec<CloudRepoData>> = if should_upload {
        if multi_service && !storage.supports_multi_service() {
            warn!(
                "Skipping index upload: {} services declared but the cloud key has no \
                 service discriminator yet, so uploads would overwrite each other. \
                 Cross-repo analysis still runs locally.",
                services.len()
            );
            None
        } else {
            Some(
                current_services_data
                    .iter()
                    .map(|data| strip_ast_nodes(data.clone()))
                    .collect(),
            )
        }
    } else {
        debug!("Skipping upload (PR/branch mode)");
        None
    };

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

    // On a PR run with a prior index, surface what this change added and
    // removed: operations in the freshly-analyzed services that the previous
    // (last-uploaded) index didn't have, and previously-indexed operations
    // that no longer exist. Because the baseline is the last uploaded index,
    // this can include an operation that landed on main since its last scan
    // rather than in this PR. Computed before `current_services_data` is
    // moved into the analyzer.
    let pr_delta = if is_pr_run && had_prior_index {
        let endpoint_ref = |service: &Option<String>, key: &crate::operation::OperationKey| {
            let (label, name) = key.display_labels();
            crate::findings::EndpointRef {
                method: label,
                path: name,
                service: service.clone(),
            }
        };
        // Sort by (method, path, service) for deterministic output even when
        // two services add or drop the same operation.
        let sort_refs = |refs: &mut Vec<crate::findings::EndpointRef>| {
            refs.sort_by(|a, b| {
                (&a.method, &a.path, &a.service).cmp(&(&b.method, &b.path, &b.service))
            });
        };

        let mut current_keys = std::collections::HashSet::new();
        let mut new_endpoints = Vec::new();
        let mut seen = std::collections::HashSet::new();
        for service_data in &current_services_data {
            for endpoint in &service_data.endpoints {
                let id = (service_data.service_name.clone(), endpoint.key.clone());
                current_keys.insert(id.clone());
                if !previous_self_keys.contains(&id) && seen.insert(id) {
                    new_endpoints.push(endpoint_ref(&service_data.service_name, &endpoint.key));
                }
            }
        }
        let mut removed_endpoints: Vec<crate::findings::EndpointRef> = previous_self_keys
            .iter()
            .filter(|id| !current_keys.contains(*id))
            .map(|(service, key)| endpoint_ref(service, key))
            .collect();
        sort_refs(&mut new_endpoints);
        sort_refs(&mut removed_endpoints);
        Some(crate::findings::PrDelta {
            new_endpoints,
            removed_endpoints,
        })
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
        match build_cross_repo_analyzer(all_repo_data, current_services_data, ts_check_dir).await {
            Ok(analyzer) => analyzer,
            Err(e) => {
                // Cross-repo analysis (which is what runs ts_check) failed. Close
                // the spinner with a warning first so the upload's own spinner
                // and log lines don't interleave with an unfinished one in
                // non-TTY CI logs, then preserve the prior behavior where the
                // per-repo index upload happened BEFORE cross-repo analysis:
                // still upload this run's data — verdict-less — so the index
                // stays fresh, then propagate the failure.
                logging::finish_spinner_warn(&sp, "Cross-repo analysis failed");
                if let Some(payloads) = &upload_payloads {
                    upload_service_payloads(storage, payloads).await?;
                }
                return Err(e);
            }
        };
    logging::finish_spinner(&sp, "Cross-repo analysis complete");

    let results = analyzer.get_results();

    // Eval harness output mode: emit a machine-readable projection of the
    // results and skip the human Markdown report + PR-comment relay. Consumed
    // by the offline scorer (Slice 1 of the evals plan). Deliberately terminal —
    // an eval run wants only the JSON, no upload or comment side effects. (Eval
    // mode never uploads, so `upload_payloads` is always `None` here.)
    if std::env::var("CARRICK_OUTPUT_JSON").is_ok() {
        let projection =
            crate::eval_output::EvalProjection::from_results(&results, &eval_type_manifest);
        println!("{}", serde_json::to_string_pretty(&projection)?);
        return Ok(());
    }

    // 6b. Upload each service's data, now carrying the per-pair ts_check compat
    //     verdicts cross-repo analysis just computed for the edges this repo's
    //     calls consume (#351). Keyed by canonical pair identity so the cloud
    //     MCP `check_compatibility` tool can surface the real verdict instead of
    //     structural-matching-only. Absent for edges ts_check didn't evaluate,
    //     which the cloud reads as "not compared" (fail closed, #324).
    if let Some(mut payloads) = upload_payloads {
        crate::cloud_storage::attach_compat_verdicts(&mut payloads, &results.cross_repo_matches);
        // The size guard already ran once inside strip_ast_nodes, but the
        // verdicts were appended after it — re-apply so a payload that was
        // near the cap can't be re-inflated past it and 413 the upload
        // (verdicts are tiny; the caches are what gets dropped).
        for payload in &mut payloads {
            enforce_payload_size_limit(payload);
        }
        upload_service_payloads(storage, &payloads).await?;
    }

    let topology = crate::findings::Topology {
        repo_name: repo_name.clone(),
        local_service_count,
        peer_repo_count,
    };

    // On pull_request runs we deliberately skip the index upload (see
    // should_upload_data — PR-branch data must not pollute the cross-repo
    // index), but we still relay the structured findings to the cloud, which
    // renders and posts (and updates in place on later pushes) a single PR
    // comment + check run via the GitHub App, gated on the project's
    // pr_comments_enabled toggle. Best-effort: a relay failure is logged,
    // never fatal. Assembled before `results` moves into the formatter.
    let pr_result = pr_number_from_env().map(|pr_number| crate::findings::PrResultPayload {
        repo: repo_name.clone(),
        pr_number,
        head_sha: head_sha_from_event(),
        run_id: run_id_from_env(),
        topology: topology.clone(),
        stats: crate::findings::ScanStats {
            endpoints: results.endpoints.len(),
            calls: results.calls.len(),
        },
        findings: results.findings.clone(),
        delta: pr_delta.clone(),
        verified: results
            .verified_endpoints
            .iter()
            .map(
                |(method, path, provenance)| crate::findings::VerifiedEndpoint {
                    method: method.clone(),
                    path: path.clone(),
                    provenance: *provenance,
                },
            )
            .collect(),
        graphql: crate::findings::GraphqlStatus {
            libraries: results.detected_graphql_libraries.clone(),
            operations_indexed: results.graphql_operations_indexed,
        },
    });

    let formatted = crate::formatter::FormattedOutput::new(results, topology, pr_delta);
    formatted.print();

    if let Some(payload) = pr_result
        && let Err(e) = storage.post_pr_result(&payload).await
    {
        warn!("Failed to post PR result: {}", e);
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

/// Upload each already-prepared service payload to the cloud index, in order.
/// On a mid-sequence failure, reports which services made it and which didn't:
/// uploads are keyed per (repo, service) and idempotent, so a re-run restores
/// consistency, but until then the index is mixed-generation for this repo.
async fn upload_service_payloads<T: CloudStorage>(
    storage: &T,
    payloads: &[CloudRepoData],
) -> Result<(), Box<dyn std::error::Error>> {
    let sp = logging::spinner("Uploading results...");
    for (i, payload) in payloads.iter().enumerate() {
        if let Err(e) = storage.upload_repo_data(payload).await {
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
    Ok(())
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

    enforce_payload_size_limit(&mut data);

    data
}

/// Payload size guard: Lambda function URLs have a 6MB request payload limit.
/// If serialized data exceeds ~5MB, drop the incremental caches (file_results
/// being the bulk) to stay under the limit; warn loudly if it is still over
/// Lambda's hard limit afterwards.
///
/// Called twice per upload payload: inside [`strip_ast_nodes`] when the payload
/// is prepared, and again after [`crate::cloud_storage::attach_compat_verdicts`]
/// adds the per-pair type verdicts (#351) — anything appended after the first
/// pass could otherwise re-inflate the JSON past the cap and 413 the upload.
/// Degradation order is deliberate: verdicts are tiny (a handful of short
/// strings per cross-repo edge) while `file_results` is the multi-MB bulk, so
/// the caches are always what gets dropped; verdicts are kept.
fn enforce_payload_size_limit(data: &mut CloudRepoData) {
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

            // Stage B2: GraphQL producer field-list from the service's SDL,
            // derived deterministically so the file-analyzer can emit
            // `graphql_operations` linking resolvers to schema fields. Scanned
            // over the full service `files` (not just `files_to_analyze`) so the
            // producer list is complete; empty for non-GraphQL services.
            let graphql_producer_hints = crate::graphql::GraphqlProducerHints::collect(
                service_graphql_roots(repo_path, service),
                &files,
            );
            // #268: the consumer mirror — document consumers with no
            // deterministic call-site anchor, so the file-analyzer can locate
            // their co-located result type. Same scan-root/files inputs as the
            // producer hints; empty for services with no unanchored consumers.
            let graphql_consumer_hints = crate::graphql::GraphqlConsumerHints::collect(
                service_graphql_roots(repo_path, service),
                &files,
            );

            let normalizer = UrlNormalizer::new(config);
            let service_root = service_scan_root(repo_path, config);
            let new_file_results = if !files_to_analyze.is_empty() {
                let result = file_orchestrator
                    .analyze_files(
                        &files_to_analyze,
                        &guidance,
                        &detection,
                        &service_root,
                        &graphql_producer_hints,
                        &graphql_consumer_hints,
                        &normalizer,
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
            // `merged_results` keys were normalized to repo-relative paths
            // above, so provenance classification resolves against "" here.
            let mut mount_graph = graph_orchestrator.build_mount_graph(
                &merged_results,
                &normalizer,
                std::path::Path::new(""),
            );

            // Deterministic protocol scans run BEFORE the graph is projected:
            // the GraphQL consumer file set folds transport data calls out of
            // the graph (#307) so every downstream surface (cloud projection,
            // type manifest, type requests) sees the same call set.
            let protocol_extractions =
                scan_protocol_extractions(repo_path, service, &files, &merged_results);
            fold_graphql_transport_calls(&mut mount_graph, &protocol_extractions.graphql);

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
            append_deterministic_protocol_operations(
                &mut cloud_data,
                &protocol_extractions,
                &merged_results,
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
            let mut manifest_entries = build_type_manifest_entries(&mount_graph, config, repo_path);
            stamp_manifest_anchor_symbols(&mut manifest_entries, &merged_results);
            append_protocol_manifest_entries(&mut manifest_entries, &protocol_extractions);
            append_pubsub_manifest_entries(
                &mut manifest_entries,
                &merged_results,
                &protocol_extractions.sockets,
                repo_path,
            );
            if !manifest_entries.is_empty() {
                cloud_data.type_manifest = Some(manifest_entries);
            }

            // Socket payload anchors and GraphQL consumer result-type anchors
            // both resolve through the same sidecar bundle path as HTTP explicit
            // symbols (#245/#248). Concatenate both into the extra-explicit slice.
            let mut protocol_requests = file_orchestrator
                .collect_socket_type_requests(&protocol_extractions.sockets, repo_path);
            protocol_requests.extend(
                file_orchestrator
                    .collect_graphql_type_requests(&protocol_extractions.graphql, repo_path),
            );
            // Pub/sub ops are LLM-sourced in `merged_results`, not in the
            // deterministic `protocol_extractions`, so their payload anchors
            // bundle through the same path (#corpus-2 resolution dim).
            protocol_requests
                .extend(file_orchestrator.collect_pubsub_type_requests(&merged_results, repo_path));

            // GraphQL producers take the infer path, not the bundle path: their
            // response contract is the resolver's expanded RETURN type, so they
            // become `FunctionReturn` infer requests (Stage B1).
            let mut protocol_infer = file_orchestrator
                .collect_graphql_producer_infer_requests(&protocol_extractions.graphql, repo_path);
            // Pub/sub payloads with no named symbol (wrapper patterns:
            // topic-map emitters, schema-catalog workers, generic channel
            // handles) resolve via the LLM-located payload expression through
            // the same infer path.
            protocol_infer.extend(
                file_orchestrator.collect_pubsub_infer_requests(&merged_results, repo_path),
            );

            // Type resolution via sidecar
            resolve_types_if_available(
                sidecar,
                &file_orchestrator,
                &merged_results,
                repo_path,
                extraction_config.as_ref(),
                &mount_graph,
                config,
                &protocol_requests,
                &protocol_infer,
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
    // The intent content-hash cache is keyed purely on content
    // (INTENT_CACHE_VERSION + body + callee intents), so it stays valid even
    // when the ANALYSIS cache is unusable (cache_version bump, missing
    // file_results, shallow clone). Seed the full scan with the previous
    // scan's intents so a full re-analysis re-pays /generate-intent only for
    // functions whose content actually changed.
    let prev_intents = previous_data
        .map(|prev| intents_by_hash(&prev.function_definitions))
        .unwrap_or_default();
    let cloud_data =
        analyze_current_repo(repo_path, config, packages, sidecar, &prev_intents).await?;

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

/// Run the deterministic protocol scans (GraphQL SDL/documents, Socket.IO)
/// and join in the file-analyzer's located types. Split from
/// `append_deterministic_protocol_operations` so the extractions exist BEFORE
/// the mount graph is projected into cloud data — the GraphQL consumer file
/// set drives `fold_graphql_transport_calls` on the graph first (#307).
fn scan_protocol_extractions(
    repo_path: &str,
    service: &Config,
    files: &[PathBuf],
    file_results: &HashMap<String, crate::agents::file_analyzer_agent::FileAnalysisResult>,
) -> ProtocolExtractions {
    let scan_roots = service_graphql_roots(repo_path, service);
    let mut graphql = crate::graphql::scan_repo(&scan_roots, files);
    merge_graphql_resolver_locations(&mut graphql, file_results);
    merge_graphql_consumer_locations(&mut graphql, file_results);
    let sockets = crate::socket_io::scan_files(files);
    ProtocolExtractions { graphql, sockets }
}

/// #307 (class 2): drop LLM HTTP data calls that are the TRANSPORT of
/// deterministically-extracted GraphQL consumer operations — one contract must
/// not be indexed twice. A file whose `gql` documents produced consumer ops
/// executes them over a POST to the client's endpoint URL, which the
/// file-analyzer also reports as an HTTP data call (`POST ${GQL_URL}/graphql`);
/// the document ops are the real modeled contract, so the transport call is
/// folded into them. Only env-templated / absolute-URL targets are folded: a
/// plain relative literal path in the same file is a distinct same-origin REST
/// call and is kept. The shape test reads the RAW `target_url`, not
/// `canonical_path` — `consumer_call_path` strips a declared-internal env-var
/// base (`${GQL_URL}/graphql` → `/graphql`), which would otherwise let the
/// transport leak for exactly the users who configured `internalEnvVars`.
/// Known limitation (logged): a REST call built on a DIFFERENT env-var base
/// inside a gql-consumer file is folded too — accepted over leaking a phantom
/// HTTP contract for every GraphQL client file.
fn fold_graphql_transport_calls(
    mount_graph: &mut crate::mount_graph::MountGraph,
    graphql: &crate::graphql::GraphqlExtraction,
) {
    if graphql.consumers.is_empty() {
        return;
    }
    // Normalize both sides component-wise so a `./`-prefixed walk path and a
    // bare file key still join (`components()` keeps a LEADING CurDir, so it
    // is skipped explicitly).
    let normalize_file = |p: &Path| -> PathBuf {
        p.components()
            .skip_while(|c| matches!(c, std::path::Component::CurDir))
            .collect()
    };
    let consumer_files: HashSet<PathBuf> = graphql
        .consumers
        .iter()
        .map(|op| normalize_file(&op.file_path))
        .collect();
    mount_graph.data_calls.retain(|call| {
        let raw = call.target_url.trim_matches(['`', '"', '\'']);
        let is_transport_shape = raw.contains("${")
            || raw.contains("process.env.")
            || raw.starts_with("http://")
            || raw.starts_with("https://");
        if !is_transport_shape {
            return true;
        }
        let Some((file, _line)) = call.file_location.rsplit_once(':') else {
            return true;
        };
        let file_norm = normalize_file(Path::new(file));
        if consumer_files.contains(&file_norm) {
            debug!(
                "Folding GraphQL transport call {} {} ({}) into its document operations",
                call.method, call.canonical_path, file
            );
            false
        } else {
            true
        }
    });
}

fn append_deterministic_protocol_operations(
    cloud_data: &mut CloudRepoData,
    extractions: &ProtocolExtractions,
    file_results: &HashMap<String, crate::agents::file_analyzer_agent::FileAnalysisResult>,
) {
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
        // Provenance classification is HTTP-only today (#380): non-HTTP op
        // paths are not root-stripped here, and segment-matching an
        // un-relativized path would misfire on scan-prefix directories.
        provenance: Default::default(),
    };

    let graphql = &extractions.graphql;
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

    let sockets = &extractions.sockets;
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

    append_pubsub_operations(cloud_data, file_results, &extractions.sockets, &to_details);
}

/// Component-wise path normalization used by the protocol folds: strip a leading
/// `./` (a `CurDir` component) so a walk-derived path (`./realtime/server.ts`)
/// and a repo-relative file_results key (`realtime/server.ts`) collapse to the
/// same value. Mirrors the local normalizer in `fold_graphql_transport_calls`.
fn normalize_protocol_file(p: &Path) -> PathBuf {
    p.components()
        .skip_while(|c| matches!(c, std::path::Component::CurDir))
        .collect()
}

/// Structural fold set: normalized file → the event names for which the
/// deterministic Socket.IO scan already emitted an operation in that file
/// (emitter OR listener). The file-analyzer sometimes reports a single
/// `socket.emit("x", …)` / `socket.on("x", …)` site as BOTH a socket event and
/// a pub/sub op; the deterministic socket op is the modeled contract, so a
/// pub/sub op sharing the SAME file AND the SAME event/topic string is folded
/// away (dropped) in favor of it — otherwise the emit is indexed twice (once
/// `socket|…`, once `pubsub|…`), inflating the call set.
///
/// The match keys purely on structural coincidence (same file + same name),
/// never on a library/broker name, so a genuine Kafka/NATS/Redis/BullMQ publish
/// in a file that has NO socket op on that event is untouched. Requiring the
/// same-file socket twin is what keeps real pub/sub (which lives in files
/// without socket ops) safe.
///
/// Keyed as a map of file → event set (rather than a set of owned pairs) so
/// membership checks borrow `&Path`/`&str` without per-op cloning.
fn socket_event_twins(
    sockets: &crate::socket_io::SocketExtraction,
) -> HashMap<PathBuf, HashSet<String>> {
    let mut twins: HashMap<PathBuf, HashSet<String>> = HashMap::new();
    for op in sockets.listeners.iter().chain(sockets.emitters.iter()) {
        if let Some(event) = op.key.socket_event() {
            twins
                .entry(normalize_protocol_file(&op.file_path))
                .or_default()
                .insert(event.to_string());
        }
    }
    twins
}

/// Membership check against [`socket_event_twins`]'s map using borrowed keys.
fn has_socket_twin(
    twins: &HashMap<PathBuf, HashSet<String>>,
    file_norm: &Path,
    topic: &str,
) -> bool {
    twins
        .get(file_norm)
        .is_some_and(|events| events.contains(topic))
}

/// Fold the file-analyzer's `pubsub_operations` into `cloud_data` so they reach
/// the exact-key matcher (#corpus-2 edge #4). A subscriber registers a handler
/// and is the contract producer → `cloud_data.endpoints`; a publisher sends and
/// is the consumer → `cloud_data.calls`. Identity is the topic alone
/// (`OperationKey::pubsub`), so a subscriber and a publisher on the same topic in
/// two repos share one key and match.
///
/// Unlike GraphQL there is no SDL backstop, so we push every LLM op directly
/// (the deterministic append path), NOT through `merge_graphql_resolver_locations`
/// which would discard ops with no schema producer. Repo identity is back-filled
/// later by `AnalyzerBuilder::build_from_repo_data`; the only requirement is that
/// these ops sit in `cloud_data` before serialization.
///
/// An op whose `role` is `None` (model omitted it or emitted an off-enum value,
/// absorbed leniently) can't be placed on either side and is dropped with a debug
/// log. Only literal topics are extracted today (env-template collapse is deferred).
fn append_pubsub_operations(
    cloud_data: &mut CloudRepoData,
    file_results: &HashMap<String, crate::agents::file_analyzer_agent::FileAnalysisResult>,
    sockets: &crate::socket_io::SocketExtraction,
    to_details: &impl Fn(OperationKey, &Path, u32) -> ApiEndpointDetails,
) {
    use crate::operation::PubsubRole;

    let socket_twins = socket_event_twins(sockets);
    let mut subscribers = 0usize;
    let mut publishers = 0usize;
    let mut dropped = 0usize;
    let mut folded = 0usize;
    // Deterministic order: HashMap iteration is unordered, so sort by path
    // before pushing endpoints/calls (keeps scanner output stable).
    let mut paths: Vec<&String> = file_results.keys().collect();
    paths.sort();
    for path in paths {
        let result = &file_results[path];
        let file_norm = normalize_protocol_file(Path::new(path));
        for op in &result.pubsub_operations {
            // Same-file socket twin → the file-analyzer double-classified a
            // socket emit/listen site; keep the deterministic socket op, drop
            // this pub/sub form so the site is indexed once.
            if has_socket_twin(&socket_twins, &file_norm, &op.topic) {
                debug!(
                    topic = %op.topic,
                    file = %path,
                    "pub/sub op folded into same-file socket twin"
                );
                folded += 1;
                continue;
            }
            let line = u32::try_from(op.line_number).unwrap_or(0);
            let file_path = PathBuf::from(path);
            let key = OperationKey::pubsub(op.topic.clone());
            match op.role {
                Some(PubsubRole::Subscriber) => {
                    cloud_data.endpoints.push(to_details(key, &file_path, line));
                    subscribers += 1;
                }
                Some(PubsubRole::Publisher) => {
                    cloud_data.calls.push(to_details(key, &file_path, line));
                    publishers += 1;
                }
                None => {
                    debug!(
                        topic = %op.topic,
                        file = %path,
                        "pubsub_operation has no role; dropping"
                    );
                    dropped += 1;
                }
            }
        }
    }
    if subscribers + publishers + dropped + folded > 0 {
        debug!(
            subscribers,
            publishers, dropped, folded, "Indexing pub/sub operations"
        );
    }
}

/// Emit type-manifest entries for the LLM-extracted pub/sub operations so they
/// carry a type anchor, mirroring the Socket.IO manifest path exactly (#PR-4).
///
/// `append_pubsub_operations` already places pub/sub ops in
/// `cloud_data.endpoints/calls`, which is enough for the exact-key matcher to
/// MATCH a subscriber against a publisher — but without a manifest entry the op
/// has no `primary_type_symbol` anchor, so the anchor + resolution dimensions
/// treat every extracted pub/sub op as an untyped miss. This re-walks the same
/// `file_results` and, for each op carrying a decoded-payload
/// `primary_type_symbol`, emits one Response-kind manifest entry: a subscriber
/// (the contract producer) → `ManifestRole::Producer`; a publisher (the
/// consumer) → `ManifestRole::Consumer`. The `primary_type_symbol` is threaded
/// straight onto the entry so the sidecar resolves it through the same
/// SymbolRequest bundle path Socket.IO payloads use.
///
/// Pub/sub ops live in `file_results` (LLM-sourced), not the deterministic
/// `ProtocolExtractions` struct, so this is a sibling of
/// `append_protocol_manifest_entries` rather than a branch inside it. It is
/// called at both manifest call sites (incremental + full) right after the
/// deterministic protocols are folded in.
///
/// Mirroring socket's null handling: an op with `primary_type_symbol: None`
/// (untyped or inline-object payload) still gets a manifest entry, just with a
/// `None` symbol — the entry stays `Unknown`, exactly as a socket emitter whose
/// payload type the extractor couldn't capture. An op with no role is skipped
/// (it was already dropped from `cloud_data` and has nothing to anchor).
///
/// A pub/sub op folded away by the same-file socket-twin guard (see
/// `socket_event_twins`) is also skipped here: it was dropped from `cloud_data`
/// by `append_pubsub_operations`, so leaving a manifest anchor for it would
/// orphan the anchor. The `sockets` extraction feeds the same fold set both
/// places.
fn append_pubsub_manifest_entries(
    entries: &mut Vec<TypeManifestEntry>,
    file_results: &HashMap<String, crate::agents::file_analyzer_agent::FileAnalysisResult>,
    sockets: &crate::socket_io::SocketExtraction,
    repo_root: &str,
) {
    use crate::operation::PubsubRole;

    let socket_twins = socket_event_twins(sockets);
    // Deterministic order: sort paths before emitting manifest entries.
    let mut paths: Vec<&String> = file_results.keys().collect();
    paths.sort();
    for path in paths {
        let result = &file_results[path];
        let file_norm = normalize_protocol_file(Path::new(path));
        for op in &result.pubsub_operations {
            // Folded into a same-file socket twin: dropped from cloud_data, so
            // emit no orphan anchor here either.
            if has_socket_twin(&socket_twins, &file_norm, &op.topic) {
                continue;
            }
            let role = match op.role {
                Some(PubsubRole::Subscriber) => ManifestRole::Producer,
                Some(PubsubRole::Publisher) => ManifestRole::Consumer,
                // No role → not placed on either side of `cloud_data`; nothing
                // to anchor, so emit no manifest entry.
                None => continue,
            };
            let key = OperationKey::pubsub(op.topic.clone());
            // Clamp to a valid 1-based line. A degenerate (<= 0) line must still
            // hash identically here and on the SymbolRequest side, and 0 is an
            // invalid anchor everywhere else (`parse_file_location` et al.).
            let line = u32::try_from(op.line_number).unwrap_or(0).max(1);
            // Publishers (consumers) disambiguate by call site. Two repos
            // publishing to the same topic (fan-in — the common event-driven
            // shape) otherwise hash to ONE consumer alias, and ts_check's bundled
            // `cross-repo-consumers` types then declare that interface twice with
            // different bodies — one publisher's payload masks the other's,
            // yielding a spurious compat mismatch on whichever loses. Mirror the
            // HTTP consumer path (`add_manifest_pair` + `build_call_site_id`).
            // Subscribers (producers) keep the plain alias: one definition per
            // topic per repo, exactly like an HTTP endpoint.
            let call_id = match role {
                ManifestRole::Consumer => Some(build_call_site_id(path, line, &key, repo_root)),
                ManifestRole::Producer => None,
            };
            add_protocol_manifest_entry(
                entries,
                &key,
                role,
                path,
                line,
                op.primary_type_symbol.clone(),
                call_id.as_deref(),
            );
        }
    }
}

/// Fold the file-analyzer's `graphql_operations` into the SDL-derived producers
/// (Stage B1). The SDL `scan_repo` gives the producer's canonical
/// `OperationKey` and its SDL anchor, but NOT where the resolver lives — and the
/// producer's real response contract is the resolver function's RETURN type
/// expanded (`Promise<ApiResponse<Order>>` → `{ data: …, errors }`), which only
/// a `FunctionReturn` infer at the resolver's file/line can give.
///
/// For each LLM `graphql_operation`, build its canonical `OperationKey` and match
/// it to the SDL producer with the same key; populate that producer's
/// `resolver_file` (the file the op came from — `file_results` is keyed by path)
/// and `resolver_line`. An LLM op with no matching SDL producer is ignored
/// (logged at debug): without an SDL producer there is no manifest entry to join
/// back to, so a resolver location alone is inert.
///
/// Consumers are never touched — they anchor on `payload_type_symbol`.
fn merge_graphql_resolver_locations(
    graphql: &mut crate::graphql::GraphqlExtraction,
    file_results: &HashMap<String, crate::agents::file_analyzer_agent::FileAnalysisResult>,
) {
    // Canonical producer key -> index, so each LLM op joins in O(1) without an
    // N×M scan. A schema field has at most one root producer, so the last write
    // wins is moot (keys are unique across producers).
    let mut by_key: HashMap<String, usize> = HashMap::new();
    for (idx, op) in graphql.producers.iter().enumerate() {
        by_key.insert(op.key.canonical(), idx);
    }

    for (path, result) in file_results {
        for llm_op in &result.graphql_operations {
            let key = OperationKey::graphql(llm_op.kind, llm_op.field.clone());
            let Some(&idx) = by_key.get(&key.canonical()) else {
                debug!(
                    op = %key.canonical(),
                    file = %path,
                    "graphql_operation has no matching SDL producer; ignoring resolver location"
                );
                continue;
            };
            let producer = &mut graphql.producers[idx];
            producer.resolver_file = Some(PathBuf::from(path));
            // Trim before the emptiness check: a whitespace-only string (e.g.
            // " ") from the model is not a resolver function name, and letting
            // it through here would wrongly take the FunctionReturn path and
            // block the type-locate fallback below, leaving the producer
            // unanchored.
            // The FunctionReturn path needs BOTH a real resolver name AND a
            // usable line: `collect_graphql_producer_infer_requests` skips any
            // producer missing either, so taking this branch with a dead
            // locator would leave the producer with no type request of any
            // kind. The LLM line is a 1-based source line; clamp non-positive
            // values to None rather than wrapping into a bogus u32 (the infer
            // request's line_number is u32, and a 0/negative line can't anchor
            // a fn), and treat a clamped-out line exactly like a missing
            // resolver: fall through to the backing-type fallback.
            let resolver_line = llm_op
                .resolver_function
                .as_deref()
                .filter(|f| !f.trim().is_empty())
                .and(llm_op.resolver_line)
                .and_then(|line| u32::try_from(line).ok())
                .filter(|&line| line > 0);
            if resolver_line.is_some() {
                // Resolver located: its concrete return type carries the
                // wrappers (Promise / ApiResponse envelope / async-iterator) the
                // bare SDL-backed type can't, so the FunctionReturn path wins.
                producer.resolver_line = resolver_line;
            } else if let Some(symbol) = llm_op.backing_type_symbol.clone() {
                // No resolver function: fall back to the co-located backing type
                // the LLM located (#248). Keyed on the dedicated
                // `backing_type_symbol` (never `primary_type_symbol`, which
                // describes a resolver's return type) so this can only fire for a
                // genuinely resolver-less field. The sidecar bundles + structurally
                // expands it and wraps it in the SDL list depth.
                producer.response_type_symbol = Some(symbol);
                producer.response_type_source = llm_op.backing_type_source.clone();
            }
        }
    }
}

/// Fold the file-analyzer's `graphql_consumer_locates` onto document consumers
/// with no deterministic anchor (#268 — the consumer mirror of
/// `merge_graphql_resolver_locations`'s #248 producer backing-type fallback).
///
/// KEYING IS THE LOAD-BEARING DIFFERENCE from the producer merge: a producer's
/// canonical `OperationKey` alone is enough to join on (a schema field has at
/// most one root producer, service-wide). A consumer field has no such
/// uniqueness — the SAME field can be consumed from N different files in a
/// fan-in (a `query order` document duplicated across `web-frontend` and
/// `admin-dashboard`, say), each potentially binding its own local result
/// type. Joining on the canonical key alone would collide every file's locate
/// entry onto whichever consumer op happened to occupy that key first. So this
/// joins on the triple `(file_path, kind, field)`: each file's located type is
/// scoped strictly to its own consumer op.
///
/// ISOLATION GUARD: an op that already carries `payload_type_symbol` (the
/// deterministic `TaggedTplVisitor::capture_request_call` explicit-generic
/// anchor) is left untouched. The file-analyzer is instructed not to emit a
/// `graphql_consumer_locates` entry for an already-anchored op, but a
/// stray/hallucinated entry must never be allowed to override it regardless —
/// mirrors 186cb27's resolver-first gate on the producer side.
fn merge_graphql_consumer_locations(
    graphql: &mut crate::graphql::GraphqlExtraction,
    file_results: &HashMap<String, crate::agents::file_analyzer_agent::FileAnalysisResult>,
) {
    // (file_path, canonical_key) -> index. A consumer op's identity for this
    // join is its file AND its operation key together — never the key alone.
    let mut by_file_key: HashMap<(String, String), usize> = HashMap::new();
    for (idx, op) in graphql.consumers.iter().enumerate() {
        by_file_key.insert(
            (
                op.file_path.to_string_lossy().to_string(),
                op.key.canonical(),
            ),
            idx,
        );
    }

    for (path, result) in file_results {
        for locate in &result.graphql_consumer_locates {
            let key = OperationKey::graphql(locate.kind, locate.field.clone());
            let Some(&idx) = by_file_key.get(&(path.clone(), key.canonical())) else {
                debug!(
                    op = %key.canonical(),
                    file = %path,
                    "graphql_consumer_locate has no matching consumer op in this file; ignoring"
                );
                continue;
            };
            let consumer = &mut graphql.consumers[idx];
            if consumer.payload_type_symbol.is_some() {
                // Isolation guard: an explicit call-site generic already
                // anchored this op — never let a located type override it.
                continue;
            }
            consumer.consumer_located_type_symbol = Some(locate.result_type_symbol.clone());
            consumer.consumer_located_type_source = locate.result_type_source.clone();
        }
    }
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
/// SymbolRequest path. GraphQL SDL producers carry their deterministic anchor
/// too (#248): the root field's SDL type expression (`Order`, `[Order!]!`),
/// the only anchor available without a framework-specific SDL-field → TS-resolver
/// mapping. GraphQL document consumers have no SDL type, so their anchor is
/// the call-site-bound `payload_type_symbol`, falling back to the
/// file-analyzer-located `consumer_located_type_symbol` (#268) when the
/// deterministic pass found no explicit call-site generic.
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
            op.primary_type_symbol.clone(),
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
            // The consumer's real anchor is the bound TS result type captured at
            // the `request<T>(DOC)` call site (#248 consumer side), not the
            // SDL-derived `primary_type_symbol` (always `None` for documents).
            // When the deterministic pass found no explicit call-site generic,
            // fall back to the file-analyzer-located co-located type (#268) —
            // the engine merge's isolation guard already guarantees an op never
            // carries both, so this is a plain either/or, not a priority
            // decision made here.
            op.payload_type_symbol
                .clone()
                .or_else(|| op.consumer_located_type_symbol.clone()),
            // Fan-in consumers (multiple repos reading the same field) carry the
            // same latent alias-collision risk the pub/sub publisher path fixes,
            // but neither corpus exercises it today; deferred to #291.
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
            None,
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
            // Same deferred fan-in caveat as the graphql consumer above (#291).
            None,
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
/// `build_manifest_type_alias_with_call_id(key, role, Response, call_id)` the
/// SymbolRequest side uses — same key, same role, same kind, AND the same
/// `call_id` (see the `call_id` param) — or the enrich-join silently fails to
/// flip `Unknown` → resolved.
fn add_protocol_manifest_entry(
    entries: &mut Vec<TypeManifestEntry>,
    key: &OperationKey,
    role: ManifestRole,
    file_path: &str,
    line_number: u32,
    primary_type_symbol: Option<String>,
    // Per-call-site disambiguator. `None` keeps the plain key-only alias (one
    // definition per key per repo — correct for producers/endpoints). `Some`
    // appends a `_Call<id>` suffix so multiple consumers of the same key in one
    // bundle (fan-in) don't collide on a single alias; see the pub/sub publisher
    // path in `append_pubsub_manifest_entries`. Must equal the `call_id` the
    // SymbolRequest side computes for the same op, or the resolution join breaks.
    call_id: Option<&str>,
) {
    let type_kind = ManifestTypeKind::Response;
    let type_alias =
        crate::type_manifest::build_manifest_type_alias_with_call_id(key, role, type_kind, call_id);
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
/// Repo-relative paths for cloud-bound function definitions. The scan runs
/// against a canonicalized absolute repo root (in CI the runner checkout,
/// e.g. `/home/runner/work/<dir>/<repo>`), and the extractor stamps that
/// absolute path onto every definition — plus the compiler leaks it into
/// signatures via `import("/abs/path/x")` type references. Uploading those
/// verbatim breaks every consumer that joins the path back to the repo
/// (GitHub deep links, MCP tools telling agents to fetch
/// `repos/{owner}/{repo}/contents/{file_path}`). Strip the root at the
/// cloud-projection boundary only — internal passes (sidecar type
/// resolution, git-diff comparisons) still operate on absolute paths.
fn relativize_function_definition_paths(
    function_definitions: &mut HashMap<String, FunctionDefinition>,
    repo_path: &str,
) {
    let root = std::path::Path::new(repo_path);
    let prefix = format!("{}/", repo_path.trim_end_matches('/'));
    for def in function_definitions.values_mut() {
        if let Ok(stripped) = def.file_path.strip_prefix(root) {
            def.file_path = stripped.to_path_buf();
        }
        for call in &mut def.calls {
            if let Some(stripped) = call.file_path.strip_prefix(&prefix) {
                call.file_path = stripped.to_string();
            }
        }
        if let Some(sig) = &mut def.signature
            && sig.contains(&prefix)
        {
            *sig = sig.replace(&prefix, "");
        }
    }
}

fn build_cloud_data_from_mount_graph(
    repo_name: &str,
    repo_path: &str,
    mount_graph: &MountGraph,
    config: &Config,
    packages: &Packages,
    function_definitions: HashMap<String, FunctionDefinition>,
) -> CloudRepoData {
    let mut function_definitions = function_definitions;
    relativize_function_definition_paths(&mut function_definitions, repo_path);
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

    // Project endpoints + consumer calls through the shared helper so the
    // consumer key is the pre-computed `canonical_path` (identical to the
    // manifest join key).
    let (endpoints, calls) = mount_graph_to_api_details(mount_graph);

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
        compat_verdicts: None,
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
/// The root that file-based route derivation strips before matching a
/// convention's root globs. Conventions declare app-root globs like `app` /
/// `src/app`, which are SERVICE-relative: in a monorepo the app lives at e.g.
/// `apps/web/app/**`, so stripping only the repo root would leave a path no
/// glob matches and silently derive zero routes for every declared service.
fn service_scan_root(repo_path: &str, service: &Config) -> std::path::PathBuf {
    match &service.directory {
        Some(dir) => Path::new(repo_path).join(dir),
        None => Path::new(repo_path).to_path_buf(),
    }
}

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
    extra_infer: &[crate::services::type_sidecar::InferRequestItem],
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
                extra_infer,
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
    let mut packages = if let Some(package_path) = package_json_path {
        debug!("Found package.json: {}", package_path.display());
        Packages::new(vec![package_path.clone()])
            .map_err(|e| format!("Failed to parse {}: {}", package_path.display(), e))?
    } else {
        Packages::default()
    };
    // Names of every package.json in the WHOLE repo tree (not just this
    // service's): a workspace member like `packages/contracts` is not a
    // service, but a dependency on it is internal, not a registry package.
    packages.internal_names =
        crate::packages::collect_internal_package_names(std::path::Path::new(repo_path));
    Ok(packages)
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
    repo_root: &str,
) -> Vec<TypeManifestEntry> {
    let normalizer = UrlNormalizer::new(config);
    let mut entries = Vec::new();

    // Producers carry the plain key-only alias (no `_Call<id>` suffix — that
    // disambiguator exists for consumer fan-in, and the SymbolRequest side
    // computes the same key-only alias for producers). So two endpoints
    // colliding on (method, path) — a mis-extracted duplicate route (#332) or a
    // genuinely twice-declared one — would share one alias and one resolved
    // definition would silently clobber the other in the bundle (#334). Keep
    // the first declaration, drop the rest with a warning.
    let mut seen_producer_keys: HashSet<OperationKey> = HashSet::new();

    for endpoint in mount_graph.get_resolved_endpoints() {
        let method = normalize_manifest_method(&endpoint.method);
        if !is_http_method(&method) {
            continue;
        }
        // Call-site-evidence entries (#379) never anchor Producer types: they
        // are client encodings of an external contract, and a producer
        // manifest entry would make ts_check run a request-vs-request
        // comparison mislabelled as a producer-contract verdict. Their pairs
        // are verdict-exempt at the source; the site's Consumer entry is
        // still emitted from the twin data call below.
        if endpoint.evidence == carrick_match::MatchEvidence::CallSite {
            continue;
        }
        let path = endpoint.full_path.clone();
        if !path.starts_with('/') {
            continue;
        }
        let (file_path, line_number) = parse_file_location(&endpoint.file_location);

        let key = OperationKey::http(&method, path);
        if !seen_producer_keys.insert(key.clone()) {
            warn!(
                "Duplicate producer endpoint {} at {} shares a manifest alias with an \
                 earlier declaration; dropping this entry to avoid clobbering its \
                 resolved type definition",
                key, endpoint.file_location
            );
            continue;
        }

        add_manifest_pair(
            &mut entries,
            key,
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
        // Key on the canonical path computed once at mount-graph build time, so
        // the manifest join key is byte-identical to the projection key.
        let path = call.canonical_path.clone();
        // Only anchor types for a BARE route (internal/declared or relative
        // target). An external or unclassified call keeps its raw `${HOST}/path`
        // / full-URL canonical form: it has no internal producer to match, so a
        // type anchor would be unused. Mirrors main, where the raw projection
        // key never joined an external call's `extract_path`-keyed manifest
        // entry.
        if !path.starts_with('/') {
            continue;
        }
        let key = OperationKey::http(&method, path);
        let call_id = build_call_site_id(&file_path, line_number, &key, repo_root);

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

    // Deterministic anchor source (#240): the sidecar resolves each inferred
    // type's real source symbol (`Payment`) off the ts-morph `Type`, so a
    // manifest entry whose anchor the LLM left unset can be filled from it.
    // Join by `alias` — the same key the resolved-type lookup below uses to
    // marry an `InferredType` to its manifest entry. A `(file_path, line)` join
    // would be fragile: the sidecar's `source_location` is an absolute ts-morph
    // path while `entry.file_path` is repo-relative, so the coordinates need not
    // line up. First non-None wins per alias, so a later inferred entry can't
    // clobber an earlier real symbol.
    let mut inferred_symbols: HashMap<String, String> = HashMap::new();
    for inferred in &type_resolution.inferred_types {
        if let Some(symbol) = inferred.primary_type_symbol.as_ref() {
            inferred_symbols
                .entry(inferred.alias.clone())
                .or_insert_with(|| symbol.clone());
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
        // Fill the deterministic anchor ONLY when the LLM left it unset, so the
        // ops where the model already emitted a correct symbol (POST /payments,
        // socket) are never regressed. Stamping runs before enrichment, so any
        // entry still `None` here had no LLM anchor.
        if entry.primary_type_symbol.is_none()
            && let Some(symbol) = inferred_symbols.get(&entry.type_alias)
        {
            entry.primary_type_symbol = Some(symbol.clone());
        }

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
    prev_intents_by_hash: &HashMap<String, String>,
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

    // Stage B2: derive the GraphQL producer field-list from the service's SDL
    // (deterministic, cheap) so the file-analyzer can link resolver functions to
    // schema fields and emit `graphql_operations`. Empty (a no-op) for non-GraphQL
    // services. `append_deterministic_protocol_operations` scans the SDL again
    // later for the operation index; the duplicate parse is acceptable.
    let graphql_producer_hints = crate::graphql::GraphqlProducerHints::collect(
        service_graphql_roots(repo_path, service),
        &files,
    );
    // #268: the consumer mirror — document consumers with no deterministic
    // call-site anchor, so the file-analyzer can locate their co-located
    // result type. Same duplicate-parse tradeoff as the producer hints above.
    let graphql_consumer_hints = crate::graphql::GraphqlConsumerHints::collect(
        service_graphql_roots(repo_path, service),
        &files,
    );

    // 4. Run the complete multi-agent analysis
    let normalizer = UrlNormalizer::new(config);
    let service_root = service_scan_root(repo_path, config);
    let analysis_result = orchestrator
        .run_complete_analysis(
            files.clone(),
            packages,
            &all_imported_symbols,
            &service_root.to_string_lossy(),
            &graphql_producer_hints,
            &graphql_consumer_hints,
            &normalizer,
        )
        .await?;

    // 4b. Generate function intents using LLM
    let mut function_definitions = function_definitions;
    {
        let intent_agent = AgentService::new();
        // Even on a full scan, intents whose content hash matches the
        // previous scan are reused — the intent cache is content-addressed
        // and independent of the analysis cache's validity (see the caller).
        generate_function_intents(
            &intent_agent,
            &mut function_definitions,
            &all_imported_symbols,
            prev_intents_by_hash,
        )
        .await;
    }

    // 4c. Compose function signatures, inferring unannotated slots via sidecar.
    populate_function_signatures(sidecar, &mut function_definitions, repo_path);

    // 4d. Deterministic protocol scans run BEFORE the graph is projected: the
    // GraphQL consumer file set folds transport data calls out of the mount
    // graph (#307) so every downstream surface (cloud projection, type
    // manifest, type requests) sees the same call set.
    let protocol_extractions =
        scan_protocol_extractions(repo_path, service, &files, &analysis_result.file_results);
    let mut analysis_result = analysis_result;
    fold_graphql_transport_calls(
        &mut analysis_result.mount_graph,
        &protocol_extractions.graphql,
    );
    let analysis_result = analysis_result;

    // Cloud-bound paths must be repo-relative. The incremental path gets
    // this from build_cloud_data_from_mount_graph; this full path constructs
    // CloudRepoData directly, so relativize here (after signatures are
    // composed — they embed the same absolute prefix).
    relativize_function_definition_paths(&mut function_definitions, repo_path);

    // 5. Build CloudRepoData directly from multi-agent results (bypassing Analyzer adapter layer)
    let mut cloud_data = CloudRepoData::from_multi_agent_results(
        repo_name.clone(),
        repo_path,
        &analysis_result,
        serde_json::to_string(config).ok(),
        serde_json::to_string(packages).ok(),
        Some(packages.clone()),
        function_definitions,
    );
    append_deterministic_protocol_operations(
        &mut cloud_data,
        &protocol_extractions,
        &analysis_result.file_results,
    );

    let mut manifest_entries =
        build_type_manifest_entries(&analysis_result.mount_graph, config, repo_path);
    stamp_manifest_anchor_symbols(&mut manifest_entries, &analysis_result.file_results);
    append_protocol_manifest_entries(&mut manifest_entries, &protocol_extractions);
    append_pubsub_manifest_entries(
        &mut manifest_entries,
        &analysis_result.file_results,
        &protocol_extractions.sockets,
        repo_path,
    );
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

    // Socket payload anchors and GraphQL consumer result-type anchors both
    // resolve through the same sidecar bundle path as HTTP explicit symbols
    // (#245/#248). Concatenate both into the extra-explicit slice.
    let mut protocol_requests =
        file_orchestrator.collect_socket_type_requests(&protocol_extractions.sockets, repo_path);
    protocol_requests.extend(
        file_orchestrator.collect_graphql_type_requests(&protocol_extractions.graphql, repo_path),
    );
    // Pub/sub ops are LLM-sourced in `analysis_result.file_results`, not in the
    // deterministic `protocol_extractions`, so their payload anchors bundle
    // through the same path (#corpus-2 resolution dim).
    protocol_requests.extend(
        file_orchestrator.collect_pubsub_type_requests(&analysis_result.file_results, repo_path),
    );

    // GraphQL producers take the infer path, not the bundle path: their response
    // contract is the resolver's expanded RETURN type, so they become
    // `FunctionReturn` infer requests (Stage B1).
    let mut protocol_infer = file_orchestrator
        .collect_graphql_producer_infer_requests(&protocol_extractions.graphql, repo_path);
    // Pub/sub payloads with no named symbol (wrapper patterns: topic-map
    // emitters, schema-catalog workers, generic channel handles) resolve via
    // the LLM-located payload expression through the same infer path.
    protocol_infer.extend(
        file_orchestrator.collect_pubsub_infer_requests(&analysis_result.file_results, repo_path),
    );

    resolve_types_if_available(
        sidecar,
        &file_orchestrator,
        &analysis_result.file_results,
        repo_path,
        extraction_config.as_ref(),
        &analysis_result.mount_graph,
        config,
        &protocol_requests,
        &protocol_infer,
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

/// Compute the cross-repo bundle file stem (`<stem>_types.d.ts`) for each
/// `CloudRepoData`, parallel to `all_repo_data`.
///
/// A monorepo `carrick.json` declares N services under one git repo, so N
/// `CloudRepoData` share the same `repo_name` and differ only by `service_name`.
/// Keying the bundle file by `repo_name` alone made every service in the repo
/// write to the same `<repo>_types.d.ts`, so the last service silently clobbered
/// the earlier ones — and a producer whose type lived in a clobbered service
/// (e.g. orders-pkg `GET /orders/:id` → `Order`) vanished from the bundle
/// entirely, not even leaving the `= unknown` placeholder. ts_check then
/// reported "Producer type not found in project" and the edge's compat verdict
/// collapsed to unverifiable.
///
/// Key the stem by `service_name ?? repo_name` (the same attribution convention
/// `build_cross_repo_analyzer` uses for packages) so each service gets its own
/// bundle. The type aliases inside are globally unique (`Endpoint_<hash>` keyed
/// on the operation) and ts_check loads every `*.d.ts` in the output dir, so one
/// file per service is exactly what it needs. Collisions (two services resolving
/// to the same base stem, e.g. both missing a `service_name`) are suffixed so no
/// write clobbers a prior one.
fn bundle_file_stems(all_repo_data: &[CloudRepoData]) -> Vec<String> {
    let mut used_stems: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
    all_repo_data
        .iter()
        .map(|repo_data| {
            let base_stem = repo_data
                .service_name
                .as_deref()
                .unwrap_or(&repo_data.repo_name)
                .replace(['/', '\\'], "_");
            match used_stems.entry(base_stem.clone()) {
                std::collections::hash_map::Entry::Occupied(mut e) => {
                    let n = e.get_mut();
                    *n += 1;
                    format!("{base_stem}_{n}")
                }
                std::collections::hash_map::Entry::Vacant(e) => {
                    e.insert(0);
                    base_stem
                }
            }
        })
        .collect()
}

/// Write each repo/service's bundled `.d.ts` into `output_dir`, one file per
/// service (see `bundle_file_stems`). The per-service split is what stops a
/// monorepo's services from clobbering each other down to a single bundle file
/// and silently dropping a whole service's producer types. ts_check loads every
/// `*.d.ts` in `output_dir`, so the cross-file `Endpoint_<hash>` aliases still
/// resolve regardless of which service-file each lives in.
fn write_bundle_files(all_repo_data: &[CloudRepoData], output_dir: &std::path::Path) {
    let stems = bundle_file_stems(all_repo_data);
    for (repo_data, stem) in all_repo_data.iter().zip(stems) {
        if let Some(bundled_types) = &repo_data.bundled_types {
            let file_name = format!("{stem}_types.d.ts");
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
}

// End-to-end flow and failure modes of manifest-based cross-repo type
// checking: docs/reference/type-checking-flow.md
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

    write_bundle_files(all_repo_data, output_dir);

    write_manifest_files(all_repo_data, output_dir)?;

    // Recreate package.json and tsconfig.json after writing type files
    recreate_package_and_tsconfig(
        output_dir,
        packages,
        &corpus_internal_package_names(all_repo_data),
    )?;

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
                // HTTP, socket, and graphql entries all reach the ts_check manifest
                // unfiltered. ts_check checks HTTP by `method`/`path`, socket by the
                // canonical `OperationKey` (event+direction), and graphql by
                // (kind+field), reusing the same `TypeCompatibilityChecker`
                // assignability for all three. The matcher drops any stray
                // non-checkable entry defensively rather than throwing (#253).
                //
                // An UNRESOLVED entry — `type_state == Unknown`, or an alias that
                // dangles to `any`/`unknown` in the bundle — is NOT dropped here.
                // ts_check still matches its pair and reports it as an `unknownPair`
                // (unverifiable), which `apply_compat_verdicts` maps to a `None`
                // verdict. Dropping it instead removes the pair from ts_check's
                // output entirely, and `apply_compat_verdicts` treats an edge absent
                // from BOTH `mismatches` and `unknownPairs` as compatible — a FALSE
                // `Some(true)`. That was the `graphql|subscription|orderUpdated`
                // false-positive: the consumer's only alias is the synthetic
                // `Endpoint_<hash> = unknown` missing-alias fallback, so the entry
                // was dropped, the edge went absent, and the verdict defaulted to
                // compatible. Keeping it lets the `any`/`unknown` comparand guard in
                // `type-checker.ts` route the pair to `unknownPairs` → `None`. This
                // is exactly how HTTP/socket Unknown entries have always been
                // handled; graphql is no longer a special case.
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

/// Dependencies for the synthetic `carrick-type-check` package.json: the
/// merged repo dependencies with (a) unusable versions dropped (empty, the
/// literal "undefined", or digit-less — npm turns `typescript@undefined` into
/// a hard ERESOLVE that aborts the whole cross-repo type pass), and (b)
/// packages the scanned repos declare THEMSELVES dropped — a workspace-internal
/// package (a monorepo's `@meridian/contracts`) resolves via the workspace
/// link, not the registry, so `npm install` 404s and (per the #149 fail-loud)
/// would abort type checking. Nothing is lost by dropping it: the bundle
/// carries its shapes structurally inlined.
///
/// `corpus_internal` extends (b) across the whole scanned corpus: a repo that
/// depends on a package which IS another repo/service in the corpus (an org
/// depending on its own, possibly unpublished, workspace package) gets that
/// dependency excluded too — its types arrive via that service's sidecar
/// `.d.ts` bundle, and the registry copy ETARGETs when the version was never
/// published (#390).
fn synthetic_type_check_dependencies(
    packages: &Packages,
    corpus_internal: &std::collections::HashSet<String>,
) -> std::collections::HashMap<String, String> {
    let mut internal = packages.internal_package_names();
    internal.extend(corpus_internal.iter().cloned());
    // Version specs npm cannot (or must not) resolve from the registry. These
    // are package-manager resolution protocols (yarn/pnpm/npm define them, not
    // the scanned frameworks): a digit-containing spec like
    // `patch:@scope/pkg@npm%3A1.2.3#…` or `workspace:^1.2.3` passes the digit
    // filter below but 404s/EUSAGEs the whole install (and per the #149
    // fail-loud, aborts type checking). Remote tarball/git URL specs (`https:`,
    // `ssh:`) are npm-installable but fetch arbitrary URLs from an untrusted
    // repo's dependency list — dropped on the same security stance as
    // --ignore-scripts. Dropping loses nothing: the bundle carries the shapes
    // structurally. `npm:` aliases are NOT listed — npm resolves those from
    // the registry.
    const NON_REGISTRY_PROTOCOLS: &[&str] = &[
        "workspace:",
        "patch:",
        "portal:",
        "link:",
        "file:",
        "catalog:",
        "git+",
        "git:",
        "github:",
        "http:",
        "https:",
        "ssh:",
    ];
    // yarn/pnpm `resolutions` npm-aliases remap locally-invented dependency
    // names to real registry packages (`"@types/readable-stream-2":
    // "npm:@types/readable-stream@^2.3.15"`). The merged version for such a
    // name is the bare range, so installing `name@range` 404s — apply the
    // alias spec instead (npm installs `npm:` aliases natively). Resolution
    // keys may carry a `@range` selector suffix; scoped names keep their
    // leading `@`, so the selector is everything after the LAST `@` at
    // index > 0.
    let mut resolution_aliases: std::collections::HashMap<String, String> =
        std::collections::HashMap::new();
    for package_json in &packages.package_jsons {
        for (key, value) in &package_json.resolutions {
            if !value.starts_with("npm:") {
                continue;
            }
            let name = match key.rfind('@') {
                Some(pos) if pos > 0 => &key[..pos],
                _ => key.as_str(),
            };
            resolution_aliases.insert(name.to_string(), value.clone());
        }
    }
    let mut dependencies = std::collections::HashMap::new();
    for (name, package_info) in packages.get_dependencies() {
        let version = package_info.version.trim();
        if version.is_empty()
            || version == "undefined"
            || !version.chars().any(|c| c.is_ascii_digit())
        {
            debug!("Skipping dependency {name} with unusable version {version:?}");
            continue;
        }
        if NON_REGISTRY_PROTOCOLS
            .iter()
            .any(|proto| version.starts_with(proto))
        {
            debug!("Skipping dependency {name} with non-registry version protocol {version:?}");
            continue;
        }
        if let Some(alias) = resolution_aliases.get(name) {
            // Validate the alias TARGET too: `npm:<real-name>@<range>` must
            // carry a registry-resolvable range — an alias like
            // `npm:pkg@github:user/repo` would smuggle a non-registry spec
            // past the protocol filter above. The range is everything after
            // the LAST `@` past the `npm:` prefix (the target name itself may
            // be scoped).
            let target = &alias["npm:".len()..];
            let range = match target.rfind('@') {
                Some(pos) if pos > 0 => &target[pos + 1..],
                _ => target,
            };
            let range_ok = range.chars().any(|c| c.is_ascii_digit())
                && !NON_REGISTRY_PROTOCOLS
                    .iter()
                    .any(|proto| range.starts_with(proto));
            if range_ok {
                debug!("Applying resolutions alias for {name}: {alias}");
                dependencies.insert(name.clone(), alias.clone());
            } else {
                debug!(
                    "Skipping dependency {name}: resolutions alias {alias:?} has a non-registry target"
                );
            }
            continue;
        }
        if internal.contains(name) {
            debug!(
                "Skipping workspace-internal dependency {name} (declared by a scanned \
                 package.json; not registry-resolvable)"
            );
            continue;
        }
        dependencies.insert(name.clone(), version.to_string());
    }
    dependencies
}

/// Package names declared by ANY package.json of ANY repo/service in the
/// scanned corpus (each service's `Packages` carries its own tree-walked
/// `internal_names`; older cloud payloads without the structured `packages`
/// field fall back to the serialized `package_json`). A dependency on one of
/// these is a link to a sibling service in the corpus, not an installable
/// third-party package — its types arrive via that service's sidecar `.d.ts`
/// bundle, so the npm copy is redundant, and (an org's own unpublished
/// workspace package) often ETARGETs, which used to abort the whole type pass.
fn corpus_internal_package_names(
    all_repo_data: &[CloudRepoData],
) -> std::collections::HashSet<String> {
    let mut names = std::collections::HashSet::new();
    for repo_data in all_repo_data {
        if let Some(packages) = &repo_data.packages {
            names.extend(packages.internal_package_names());
        } else if let Some(json) = &repo_data.package_json
            && let Ok(packages) = serde_json::from_str::<Packages>(json)
        {
            names.extend(packages.internal_package_names());
        }
    }
    names
}

/// Package names npm's OWN resolution-failure output blames for an install
/// failure. Two message shapes, stable across npm major versions (only the
/// `npm ERR!` vs `npm error` line prefix differs, which this deliberately
/// keys past):
///
///   ETARGET: `notarget No matching version found for <name>@<spec>.`
///   E404:    `404  '<name>@<spec>' is not in this registry.`
///
/// The `<name>@<spec>` split is at the LAST `@` at index > 0 so scoped names
/// keep their leading `@`. No name heuristics beyond what npm itself states:
/// any other failure class (ERESOLVE, network, EUSAGE, …) parses to empty,
/// so the caller keeps the hard-fail path — dropping packages cannot fix
/// those.
fn npm_unresolvable_packages(stderr: &str) -> Vec<String> {
    let etarget =
        regex::Regex::new(r"No matching version found for\s+(\S+)").expect("static regex");
    let e404 = regex::Regex::new(r"'([^']+)'\s+is not in this registry").expect("static regex");
    let mut names: Vec<String> = Vec::new();
    for caps in etarget
        .captures_iter(stderr)
        .chain(e404.captures_iter(stderr))
    {
        let spec = caps[1].trim_end_matches(['.', ',']);
        let name = match spec.rfind('@') {
            Some(pos) if pos > 0 => &spec[..pos],
            _ => spec,
        };
        if !name.is_empty() && !names.iter().any(|n| n == name) {
            names.push(name.to_string());
        }
    }
    names
}

/// Manifest keys to drop for the packages npm blamed: a key matches when it
/// IS the blamed name, or when its value is an `npm:` alias whose target is
/// the blamed name (npm's resolution errors report the alias TARGET, not the
/// manifest key it hangs off). A blamed name matching neither is a transitive
/// dependency this manifest cannot drop. Sorted for deterministic drop
/// order and logs.
fn unresolvable_manifest_keys(
    dependencies: &std::collections::HashMap<String, String>,
    blamed: &[String],
) -> Vec<String> {
    let mut keys: Vec<String> = dependencies
        .iter()
        .filter(|(key, value)| {
            blamed.iter().any(|name| {
                key.as_str() == name
                    || value.strip_prefix("npm:").is_some_and(|target| {
                        let target_name = match target.rfind('@') {
                            Some(pos) if pos > 0 => &target[..pos],
                            _ => target,
                        };
                        target_name == name
                    })
            })
        })
        .map(|(key, _)| key.clone())
        .collect();
    keys.sort();
    keys
}

/// Write the synthetic `carrick-type-check` package.json. Called once at
/// assembly and again after each retry-drop so the manifest on disk always
/// matches the dependency map npm is asked to install.
fn write_type_check_package_json(
    output_dir: &std::path::Path,
    dependencies: &std::collections::HashMap<String, String>,
) -> Result<(), Box<dyn std::error::Error>> {
    let package_json_content = serde_json::json!({
        "name": "carrick-type-check",
        "version": "1.0.0",
        "dependencies": dependencies
    });
    std::fs::write(
        output_dir.join("package.json"),
        serde_json::to_string_pretty(&package_json_content)?,
    )?;
    Ok(())
}

/// Last ~2000 chars of npm stderr, for error messages.
fn stderr_tail(stderr: &str) -> &str {
    let tail_start = stderr
        .char_indices()
        .rev()
        .nth(1999)
        .map(|(i, _)| i)
        .unwrap_or(0);
    &stderr[tail_start..]
}

/// Retry-drop rounds allowed when npm's resolution errors name specific
/// packages. Each round drops only what npm itself blamed and reinstalls.
const MAX_INSTALL_DROP_ROUNDS: usize = 3;

/// `npm install` for the synthetic type-check package, degrading gracefully
/// on per-package resolution failures instead of all-or-nothing (#390):
///
/// - On ETARGET/E404 the offending package(s) — exactly the ones npm's own
///   error output names, no heuristics — are dropped from the manifest and
///   the install retried, at most [`MAX_INSTALL_DROP_ROUNDS`] times, each
///   drop logged loudly. Types that then dangle resolve to `any`/`unknown`
///   in ts_check and are reported as unverifiable (the existing abstain
///   semantics), NOT compatible.
/// - Every other failure class, and exhausted retries, keep the #149
///   fail-loud hard error — with the drop history in the message — because a
///   swallowed install failure used to let the run print "✓ Cross-repo
///   analysis complete" while type checking silently degraded.
fn install_type_check_dependencies(
    output_dir: &std::path::Path,
    dependencies: &mut std::collections::HashMap<String, String>,
) -> Result<(), Box<dyn std::error::Error>> {
    use std::process::Command;
    let mut rounds_left = MAX_INSTALL_DROP_ROUNDS;
    let mut dropped: Vec<String> = Vec::new();
    loop {
        debug!("Installing dependencies...");

        // `--legacy-peer-deps` so a transitive peer-range disagreement (e.g.
        // ts-node's `typescript@>=2.7` vs a repo's pinned major) can't abort
        // the install. This package only feeds ts-morph type extraction, so a
        // looser peer graph is harmless.
        //
        // `--ignore-scripts` because only the installed packages' .d.ts files
        // are consumed — lifecycle scripts add nothing here, execute untrusted
        // code from the scanned repos' dependency trees on the scanning
        // machine, and security-guard packages (e.g. LavaMoat's
        // @lavamoat/preinstall-always-fail, shipped by MetaMask repos)
        // DELIBERATELY fail any install that runs scripts.
        let install_output = Command::new("npm")
            .arg("install")
            .arg("--legacy-peer-deps")
            .arg("--ignore-scripts")
            .current_dir(output_dir)
            .output()
            .map_err(|e| format!("Failed to run npm install: {}", e))?;

        if install_output.status.success() {
            if dropped.is_empty() {
                debug!("Dependencies installed successfully");
            } else {
                warn!(
                    "Cross-repo type-check install succeeded after dropping unresolvable \
                     dependencies: {}. Types resolving through them will be reported as \
                     unverifiable, not compatible.",
                    dropped.join(", ")
                );
            }
            return Ok(());
        }

        let stderr = String::from_utf8_lossy(&install_output.stderr).into_owned();

        if rounds_left > 0 {
            let blamed = npm_unresolvable_packages(&stderr);
            let drop_keys = unresolvable_manifest_keys(dependencies, &blamed);
            if !drop_keys.is_empty() {
                for key in &drop_keys {
                    warn!(
                        "npm cannot resolve dependency {key} (ETARGET/E404) — dropping it \
                         from the cross-repo type-check package and retrying. Its types will \
                         be reported as unverifiable, not compatible."
                    );
                    dependencies.remove(key);
                    dropped.push(key.clone());
                }
                write_type_check_package_json(output_dir, dependencies)?;
                // A lockfile written during the failed attempt could still pin
                // the dropped names.
                let _ = std::fs::remove_file(output_dir.join("package-lock.json"));
                rounds_left -= 1;
                continue;
            }
        }

        let drop_history = if dropped.is_empty() {
            String::new()
        } else {
            format!(
                " Dropped unresolvable dependencies before failing: {}.",
                dropped.join(", ")
            )
        };
        return Err(format!(
            "npm install failed for the cross-repo type-check package — type checking \
             cannot run. Set CARRICK_SKIP_NPM_INSTALL=1 to bypass (type checking will \
             be skipped).{drop_history} npm stderr (tail):\n{}",
            stderr_tail(&stderr)
        )
        .into());
    }
}

/// Recreate package.json and tsconfig.json in the output directory
fn recreate_package_and_tsconfig(
    output_dir: &std::path::Path,
    packages: &Packages,
    corpus_internal: &std::collections::HashSet<String>,
) -> Result<(), Box<dyn std::error::Error>> {
    // Create package.json
    let package_json_path = output_dir.join("package.json");
    let mut dependencies = synthetic_type_check_dependencies(packages, corpus_internal);

    // Pin the TypeScript toolchain we control for this synthetic type-check
    // package. Overwrite (not insert-if-missing): a merged repo may pin a
    // different/older typescript that conflicts with ts-node's peer range, so
    // forcing a known-good pair is what keeps `npm install` resolvable. These
    // match ts_check/package.json's pins.
    dependencies.insert("typescript".to_string(), "5.8.3".to_string());
    dependencies.insert("ts-node".to_string(), "10.9.2".to_string());

    write_type_check_package_json(output_dir, &dependencies)?;
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

        install_type_check_dependencies(output_dir, &mut dependencies)?;
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

    #[test]
    fn service_scan_root_joins_declared_directory() {
        let flat = Config::default();
        assert_eq!(
            service_scan_root("/repo", &flat),
            std::path::PathBuf::from("/repo")
        );

        let service = Config {
            directory: Some("apps/web".to_string()),
            ..Config::default()
        };
        assert_eq!(
            service_scan_root("/repo", &service),
            std::path::PathBuf::from("/repo/apps/web")
        );
    }
    use crate::cloud_storage::TypeEvidence;
    use crate::services::type_sidecar::{InferredType, SourceLocation};
    use crate::visitor::{OwnerType, TypeReference};
    use std::path::PathBuf;

    /// Cloud-bound function definitions must carry repo-relative paths: the
    /// extractor stamps the absolute CI checkout path (and the compiler leaks
    /// it into signatures via `import("...")`), which breaks GitHub deep
    /// links and the MCP tools' `gh api .../contents/{file_path}` hint.
    #[test]
    fn cloud_projection_relativizes_function_paths() {
        use crate::visitor::{FunctionCallRef, FunctionDefinition};

        let repo_path = "/home/runner/work/acme/acme";
        let mut defs = HashMap::new();
        defs.insert(
            "handler".to_string(),
            FunctionDefinition {
                name: "handler".to_string(),
                file_path: PathBuf::from("/home/runner/work/acme/acme/src/api/handler.ts"),
                node_type: Default::default(),
                arguments: vec![],
                body_source: None,
                is_exported: true,
                line_number: 3,
                intent: Some("handles the thing".to_string()),
                calls: vec![FunctionCallRef {
                    name: "helper".to_string(),
                    file_path: "/home/runner/work/acme/acme/src/lib/helper.ts".to_string(),
                    line_number: 9,
                }],
                return_type: None,
                return_is_explicit: false,
                signature: Some(
                    "(req: import(\"/home/runner/work/acme/acme/src/types\").Req) => void"
                        .to_string(),
                ),
                intent_input_hash: None,
            },
        );
        // A path outside the repo root is left as-is (matches the dashboard's
        // read-side posture: never mangle what we can't confidently strip).
        defs.insert(
            "external".to_string(),
            FunctionDefinition {
                name: "external".to_string(),
                file_path: PathBuf::from("/opt/other/place.ts"),
                node_type: Default::default(),
                arguments: vec![],
                body_source: None,
                is_exported: true,
                line_number: 1,
                intent: None,
                calls: vec![],
                return_type: None,
                return_is_explicit: false,
                signature: None,
                intent_input_hash: None,
            },
        );

        relativize_function_definition_paths(&mut defs, repo_path);

        let handler = &defs["handler"];
        assert_eq!(handler.file_path, PathBuf::from("src/api/handler.ts"));
        assert_eq!(handler.calls[0].file_path, "src/lib/helper.ts");
        assert_eq!(
            handler.signature.as_deref(),
            Some("(req: import(\"src/types\").Req) => void"),
        );
        assert_eq!(
            defs["external"].file_path,
            PathBuf::from("/opt/other/place.ts")
        );
    }

    #[test]
    fn synthetic_type_check_deps_exclude_workspace_internal_packages() {
        // catalog-api depends on the workspace package @meridian/contracts.
        // Installing that from the registry 404s and (fail-loud #149) aborts
        // the whole type pass — the synthetic package must exclude any name a
        // scanned package.json declares itself, while keeping real deps from
        // both sides of the workspace.
        use crate::packages::{PackageJson, Packages};
        use std::collections::HashMap;

        let mut packages = Packages::default();
        packages.package_jsons.push(PackageJson {
            name: Some("@meridian/contracts".to_string()),
            version: Some("0.1.0".to_string()),
            dependencies: HashMap::from([("zod".to_string(), "3.23.0".to_string())]),
            dev_dependencies: HashMap::new(),
            peer_dependencies: HashMap::new(),
            resolutions: HashMap::new(),
        });
        packages.package_jsons.push(PackageJson {
            name: Some("catalog-api".to_string()),
            version: Some("1.0.0".to_string()),
            dependencies: HashMap::from([
                ("@meridian/contracts".to_string(), "0.1.0".to_string()),
                ("koa".to_string(), "2.15.3".to_string()),
            ]),
            dev_dependencies: HashMap::new(),
            peer_dependencies: HashMap::new(),
            resolutions: HashMap::new(),
        });
        packages
            .source_paths
            .push(PathBuf::from("packages/contracts/package.json"));
        packages
            .source_paths
            .push(PathBuf::from("packages/catalog-api/package.json"));
        packages.resolve_dependencies();

        let deps = synthetic_type_check_dependencies(&packages, &Default::default());
        assert!(
            !deps.contains_key("@meridian/contracts"),
            "workspace-internal package must not be npm-installed"
        );
        assert_eq!(deps.get("koa").map(String::as_str), Some("2.15.3"));
        assert_eq!(deps.get("zod").map(String::as_str), Some("3.23.0"));
    }

    #[test]
    fn synthetic_type_check_deps_exclude_non_registry_version_protocols() {
        // yarn/pnpm resolution-protocol specs pass the digit filter when they
        // embed a version (`patch:…@npm%3A11.1.0#…`, `workspace:^1.2.3`) but
        // npm cannot resolve them from the registry — the install 404s/EUSAGEs
        // and (fail-loud #149) aborts the whole type pass. Real-world case:
        // metamask-extension's `patch:` specs killed the first external-OSS
        // eval probe. Registry-resolvable specs, including `npm:` aliases,
        // must survive.
        use crate::packages::{PackageJson, Packages};
        use std::collections::HashMap;

        let mut packages = Packages::default();
        packages.package_jsons.push(PackageJson {
            name: Some("app".to_string()),
            version: Some("1.0.0".to_string()),
            dependencies: HashMap::from([
                (
                    "@metamask/controller-utils".to_string(),
                    "patch:@metamask/controller-utils@npm%3A11.1.0#~/.yarn/patches/x.patch"
                        .to_string(),
                ),
                ("shared-lib".to_string(), "workspace:^1.2.3".to_string()),
                ("linked".to_string(), "portal:../linked-1.0".to_string()),
                ("local".to_string(), "file:../local-2.0".to_string()),
                ("pinned".to_string(), "github:user/repo#v1.2.3".to_string()),
                (
                    "tarball".to_string(),
                    "https://example.com/pkg-1.2.3.tgz".to_string(),
                ),
                (
                    "sshgit".to_string(),
                    "ssh://git@example.com/org/repo.git#semver:1.2.3".to_string(),
                ),
                ("aliased".to_string(), "npm:real-pkg@2.1.0".to_string()),
                ("koa".to_string(), "2.15.3".to_string()),
            ]),
            dev_dependencies: HashMap::new(),
            peer_dependencies: HashMap::new(),
            resolutions: HashMap::new(),
        });
        packages.source_paths.push(PathBuf::from("package.json"));
        packages.resolve_dependencies();

        let deps = synthetic_type_check_dependencies(&packages, &Default::default());
        for dropped in [
            "@metamask/controller-utils",
            "shared-lib",
            "linked",
            "local",
            "pinned",
            "tarball",
            "sshgit",
        ] {
            assert!(
                !deps.contains_key(dropped),
                "non-registry protocol spec for {dropped} must be dropped, got {:?}",
                deps.get(dropped)
            );
        }
        assert_eq!(
            deps.get("aliased").map(String::as_str),
            Some("npm:real-pkg@2.1.0"),
            "npm: aliases are registry-resolvable and must survive"
        );
        assert_eq!(deps.get("koa").map(String::as_str), Some("2.15.3"));
    }

    #[test]
    fn synthetic_type_check_deps_apply_resolutions_npm_aliases() {
        // metamask-extension declares invented dependency names
        // (`@types/readable-stream-2@^2.3.15`) that only resolve through a
        // yarn `resolutions` npm-alias. The merged version is the bare range,
        // so installing `name@range` 404s (killed the second external-OSS
        // probe) — the synthetic install must carry the alias spec instead.
        // Resolution keys may be `name@range` selectors (scoped names keep
        // their leading @) or plain names.
        use crate::packages::{PackageJson, Packages};
        use std::collections::HashMap;

        let mut packages = Packages::default();
        packages.package_jsons.push(PackageJson {
            name: Some("app".to_string()),
            version: Some("1.0.0".to_string()),
            dependencies: HashMap::from([
                (
                    "@types/readable-stream-2".to_string(),
                    "^2.3.15".to_string(),
                ),
                ("readable-stream-3".to_string(), "^3.6.2".to_string()),
                ("koa".to_string(), "2.15.3".to_string()),
                ("sneaky".to_string(), "^1.0.0".to_string()),
            ]),
            dev_dependencies: HashMap::new(),
            peer_dependencies: HashMap::new(),
            resolutions: HashMap::from([
                (
                    "@types/readable-stream-2@^2.3.15".to_string(),
                    "npm:@types/readable-stream@^2.3.15".to_string(),
                ),
                (
                    "readable-stream-3".to_string(),
                    "npm:readable-stream@^3.6.2".to_string(),
                ),
                ("koa".to_string(), "2.15.3".to_string()),
                // alias whose TARGET is itself a non-registry spec: must NOT
                // be applied (would smuggle a git spec past the filter)
                ("sneaky".to_string(), "npm:pkg@github:user/repo".to_string()),
            ]),
        });
        packages.source_paths.push(PathBuf::from("package.json"));
        packages.resolve_dependencies();

        let deps = synthetic_type_check_dependencies(&packages, &Default::default());
        assert_eq!(
            deps.get("@types/readable-stream-2").map(String::as_str),
            Some("npm:@types/readable-stream@^2.3.15"),
            "selector-keyed resolutions alias must replace the bare range"
        );
        assert_eq!(
            deps.get("readable-stream-3").map(String::as_str),
            Some("npm:readable-stream@^3.6.2"),
            "plain-keyed resolutions alias must replace the bare range"
        );
        assert_eq!(
            deps.get("koa").map(String::as_str),
            Some("2.15.3"),
            "non-alias resolutions (plain version pins) must not remap"
        );
        assert!(
            !deps.values().any(|v| v.contains("github:")),
            "an npm: alias with a non-registry target must not be applied, got {deps:?}"
        );
    }

    #[test]
    fn synthetic_type_check_deps_exclude_tree_walked_internal_names() {
        // Production shape (the corpus-3 baseline failure): only the
        // SERVICE's package.json is loaded into package_jsons — a workspace
        // member like packages/contracts never is — so the exclusion must
        // also honour the tree-walked internal_names set.
        use crate::packages::{PackageJson, Packages};
        use std::collections::HashMap;

        let mut packages = Packages::default();
        packages.package_jsons.push(PackageJson {
            name: Some("catalog-api".to_string()),
            version: Some("1.0.0".to_string()),
            dependencies: HashMap::from([
                ("@meridian/contracts".to_string(), "0.1.0".to_string()),
                ("koa".to_string(), "2.15.3".to_string()),
            ]),
            dev_dependencies: HashMap::new(),
            peer_dependencies: HashMap::new(),
            resolutions: HashMap::new(),
        });
        packages
            .source_paths
            .push(PathBuf::from("packages/catalog-api/package.json"));
        packages.resolve_dependencies();
        packages
            .internal_names
            .insert("@meridian/contracts".to_string());

        let deps = synthetic_type_check_dependencies(&packages, &Default::default());
        assert!(
            !deps.contains_key("@meridian/contracts"),
            "tree-walked internal name must not be npm-installed"
        );
        assert_eq!(deps.get("koa").map(String::as_str), Some("2.15.3"));
    }

    /// Test-local CloudRepoData with only the package-related fields set.
    fn repo_data_with_packages(
        repo_name: &str,
        packages: Option<crate::packages::Packages>,
        package_json: Option<String>,
    ) -> CloudRepoData {
        CloudRepoData {
            repo_name: repo_name.to_string(),
            service_name: None,
            endpoints: vec![],
            calls: vec![],
            mounts: vec![],
            apps: std::collections::HashMap::new(),
            imported_handlers: vec![],
            function_definitions: std::collections::HashMap::new(),
            config_json: None,
            package_json,
            packages,
            last_updated: chrono::Utc::now(),
            commit_hash: "deadbeef".to_string(),
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
            compat_verdicts: None,
        }
    }

    #[test]
    fn synthetic_type_check_deps_exclude_corpus_repo_packages() {
        // repo-a depends on @acme/shared-contracts at a version that was never
        // published to the registry — the package IS another repo/service in
        // the same scanned corpus. Its types arrive via that service's sidecar
        // .d.ts bundle, so the npm copy is redundant AND unresolvable (the
        // ETARGET used to abort the whole type pass, #390). The assembly must
        // exclude any name declared by any package.json in the corpus, not
        // just the current repo's own.
        use crate::packages::{PackageJson, Packages};
        use std::collections::HashMap;

        let mut packages = Packages::default();
        packages.package_jsons.push(PackageJson {
            name: Some("repo-a-api".to_string()),
            version: Some("1.0.0".to_string()),
            dependencies: HashMap::from([
                ("@acme/shared-contracts".to_string(), "0.4.2".to_string()),
                ("koa".to_string(), "2.15.3".to_string()),
            ]),
            dev_dependencies: HashMap::new(),
            peer_dependencies: HashMap::new(),
            resolutions: HashMap::new(),
        });
        packages.source_paths.push(PathBuf::from("package.json"));
        packages.resolve_dependencies();

        let corpus_internal =
            std::collections::HashSet::from(["@acme/shared-contracts".to_string()]);
        let deps = synthetic_type_check_dependencies(&packages, &corpus_internal);
        assert!(
            !deps.contains_key("@acme/shared-contracts"),
            "a package that is itself a repo/service in the scanned corpus must not be \
             npm-installed"
        );
        assert_eq!(deps.get("koa").map(String::as_str), Some("2.15.3"));
    }

    #[test]
    fn corpus_internal_names_union_all_repos_with_serialized_fallback() {
        use crate::packages::{PackageJson, Packages};
        use std::collections::HashMap;

        // Repo A: structured `packages` with a declared name + tree-walked
        // workspace member.
        let mut packages_a = Packages::default();
        packages_a.package_jsons.push(PackageJson {
            name: Some("@acme/shared-contracts".to_string()),
            version: Some("0.4.2".to_string()),
            dependencies: HashMap::new(),
            dev_dependencies: HashMap::new(),
            peer_dependencies: HashMap::new(),
            resolutions: HashMap::new(),
        });
        packages_a
            .internal_names
            .insert("@acme/shared-tooling".to_string());

        // Repo B: pre-`packages`-field payload — only the serialized
        // `package_json` string is available.
        let mut packages_b = Packages::default();
        packages_b.package_jsons.push(PackageJson {
            name: Some("repo-b-api".to_string()),
            version: Some("1.0.0".to_string()),
            dependencies: HashMap::new(),
            dev_dependencies: HashMap::new(),
            peer_dependencies: HashMap::new(),
            resolutions: HashMap::new(),
        });
        let serialized_b = serde_json::to_string(&packages_b).unwrap();

        let all_repo_data = vec![
            repo_data_with_packages("acme/repo-a", Some(packages_a), None),
            repo_data_with_packages("acme/repo-b", None, Some(serialized_b)),
        ];

        let names = corpus_internal_package_names(&all_repo_data);
        assert!(names.contains("@acme/shared-contracts"));
        assert!(names.contains("@acme/shared-tooling"));
        assert!(
            names.contains("repo-b-api"),
            "serialized fallback must count"
        );
    }

    #[test]
    fn npm_unresolvable_packages_parses_etarget_stderr() {
        // Synthetic stderr in npm 10's format (generic package names).
        let stderr = "\
npm error code ETARGET
npm error notarget No matching version found for @acme/shared-contracts@0.4.2.
npm error notarget In most cases you or one of your dependencies are requesting
npm error notarget a package version that doesn't exist.
npm error A complete log of this run can be found in: /home/user/.npm/_logs/debug-0.log
";
        assert_eq!(
            npm_unresolvable_packages(stderr),
            vec!["@acme/shared-contracts".to_string()]
        );
    }

    #[test]
    fn npm_unresolvable_packages_parses_unscoped_and_legacy_prefix() {
        // npm 8/9 prefix lines with `npm ERR!` instead of `npm error`; the
        // parser keys on the message text, not the prefix. Also dedupes.
        let stderr = "\
npm ERR! code ETARGET
npm ERR! notarget No matching version found for left-pad-utils@9.9.9.
npm ERR! notarget No matching version found for left-pad-utils@9.9.9.
";
        assert_eq!(
            npm_unresolvable_packages(stderr),
            vec!["left-pad-utils".to_string()]
        );
    }

    #[test]
    fn npm_unresolvable_packages_parses_e404_stderr() {
        let stderr = "\
npm error code E404
npm error 404 Not Found - GET https://registry.npmjs.org/@acme%2fghost-pkg - Not found
npm error 404
npm error 404  '@acme/ghost-pkg@^1.0.0' is not in this registry.
npm error 404
npm error 404 Note that you can also install from a
npm error 404 tarball, folder, http url, or git url.
";
        assert_eq!(
            npm_unresolvable_packages(stderr),
            vec!["@acme/ghost-pkg".to_string()]
        );
    }

    #[test]
    fn npm_unresolvable_packages_ignores_other_failure_classes() {
        // An ERESOLVE conflict names packages too, but is NOT a resolution
        // 404/ETARGET — dropping deps can't fix it, so it must stay a hard
        // error (empty parse → no retry).
        let stderr = "\
npm error code ERESOLVE
npm error ERESOLVE unable to resolve dependency tree
npm error Found: typescript@5.8.3
npm error Could not resolve dependency:
npm error peer typescript@\">=4.2\" from ts-node@10.9.2
";
        assert!(npm_unresolvable_packages(stderr).is_empty());
    }

    #[test]
    fn unresolvable_manifest_keys_match_direct_and_alias_targets() {
        use std::collections::HashMap;

        let deps = HashMap::from([
            ("@acme/ghost-pkg".to_string(), "1.0.0".to_string()),
            ("aliased".to_string(), "npm:ghost-target@2.0.0".to_string()),
            ("koa".to_string(), "2.15.3".to_string()),
        ]);
        let blamed = vec![
            "@acme/ghost-pkg".to_string(),
            // npm blames the alias TARGET, not the manifest key.
            "ghost-target".to_string(),
            // Transitive dependency (not a manifest key): nothing to drop.
            "not-in-manifest".to_string(),
        ];
        assert_eq!(
            unresolvable_manifest_keys(&deps, &blamed),
            vec!["@acme/ghost-pkg".to_string(), "aliased".to_string()]
        );
    }

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
            provenance: Default::default(),
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
            compat_verdicts: None,
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
            compat_verdicts: None,
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
            provenance: Default::default(),
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
            compat_verdicts: None,
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
            graphql_consumer_locates: vec![],
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
            graphql_operations: vec![],
            pubsub_operations: vec![],
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
        // Declared-internal env-var bases must be stripped from the consumer
        // manifest key. The canonical path is computed ONCE (via
        // `consumer_call_path`) at mount-graph build time and stored on the call;
        // `build_type_manifest_entries` reads that stored `canonical_path` so the
        // manifest key is byte-identical to the projection key for the same call.
        let config = Config {
            internal_env_vars: ["USER_SERVICE_URL", "NOTIFICATION_SERVICE_URL"]
                .iter()
                .map(|s| s.to_string())
                .collect(),
            ..Config::default()
        };
        let normalizer = UrlNormalizer::new(&config);
        let mk_call = |target: &str, file: &str| {
            let target = target.to_string();
            crate::mount_graph::DataFetchingCall {
                method: "GET".to_string(),
                canonical_path: normalizer.consumer_call_path(&target),
                target_url: target,
                client: "fetch".to_string(),
                file_location: file.to_string(),
                call_kind: None,
                repo_name: None,
                service_name: None,
            }
        };
        let mut mount_graph = MountGraph::new();
        mount_graph.data_calls = vec![
            mk_call(
                "`${USER_SERVICE_URL}/api/users/${order.userId}`",
                "src/orders.ts:42",
            ),
            mk_call(
                "`${NOTIFICATION_SERVICE_URL}/api/notifications/status`",
                "src/notify.ts:7",
            ),
        ];

        let entries = build_type_manifest_entries(&mount_graph, &config, ".");

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

    /// Regression for #334: two producer endpoints colliding on (method, path)
    /// share the plain key-only alias (producers carry no `_Call<id>` suffix),
    /// so both manifest entries got the same `type_alias` and one resolved
    /// definition silently clobbered the other in the bundle. Live trigger:
    /// the file-analyzer emitted a root route as "/:id", duplicating the real
    /// `GET /api/orders/:id` (#332). The first declaration wins; the duplicate
    /// is dropped with a warning.
    #[test]
    fn test_duplicate_producer_keys_yield_one_manifest_entry() {
        let config = Config::default();
        let mk_endpoint = |file: &str| crate::mount_graph::ResolvedEndpoint {
            method: "GET".to_string(),
            path: "/api/orders/:id".to_string(),
            full_path: "/api/orders/:id".to_string(),
            handler: None,
            owner: "app".to_string(),
            file_location: file.to_string(),
            middleware_chain: vec![],
            repo_name: None,
            service_name: None,
            provenance: Default::default(),
            evidence: carrick_match::MatchEvidence::RouteDefinition,
        };
        let mut mount_graph = MountGraph::new();
        mount_graph.endpoints = vec![
            mk_endpoint("src/routes/orders.ts:11"),
            mk_endpoint("src/routes/orders.ts:42"),
        ];

        let entries = build_type_manifest_entries(&mount_graph, &config, ".");

        let producer_aliases: Vec<&str> = entries
            .iter()
            .filter(|e| e.role == ManifestRole::Producer)
            .map(|e| e.type_alias.as_str())
            .collect();
        let unique: std::collections::HashSet<&&str> = producer_aliases.iter().collect();
        assert_eq!(
            producer_aliases.len(),
            unique.len(),
            "same-key producers must not share a manifest alias: {:?}",
            producer_aliases
        );
        // Lock in the drop-with-warning behavior: the duplicate is dropped,
        // not disambiguated, so exactly one producer entry survives.
        assert_eq!(
            producer_aliases.len(),
            1,
            "duplicate same-key producer must be dropped, leaving one entry: {:?}",
            producer_aliases
        );
        // The surviving entry is the first declaration.
        let survivor = entries
            .iter()
            .find(|e| e.role == ManifestRole::Producer)
            .expect("one producer entry expected");
        assert_eq!(survivor.file_path, "src/routes/orders.ts");
        assert_eq!(survivor.line_number, 11);
    }

    /// #379: a call-site-evidence entry never anchors Producer manifest
    /// types — a producer entry would make ts_check run a request-vs-request
    /// comparison mislabelled as a producer-contract verdict. The twin data
    /// call's Consumer entries are unaffected.
    #[test]
    fn test_call_site_evidence_endpoint_emits_no_producer_manifest_entries() {
        let config = Config::default();
        let mut mount_graph = MountGraph::new();
        mount_graph.endpoints = vec![crate::mount_graph::ResolvedEndpoint {
            method: "POST".to_string(),
            path: "/v2/widgets".to_string(),
            full_path: "/v2/widgets".to_string(),
            handler: None,
            owner: "app".to_string(),
            file_location: "operations/create-widget.ts:14".to_string(),
            middleware_chain: vec![],
            repo_name: None,
            service_name: None,
            provenance: Default::default(),
            evidence: carrick_match::MatchEvidence::CallSite,
        }];
        mount_graph.data_calls = vec![crate::mount_graph::DataFetchingCall {
            method: "POST".to_string(),
            target_url: "/v2/widgets".to_string(),
            canonical_path: "/v2/widgets".to_string(),
            client: "request".to_string(),
            file_location: "operations/create-widget.ts:14".to_string(),
            call_kind: None,
            repo_name: None,
            service_name: None,
        }];

        let entries = build_type_manifest_entries(&mount_graph, &config, ".");

        assert!(
            entries.iter().all(|e| e.role != ManifestRole::Producer),
            "call-site evidence must not produce Producer manifest entries: {:?}",
            entries
                .iter()
                .map(|e| (&e.role, &e.type_alias))
                .collect::<Vec<_>>()
        );
        assert!(
            entries.iter().any(|e| e.role == ManifestRole::Consumer),
            "the twin call's Consumer entries are still emitted"
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
                    graphql_consumer_locates: vec![],
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
                    graphql_operations: vec![],
                    pubsub_operations: vec![],
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
            compat_verdicts: None,
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
            compat_verdicts: None,
        };

        let stripped = strip_ast_nodes(data);

        // file_results should be preserved (small payload)
        assert!(
            stripped.file_results.is_some(),
            "file_results should be preserved when payload is small"
        );
    }

    /// #351 regression: `attach_compat_verdicts` runs AFTER the strip_ast_nodes
    /// size-guard pass, so verdicts appended to a payload that squeaked under
    /// the 5MB cap could re-inflate it past the Lambda limit and 413 the
    /// upload. The engine re-applies `enforce_payload_size_limit` after
    /// attachment; this pins that a near-limit payload with verdicts attached
    /// still respects the cap, with the SAME degradation order as the first
    /// pass — the bulky caches are dropped, the tiny verdicts are kept.
    #[test]
    fn test_payload_size_guard_reapplied_after_verdict_attachment() {
        const MAX_PAYLOAD_BYTES: usize = 5 * 1024 * 1024; // mirrors the guard

        let base = CloudRepoData {
            repo_name: "consumer-svc".to_string(),
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
            file_results: None,
            cached_detection: None,
            cached_guidance: None,
            cached_extraction_config: None,
            package_json_hash: None,
            cache_version: Some(CACHE_VERSION),
            type_extraction_status: None,
            compat_verdicts: None,
        };

        // Size the file_results filler so the payload lands just UNDER the 5MB
        // cap: the strip_ast_nodes guard pass keeps everything, and only the
        // verdicts appended afterwards push it over.
        let base_len = serde_json::to_string(&base).unwrap().len();
        let mut result = make_file_result(vec!["/api/orders"], vec![]);
        result.endpoints[0].payload_expression_text = Some(String::new());
        let mut probe = HashMap::new();
        probe.insert("src/big.ts".to_string(), result.clone());
        let mut with_empty = base.clone();
        with_empty.file_results = Some(probe);
        let overhead = serde_json::to_string(&with_empty).unwrap().len() - base_len;
        // 200 bytes of headroom below the cap — less than the verdicts add.
        let filler_len = MAX_PAYLOAD_BYTES - base_len - overhead - 200;
        result.endpoints[0].payload_expression_text = Some("x".repeat(filler_len));
        let mut file_results = HashMap::new();
        file_results.insert("src/big.ts".to_string(), result);
        let mut data = base;
        data.file_results = Some(file_results);

        // First guard pass (as strip_ast_nodes runs it): under the cap, so the
        // caches survive.
        let mut payloads = vec![strip_ast_nodes(data)];
        assert!(
            payloads[0].file_results.is_some(),
            "test setup: payload must start under the cap with caches intact"
        );
        let before = serde_json::to_string(&payloads[0]).unwrap().len();
        assert!(before <= MAX_PAYLOAD_BYTES);

        // Verdict attachment re-inflates past the cap (mismatch reason > the
        // 200-byte headroom).
        let matches = vec![crate::analyzer::CrossRepoMatch {
            producer_repo: "producer-svc".to_string(),
            producer_key: "http|GET|/api/orders/:id".to_string(),
            consumer_repo: "consumer-svc".to_string(),
            consumer_key: "http|GET|/api/orders/:id".to_string(),
            consumer_location: Some("src/client.ts".to_string()),
            match_score: 1.0,
            type_compatible: Some(false),
            mismatch_reason: Some("y".repeat(400)),
            producer_provenance: Default::default(),
            relationship: carrick_match::MatchRelationship::ProducerConsumer,
        }];
        crate::cloud_storage::attach_compat_verdicts(&mut payloads, &matches);
        assert!(
            serde_json::to_string(&payloads[0]).unwrap().len() > MAX_PAYLOAD_BYTES,
            "test setup: verdicts must push the payload over the cap"
        );

        // The engine's post-attachment pass: back under the cap, caches
        // dropped, verdicts kept.
        enforce_payload_size_limit(&mut payloads[0]);
        let after = serde_json::to_string(&payloads[0]).unwrap().len();
        assert!(
            after <= MAX_PAYLOAD_BYTES,
            "payload must respect the cap after verdict attachment ({}KB > {}KB)",
            after / 1024,
            MAX_PAYLOAD_BYTES / 1024
        );
        assert!(
            payloads[0].file_results.is_none(),
            "the bulky caches are what gets dropped"
        );
        assert!(
            payloads[0]
                .compat_verdicts
                .as_ref()
                .is_some_and(|v| v.len() == 1),
            "the tiny verdicts are kept"
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
                messaging_clients: vec![],
                notes: "test".to_string(),
            }),
            cached_guidance: None,
            cached_extraction_config: None,
            package_json_hash: Some("abc123hash".to_string()),
            cache_version: Some(CACHE_VERSION),
            type_extraction_status: None,
            compat_verdicts: None,
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
            compat_verdicts: None,
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
            primary_type_symbol: None,
            array_depth: None,
            primary_type_symbol_source: None,
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
            primary_type_symbol: None,
            array_depth: None,
            primary_type_symbol_source: None,
        });

        enrich_manifest_with_type_resolution(&mut manifest, &resolution, None);

        assert_eq!(manifest[0].type_state, ManifestTypeState::Unknown);
    }

    /// An inferred type carrying a deterministic symbol, keyed by the same
    /// `alias` as the manifest entry it enriches — the join the anchor fill
    /// uses. `type_string` is a resolved object, so only the anchor (not the
    /// type-state path) is under test here.
    fn inferred_with_symbol(symbol: &str) -> InferredType {
        InferredType {
            // Matches `consumer_entry("OrderView").type_alias`, so the anchor
            // join (by alias) fires.
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
            infer_kind: InferKind::ResponseBody,
            primary_type_symbol: Some(symbol.to_string()),
            array_depth: None,
            primary_type_symbol_source: None,
        }
    }

    /// #240: the deterministic anchor fills `primary_type_symbol` from the
    /// inferred symbol, joined by `alias`, when the LLM left it None.
    #[test]
    fn enrich_fills_anchor_from_inferred_symbol_when_llm_none() {
        let mut manifest = vec![consumer_entry("OrderView")];
        // The LLM stamped nothing onto this op.
        assert_eq!(manifest[0].primary_type_symbol, None);

        let mut resolution = empty_resolution();
        resolution
            .inferred_types
            .push(inferred_with_symbol("OrderView"));

        enrich_manifest_with_type_resolution(&mut manifest, &resolution, None);

        assert_eq!(
            manifest[0].primary_type_symbol.as_deref(),
            Some("OrderView"),
            "anchor must be filled from the inferred symbol when the LLM left it None"
        );
    }

    /// #240: an op the LLM already anchored correctly must NOT be overwritten by
    /// the inferred symbol — the deterministic fill is None-only so POST
    /// /payments and socket ops keep their model-emitted symbol.
    #[test]
    fn enrich_does_not_override_existing_llm_anchor() {
        let mut manifest = vec![consumer_entry("OrderView")];
        // The LLM already stamped the real symbol for this op.
        manifest[0].primary_type_symbol = Some("Payment".to_string());

        let mut resolution = empty_resolution();
        // The sidecar inferred a DIFFERENT symbol at the same location.
        resolution
            .inferred_types
            .push(inferred_with_symbol("OrderView"));

        enrich_manifest_with_type_resolution(&mut manifest, &resolution, None);

        assert_eq!(
            manifest[0].primary_type_symbol.as_deref(),
            Some("Payment"),
            "an existing LLM anchor must never be regressed by the inferred symbol"
        );
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
        anchor: Option<&str>,
    ) -> crate::graphql::GraphqlOp {
        crate::graphql::GraphqlOp {
            key: OperationKey::graphql(kind, field),
            file_path: PathBuf::from("src/schema.graphql"),
            line: 3,
            primary_type_symbol: anchor.map(String::from),
            payload_type_symbol: None,
            payload_type_source: None,
            resolver_file: None,
            resolver_line: None,
            response_type_symbol: None,
            response_type_source: None,
            consumer_located_type_symbol: None,
            consumer_located_type_source: None,
        }
    }

    /// A GraphQL consumer op carrying a `request<T>` call-site anchor in
    /// `payload_type_symbol` (the field SDL producers can't provide).
    fn graphql_consumer_op(
        field: &str,
        payload_symbol: Option<&str>,
        payload_source: Option<&str>,
    ) -> crate::graphql::GraphqlOp {
        crate::graphql::GraphqlOp {
            key: OperationKey::graphql(crate::operation::GraphqlOperationKind::Query, field),
            file_path: PathBuf::from("web-frontend/lib/graphql.ts"),
            line: 76,
            primary_type_symbol: None,
            payload_type_symbol: payload_symbol.map(String::from),
            payload_type_source: payload_source.map(String::from),
            resolver_file: None,
            resolver_line: None,
            response_type_symbol: None,
            response_type_source: None,
            consumer_located_type_symbol: None,
            consumer_located_type_source: None,
        }
    }

    /// #307 (class 2) helper: an LLM HTTP data call at a given file/target.
    fn transport_call(target: &str, file_location: &str) -> crate::mount_graph::DataFetchingCall {
        crate::mount_graph::DataFetchingCall {
            method: "POST".to_string(),
            target_url: target.to_string(),
            canonical_path: target.to_string(),
            client: "fetch(".to_string(),
            file_location: file_location.to_string(),
            call_kind: None,
            repo_name: None,
            service_name: None,
        }
    }

    /// #307 (class 2): an env-templated HTTP call in a file whose gql documents
    /// produced consumer ops is that file's transport — folded. A relative
    /// literal call in the same file (same-origin REST) and an env-templated
    /// call in an unrelated file both stay.
    #[test]
    fn fold_drops_graphql_transport_calls_only() {
        let mut mount_graph = MountGraph::new();
        mount_graph.data_calls = vec![
            transport_call("${SUPPORT_GQL_URL}/graphql", "src/gql.ts:25"),
            transport_call("/api/tickets", "src/gql.ts:30"),
            transport_call("${ORDERS_API}/orders", "src/orders.ts:12"),
        ];
        let graphql = crate::graphql::GraphqlExtraction {
            producers: vec![],
            consumers: vec![graphql_consumer_op_at(
                crate::operation::GraphqlOperationKind::Mutation,
                "escalateTicket",
                "src/gql.ts",
                None,
            )],
        };

        fold_graphql_transport_calls(&mut mount_graph, &graphql);

        let targets: Vec<&str> = mount_graph
            .data_calls
            .iter()
            .map(|c| c.target_url.as_str())
            .collect();
        assert_eq!(targets, vec!["/api/tickets", "${ORDERS_API}/orders"]);
    }

    /// A declared-internal env-var base strips to a bare path in
    /// `canonical_path` (`${GQL_URL}/graphql` → `/graphql`), so the transport
    /// shape must be read off the RAW target or the fold would leak for
    /// exactly the users who configured `internalEnvVars` (Copilot review).
    #[test]
    fn fold_reads_raw_target_not_stripped_canonical() {
        let mut mount_graph = MountGraph::new();
        mount_graph.data_calls = vec![crate::mount_graph::DataFetchingCall {
            method: "POST".to_string(),
            target_url: "`${SUPPORT_GQL_URL}/graphql`".to_string(),
            canonical_path: "/graphql".to_string(),
            client: "fetch(".to_string(),
            file_location: "src/gql.ts:25".to_string(),
            call_kind: None,
            repo_name: None,
            service_name: None,
        }];
        let graphql = crate::graphql::GraphqlExtraction {
            producers: vec![],
            consumers: vec![graphql_consumer_op_at(
                crate::operation::GraphqlOperationKind::Mutation,
                "escalateTicket",
                "src/gql.ts",
                None,
            )],
        };

        fold_graphql_transport_calls(&mut mount_graph, &graphql);

        assert!(
            mount_graph.data_calls.is_empty(),
            "internal-stripped canonical must still fold via the raw target"
        );
    }

    /// The file join must normalize path components: the graphql walk can
    /// yield a `./`-prefixed path while the data call's file key is bare.
    #[test]
    fn fold_joins_dot_prefixed_walk_paths() {
        let mut mount_graph = MountGraph::new();
        mount_graph.data_calls = vec![transport_call(
            "https://support.example.com/graphql",
            "src/gql.ts:25",
        )];
        let graphql = crate::graphql::GraphqlExtraction {
            producers: vec![],
            consumers: vec![graphql_consumer_op_at(
                crate::operation::GraphqlOperationKind::Query,
                "ticket",
                "./src/gql.ts",
                None,
            )],
        };

        fold_graphql_transport_calls(&mut mount_graph, &graphql);

        assert!(
            mount_graph.data_calls.is_empty(),
            "absolute-URL transport in a ./-walked gql-consumer file must fold"
        );
    }

    /// Producer-only extractions (an SDL service with no documents) must not
    /// fold anything — the transport fold is a CONSUMER-side dedup.
    #[test]
    fn fold_ignores_producer_only_extractions() {
        let mut mount_graph = MountGraph::new();
        mount_graph.data_calls = vec![transport_call("${SOME_API}/things", "src/schema.ts:5")];
        let graphql = crate::graphql::GraphqlExtraction {
            producers: vec![graphql_op(
                crate::operation::GraphqlOperationKind::Query,
                "order",
                Some("Order"),
            )],
            consumers: vec![],
        };

        fold_graphql_transport_calls(&mut mount_graph, &graphql);

        assert_eq!(mount_graph.data_calls.len(), 1);
    }

    /// Variant of `graphql_consumer_op` with a caller-chosen `kind` and
    /// `file_path`, for the #268 per-file/per-kind join tests: the consumer
    /// locate merge is keyed on `(file_path, kind, field)`, so exercising
    /// fan-in across files or a non-Query kind needs a helper that can vary
    /// both (the plain `graphql_consumer_op` fixes kind=Query and
    /// file_path="web-frontend/lib/graphql.ts").
    fn graphql_consumer_op_at(
        kind: crate::operation::GraphqlOperationKind,
        field: &str,
        file_path: &str,
        payload_symbol: Option<&str>,
    ) -> crate::graphql::GraphqlOp {
        crate::graphql::GraphqlOp {
            key: OperationKey::graphql(kind, field),
            file_path: PathBuf::from(file_path),
            line: 10,
            primary_type_symbol: None,
            payload_type_symbol: payload_symbol.map(String::from),
            payload_type_source: None,
            resolver_file: None,
            resolver_line: None,
            response_type_symbol: None,
            response_type_source: None,
            consumer_located_type_symbol: None,
            consumer_located_type_source: None,
        }
    }

    /// A pub/sub op the file-analyzer would emit: topic + side + decoded-payload
    /// `primary_type_symbol`. Mirrors `socket_op`/`graphql_op` for the manifest
    /// tests.
    fn pubsub_op(
        topic: &str,
        role: crate::operation::PubsubRole,
        symbol: Option<&str>,
        source: Option<&str>,
    ) -> crate::agents::file_analyzer_agent::PubsubOperation {
        crate::agents::file_analyzer_agent::PubsubOperation {
            topic: topic.to_string(),
            role: Some(role),
            line_number: 14,
            primary_type_symbol: symbol.map(String::from),
            type_import_source: source.map(String::from),
            broker: Some("redis".to_string()),
            payload_expression_text: None,
            payload_expression_line: None,
        }
    }

    /// Stage B1 merge: the file-analyzer's `graphql_operations` join their
    /// resolver location onto the SDL producer sharing the same canonical key.
    /// The SDL producer alone has no resolver location; after the merge it points
    /// at the resolver file/line so the producer can take the `FunctionReturn`
    /// infer path (its real response contract is the resolver's expanded return).
    #[test]
    fn merge_graphql_resolver_locations_joins_llm_op_onto_sdl_producer() {
        use crate::agents::file_analyzer_agent::GraphqlOperation;
        use crate::operation::GraphqlOperationKind;

        // SDL producer `graphql|query|order` with no resolver location yet.
        let mut graphql = crate::graphql::GraphqlExtraction {
            producers: vec![
                graphql_op(GraphqlOperationKind::Query, "order", Some("Order")),
                // A second producer the LLM never reports a resolver for: it must
                // stay `None` (no spurious join).
                graphql_op(GraphqlOperationKind::Query, "orders", Some("[Order!]!")),
            ],
            consumers: vec![],
        };

        // file_results keyed by path, carrying the matching LLM graphql_operation
        // plus one op (`createOrder`) with NO SDL producer (must be ignored).
        let mut file_results: HashMap<String, FileAnalysisResult> = HashMap::new();
        file_results.insert(
            "packages/gateway/src/orders.resolver.ts".to_string(),
            FileAnalysisResult {
                graphql_consumer_locates: vec![],
                mounts: vec![],
                endpoints: vec![],
                data_calls: vec![],
                graphql_operations: vec![
                    GraphqlOperation {
                        kind: GraphqlOperationKind::Query,
                        field: "order".to_string(),
                        resolver_function: Some("resolveOrder".to_string()),
                        resolver_line: Some(38),
                        primary_type_symbol: Some("ApiResponse".to_string()),
                        type_import_source: None,
                        // Even if the model ALSO emits a backing type here, the
                        // resolver path must win (regression guard: bundling the
                        // bare `Order` would drop the ApiResponse envelope).
                        backing_type_symbol: Some("Order".to_string()),
                        backing_type_source: None,
                    },
                    GraphqlOperation {
                        kind: GraphqlOperationKind::Mutation,
                        field: "createOrder".to_string(),
                        resolver_function: Some("createOrder".to_string()),
                        resolver_line: Some(7),
                        primary_type_symbol: None,
                        type_import_source: None,
                        backing_type_symbol: None,
                        backing_type_source: None,
                    },
                ],
                pubsub_operations: vec![],
            },
        );

        merge_graphql_resolver_locations(&mut graphql, &file_results);

        let order = graphql
            .producers
            .iter()
            .find(|op| op.key.canonical() == "graphql|query|order")
            .expect("order producer");
        assert_eq!(
            order.resolver_file,
            Some(PathBuf::from("packages/gateway/src/orders.resolver.ts")),
            "the resolver file must come from the file_results key"
        );
        assert_eq!(order.resolver_line, Some(38));
        // The SDL anchor is untouched by the merge.
        assert_eq!(order.primary_type_symbol.as_deref(), Some("Order"));
        // A resolver was matched, so the FunctionReturn path wins even though the
        // op also carries a `backing_type_symbol`: the type-locate fallback must
        // NOT fire (bundling the bare `Order` would drop the ApiResponse
        // envelope — the exact live-eval regression this guards against).
        assert_eq!(order.response_type_symbol, None);

        // The producer with no matching LLM op keeps both resolver fields None
        // and gains no type-locate fallback.
        let orders = graphql
            .producers
            .iter()
            .find(|op| op.key.canonical() == "graphql|query|orders")
            .expect("orders producer");
        assert_eq!(orders.resolver_file, None);
        assert_eq!(orders.resolver_line, None);
        assert_eq!(orders.response_type_symbol, None);

        // The LLM op with no SDL producer (`mutation createOrder`) created no
        // new producer — it was ignored.
        assert!(
            !graphql
                .producers
                .iter()
                .any(|op| op.key.canonical() == "graphql|mutation|createOrder"),
            "an LLM op with no matching SDL producer must not create a producer"
        );
    }

    /// #248: an SDL producer field with NO resolver function but a co-located
    /// backing type (the LLM emits `primary_type_symbol` with a null
    /// `resolver_function`) picks up the type-locate fallback — the scanner
    /// records the backing type so the sidecar can bundle + list-wrap it. The
    /// resolver locators stay `None` (no FunctionReturn), and `resolver_file` is
    /// still stamped with the file the entry came from.
    #[test]
    fn merge_graphql_type_locate_for_resolverless_field() {
        use crate::agents::file_analyzer_agent::GraphqlOperation;
        use crate::operation::GraphqlOperationKind;

        let mut graphql = crate::graphql::GraphqlExtraction {
            producers: vec![graphql_op(
                GraphqlOperationKind::Query,
                "orders",
                Some("[Order!]!"),
            )],
            consumers: vec![],
        };

        let mut file_results: HashMap<String, FileAnalysisResult> = HashMap::new();
        file_results.insert(
            "packages/gateway/src/orders.resolver.ts".to_string(),
            FileAnalysisResult {
                graphql_consumer_locates: vec![],
                mounts: vec![],
                endpoints: vec![],
                data_calls: vec![],
                graphql_operations: vec![GraphqlOperation {
                    kind: GraphqlOperationKind::Query,
                    field: "orders".to_string(),
                    // No resolver — the field is backed only by a co-located type,
                    // carried on the dedicated backing_type_symbol.
                    resolver_function: None,
                    resolver_line: None,
                    primary_type_symbol: None,
                    type_import_source: None,
                    backing_type_symbol: Some("Order".to_string()),
                    backing_type_source: None,
                }],
                pubsub_operations: vec![],
            },
        );

        merge_graphql_resolver_locations(&mut graphql, &file_results);

        let orders = graphql
            .producers
            .iter()
            .find(|op| op.key.canonical() == "graphql|query|orders")
            .expect("orders producer");
        assert_eq!(
            orders.resolver_file,
            Some(PathBuf::from("packages/gateway/src/orders.resolver.ts")),
            "the file the entry came from is still recorded"
        );
        // No FunctionReturn: the resolver locators stay unset.
        assert_eq!(orders.resolver_line, None);
        // The type-locate fallback carries the backing type for the sidecar.
        assert_eq!(orders.response_type_symbol.as_deref(), Some("Order"));
        assert_eq!(orders.response_type_source, None);
        // The SDL anchor is untouched.
        assert_eq!(orders.primary_type_symbol.as_deref(), Some("[Order!]!"));
    }

    /// A whitespace-only `resolver_function` (e.g. `" "`) must be treated the
    /// same as `None`/empty: it is not a real function name, so it must not
    /// take the FunctionReturn path, and the type-locate fallback (backing
    /// type) must still fire.
    #[test]
    fn merge_graphql_whitespace_only_resolver_function_is_treated_as_absent() {
        use crate::agents::file_analyzer_agent::GraphqlOperation;
        use crate::operation::GraphqlOperationKind;

        let mut graphql = crate::graphql::GraphqlExtraction {
            producers: vec![graphql_op(
                GraphqlOperationKind::Query,
                "orders",
                Some("[Order!]!"),
            )],
            consumers: vec![],
        };

        let mut file_results: HashMap<String, FileAnalysisResult> = HashMap::new();
        file_results.insert(
            "packages/gateway/src/orders.resolver.ts".to_string(),
            FileAnalysisResult {
                graphql_consumer_locates: vec![],
                mounts: vec![],
                endpoints: vec![],
                data_calls: vec![],
                graphql_operations: vec![GraphqlOperation {
                    kind: GraphqlOperationKind::Query,
                    field: "orders".to_string(),
                    // Whitespace only, not a real resolver name.
                    resolver_function: Some("  ".to_string()),
                    resolver_line: Some(38),
                    primary_type_symbol: None,
                    type_import_source: None,
                    backing_type_symbol: Some("Order".to_string()),
                    backing_type_source: None,
                }],
                pubsub_operations: vec![],
            },
        );

        merge_graphql_resolver_locations(&mut graphql, &file_results);

        let orders = graphql
            .producers
            .iter()
            .find(|op| op.key.canonical() == "graphql|query|orders")
            .expect("orders producer");
        // No FunctionReturn: a whitespace-only name must not anchor a resolver.
        assert_eq!(orders.resolver_line, None);
        // The type-locate fallback still fires (the producer stays anchored).
        assert_eq!(orders.response_type_symbol.as_deref(), Some("Order"));
    }

    /// A named `resolver_function` whose `resolver_line` is unusable (absent,
    /// or non-positive so it clamps to `None`) must NOT take the
    /// FunctionReturn path with a dead locator:
    /// `collect_graphql_producer_infer_requests` requires BOTH `resolver_file`
    /// and `resolver_line`, so the op would emit no infer request, and the
    /// skipped backing-type fallback would emit no `SymbolRequest` either —
    /// the producer ends up with no type request of any kind. When the line
    /// is unusable, the backing-type fallback must fire instead.
    #[test]
    fn merge_graphql_unusable_resolver_line_falls_back_to_backing_type() {
        use crate::agents::file_analyzer_agent::GraphqlOperation;
        use crate::operation::GraphqlOperationKind;

        let mut graphql = crate::graphql::GraphqlExtraction {
            producers: vec![
                graphql_op(GraphqlOperationKind::Query, "order", Some("Order")),
                graphql_op(GraphqlOperationKind::Query, "orders", Some("[Order!]!")),
            ],
            consumers: vec![],
        };

        let mut file_results: HashMap<String, FileAnalysisResult> = HashMap::new();
        file_results.insert(
            "packages/gateway/src/orders.resolver.ts".to_string(),
            FileAnalysisResult {
                graphql_consumer_locates: vec![],
                mounts: vec![],
                endpoints: vec![],
                data_calls: vec![],
                graphql_operations: vec![
                    // Named resolver, but the model omitted the line.
                    GraphqlOperation {
                        kind: GraphqlOperationKind::Query,
                        field: "order".to_string(),
                        resolver_function: Some("resolveOrder".to_string()),
                        resolver_line: None,
                        primary_type_symbol: None,
                        type_import_source: None,
                        backing_type_symbol: Some("Order".to_string()),
                        backing_type_source: None,
                    },
                    // Named resolver with a non-positive line (clamps to None).
                    GraphqlOperation {
                        kind: GraphqlOperationKind::Query,
                        field: "orders".to_string(),
                        resolver_function: Some("resolveOrders".to_string()),
                        resolver_line: Some(0),
                        primary_type_symbol: None,
                        type_import_source: None,
                        backing_type_symbol: Some("Order".to_string()),
                        backing_type_source: None,
                    },
                ],
                pubsub_operations: vec![],
            },
        );

        merge_graphql_resolver_locations(&mut graphql, &file_results);

        for field in ["order", "orders"] {
            let producer = graphql
                .producers
                .iter()
                .find(|op| op.key.canonical() == format!("graphql|query|{field}"))
                .expect("producer");
            // No FunctionReturn: an unusable line cannot anchor a resolver.
            assert_eq!(
                producer.resolver_line, None,
                "{field}: an unusable resolver_line must stay None"
            );
            // The backing-type fallback must fire so the producer stays anchored.
            assert_eq!(
                producer.response_type_symbol.as_deref(),
                Some("Order"),
                "{field}: the backing-type fallback must fire when the \
                 resolver line is unusable"
            );
        }

        // End to end: no dead FunctionReturn infer requests, and each producer
        // gets a backing-type SymbolRequest — a type request of SOME kind.
        let orchestrator = FileOrchestrator::new(AgentService::new());
        assert!(
            orchestrator
                .collect_graphql_producer_infer_requests(&graphql, ".")
                .is_empty(),
            "no resolver line means no FunctionReturn infer request"
        );
        let requests = orchestrator.collect_graphql_type_requests(&graphql, ".");
        assert_eq!(
            requests.len(),
            2,
            "each producer must emit a backing-type SymbolRequest, got: {:?}",
            requests
        );
    }

    /// #268 isolation guard: a consumer op with an explicit call-site generic
    /// (`payload_type_symbol` already set by the deterministic
    /// `TaggedTplVisitor::capture_request_call` pass) must keep that anchor
    /// even when a stray `graphql_consumer_locates` entry also matches its
    /// `(file_path, kind, field)` — mirrors 186cb27's resolver-first
    /// regression guard on the producer side.
    #[test]
    fn merge_graphql_consumer_locations_never_overrides_explicit_generic() {
        use crate::agents::file_analyzer_agent::GraphqlConsumerLocate;
        use crate::operation::GraphqlOperationKind;

        let anchored = graphql_consumer_op_at(
            GraphqlOperationKind::Query,
            "order",
            "web-frontend/lib/graphql.ts",
            Some("OrderView"),
        );
        let mut graphql = crate::graphql::GraphqlExtraction {
            producers: vec![],
            consumers: vec![anchored],
        };

        let mut file_results: HashMap<String, FileAnalysisResult> = HashMap::new();
        file_results.insert(
            "web-frontend/lib/graphql.ts".to_string(),
            FileAnalysisResult {
                // A stray/hallucinated locate entry for an op that is already
                // anchored — must be ignored.
                graphql_consumer_locates: vec![GraphqlConsumerLocate {
                    kind: GraphqlOperationKind::Query,
                    field: "order".to_string(),
                    result_type_symbol: "StrayType".to_string(),
                    result_type_source: None,
                }],
                ..Default::default()
            },
        );

        merge_graphql_consumer_locations(&mut graphql, &file_results);

        let order = &graphql.consumers[0];
        assert_eq!(
            order.payload_type_symbol.as_deref(),
            Some("OrderView"),
            "the explicit call-site anchor must be untouched"
        );
        assert_eq!(
            order.consumer_located_type_symbol, None,
            "a stray locate entry must NEVER populate the fallback when the op \
             is already anchored"
        );
    }

    /// #268 per-file join proof: the SAME `(kind, field)` consumed from TWO
    /// different files (fan-in) must each get their OWN located type — the
    /// join is keyed on `(file_path, kind, field)`, not the canonical key
    /// alone. A canonical-key-only map (the producer merge's approach) would
    /// collide every file's locate entry onto whichever consumer op happened
    /// to occupy that key first; this is the load-bearing difference from
    /// `merge_graphql_resolver_locations`.
    #[test]
    fn merge_graphql_consumer_locations_scopes_fan_in_per_file() {
        use crate::agents::file_analyzer_agent::GraphqlConsumerLocate;
        use crate::operation::GraphqlOperationKind;

        let consumer_a = graphql_consumer_op_at(
            GraphqlOperationKind::Subscription,
            "orderUpdated",
            "web-frontend/lib/graphql.ts",
            None,
        );
        let consumer_b = graphql_consumer_op_at(
            GraphqlOperationKind::Subscription,
            "orderUpdated",
            "admin-dashboard/lib/graphql.ts",
            None,
        );
        let mut graphql = crate::graphql::GraphqlExtraction {
            producers: vec![],
            consumers: vec![consumer_a, consumer_b],
        };

        let mut file_results: HashMap<String, FileAnalysisResult> = HashMap::new();
        file_results.insert(
            "web-frontend/lib/graphql.ts".to_string(),
            FileAnalysisResult {
                graphql_consumer_locates: vec![GraphqlConsumerLocate {
                    kind: GraphqlOperationKind::Subscription,
                    field: "orderUpdated".to_string(),
                    result_type_symbol: "OrderUpdate".to_string(),
                    result_type_source: None,
                }],
                ..Default::default()
            },
        );
        file_results.insert(
            "admin-dashboard/lib/graphql.ts".to_string(),
            FileAnalysisResult {
                graphql_consumer_locates: vec![GraphqlConsumerLocate {
                    kind: GraphqlOperationKind::Subscription,
                    field: "orderUpdated".to_string(),
                    result_type_symbol: "AdminOrderUpdate".to_string(),
                    result_type_source: None,
                }],
                ..Default::default()
            },
        );

        merge_graphql_consumer_locations(&mut graphql, &file_results);

        let web = graphql
            .consumers
            .iter()
            .find(|op| op.file_path == Path::new("web-frontend/lib/graphql.ts"))
            .expect("web-frontend consumer");
        assert_eq!(
            web.consumer_located_type_symbol.as_deref(),
            Some("OrderUpdate"),
            "web-frontend must get its OWN located type"
        );
        let admin = graphql
            .consumers
            .iter()
            .find(|op| op.file_path == Path::new("admin-dashboard/lib/graphql.ts"))
            .expect("admin-dashboard consumer");
        assert_eq!(
            admin.consumer_located_type_symbol.as_deref(),
            Some("AdminOrderUpdate"),
            "admin-dashboard must get its OWN located type, not web-frontend's"
        );
    }

    /// #268: a `graphql_consumer_locates` entry with no matching consumer op
    /// (wrong file, or wrong kind/field) is ignored — it must not create a new
    /// consumer op, touch an unrelated one, or panic.
    #[test]
    fn merge_graphql_consumer_locations_ignores_unmatched_locate_entry() {
        use crate::agents::file_analyzer_agent::GraphqlConsumerLocate;
        use crate::operation::GraphqlOperationKind;

        let consumer = graphql_consumer_op_at(
            GraphqlOperationKind::Query,
            "order",
            "web-frontend/lib/graphql.ts",
            None,
        );
        let mut graphql = crate::graphql::GraphqlExtraction {
            producers: vec![],
            consumers: vec![consumer],
        };

        let mut file_results: HashMap<String, FileAnalysisResult> = HashMap::new();
        file_results.insert(
            "web-frontend/lib/graphql.ts".to_string(),
            FileAnalysisResult {
                // A locate entry for a field with no matching consumer op in
                // this file.
                graphql_consumer_locates: vec![GraphqlConsumerLocate {
                    kind: GraphqlOperationKind::Mutation,
                    field: "refundOrder".to_string(),
                    result_type_symbol: "RefundReceipt".to_string(),
                    result_type_source: None,
                }],
                ..Default::default()
            },
        );

        merge_graphql_consumer_locations(&mut graphql, &file_results);

        assert_eq!(graphql.consumers.len(), 1, "no new consumer op was created");
        assert_eq!(
            graphql.consumers[0].consumer_located_type_symbol, None,
            "the unrelated `order` consumer must not pick up the unmatched entry"
        );
    }

    /// A typed socket emitter produces a Response-kind manifest entry keyed by
    /// the socket OperationKey, carrying the captured payload symbol as the
    /// anchor. A GraphQL SDL producer carries its deterministic SDL type
    /// expression as the anchor (#248); a document consumer has no SDL type so
    /// its anchor stays `None`.
    #[test]
    fn protocol_manifest_entries_anchor_sockets_and_graphql_producers() {
        use crate::operation::{GraphqlOperationKind, SocketDirection};

        let extractions = ProtocolExtractions {
            graphql: crate::graphql::GraphqlExtraction {
                producers: vec![graphql_op(
                    GraphqlOperationKind::Query,
                    "order",
                    Some("Order"),
                )],
                consumers: vec![graphql_op(GraphqlOperationKind::Query, "order", None)],
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

        // The SDL producer carries its deterministic SDL-type anchor (#248).
        let graphql_producer = entries
            .iter()
            .find(|e| {
                e.key.canonical() == "graphql|query|order" && e.role == ManifestRole::Producer
            })
            .expect("graphql producer manifest entry");
        assert_eq!(
            graphql_producer.primary_type_symbol.as_deref(),
            Some("Order"),
            "the SDL producer anchor must be the field's SDL type expression"
        );
        assert!(
            !graphql_producer.type_alias.is_empty(),
            "graphql op must get a stable type_alias"
        );
        assert_eq!(graphql_producer.type_state, ManifestTypeState::Unknown);

        // The document consumer has no SDL type, so its anchor stays unset.
        let graphql_consumer = entries
            .iter()
            .find(|e| {
                e.key.canonical() == "graphql|query|order" && e.role == ManifestRole::Consumer
            })
            .expect("graphql consumer manifest entry");
        assert_eq!(graphql_consumer.primary_type_symbol, None);
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

    /// The same fragile alias contract for pub/sub (PR-6, corpus-2 resolution dim):
    /// the SymbolRequest alias produced by `collect_pubsub_type_requests` MUST
    /// byte-match the manifest entry's alias from `append_pubsub_manifest_entries`,
    /// or the resolved payload `.d.ts` never joins back and the op stays
    /// `Unknown` — silently. A subscriber is the producer side.
    #[test]
    fn pubsub_symbol_request_alias_matches_manifest_alias() {
        use crate::operation::PubsubRole;

        let mut file_results: HashMap<String, FileAnalysisResult> = HashMap::new();
        file_results.insert(
            "web-dashboard/lib/realtime.ts".to_string(),
            FileAnalysisResult {
                pubsub_operations: vec![pubsub_op(
                    "metrics.page_view",
                    PubsubRole::Subscriber,
                    Some("PageView"),
                    Some("./types/metrics"),
                )],
                ..Default::default()
            },
        );

        let mut entries = Vec::new();
        append_pubsub_manifest_entries(
            &mut entries,
            &file_results,
            &crate::socket_io::SocketExtraction::default(),
            ".",
        );
        let manifest_alias = entries
            .iter()
            .find(|e| e.key.canonical() == "pubsub|metrics.page_view")
            .map(|e| e.type_alias.clone())
            .expect("pubsub manifest entry");

        let orchestrator = FileOrchestrator::new(AgentService::new());
        let requests = orchestrator.collect_pubsub_type_requests(&file_results, ".");
        let request = requests
            .iter()
            .find(|r| r.symbol_name == "PageView")
            .expect("pubsub SymbolRequest");

        assert_eq!(
            request.alias.as_deref(),
            Some(manifest_alias.as_str()),
            "pubsub SymbolRequest.alias must byte-match the manifest entry's alias \
             or the resolution enrich-join silently breaks"
        );
        // Independently confirm both equal the canonical builder output
        // (subscriber = producer side).
        let expected = crate::type_manifest::build_manifest_type_alias(
            &OperationKey::pubsub("metrics.page_view"),
            ManifestRole::Producer,
            ManifestTypeKind::Response,
        );
        assert_eq!(manifest_alias, expected);
        assert_eq!(request.alias.as_deref(), Some(expected.as_str()));
    }

    /// Fan-in regression (the corpus-2 compat false-negative): two publishers on
    /// the SAME topic but in different repos/files must get DISTINCT consumer
    /// aliases. They previously hashed to one alias (`build_manifest_type_alias`
    /// keys only on `topic|consumer|Response`), so ts_check's bundled
    /// `cross-repo-consumers` declared that interface twice with different bodies
    /// — one publisher's payload type masked the other's and the masked edge
    /// reported a spurious compat mismatch. The publisher alias now disambiguates
    /// by call site, and each publisher's SymbolRequest alias still byte-matches
    /// its own manifest entry so the resolution join holds.
    #[test]
    fn pubsub_fan_in_publishers_get_distinct_consumer_aliases() {
        use crate::operation::PubsubRole;

        let mut file_results: HashMap<String, FileAnalysisResult> = HashMap::new();
        // Two repos publishing `order.placed` with a same-named `OrderPlaced`
        // symbol whose definition differs per repo — the exact corpus-2 shape.
        file_results.insert(
            "orders-engine/src/publish.ts".to_string(),
            FileAnalysisResult {
                pubsub_operations: vec![pubsub_op(
                    "order.placed",
                    PubsubRole::Publisher,
                    Some("OrderPlaced"),
                    Some("./types/order"),
                )],
                ..Default::default()
            },
        );
        file_results.insert(
            "billing-svc/src/emit.ts".to_string(),
            FileAnalysisResult {
                pubsub_operations: vec![pubsub_op(
                    "order.placed",
                    PubsubRole::Publisher,
                    Some("OrderPlaced"),
                    Some("./types/order"),
                )],
                ..Default::default()
            },
        );

        let mut entries = Vec::new();
        append_pubsub_manifest_entries(
            &mut entries,
            &file_results,
            &crate::socket_io::SocketExtraction::default(),
            ".",
        );
        let manifest_aliases: HashSet<String> = entries
            .iter()
            .filter(|e| e.key.canonical() == "pubsub|order.placed")
            .map(|e| e.type_alias.clone())
            .collect();
        assert_eq!(
            manifest_aliases.len(),
            2,
            "two fan-in publishers must yield two DISTINCT consumer aliases, not \
             one collided alias (which masks one payload type in ts_check)"
        );

        // Each publisher's SymbolRequest alias must byte-match its manifest alias
        // (same call site → same call_id), so the resolution enrich-join holds.
        let orchestrator = FileOrchestrator::new(AgentService::new());
        let requests = orchestrator.collect_pubsub_type_requests(&file_results, ".");
        let request_aliases: HashSet<String> =
            requests.iter().filter_map(|r| r.alias.clone()).collect();
        assert_eq!(
            request_aliases, manifest_aliases,
            "each publisher's SymbolRequest alias must byte-match its manifest \
             entry's alias across the fan-in set"
        );
    }

    /// The same fragile alias contract for the pub/sub INFER path (wrapper
    /// patterns whose payload type is generic-bound, never a named symbol):
    /// the `InferRequestItem` alias produced by `collect_pubsub_infer_requests`
    /// MUST byte-match the manifest entry's alias from
    /// `append_pubsub_manifest_entries` — same plain alias for subscribers
    /// (producers), same call-site-disambiguated alias for publishers
    /// (consumers) — or the resolved payload type never joins back and the op
    /// stays `Unknown`, silently. Also pins the role → InferKind routing, the
    /// two-anchor co-emission (#413: a named anchor no longer suppresses the
    /// infer request — the sidecar arbitrates), and the envelope-copy guard.
    #[test]
    fn pubsub_infer_request_alias_matches_manifest_alias() {
        use crate::operation::PubsubRole;
        use crate::services::type_sidecar::InferKind;

        let locator_op = |topic: &str, role: PubsubRole, line: i32, text: &str| {
            crate::agents::file_analyzer_agent::PubsubOperation {
                topic: topic.to_string(),
                role: Some(role),
                line_number: line,
                primary_type_symbol: None,
                type_import_source: None,
                broker: None,
                payload_expression_text: Some(text.to_string()),
                payload_expression_line: Some(line),
            }
        };

        let mut file_results: HashMap<String, FileAnalysisResult> = HashMap::new();
        file_results.insert(
            "relay/src/relay.ts".to_string(),
            FileAnalysisResult {
                pubsub_operations: vec![locator_op(
                    "itemArchived",
                    PubsubRole::Subscriber,
                    13,
                    "{ time, item }",
                )],
                ..Default::default()
            },
        );
        file_results.insert(
            "dispatch/src/dispatch.ts".to_string(),
            FileAnalysisResult {
                pubsub_operations: vec![
                    locator_op("itemArchived", PubsubRole::Publisher, 9, "event"),
                    // Named anchor present WITH a usable locator → the op
                    // co-emits an infer request (#413) so the sidecar can
                    // arbitrate the two anchors; the explicit bundle still
                    // wins unless a borrow witness plus a root disagreement
                    // demotes it.
                    crate::agents::file_analyzer_agent::PubsubOperation {
                        topic: "orders.placed".to_string(),
                        role: Some(PubsubRole::Publisher),
                        line_number: 30,
                        primary_type_symbol: Some("OrderPlaced".to_string()),
                        type_import_source: None,
                        broker: None,
                        payload_expression_text: Some("order".to_string()),
                        payload_expression_line: Some(30),
                    },
                    // Envelope-copy guard: the locator text contains the op's
                    // own topic literal (the model copied the whole enqueue
                    // options object), so it must be dropped — an envelope's
                    // type on the manifest is a false compat verdict waiting
                    // to happen; Unknown is recoverable.
                    locator_op(
                        "records.reindex",
                        PubsubRole::Publisher,
                        41,
                        "{ id, job: \"records.reindex\", payload: { resourceId } }",
                    ),
                ],
                ..Default::default()
            },
        );

        let mut entries = Vec::new();
        append_pubsub_manifest_entries(
            &mut entries,
            &file_results,
            &crate::socket_io::SocketExtraction::default(),
            ".",
        );
        let manifest_aliases: HashMap<ManifestRole, String> = entries
            .iter()
            .filter(|e| e.key.canonical() == "pubsub|itemArchived")
            .map(|e| (e.role, e.type_alias.clone()))
            .collect();

        let orchestrator = FileOrchestrator::new(AgentService::new());
        let requests = orchestrator.collect_pubsub_infer_requests(&file_results, ".");

        // Co-emission + envelope guard: the two locator-anchored itemArchived
        // ops AND the named-anchor orders.placed op produce requests (#413);
        // only the topic-containing envelope copy is excluded.
        assert_eq!(requests.len(), 3, "requests: {requests:?}");
        assert!(
            requests
                .iter()
                .any(|r| r.expression_text.as_deref() == Some("order")),
            "an op with a primary_type_symbol and a usable locator must ALSO \
             emit an infer request so the sidecar can arbitrate the anchors"
        );
        assert!(
            !requests.iter().any(|r| r
                .expression_text
                .as_deref()
                .is_some_and(|t| t.contains("records.reindex"))),
            "a locator containing the op's topic literal is an envelope copy \
             and must be dropped, not resolved"
        );

        // Subscriber (producer side): FunctionParam, param_name = locator text,
        // plain alias byte-matching the manifest.
        let subscriber = requests
            .iter()
            .find(|r| r.infer_kind == InferKind::FunctionParam)
            .expect("subscriber infer request");
        assert_eq!(subscriber.param_name.as_deref(), Some("{ time, item }"));
        assert_eq!(
            subscriber.alias.as_deref(),
            manifest_aliases
                .get(&ManifestRole::Producer)
                .map(|s| s.as_str()),
            "subscriber infer alias must byte-match the Producer manifest alias"
        );

        // Publisher (consumer side): Expression, expression_text = locator
        // text, call-site alias byte-matching the manifest.
        let publisher = requests
            .iter()
            .find(|r| r.infer_kind == InferKind::Expression)
            .expect("publisher infer request");
        assert_eq!(publisher.expression_text.as_deref(), Some("event"));
        assert_eq!(
            publisher.alias.as_deref(),
            manifest_aliases
                .get(&ManifestRole::Consumer)
                .map(|s| s.as_str()),
            "publisher infer alias must byte-match the Consumer manifest alias \
             (same build_call_site_id over the same path/line/key)"
        );
    }

    /// A publisher locator whose `payload_expression_line` the model omitted
    /// must still anchor the sidecar's text search: the collector defaults
    /// `expression_line` to the operation's own line. An unanchored request
    /// searches the whole file, and identical locator text at another site
    /// can resolve a confidently wrong type — an anchored miss degrades to
    /// Unknown instead (see the sidecar's `matchByText` window/proximity
    /// selection, pinned by `infer-expression-line-anchor.test.ts`).
    #[test]
    fn pubsub_publisher_locator_defaults_expression_line_to_op_line() {
        use crate::operation::PubsubRole;
        use crate::services::type_sidecar::InferKind;

        let mut file_results: HashMap<String, FileAnalysisResult> = HashMap::new();
        file_results.insert(
            "dispatch/src/dispatch.ts".to_string(),
            FileAnalysisResult {
                pubsub_operations: vec![crate::agents::file_analyzer_agent::PubsubOperation {
                    topic: "itemArchived".to_string(),
                    role: Some(PubsubRole::Publisher),
                    line_number: 9,
                    primary_type_symbol: None,
                    type_import_source: None,
                    broker: None,
                    payload_expression_text: Some("event".to_string()),
                    payload_expression_line: None,
                }],
                ..Default::default()
            },
        );

        let orchestrator = FileOrchestrator::new(AgentService::new());
        let requests = orchestrator.collect_pubsub_infer_requests(&file_results, ".");
        assert_eq!(requests.len(), 1);
        let request = &requests[0];
        assert_eq!(request.infer_kind, InferKind::Expression);
        assert_eq!(
            request.expression_line,
            Some(9),
            "a missing payload_expression_line must default to the op's line, \
             not fall through to an unanchored file-wide search"
        );
    }

    /// PR-4: a subscriber pub/sub op carrying a decoded-payload
    /// `primary_type_symbol` reaches BOTH `cloud_data.endpoints` (via
    /// `append_pubsub_operations`, PR-3) AND the type manifest as a
    /// `ManifestRole::Producer` entry anchored on that symbol (via
    /// `append_pubsub_manifest_entries`, PR-4); a publisher op becomes a
    /// `ManifestRole::Consumer` manifest entry. This is the Socket.IO manifest
    /// path mirrored for pub/sub, so the anchor + resolution dimensions stop
    /// treating extracted pub/sub ops as untyped misses.
    #[test]
    fn pubsub_ops_reach_cloud_data_and_manifest_with_payload_anchor() {
        use crate::operation::PubsubRole;

        let mut file_results: HashMap<String, FileAnalysisResult> = HashMap::new();
        file_results.insert(
            "metrics-service/src/consumer.ts".to_string(),
            FileAnalysisResult {
                pubsub_operations: vec![pubsub_op(
                    "metrics.page_view",
                    PubsubRole::Subscriber,
                    Some("PageView"),
                    Some("./types/page-view"),
                )],
                ..Default::default()
            },
        );
        file_results.insert(
            "web-frontend/src/track.ts".to_string(),
            FileAnalysisResult {
                pubsub_operations: vec![pubsub_op(
                    "metrics.page_view",
                    PubsubRole::Publisher,
                    Some("PageView"),
                    Some("./types/page-view"),
                )],
                ..Default::default()
            },
        );

        // PR-3 side: the subscriber lands in endpoints (producer), the publisher
        // in calls (consumer), keyed identically so they match cross-repo.
        let mut cloud_data = repo_with_bundle("metrics-monorepo", None, "");
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
            provenance: Default::default(),
        };
        append_pubsub_operations(
            &mut cloud_data,
            &file_results,
            &crate::socket_io::SocketExtraction::default(),
            &to_details,
        );
        assert_eq!(
            cloud_data
                .endpoints
                .iter()
                .filter(|e| e.key.canonical() == "pubsub|metrics.page_view")
                .count(),
            1,
            "the subscriber must register as a producer endpoint"
        );
        assert_eq!(
            cloud_data
                .calls
                .iter()
                .filter(|c| c.key.canonical() == "pubsub|metrics.page_view")
                .count(),
            1,
            "the publisher must register as a consumer call"
        );

        // PR-4 side: both ops emit a manifest entry anchored on the payload type.
        let mut entries = Vec::new();
        append_pubsub_manifest_entries(
            &mut entries,
            &file_results,
            &crate::socket_io::SocketExtraction::default(),
            ".",
        );

        let producer = entries
            .iter()
            .find(|e| {
                e.key.canonical() == "pubsub|metrics.page_view" && e.role == ManifestRole::Producer
            })
            .expect("subscriber pub/sub op must emit a Producer manifest entry");
        assert_eq!(producer.type_kind, ManifestTypeKind::Response);
        assert_eq!(producer.primary_type_symbol.as_deref(), Some("PageView"));
        assert_eq!(producer.type_state, ManifestTypeState::Unknown);
        assert!(
            !producer.type_alias.is_empty(),
            "pub/sub op must get a stable type_alias"
        );

        let consumer = entries
            .iter()
            .find(|e| {
                e.key.canonical() == "pubsub|metrics.page_view" && e.role == ManifestRole::Consumer
            })
            .expect("publisher pub/sub op must emit a Consumer manifest entry");
        assert_eq!(consumer.primary_type_symbol.as_deref(), Some("PageView"));

        // Exactly one entry per op — no phantom Request alias, mirroring socket.
        assert_eq!(
            entries
                .iter()
                .filter(|e| e.key.canonical() == "pubsub|metrics.page_view")
                .count(),
            2
        );
    }

    /// Regression (xrepo-corpus-1): the file-analyzer double-classifies a single
    /// `socket.emit("payment:settled", …)` site as BOTH a deterministic socket
    /// op AND an LLM pub/sub op, so the emit is indexed twice — once
    /// `socket|SERVER->CLIENT|payment:settled` (correct, ground truth), once
    /// `pubsub|payment:settled` (spurious) — inflating the call set. The
    /// same-file socket-twin fold drops the pub/sub form. A REAL pub/sub op in a
    /// file with no socket twin (`orders.created`) is untouched, proving the
    /// fold keys on the same-file coincidence and not on the topic string or a
    /// broker name. The socket file uses a `./`-prefixed path to exercise the
    /// component-wise normalization against the repo-relative file_results key.
    #[test]
    fn pubsub_op_folded_when_same_file_socket_twin_present() {
        use crate::operation::{PubsubRole, SocketDirection};

        let emit_file = "payments-svc/realtime/server.ts";
        let publish_file = "payments-svc/events/orders.ts";

        let extractions = ProtocolExtractions {
            graphql: crate::graphql::GraphqlExtraction::default(),
            sockets: crate::socket_io::SocketExtraction {
                listeners: vec![],
                emitters: vec![crate::socket_io::SocketOp {
                    key: OperationKey::socket("payment:settled", SocketDirection::ServerToClient),
                    // `./`-prefixed to prove the walk-path vs file_results-key join.
                    file_path: PathBuf::from(format!("./{emit_file}")),
                    line: 28,
                    payload_type_symbol: Some("Payment".to_string()),
                    payload_type_source: Some("../src/types".to_string()),
                }],
            },
        };

        let mut file_results: HashMap<String, FileAnalysisResult> = HashMap::new();
        // The spurious twin: same file + same event name as the socket emit.
        file_results.insert(
            emit_file.to_string(),
            FileAnalysisResult {
                pubsub_operations: vec![pubsub_op(
                    "payment:settled",
                    PubsubRole::Publisher,
                    Some("Payment"),
                    Some("../src/types"),
                )],
                ..Default::default()
            },
        );
        // A genuine pub/sub publish in a DIFFERENT file with no socket twin —
        // must survive the fold untouched.
        file_results.insert(
            publish_file.to_string(),
            FileAnalysisResult {
                pubsub_operations: vec![pubsub_op(
                    "orders.created",
                    PubsubRole::Publisher,
                    Some("OrderCreated"),
                    Some("./types/order"),
                )],
                ..Default::default()
            },
        );

        let mut cloud_data = repo_with_bundle("payments-svc", None, "");
        append_deterministic_protocol_operations(&mut cloud_data, &extractions, &file_results);

        // The socket emit is indexed exactly once, as the socket op.
        assert_eq!(
            cloud_data
                .calls
                .iter()
                .filter(|c| c.key.canonical() == "socket|SERVER->CLIENT|payment:settled")
                .count(),
            1,
            "the deterministic socket emitter must be indexed as a call"
        );
        // The spurious pub/sub twin was folded away (pre-fix: this is 1).
        assert_eq!(
            cloud_data
                .calls
                .iter()
                .filter(|c| c.key.canonical() == "pubsub|payment:settled")
                .count(),
            0,
            "the same-file socket-twin pub/sub op must be folded, not double-indexed"
        );
        // The unrelated real pub/sub publish is untouched.
        assert_eq!(
            cloud_data
                .calls
                .iter()
                .filter(|c| c.key.canonical() == "pubsub|orders.created")
                .count(),
            1,
            "a real pub/sub op with no same-file socket twin must survive"
        );

        // Manifest side folds identically: no orphan anchor for the folded op,
        // the real pub/sub op still anchors.
        let mut entries = Vec::new();
        append_pubsub_manifest_entries(&mut entries, &file_results, &extractions.sockets, ".");
        assert_eq!(
            entries
                .iter()
                .filter(|e| e.key.canonical() == "pubsub|payment:settled")
                .count(),
            0,
            "folded pub/sub op must leave no orphan manifest anchor"
        );
        assert_eq!(
            entries
                .iter()
                .filter(|e| e.key.canonical() == "pubsub|orders.created")
                .count(),
            1,
            "the real pub/sub op must still anchor a manifest entry"
        );
    }

    /// A pub/sub op with no decoded payload type (`primary_type_symbol: None`)
    /// still gets a manifest entry, just with a `None` symbol — exactly how a
    /// socket emitter whose payload the extractor couldn't capture is handled.
    /// An op with no role anchors nothing and emits no entry.
    #[test]
    fn pubsub_op_without_payload_symbol_still_anchors_and_no_role_skips() {
        use crate::operation::PubsubRole;

        let mut file_results: HashMap<String, FileAnalysisResult> = HashMap::new();
        file_results.insert(
            "svc/src/handlers.ts".to_string(),
            FileAnalysisResult {
                pubsub_operations: vec![
                    pubsub_op("orders.created", PubsubRole::Subscriber, None, None),
                    crate::agents::file_analyzer_agent::PubsubOperation {
                        topic: "orders.shipped".to_string(),
                        role: None,
                        line_number: 9,
                        primary_type_symbol: Some("Shipment".to_string()),
                        type_import_source: None,
                        broker: None,
                        payload_expression_text: None,
                        payload_expression_line: None,
                    },
                ],
                ..Default::default()
            },
        );

        let mut entries = Vec::new();
        append_pubsub_manifest_entries(
            &mut entries,
            &file_results,
            &crate::socket_io::SocketExtraction::default(),
            ".",
        );

        let untyped = entries
            .iter()
            .find(|e| e.key.canonical() == "pubsub|orders.created")
            .expect("untyped subscriber still emits a manifest entry");
        assert_eq!(untyped.role, ManifestRole::Producer);
        assert_eq!(untyped.primary_type_symbol, None);

        // The roleless op was dropped — no entry, regardless of its symbol.
        assert!(
            !entries
                .iter()
                .any(|e| e.key.canonical() == "pubsub|orders.shipped"),
            "a pub/sub op with no role must emit no manifest entry"
        );
    }

    /// Same fragile contract for GraphQL consumers: the alias on the consumer
    /// manifest entry (EDIT 2, `append_protocol_manifest_entries`) MUST byte-match
    /// the alias on the `collect_graphql_type_requests` SymbolRequest (EDIT 3),
    /// both `build_manifest_type_alias(key, Consumer, Response)`. If they diverge
    /// the resolved `.d.ts` never joins back and the entry stays `Unknown`.
    #[test]
    fn graphql_symbol_request_alias_matches_manifest_alias() {
        let consumer = graphql_consumer_op("order", Some("OrderView"), Some("./types"));
        let extractions = ProtocolExtractions {
            graphql: crate::graphql::GraphqlExtraction {
                producers: vec![],
                consumers: vec![consumer.clone()],
            },
            sockets: crate::socket_io::SocketExtraction::default(),
        };

        let mut entries = Vec::new();
        append_protocol_manifest_entries(&mut entries, &extractions);
        let manifest_entry = entries
            .iter()
            .find(|e| {
                e.key.canonical() == "graphql|query|order" && e.role == ManifestRole::Consumer
            })
            .expect("graphql consumer manifest entry");
        // EDIT 2: the consumer entry's anchor is the call-site payload symbol.
        assert_eq!(
            manifest_entry.primary_type_symbol.as_deref(),
            Some("OrderView"),
            "consumer entry must carry the request<T> anchor, not the SDL None"
        );
        let manifest_alias = manifest_entry.type_alias.clone();

        let orchestrator = FileOrchestrator::new(AgentService::new());
        let requests = orchestrator.collect_graphql_type_requests(&extractions.graphql, ".");
        let request = requests
            .iter()
            .find(|r| r.symbol_name == "OrderView")
            .expect("graphql SymbolRequest");

        assert_eq!(
            request.alias.as_deref(),
            Some(manifest_alias.as_str()),
            "SymbolRequest.alias must byte-match the manifest entry's alias \
             (both build_manifest_type_alias(key, Consumer, Response)) or the \
             enrich-join silently breaks"
        );
        // Independently confirm both equal the canonical builder output.
        let expected = crate::type_manifest::build_manifest_type_alias(
            &consumer.key,
            ManifestRole::Consumer,
            ManifestTypeKind::Response,
        );
        assert_eq!(manifest_alias, expected);
        assert_eq!(request.alias.as_deref(), Some(expected.as_str()));
    }

    /// #268: `collect_graphql_type_requests` falls back to
    /// `consumer_located_type_symbol` ONLY when `payload_type_symbol` is
    /// absent — the deterministic call-site anchor always wins when both are
    /// present (pinning the orchestrator's own precedence independently of
    /// the engine merge's isolation guard, which already keeps the two
    /// mutually exclusive per op in practice).
    #[test]
    fn collect_graphql_type_requests_falls_back_to_located_type_only_when_unanchored() {
        use crate::operation::GraphqlOperationKind;

        // No call-site anchor, but a located type — the fallback must fire.
        let mut located_only = graphql_consumer_op_at(
            GraphqlOperationKind::Subscription,
            "orderUpdated",
            "web-frontend/lib/graphql.ts",
            None,
        );
        located_only.consumer_located_type_symbol = Some("OrderUpdate".to_string());

        // BOTH a call-site anchor and a (stray) located type — the explicit
        // anchor must win, exactly mirroring the resolver-first precedent.
        let mut both = graphql_consumer_op("order", Some("OrderView"), Some("./types"));
        both.consumer_located_type_symbol = Some("StrayType".to_string());

        // Neither anchor — no request at all.
        let neither = graphql_consumer_op_at(
            GraphqlOperationKind::Query,
            "unanchored",
            "web-frontend/lib/graphql.ts",
            None,
        );

        let extraction = crate::graphql::GraphqlExtraction {
            producers: vec![],
            consumers: vec![located_only, both, neither],
        };

        let orchestrator = FileOrchestrator::new(AgentService::new());
        let requests = orchestrator.collect_graphql_type_requests(&extraction, ".");

        assert!(
            requests.iter().any(|r| r.symbol_name == "OrderUpdate"),
            "the located type must bundle when there is no call-site anchor, got: {:?}",
            requests
        );
        assert!(
            requests.iter().any(|r| r.symbol_name == "OrderView"),
            "the call-site anchor must still bundle, got: {:?}",
            requests
        );
        assert!(
            !requests.iter().any(|r| r.symbol_name == "StrayType"),
            "a stray located type on an already-anchored op must NEVER bundle, got: {:?}",
            requests
        );
        assert_eq!(
            requests.len(),
            2,
            "exactly two requests (OrderUpdate + OrderView) — the fully-unanchored \
             op produces none, got: {:?}",
            requests
        );
    }

    /// Stage B1 producer infer join: a GraphQL PRODUCER whose resolver location
    /// was merged in (`resolver_file`/`resolver_line`) becomes a `FunctionReturn`
    /// infer request whose alias byte-matches the PRODUCER manifest entry's
    /// `type_alias` — both `build_manifest_type_alias(key, Producer, Response)`.
    /// This is the load-bearing join: if they diverge, the inferred expanded
    /// resolver-return `.d.ts` never lands on the producer entry and it stays
    /// `Unknown`. Mirrors `graphql_symbol_request_alias_matches_manifest_alias`,
    /// but on the producer/infer side.
    #[test]
    fn graphql_producer_infer_request_alias_matches_manifest_alias() {
        use crate::operation::GraphqlOperationKind;

        // A producer with its SDL anchor AND a resolver location (post-merge).
        let mut producer = graphql_op(GraphqlOperationKind::Query, "order", Some("Order"));
        producer.resolver_file = Some(PathBuf::from("packages/gateway/src/orders.resolver.ts"));
        producer.resolver_line = Some(38);

        let extractions = ProtocolExtractions {
            graphql: crate::graphql::GraphqlExtraction {
                producers: vec![producer.clone()],
                consumers: vec![],
            },
            sockets: crate::socket_io::SocketExtraction::default(),
        };

        // The producer manifest entry's alias (Producer, Response).
        let mut entries = Vec::new();
        append_protocol_manifest_entries(&mut entries, &extractions);
        let manifest_entry = entries
            .iter()
            .find(|e| {
                e.key.canonical() == "graphql|query|order" && e.role == ManifestRole::Producer
            })
            .expect("graphql producer manifest entry");
        let manifest_alias = manifest_entry.type_alias.clone();

        let orchestrator = FileOrchestrator::new(AgentService::new());
        let infer = orchestrator.collect_graphql_producer_infer_requests(&extractions.graphql, ".");
        assert_eq!(infer.len(), 1, "exactly one producer infer request");
        let request = &infer[0];

        // The load-bearing alias join.
        assert_eq!(
            request.alias.as_deref(),
            Some(manifest_alias.as_str()),
            "InferRequestItem.alias must byte-match the producer manifest entry's \
             alias (both build_manifest_type_alias(key, Producer, Response)) or the \
             expanded resolver-return type never joins back and the entry stays Unknown"
        );
        // Independently confirm both equal the canonical builder output.
        let expected = crate::type_manifest::build_manifest_type_alias(
            &producer.key,
            ManifestRole::Producer,
            ManifestTypeKind::Response,
        );
        assert_eq!(manifest_alias, expected);
        assert_eq!(request.alias.as_deref(), Some(expected.as_str()));

        // The producer takes the INFER path (FunctionReturn at the resolver), not
        // the bundle path: file/line come from the merged resolver location.
        assert_eq!(request.infer_kind, InferKind::FunctionReturn);
        assert_eq!(request.line_number, 38);
        assert!(
            request
                .file_path
                .ends_with("packages/gateway/src/orders.resolver.ts"),
            "infer file must be the resolver file, got: {}",
            request.file_path
        );

        // A producer without a merged resolver location yields no infer request.
        let bare = ProtocolExtractions {
            graphql: crate::graphql::GraphqlExtraction {
                producers: vec![graphql_op(
                    GraphqlOperationKind::Query,
                    "order",
                    Some("Order"),
                )],
                consumers: vec![],
            },
            sockets: crate::socket_io::SocketExtraction::default(),
        };
        assert!(
            orchestrator
                .collect_graphql_producer_infer_requests(&bare.graphql, ".")
                .is_empty(),
            "an SDL producer with no merged resolver location produces no infer request"
        );
    }

    /// `write_manifest_files` feeds the ts_check matcher, which checks HTTP,
    /// socket, AND graphql. Every entry passes through unfiltered, including an
    /// UNRESOLVED graphql entry (`type_state == Unknown`, e.g. a subscription
    /// consumer with no `request<T>` call site). Such an entry must reach ts_check
    /// so its pair is reported as an `unknownPair` (→ `None`); dropping it would
    /// make the edge absent and `apply_compat_verdicts` would default it to a
    /// false `Some(true)`. The #253 throw-guard lives in the matcher, which drops
    /// any stray non-checkable entry defensively rather than crashing the run.
    #[test]
    fn write_manifest_files_emits_http_socket_and_all_graphql() {
        use crate::cloud_storage::TypeEvidence;
        use crate::operation::{GraphqlOperationKind, OperationKey, SocketDirection};
        use crate::services::type_sidecar::InferKind;

        fn entry_with_state(
            key: OperationKey,
            alias: &str,
            type_state: ManifestTypeState,
        ) -> TypeManifestEntry {
            let is_explicit = type_state == ManifestTypeState::Explicit;
            TypeManifestEntry {
                key,
                role: ManifestRole::Producer,
                type_kind: ManifestTypeKind::Response,
                type_alias: alias.to_string(),
                file_path: "src/x.ts".to_string(),
                line_number: 1,
                is_explicit,
                type_state,
                evidence: TypeEvidence {
                    file_path: "src/x.ts".to_string(),
                    span_start: None,
                    span_end: None,
                    line_number: 1,
                    infer_kind: InferKind::ResponseBody,
                    is_explicit,
                    type_state,
                },
                resolved_definition: None,
                expanded_definition: None,
                primary_type_symbol: None,
            }
        }

        fn entry(key: OperationKey, alias: &str) -> TypeManifestEntry {
            entry_with_state(key, alias, ManifestTypeState::Explicit)
        }

        let manifest = vec![
            entry(OperationKey::http("GET", "/orders/:id"), "OrderResponse"),
            // Resolved graphql producer (anchor reached the bundle) → KEPT.
            entry(
                OperationKey::graphql(GraphqlOperationKind::Query, "order"),
                "OrderQueryResult",
            ),
            // Unresolved graphql producer (Unknown anchor) → KEPT so ts_check can
            // report its pair as an unknownPair (→ None); dropping it would make
            // the edge absent and default the verdict to a false Some(true).
            entry_with_state(
                OperationKey::graphql(GraphqlOperationKind::Subscription, "orderUpdated"),
                "OrderUpdatedUnknown",
                ManifestTypeState::Unknown,
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
            compat_verdicts: None,
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

        // HTTP, socket, and BOTH graphql producers (resolved and Unknown) survive:
        // the Unknown one must reach ts_check to be reported as an unknownPair.
        assert_eq!(
            producer.entries.len(),
            4,
            "HTTP + socket + both graphql producers reach the manifest; the \
             Unknown graphql entry is kept, not dropped"
        );
        assert!(
            producer
                .entries
                .iter()
                .any(|e| e.key.protocol() == crate::operation::Protocol::Http),
            "the HTTP producer must reach the manifest"
        );
        assert!(
            producer
                .entries
                .iter()
                .any(|e| e.key.protocol() == crate::operation::Protocol::Websocket),
            "the socket producer must reach the manifest"
        );
        let graphql_entries: Vec<_> = producer
            .entries
            .iter()
            .filter(|e| e.key.protocol() == crate::operation::Protocol::Graphql)
            .collect();
        assert_eq!(
            graphql_entries.len(),
            2,
            "both graphql producers reach the manifest (resolved + Unknown)"
        );
        assert!(
            graphql_entries
                .iter()
                .any(|e| e.type_alias == "OrderQueryResult"),
            "the resolved graphql producer is kept"
        );
        assert!(
            graphql_entries
                .iter()
                .any(|e| e.type_state == ManifestTypeState::Unknown),
            "the Unknown graphql producer is kept so ts_check can mark it \
             unverifiable rather than the edge defaulting to compatible"
        );
    }

    /// The `graphql|subscription|orderUpdated` false-positive fix: an UNRESOLVED
    /// graphql CONSUMER must REACH the consumer manifest, so ts_check reports its
    /// pair as an `unknownPair` (→ `None`). Dropping it instead removed the pair
    /// from ts_check's output, and `apply_compat_verdicts` defaulted the absent
    /// edge to a false `Some(true)` (`producer ⊑ any` was never actually run — the
    /// edge just looked compatible because nothing contradicted it).
    ///
    /// All three consumer shapes are KEPT — `type_state == Unknown`, an `Implicit`
    /// consumer with no resolved anchor symbol (the synthetic missing-alias case),
    /// and a genuinely resolved consumer. ts_check's `any`/`unknown` comparand
    /// guard distinguishes the unverifiable ones at verdict time, not here.
    #[test]
    fn write_manifest_files_keeps_unresolved_graphql_consumer() {
        use crate::cloud_storage::TypeEvidence;
        use crate::operation::{GraphqlOperationKind, OperationKey};
        use crate::services::type_sidecar::InferKind;

        fn consumer(
            field: &str,
            alias: &str,
            type_state: ManifestTypeState,
            primary_type_symbol: Option<&str>,
        ) -> TypeManifestEntry {
            let is_explicit = type_state == ManifestTypeState::Explicit;
            let key = OperationKey::graphql(GraphqlOperationKind::Subscription, field);
            TypeManifestEntry {
                key,
                role: ManifestRole::Consumer,
                type_kind: ManifestTypeKind::Response,
                type_alias: alias.to_string(),
                file_path: "lib/graphql.ts".to_string(),
                line_number: 1,
                is_explicit,
                type_state,
                evidence: TypeEvidence {
                    file_path: "lib/graphql.ts".to_string(),
                    span_start: None,
                    span_end: None,
                    line_number: 1,
                    infer_kind: InferKind::CallResult,
                    is_explicit,
                    type_state,
                },
                resolved_definition: None,
                expanded_definition: None,
                primary_type_symbol: primary_type_symbol.map(str::to_string),
            }
        }

        let manifest = vec![
            // Clean unresolved consumer (type_state Unknown) → KEPT (ts_check
            // reports its pair as an unknownPair → None).
            consumer(
                "orderUpdated",
                "Endpoint_unknown_Response",
                ManifestTypeState::Unknown,
                None,
            ),
            // Enrichment wrongly promoted the synthetic alias to Implicit, but no
            // real anchor was ever resolved (primary_type_symbol still None) →
            // KEPT; its alias dangles to `any`/`unknown` in the bundle so the
            // ts_check comparand guard marks the pair unverifiable.
            consumer(
                "orderPromoted",
                "Endpoint_promoted_Response",
                ManifestTypeState::Implicit,
                None,
            ),
            // Genuinely resolved consumer (real anchor symbol) → KEPT.
            consumer(
                "orderResolved",
                "Endpoint_resolved_Response",
                ManifestTypeState::Implicit,
                Some("OrderView"),
            ),
        ];

        let repo_data = CloudRepoData {
            repo_name: "web-frontend".to_string(),
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
            compat_verdicts: None,
        };

        #[derive(serde::Deserialize)]
        struct ManifestFileForTest {
            entries: Vec<TypeManifestEntry>,
        }

        let dir = tempfile::tempdir().expect("tempdir");
        write_manifest_files(std::slice::from_ref(&repo_data), dir.path())
            .expect("write_manifest_files");

        let consumers: ManifestFileForTest = serde_json::from_str(
            &std::fs::read_to_string(dir.path().join("consumer-manifest.json")).unwrap(),
        )
        .unwrap();

        assert_eq!(
            consumers.entries.len(),
            3,
            "all graphql consumers reach the manifest — the Unknown one and the \
             anchorless wrongly-promoted one are KEPT so ts_check can mark their \
             pairs unverifiable (→ None) instead of the edge defaulting to a \
             false Some(true)"
        );
        assert!(
            consumers
                .entries
                .iter()
                .any(|e| e.type_alias == "Endpoint_resolved_Response"),
            "the genuinely-resolved consumer is kept"
        );
        assert!(
            consumers
                .entries
                .iter()
                .any(|e| e.type_state == ManifestTypeState::Unknown),
            "the Unknown consumer is kept (unverifiable, not dropped)"
        );
        assert!(
            consumers
                .entries
                .iter()
                .any(|e| e.primary_type_symbol.is_none()),
            "the anchorless wrongly-promoted consumer is kept (unverifiable, not \
             dropped)"
        );
    }

    /// Minimal `CloudRepoData` carrying just a repo/service identity and a
    /// bundled `.d.ts`, for the bundle-file-emission tests below.
    fn repo_with_bundle(
        repo_name: &str,
        service_name: Option<&str>,
        bundled_types: &str,
    ) -> CloudRepoData {
        CloudRepoData {
            repo_name: repo_name.to_string(),
            service_name: service_name.map(str::to_string),
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
            bundled_types: Some(bundled_types.to_string()),
            type_manifest: None,
            file_results: None,
            cached_detection: None,
            cached_guidance: None,
            cached_extraction_config: None,
            package_json_hash: None,
            cache_version: None,
            type_extraction_status: None,
            compat_verdicts: None,
        }
    }

    /// `bundle_file_stems` must give every `(repo, service)` a distinct stem so
    /// the per-service `.d.ts` files don't collide. The clobbering bug was that
    /// a monorepo's services share a `repo_name`, so a `repo_name`-only stem made
    /// them all map to one file.
    #[test]
    fn bundle_file_stems_are_unique_per_service_in_a_monorepo() {
        let repos = vec![
            repo_with_bundle("orders-monorepo", Some("orders-pkg"), "// a"),
            repo_with_bundle("orders-monorepo", Some("gateway"), "// b"),
            // A service id containing a path separator must be sanitised, not
            // allowed to escape the output dir.
            repo_with_bundle("orders-monorepo", Some("scope/pkg"), "// c"),
            // Two services with no service_name fall back to the shared repo_name
            // and would still collide — the collision suffix must separate them.
            repo_with_bundle("other-repo", None, "// d"),
            repo_with_bundle("other-repo", None, "// e"),
        ];

        let stems = bundle_file_stems(&repos);

        assert_eq!(
            stems,
            vec![
                "orders-pkg".to_string(),
                "gateway".to_string(),
                "scope_pkg".to_string(),
                "other-repo".to_string(),
                "other-repo_1".to_string(),
            ]
        );
        let unique: std::collections::HashSet<&String> = stems.iter().collect();
        assert_eq!(
            unique.len(),
            stems.len(),
            "every service must get a distinct bundle stem so no write clobbers another"
        );
    }

    /// A-producer-bundle-gap regression: in a monorepo, the producer service's
    /// explicit response type (orders-pkg `GET /orders/:id` → `Order`) must land
    /// in the cross-repo bundle, resolvable — not get clobbered out by a sibling
    /// service (gateway) sharing the repo name. Before the per-service split,
    /// both wrote `orders-monorepo_types.d.ts` and the gateway write erased the
    /// `Order` shape entirely (not even a `= unknown` placeholder), so ts_check
    /// reported "Producer type not found in project" and the compat verdict
    /// collapsed to unverifiable.
    #[test]
    fn monorepo_producer_explicit_type_survives_into_bundle() {
        // orders-pkg's bundle: the explicit `Order` shape under its manifest
        // alias. This is the producer type both consumers (payments-svc,
        // web-frontend) need to resolve.
        let orders_alias = "Endpoint_5d19c4207b67a294_Response";
        let orders_bundle = format!(
            "export type {orders_alias} = {{ id: number; amountCents: number; currency: string }};\n"
        );
        // gateway's bundle: a different alias. Under the old repo_name-only
        // naming this write would clobber orders-pkg's file.
        let gateway_alias = "Endpoint_aaaaaaaaaaaaaaaa_Response";
        let gateway_bundle = format!("export type {gateway_alias} = {{ status: string }};\n");

        let repos = vec![
            repo_with_bundle("orders-monorepo", Some("orders-pkg"), &orders_bundle),
            repo_with_bundle("orders-monorepo", Some("gateway"), &gateway_bundle),
        ];

        let dir = tempfile::tempdir().expect("tempdir");
        write_bundle_files(&repos, dir.path());

        // ts_check loads every *.d.ts in the dir; concatenate them the same way
        // and assert BOTH services' producer aliases resolve to a real shape.
        let mut combined = String::new();
        let mut dts_files = 0usize;
        for entry in std::fs::read_dir(dir.path()).expect("read_dir") {
            let path = entry.expect("dir entry").path();
            if path.extension().and_then(|s| s.to_str()) == Some("ts")
                && path.to_string_lossy().ends_with(".d.ts")
            {
                dts_files += 1;
                combined.push_str(&std::fs::read_to_string(&path).expect("read d.ts"));
                combined.push('\n');
            }
        }

        assert_eq!(
            dts_files, 2,
            "each service must get its own bundle file, not share one (got {dts_files})"
        );

        // The producer's `Order` shape is present, resolvable, and NOT a dangling
        // `= unknown` placeholder — proving the type reaches ts_check.
        assert!(
            combined.contains(&format!(
                "export type {orders_alias} = {{ id: number; amountCents: number; currency: string }}"
            )),
            "the orders-pkg producer's explicit Order shape must survive into the bundle, got:\n{combined}"
        );
        assert!(
            !combined.contains(&format!("export type {orders_alias} = unknown")),
            "the producer alias must resolve to its real members, not a dangling = unknown placeholder"
        );
        // The sibling service's types must coexist, not have been clobbered.
        assert!(
            combined.contains(&format!(
                "export type {gateway_alias} = {{ status: string }}"
            )),
            "the gateway sibling's types must coexist in its own bundle file"
        );
    }
}
