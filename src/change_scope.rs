//! Which services a `--changed-only` scan analyses, and which it carries
//! (carrick#2227). Contract: `docs/reference/runner.md`.
//!
//! A carried service is not analysed: its stored generation stands in for it
//! in the run, exactly as a held-back service's does. A service is carried
//! only when nothing it reads has changed since the commit its own stored
//! blob names. What it reads (its [`ServiceInputs`]) is computed on the tree
//! being scanned: its roots, the transitive closure of its imports as the
//! scanner's own resolver reads them, the workspace packages that closure
//! enters, its tsconfig's `extends` chain and the directories that config
//! includes, its manifest and its declared GraphQL schemas. A service's inputs
//! at its stored commit can differ from these only when one of its own files
//! changed, which already counts as a change.
//!
//! When carrick.json names no `tsconfig` for a service, its tsconfig is the
//! nearest `tsconfig.json` at or above its directory.
//!
//! [`decide`] is the rule, over a stored blob, this run's config, the paths
//! changed since that blob's commit and the service's inputs. [`plan`] runs it
//! for every service of a scan and logs each answer.

use std::collections::{BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::Instant;

use tracing::info;

use crate::cloud_storage::CloudRepoData;
use crate::config::Config;
use crate::git_state::ChangedPath;

/// What `--changed-only` together with `--no-cache` is told: a usage error.
pub const NO_CACHE_CONFLICT: &str =
    "--changed-only cannot be combined with --no-cache, which re-analyses every service.";

/// The `close-scan` reason of a `--changed-only` run that writes nothing.
pub const NO_CHANGE_REASON: &str = "no_change";

fn requested_slot() -> &'static OnceLock<bool> {
    static REQUESTED: OnceLock<bool> = OnceLock::new();
    &REQUESTED
}

/// Record `--changed-only` for the run. A second call is ignored.
pub fn request() {
    let _ = requested_slot().set(true);
}

/// Whether this run was told `--changed-only`.
pub fn requested() -> bool {
    requested_slot().get().copied().unwrap_or(false)
}

/// Why a service is analysed rather than carried.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reason {
    NoStored,
    OtherScanner,
    StoredDirty,
    StoredIncomplete,
    ConfigChanged,
    CommitMissing,
    UnresolvedImport,
    ExtendsUnfollowed,
    RepoInputChanged,
    InputsChanged,
}

impl Reason {
    pub fn as_str(self) -> &'static str {
        match self {
            Reason::NoStored => "no_stored",
            Reason::OtherScanner => "other_scanner",
            Reason::StoredDirty => "stored_dirty",
            Reason::StoredIncomplete => "stored_incomplete",
            Reason::ConfigChanged => "config_changed",
            Reason::CommitMissing => "commit_missing",
            Reason::UnresolvedImport => "unresolved_import",
            Reason::ExtendsUnfollowed => "extends_unfollowed",
            Reason::RepoInputChanged => "repo_input_changed",
            Reason::InputsChanged => "inputs_changed",
        }
    }
}

/// What a `--changed-only` scan does with one service.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Scope {
    /// Its stored generation, written at `commit`, stands in for it.
    Carried { commit: String },
    /// It is analysed as a full scan would, for `reason`.
    Analysed { reason: Reason, detail: String },
}

impl Scope {
    pub fn is_carried(&self) -> bool {
        matches!(self, Scope::Carried { .. })
    }

    fn analysed(reason: Reason, detail: impl Into<String>) -> Self {
        Scope::Analysed {
            reason,
            detail: detail.into(),
        }
    }
}

/// The scanner release and cache format this run writes.
#[derive(Debug, Clone, Copy)]
pub struct ThisRun<'a> {
    pub scanner_version: &'a str,
    pub cache_version: u32,
}

/// One root a service's walk reads: a directory relative to the repository's
/// top level (`""` for all of it), minus the directories of services nested
/// inside it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Root {
    pub dir: String,
    pub left_out: Vec<String>,
}

