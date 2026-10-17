//! The census of decided sites and their records (carrick#2433,
//! carrick#2435): a measurement on real repos, offline and model-free.
//!
//! Per repo it counts discovery's records, the distinct sites where a name
//! is decided, the leads by reason, the records by bucket (every slot
//! forced, some asked, lead), and the record bytes at p50, p90 and max. It
//! compares every decided HTTP site with the request summaries' rows for the
//! same request: a row that is not on a decided site is listed. And it says
//! how many records one line inserted at the top of a file would change
//! (each record names its positions as `file:line`): for the file with the
//! most records, and on average over the files that hold any.
//!
//! Each repo needs a flag-on scan's rows (`<store>/positions`), discovery's
//! census for it (carrick#2431, `{ "sites": [...] }`), and the store's
//! `/library-claims` answer for its packages:
//!
//! `CARRICK_SITE_CENSUS='<label>=<repo>=<positions dir>=<sites.json>=<library-claims.json>[;…]'
//! [CARRICK_SITE_CENSUS_OUT=<file>] cargo test --lib site_census -- --ignored --nocapture`

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};

use serde::Serialize;

use super::decided_sites::{DiscoveryFile, SiteLabel, decided_sites, relative};
use super::site_record::{Bucket, site_records};
use crate::config::Config;
use crate::position_rows::PositionIndex;

#[derive(Debug, Default, Serialize)]
struct RepoCensus {
    discovered: usize,
    labelled: usize,
    decided_sites: usize,
    leads: BTreeMap<String, usize>,
    unplaced: usize,
    walked: usize,
    buckets: BTreeMap<String, usize>,
    lead_reasons: BTreeMap<String, usize>,
    state_bytes_p50: usize,
    state_bytes_p90: usize,
    state_bytes_max: usize,
    /// Summary rows whose request this pass decides.
    summary_rows_compared: usize,
    /// Of those, rows not on a decided site, as `file:line`.
    summary_rows_off_site: Vec<String>,
    /// Records naming a line of the file with the most records.
    edit_reasks_busiest: usize,
    busiest_file: String,
    /// Records naming a line of a file, on average over files holding any.
    edit_reasks_mean: f64,
}

fn percentile(sorted: &[usize], pct: usize) -> usize {
    if sorted.is_empty() {
        return 0;
    }
    sorted[(sorted.len() - 1) * pct / 100]
}