/// What one service reads, every path relative to the repository's top level
/// with `/` separators, so it compares with what `git diff` lists.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ServiceInputs {
    pub roots: Vec<Root>,
    /// Files outside the roots that the service reads: its import closure,
    /// its config chain, its manifest, its declared schemas.
    pub files: BTreeSet<String>,
    /// Directories read whole: a workspace package the closure enters, a
    /// directory its tsconfig includes.
    pub dirs: BTreeSet<String>,
    /// Directories where a path appearing or disappearing can change what an
    /// unchanged import resolves to: each holds a closure file, or is where a
    /// relative specifier was looked up.
    pub lookup_dirs: BTreeSet<String>,
    /// The first import nothing accounts for, as `<file> imports <specifier>`.
    pub unresolved: Option<String>,
    /// `extends` values a config it reads could not follow.
    pub unfollowed_extends: Vec<String>,
}

impl ServiceInputs {
    /// The first path in `changed` this service reads.
    pub fn first_change<'a>(&self, changed: &'a [ChangedPath]) -> Option<&'a ChangedPath> {
        changed.iter().find(|change| self.reads(change))
    }

    fn reads(&self, change: &ChangedPath) -> bool {
        let path = change.path.as_str();
        let in_root = self.roots.iter().any(|root| {
            under(path, &root.dir) && !root.left_out.iter().any(|dir| under(path, dir))
        });
        in_root
            || self.files.contains(path)
            || self.dirs.iter().any(|dir| under(path, dir))
            || (change.adds_or_deletes() && self.lookup_dirs.contains(parent_of(path)))
    }
}

/// Whether `path` is `dir` or under it. `""` is the whole repository.
fn under(path: &str, dir: &str) -> bool {
    dir.is_empty()
        || path == dir
        || path
            .strip_prefix(dir)
            .is_some_and(|rest| rest.starts_with('/'))
}

fn parent_of(path: &str) -> &str {
    path.rsplit_once('/').map_or("", |(parent, _)| parent)
}

/// The file names that are an input of every service wherever they sit: the
/// lockfiles, every manifest (internal package names are read across the
/// whole repository), the workspace files, what the walk ignores, and the
/// scan's own config.
///
/// With [`REPO_INPUT_EXTENSIONS`], [`crate::file_finder::SOURCE_EXTENSIONS`]
/// and a service's [`ServiceInputs`] (its tsconfig chain, its declared
/// GraphQL schemas), this is the one definition of the files a scan reads
/// (carrick#2227). A filter that decides whether a push needs a scan reads
/// it from here rather than restating it.
pub const REPO_INPUT_NAMES: &[&str] = &[
    "package-lock.json",
    "npm-shrinkwrap.json",
    "yarn.lock",
    "pnpm-lock.yaml",
    "bun.lock",
    "bun.lockb",
    "deno.lock",
    "package.json",
    "pnpm-workspace.yaml",
    "deno.json",
    "deno.jsonc",
    ".gitignore",
    "carrick.json",
];

/// The extensions whose files are an input of every service: the GraphQL
/// schema catalogue covers the whole repository.
pub const REPO_INPUT_EXTENSIONS: &[&str] = &["graphql", "gql", "graphqls"];

/// Whether a change to `path` re-analyses every service
/// ([`REPO_INPUT_NAMES`], [`REPO_INPUT_EXTENSIONS`]).
pub fn is_repo_input(path: &str) -> bool {
    let name = path.rsplit('/').next().unwrap_or(path);
    REPO_INPUT_NAMES.contains(&name)
        || name
            .rsplit_once('.')
            .is_some_and(|(_, extension)| REPO_INPUT_EXTENSIONS.contains(&extension))
}

/// Whether `service` is carried or analysed.
///
/// `stored` is its stored generation, `config_json` this run's carrick.json
/// entry for it serialized as a blob stores it, `changed` the paths changed
/// since the stored commit (`Err` when git could not compare), and `inputs`
/// computes what it reads, asked only when nothing cheaper decides.
pub fn decide(
    stored: Option<&CloudRepoData>,
    this_run: ThisRun<'_>,
    config_json: Option<&str>,
    changed: Result<&[ChangedPath], &str>,
    inputs: impl FnOnce() -> ServiceInputs,
) -> Scope {
    let Some(stored) = stored else {
        return Scope::analysed(Reason::NoStored, "no stored index for this service");
    };
    if stored.scanner_version.as_deref() != Some(this_run.scanner_version)
        || stored.cache_version != Some(this_run.cache_version)
    {
        return Scope::analysed(
            Reason::OtherScanner,
            format!(
                "stored by Carrick {} with cache {}, this run is Carrick {} with cache {}",
                stored
                    .scanner_version
                    .as_deref()
                    .unwrap_or("of unknown version"),
                stored
                    .cache_version
                    .map_or_else(|| "unknown".to_string(), |v| v.to_string()),
                this_run.scanner_version,
                this_run.cache_version
            ),
        );
    }
    if stored.dirty == Some(true) {
        return Scope::analysed(
            Reason::StoredDirty,
            "the stored index was read from uncommitted changes",
        );
    }
    if let Some(owed) = owed_work(stored) {
        return Scope::analysed(Reason::StoredIncomplete, owed);
    }
    if stored.config_json.as_deref() != config_json {
        return Scope::analysed(
            Reason::ConfigChanged,
            "its carrick.json entry differs from the stored one",
        );
    }
    let changed = match changed {
        Ok(changed) => changed,
        Err(reason) => {
            return Scope::analysed(
                Reason::CommitMissing,
                format!(
                    "git could not compare {}: {}",
                    short(&stored.commit_hash),
                    reason.trim()
                ),
            );
        }
    };
    let inputs = inputs();
    if let Some(unresolved) = inputs.unresolved {
        return Scope::analysed(Reason::UnresolvedImport, unresolved);
    }
    if !inputs.unfollowed_extends.is_empty() {
        return Scope::analysed(
            Reason::ExtendsUnfollowed,
            format!(
                "tsconfig extends {} could not be followed",
                inputs.unfollowed_extends.join(", ")
            ),
        );
    }
    if let Some(change) = changed.iter().find(|change| is_repo_input(&change.path)) {
        return Scope::analysed(Reason::RepoInputChanged, change.path.clone());
    }
    if let Some(change) = inputs.first_change(changed) {
        return Scope::analysed(Reason::InputsChanged, change.path.clone());
    }
    Scope::Carried {
        commit: stored.commit_hash.clone(),
    }
}

/// What a stored blob says it still owed when it was written: degraded types,
/// files the model never answered, candidates no model was asked about.
fn owed_work(stored: &CloudRepoData) -> Option<String> {
    if let Some(degraded) = &stored.types_degraded {
        return Some(format!(
            "the stored index lost its types ({})",
            degraded.stage
        ));
    }
    let boundary = stored.boundary.as_ref()?;
    if boundary.files_lost.total > 0 {
        return Some(format!(
            "the stored index owes {} file(s) model analysis",
            boundary.files_lost.total
        ));
    }
    match boundary.candidates_awaiting_model {
        Some(waiting) if waiting > 0 => Some(format!(
            "the stored index has {waiting} candidate(s) no model was asked about"
        )),
        _ => None,
    }
}

fn short(commit: &str) -> &str {
    commit.get(..7).unwrap_or(commit)
}