fn census_repo(root: &Path, positions: &Path, sites: &Path, claims: &Path) -> RepoCensus {
    let root = std::fs::canonicalize(root).expect("the repo exists");
    let config = root.join("carrick.json");
    let services = if config.exists() {
        Config::load_services(vec![config]).expect("carrick.json loads")
    } else {
        vec![Config::default()]
    };
    let discovered = serde_json::from_str::<DiscoveryFile>(
        &std::fs::read_to_string(sites).expect("the sites file reads"),
    )
    .expect("the sites file parses")
    .sites;
    let answer: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(claims).expect("the claims read"))
            .expect("the claims parse");
    let labels = SiteLabel::from_store_answer(&answer);
    let mut by_service: BTreeMap<String, PathBuf> = BTreeMap::new();
    for entry in std::fs::read_dir(positions).expect("the positions dir reads") {
        let path = entry.expect("dir entry").path();
        if path.to_string_lossy().ends_with(".jsonl.gz")
            && let Ok(rows) = crate::position_rows::read_rows_file(&path)
        {
            by_service.insert(rows.header.service.clone(), path);
        }
    }
    let mut census = RepoCensus {
        discovered: discovered.len(),
        labelled: discovered
            .iter()
            .filter(|site| {
                site.end
                    .as_ref()
                    .is_some_and(|end| SiteLabel::find(&labels, end).is_some())
            })
            .count(),
        ..RepoCensus::default()
    };
    let mut bytes: Vec<usize> = Vec::new();
    let mut per_file: HashMap<String, usize> = HashMap::new();
    for service in &services {
        let label = service
            .service_name
            .clone()
            .unwrap_or_else(|| "(root)".to_string());
        let index = match by_service.get(&label) {
            Some(path) => PositionIndex::of(
                &root,
                crate::position_rows::read_rows_file(path).expect("rows read"),
            ),
            None => PositionIndex::missing(&root),
        };
        let inputs = crate::engine::discover_service_inputs(&root, service, Some(index));
        let files: HashSet<String> = inputs
            .files
            .keys()
            .map(|file| relative(&root, file))
            .collect();
        let mine: Vec<_> = discovered
            .iter()
            .filter(|site| files.contains(&site.file))
            .cloned()
            .collect();
        let decided = decided_sites(&inputs, &root, &mine, &labels);
        census.decided_sites += decided.sites.len();
        census.unplaced += decided.unplaced.len();
        census.walked += decided.walked.len();
        for lead in &decided.leads {
            let reason = serde_json::to_value(lead.reason)
                .ok()
                .and_then(|value| value.as_str().map(str::to_string))
                .unwrap_or_default();
            *census.leads.entry(reason).or_default() += 1;
        }
        let records = site_records(&inputs, &root, &decided, &labels);
        for record in &records {
            let bucket = match record.bucket {
                Bucket::Forced => "forced",
                Bucket::Asked => "asked",
                Bucket::Lead => "lead",
            };
            *census.buckets.entry(bucket.to_string()).or_default() += 1;
            if let Some(reason) = record.lead {
                *census.lead_reasons.entry(reason.to_string()).or_default() += 1;
            }
            bytes.push(record.state_bytes());
            let state = serde_json::to_string(&record.state).unwrap_or_default();
            for file in &files {
                if state.contains(&format!("{file}:")) {
                    *per_file.entry(file.clone()).or_default() += 1;
                }
            }
        }
        // The summaries' rows against the decided sites of the same request.
        let summaries = crate::request_summary::summarize(
            &inputs,
            &crate::client_semantics::LibrarySemantics::default(),
        );
        let mut sites_of: HashMap<(String, u32), HashSet<(String, u32)>> = HashMap::new();
        for site in decided
            .sites
            .iter()
            .filter(|site| site.end.role == "http_client")
        {
            sites_of
                .entry((site.end.file.clone(), site.end.span_start))
                .or_default()
                .insert((site.file.clone(), site.line));
        }
        for file in inputs.files.keys() {
            for rows in summaries.rows(file).into_iter().flat_map(|at| at.values()) {
                for row in rows {
                    let request = (relative(&root, &row.request_at.0), row.request_at.1);
                    let Some(decided) = sites_of.get(&request) else {
                        continue;
                    };
                    census.summary_rows_compared += 1;
                    let at = (relative(&root, file), row.line);
                    if !decided.contains(&at) {
                        census
                            .summary_rows_off_site
                            .push(format!("{}:{}", at.0, at.1));
                    }
                }
            }
        }
    }
    bytes.sort_unstable();
    census.state_bytes_p50 = percentile(&bytes, 50);
    census.state_bytes_p90 = percentile(&bytes, 90);
    census.state_bytes_max = bytes.last().copied().unwrap_or_default();
    if let Some((file, count)) = per_file
        .iter()
        .max_by_key(|(file, count)| (**count, file.as_str()))
    {
        census.busiest_file = file.clone();
        census.edit_reasks_busiest = *count;
    }
    if !per_file.is_empty() {
        census.edit_reasks_mean = per_file.values().sum::<usize>() as f64 / per_file.len() as f64;
    }
    census.summary_rows_off_site.sort();
    census.summary_rows_off_site.dedup();
    census
}

#[test]
#[ignore = "a census over real repos: set CARRICK_SITE_CENSUS"]
fn site_census() {
    let Ok(spec) = std::env::var("CARRICK_SITE_CENSUS") else {
        crate::errln!("CARRICK_SITE_CENSUS is not set");
        return;
    };
    let mut out: BTreeMap<String, RepoCensus> = BTreeMap::new();
    for repo in spec.split(';').filter(|part| !part.is_empty()) {
        let parts: Vec<&str> = repo.split('=').collect();
        let [label, root, positions, sites, claims] = parts.as_slice() else {
            panic!("expected <label>=<repo>=<positions>=<sites>=<claims>, got {repo}");
        };
        out.insert(
            label.to_string(),
            census_repo(
                Path::new(root),
                Path::new(positions),
                Path::new(sites),
                Path::new(claims),
            ),
        );
    }
    let text = serde_json::to_string_pretty(&out).expect("the census serialises");
    match std::env::var("CARRICK_SITE_CENSUS_OUT") {
        Ok(path) => std::fs::write(path, &text).expect("the census writes"),
        Err(_) => crate::outln!("{text}"),
    }
}