/// Each service's scope, index for index with `services`, logged one line
/// per service with a summary and the closure walk's wall time.
pub fn plan<'a>(
    repo_path: &str,
    services: &[Config],
    stored: impl Fn(&Config) -> Option<&'a CloudRepoData>,
    this_run: ThisRun<'_>,
) -> Vec<Scope> {
    let top = crate::git_state::toplevel(repo_path).ok();
    let mut changed_by_commit: HashMap<String, Result<Vec<ChangedPath>, String>> = HashMap::new();
    let mut closure_took = std::time::Duration::ZERO;
    let mut walked = 0usize;
    let scopes: Vec<Scope> = services
        .iter()
        .map(|service| {
            let stored = stored(service);
            let config_json = serde_json::to_string(service).ok();
            let changed = stored.map(|blob| {
                changed_by_commit
                    .entry(blob.commit_hash.clone())
                    .or_insert_with(|| {
                        crate::git_state::changed_paths(repo_path, &blob.commit_hash, None)
                    })
                    .clone()
            });
            let changed = match &changed {
                Some(Ok(paths)) => Ok(paths.as_slice()),
                Some(Err(reason)) => Err(reason.as_str()),
                None => Ok(&[][..]),
            };
            let scope = decide(stored, this_run, config_json.as_deref(), changed, || {
                let started = Instant::now();
                walked += 1;
                let inputs = match &top {
                    Some(top) => service_inputs(repo_path, top, service),
                    // No top level, no paths to compare: nothing vouches.
                    None => ServiceInputs {
                        unresolved: Some("git named no top level for this checkout".to_string()),
                        ..ServiceInputs::default()
                    },
                };
                closure_took += started.elapsed();
                inputs
            });
            let label = service.service_name.as_deref().unwrap_or("(root)");
            match &scope {
                Scope::Carried { commit } => info!(
                    "scope: {label} carried (inputs unchanged since {})",
                    short(commit)
                ),
                Scope::Analysed { reason, detail } => {
                    info!("scope: {label} analysed ({}: {detail})", reason.as_str())
                }
            }
            scope
        })
        .collect();
    let carried = scopes.iter().filter(|scope| scope.is_carried()).count();
    info!(
        "scope: {} analysed, {carried} carried, of {} service(s)",
        scopes.len() - carried,
        scopes.len()
    );
    info!(
        "scope: the closure walk took {:.2}s over {walked} service(s)",
        closure_took.as_secs_f64()
    );
    scopes
}

/// What `service` reads on the tree at `repo_path`, relative to the
/// repository's top level `top`.
pub fn service_inputs(repo_path: &str, top: &Path, service: &Config) -> ServiceInputs {
    let root = Path::new(repo_path);
    let top = top.canonicalize().unwrap_or_else(|_| top.to_path_buf());
    let relative = |path: &Path| -> Option<String> { top_relative(&top, path) };
    let mut roots: Vec<Root> = Vec::new();
    let mut unresolved: Option<String> = None;

    // Its roots, as the walk reads them.
    let service_root = match &service.directory {
        Some(dir) => root.join(dir),
        None => root.to_path_buf(),
    };
    let own = crate::config::directory_segments(service.directory.as_deref());
    let left_out: Vec<String> = service
        .nested_directories
        .iter()
        .filter(|nested| {
            let nested = crate::config::directory_segments(Some(nested));
            nested.len() > own.len() && nested.starts_with(&own)
        })
        .filter_map(|nested| relative(&root.join(nested)))
        .collect();
    roots.extend(relative(&service_root).map(|dir| Root { dir, left_out }));
    for include in &service.include {
        roots.extend(relative(&root.join(include)).map(|dir| Root {
            dir,
            left_out: Vec::new(),
        }));
    }
    let in_roots = |path: &str| {
        roots
            .iter()
            .any(|root| under(path, &root.dir) && !root.left_out.iter().any(|dir| under(path, dir)))
    };

    // Its import closure: every walked file's imports, and the imports of
    // every file they reach, resolved by the scanner's own resolver.
    let walk =
        crate::file_finder::walk_service(repo_path, service, &crate::packages::MANIFEST_SKIP_DIRS);
    let service_tsconfig = service.alias_tsconfig();
    let index = crate::workspace_resolver::WorkspaceIndex::build_with_aliases(
        root,
        service_tsconfig
            .as_ref()
            .map(|(directory, config)| (directory.as_path(), config.as_path())),
    );
    let canonical = |path: &Path| path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    let mut seen: BTreeSet<PathBuf> = walk.files.iter().map(|file| canonical(file)).collect();
    let mut queue: Vec<PathBuf> = walk.files.clone();
    let mut reader = crate::parser::ModuleReader::quiet();
    let mut files: BTreeSet<String> = BTreeSet::new();
    let mut lookup_dirs: BTreeSet<String> = BTreeSet::new();
    let mut dirs: BTreeSet<String> = BTreeSet::new();
    while let Some(importer) = queue.pop() {
        for specifier in reader.loaded_specifiers(&importer) {
            let relative_specifier = specifier.starts_with("./") || specifier.starts_with("../");
            if relative_specifier && let Some(parent) = importer.parent() {
                // Where the lookup happens: a file appearing beside the one
                // it found can take its place.
                let looked_up = crate::workspace_resolver::normalize(&parent.join(&specifier));
                if let Some(dir) = looked_up.parent().and_then(relative) {
                    lookup_dirs.insert(dir);
                }
            }
            if let Some(package) = index.package_dir_of(&importer, &specifier)
                && let Some(dir) = relative(&root.join(package))
            {
                // The resolver may pick another file than the compiler does
                // (an `exports` map), so the whole package is the safe unit.
                dirs.insert(dir);
            }
            let Some(target) = index.resolve_module_path(&importer, &specifier) else {
                // A file the resolver does not read as source (a JSON file, a
                // stylesheet) is still read.
                let asset = importer
                    .parent()
                    .filter(|_| relative_specifier)
                    .map(|parent| parent.join(specifier.split(['?', '#']).next().unwrap_or("")))
                    .filter(|file| file.is_file());
                if let Some(asset) = asset {
                    files.extend(relative(&asset));
                } else if unresolved.is_none()
                    && index.names_an_unknown_module(&importer, &specifier)
                    && !index.declares_types_for(&specifier)
                {
                    unresolved = Some(format!(
                        "{} imports {specifier}",
                        relative(&importer).unwrap_or_else(|| importer.display().to_string())
                    ));
                }
                continue;
            };
            let Some(path) = relative(&target) else {
                continue;
            };
            if let Some(dir) = Path::new(&path).parent() {
                lookup_dirs.insert(dir.to_string_lossy().replace('\\', "/"));
            }
            if !in_roots(&path) {
                files.insert(path);
            }
            if seen.insert(canonical(&target)) {
                queue.push(target);
            }
        }
    }
    let mut unfollowed_extends = index.unfollowed_extends();

    // Its config chain, and what that config includes beyond the roots.
    if let Some(tsconfig) = service_tsconfig_path(root, &service_root, service) {
        let (chain, unfollowed) =
            crate::module_aliases::tsconfig_chain(root, &tsconfig, &index.internal_package_dirs());
        for spec in unfollowed {
            if !unfollowed_extends.contains(&spec) {
                unfollowed_extends.push(spec);
            }
        }
        for (key, is_files) in [("include", false), ("files", true)] {
            // The nearest config that states the key is the one that counts.
            let Some((config, entries)) = chain.iter().find_map(|(config, json)| {
                json.get(key)
                    .and_then(|value| value.as_array())
                    .map(|entries| (config, entries))
            }) else {
                continue;
            };
            let base = root.join(config.parent().unwrap_or(Path::new("")));
            for entry in entries.iter().filter_map(|entry| entry.as_str()) {
                let target = base.join(static_prefix(entry));
                let Some(path) = relative(&target) else {
                    continue;
                };
                if is_files || target.is_file() {
                    files.insert(path);
                } else {
                    dirs.insert(path);
                }
            }
        }
        for (config, _) in &chain {
            files.extend(relative(&root.join(config)));
        }
    }
    files.extend(walk.manifest.as_deref().and_then(relative));
    files.extend(
        crate::graphql::resolve_declared_schemas(root, &service.graphql_schemas)
            .files
            .iter()
            .filter_map(|file| relative(file)),
    );

    ServiceInputs {
        roots,
        files,
        dirs,
        lookup_dirs,
        unresolved,
        unfollowed_extends,
    }
}

/// The tsconfig that governs `service`: the one carrick.json names, else the
/// nearest `tsconfig.json` at or above its directory. Repo-relative.
fn service_tsconfig_path(root: &Path, service_root: &Path, service: &Config) -> Option<PathBuf> {
    let found = match &service.tsconfig {
        Some(named) => Some(service_root.join(named)).filter(|path| path.is_file()),
        None => service_root
            .ancestors()
            .take_while(|dir| dir.starts_with(root))
            .map(|dir| dir.join("tsconfig.json"))
            .find(|path| path.is_file()),
    }?;
    found.strip_prefix(root).ok().map(Path::to_path_buf)
}

/// The part of a tsconfig `include` entry before its first wildcard.
fn static_prefix(entry: &str) -> PathBuf {
    entry
        .trim_start_matches("./")
        .split('/')
        .take_while(|segment| !segment.contains(['*', '?', '{', '[']))
        .filter(|segment| !segment.is_empty())
        .collect()
}

/// `path` relative to the canonical top level `top`, with `/` separators.
/// `None` for a path outside it. A path that is not on disk is placed by its
/// nearest existing ancestor.
fn top_relative(top: &Path, path: &Path) -> Option<String> {
    let normalized = crate::workspace_resolver::normalize(path);
    let (existing, rest): (PathBuf, PathBuf) = {
        let mut existing = normalized.clone();
        let mut rest = Vec::new();
        while !existing.exists() {
            rest.push(existing.file_name()?.to_os_string());
            existing = existing.parent()?.to_path_buf();
        }
        (existing, rest.into_iter().rev().collect())
    };
    let canonical = existing.canonicalize().ok()?.join(rest);
    let relative = canonical.strip_prefix(top).ok()?;
    Some(relative.to_string_lossy().replace('\\', "/"))
}

/// Whether a carried service's join fields differ from its stored blob's: its
/// type verdicts, its SDK edges and what they left unresolved, and its
/// boundary (carrick#2227). Read on the payload as the upload would send it,
/// before it is stamped. An absent list and an empty one are the same answer,
/// and order is not a difference.
pub fn join_fields_changed(payload: &CloudRepoData, stored: &CloudRepoData) -> bool {
    fn rows<T: serde::Serialize>(value: &Option<Vec<T>>) -> Vec<String> {
        let mut rows: Vec<String> = value
            .iter()
            .flatten()
            .map(|row| serde_json::to_string(row).unwrap_or_default())
            .collect();
        rows.sort();
        rows
    }
    rows(&payload.compat_verdicts) != rows(&stored.compat_verdicts)
        || rows(&payload.sdk_edges) != rows(&stored.sdk_edges)
        || rows(&payload.sdk_unresolved) != rows(&stored.sdk_unresolved)
        || payload.boundary != stored.boundary
}

/// A carried service's payload, when this run's join changed what its stored
/// blob says: `payload` is the stored blob with this run's verdicts and SDK
/// edges attached, `stored` the blob itself. `None` when nothing changed, so
/// nothing is written.
///
/// Its boundary is the stored one with this run's SDK join folded in: the
/// rest of it states the run that analysed the service, which this run did
/// not repeat.
pub fn refreshed_carried(
    mut payload: CloudRepoData,
    stored: &CloudRepoData,
    sdk_unresolved: &[crate::cloud_storage::SdkUnresolved],
    head: &str,
    dirty: bool,
) -> Option<CloudRepoData> {
    payload.boundary = stored.boundary.clone().map(|mut boundary| {
        boundary.fold_sdk_unresolved(sdk_unresolved);
        boundary
    });
    if !join_fields_changed(&payload, stored) {
        return None;
    }
    stamp_carried(&mut payload, head, dirty);
    Some(payload)
}

/// Stamp a carried payload whose join fields changed as this run's: its
/// inputs are unchanged since its stored commit, so its answers describe
/// `head` too, and a new commit keeps the cloud's (commit, scanner) write
/// dedupe from discarding it. It sent no file to the analyzer.
pub fn stamp_carried(payload: &mut CloudRepoData, head: &str, dirty: bool) {
    payload.commit_hash = head.to_string();
    payload.dirty = dirty.then_some(true);
    payload.last_updated = chrono::Utc::now();
    payload.scanner_build = crate::cloud_storage::ScannerBuild::current();
    if let Some(boundary) = payload.boundary.as_mut() {
        boundary.commit_hash = head.to_string();
        boundary.files_attempted = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const THIS: ThisRun<'static> = ThisRun {
        scanner_version: "1.0.0",
        cache_version: 7,
    };

    fn stored(commit: &str) -> CloudRepoData {
        let mut blob: CloudRepoData = serde_json::from_value(serde_json::json!({
            "repo_name": "mono",
            "service_name": "api",
            "endpoints": [],
            "calls": [],
            "mounts": [],
            "apps": {},
            "imported_handlers": [],
            "function_definitions": {},
            "config_json": "{\"serviceName\":\"api\"}",
            "package_json": null,
            "packages": null,
            "last_updated": "2026-10-01T00:00:00Z",
            "commit_hash": commit,
        }))
        .unwrap();
        blob.scanner_version = Some("1.0.0".to_string());
        blob.cache_version = Some(7);
        blob
    }

    fn changed(entries: &[(char, &str)]) -> Vec<ChangedPath> {
        entries
            .iter()
            .map(|(status, path)| ChangedPath {
                status: *status,
                path: path.to_string(),
            })
            .collect()
    }

    fn api_inputs() -> ServiceInputs {
        ServiceInputs {
            roots: vec![Root {
                dir: "apps/api".to_string(),
                left_out: vec!["apps/api/nested".to_string()],
            }],
            files: ["packages/types/x.ts".to_string()].into(),
            dirs: ["packages/shared".to_string()].into(),
            lookup_dirs: ["libs/util".to_string()].into(),
            ..ServiceInputs::default()
        }
    }

    fn decide_with(
        blob: Option<&CloudRepoData>,
        changes: Result<&[ChangedPath], &str>,
        inputs: ServiceInputs,
    ) -> Scope {
        decide(
            blob,
            THIS,
            Some("{\"serviceName\":\"api\"}"),
            changes,
            || inputs,
        )
    }

    fn reason(scope: &Scope) -> Option<Reason> {
        match scope {
            Scope::Analysed { reason, .. } => Some(*reason),
            Scope::Carried { .. } => None,
        }
    }

    #[test]
    fn a_service_whose_inputs_did_not_change_is_carried() {
        let blob = stored("abcdef1234");
        let changes = changed(&[('M', "README.md"), ('M', "apps/web/src/a.ts")]);
        assert_eq!(
            decide_with(Some(&blob), Ok(&changes), api_inputs()),
            Scope::Carried {
                commit: "abcdef1234".to_string()
            }
        );
    }

    #[test]
    fn each_input_kind_analyses_the_service() {
        let blob = stored("abcdef1234");
        for (change, expected) in [
            (('M', "apps/api/src/server.ts"), Some(Reason::InputsChanged)),
            (('M', "apps/api/nested/src/a.ts"), None),
            (('M', "packages/types/x.ts"), Some(Reason::InputsChanged)),
            (
                ('M', "packages/shared/src/deep/y.ts"),
                Some(Reason::InputsChanged),
            ),
            (('A', "libs/util/index.tsx"), Some(Reason::InputsChanged)),
            (('M', "libs/util/other.ts"), None),
            (('D', "libs/util/other.ts"), Some(Reason::InputsChanged)),
            (('M', "apps/api2/x.ts"), None),
            (('M', "yarn.lock"), Some(Reason::RepoInputChanged)),
            (
                ('M', "apps/web/package.json"),
                Some(Reason::RepoInputChanged),
            ),
            (('M', "schema/a.graphqls"), Some(Reason::RepoInputChanged)),
            (('M', "apps/web/.gitignore"), Some(Reason::RepoInputChanged)),
            (('M', "carrick.json"), Some(Reason::RepoInputChanged)),
        ] {
            let changes = changed(&[change]);
            assert_eq!(
                reason(&decide_with(Some(&blob), Ok(&changes), api_inputs())),
                expected,
                "{change:?}"
            );
        }
    }

    #[test]
    fn the_stored_blob_decides_before_any_path_is_read() {
        let changes: Vec<ChangedPath> = Vec::new();
        assert_eq!(
            reason(&decide_with(None, Ok(&changes), api_inputs())),
            Some(Reason::NoStored)
        );
        let mut other = stored("abcdef1234");
        other.scanner_version = Some("0.9.0".to_string());
        assert_eq!(
            reason(&decide_with(Some(&other), Ok(&changes), api_inputs())),
            Some(Reason::OtherScanner)
        );
        let mut other_cache = stored("abcdef1234");
        other_cache.cache_version = Some(6);
        assert_eq!(
            reason(&decide_with(Some(&other_cache), Ok(&changes), api_inputs())),
            Some(Reason::OtherScanner)
        );
        let mut dirty = stored("abcdef1234");
        dirty.dirty = Some(true);
        assert_eq!(
            reason(&decide_with(Some(&dirty), Ok(&changes), api_inputs())),
            Some(Reason::StoredDirty)
        );
        let mut degraded = stored("abcdef1234");
        degraded.types_degraded = Some(crate::cloud_storage::TypeDegradation {
            stage: "capture".to_string(),
            detail: "died".to_string(),
        });
        assert_eq!(
            reason(&decide_with(Some(&degraded), Ok(&changes), api_inputs())),
            Some(Reason::StoredIncomplete)
        );
        let mut lost = stored("abcdef1234");
        lost.boundary = Some(crate::boundary::ServiceBoundary {
            files_lost: crate::boundary::Counted::new(2, Vec::new()),
            ..Default::default()
        });
        assert_eq!(
            reason(&decide_with(Some(&lost), Ok(&changes), api_inputs())),
            Some(Reason::StoredIncomplete)
        );
        let mut reconfigured = stored("abcdef1234");
        reconfigured.config_json = Some("{\"serviceName\":\"api\",\"include\":[\"x\"]}".into());
        assert_eq!(
            reason(&decide_with(
                Some(&reconfigured),
                Ok(&changes),
                api_inputs()
            )),
            Some(Reason::ConfigChanged)
        );
        let blob = stored("abcdef1234");
        assert_eq!(
            reason(&decide_with(Some(&blob), Err("bad object"), api_inputs())),
            Some(Reason::CommitMissing)
        );
    }

    #[test]
    fn inputs_that_cannot_be_read_whole_analyse_the_service() {
        let blob = stored("abcdef1234");
        let changes: Vec<ChangedPath> = Vec::new();
        let unresolved = ServiceInputs {
            unresolved: Some("apps/api/src/a.ts imports nowhere".to_string()),
            ..api_inputs()
        };
        assert_eq!(
            reason(&decide_with(Some(&blob), Ok(&changes), unresolved)),
            Some(Reason::UnresolvedImport)
        );
        let unfollowed = ServiceInputs {
            unfollowed_extends: vec!["@tsconfig/missing".to_string()],
            ..api_inputs()
        };
        assert_eq!(
            reason(&decide_with(Some(&blob), Ok(&changes), unfollowed)),
            Some(Reason::ExtendsUnfollowed)
        );
    }

    #[test]
    fn an_include_entry_is_read_up_to_its_first_wildcard() {
        assert_eq!(static_prefix("./types/**/*.d.ts"), PathBuf::from("types"));
        assert_eq!(static_prefix("../../types"), PathBuf::from("../../types"));
        assert_eq!(static_prefix("src/*"), PathBuf::from("src"));
        assert_eq!(static_prefix("**/*"), PathBuf::new());
    }

    #[test]
    fn join_fields_compare_as_sets_and_absent_is_empty() {
        let stored = stored("abcdef1234");
        let mut payload = stored.clone();
        payload.compat_verdicts = Some(Vec::new());
        payload.sdk_edges = Some(Vec::new());
        assert!(!join_fields_changed(&payload, &stored));
        payload.boundary = Some(crate::boundary::ServiceBoundary::default());
        assert!(join_fields_changed(&payload, &stored));
    }

    #[test]
    fn a_stamped_carried_payload_names_this_run() {
        let mut payload = stored("abcdef1234");
        payload.boundary = Some(crate::boundary::ServiceBoundary {
            commit_hash: "abcdef1234".to_string(),
            files_attempted: 3,
            ..Default::default()
        });
        stamp_carried(&mut payload, "1234567fff", false);
        assert_eq!(payload.commit_hash, "1234567fff");
        assert_eq!(payload.dirty, None);
        let boundary = payload.boundary.unwrap();
        assert_eq!(boundary.commit_hash, "1234567fff");
        assert_eq!(boundary.files_attempted, 0);
    }
}
