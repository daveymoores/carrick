//! The project config Carrick holds for this repository (carrick#2272).
//!
//! A scan reads its services from one place: the `carrick.project-config/1`
//! document whose path is in `CARRICK_CONFIG`, which the Action and the runner
//! fetch from the cloud's `project-config` action before they scan. A
//! `carrick.json` in the tree is never read. Without the variable, or without
//! an entry for this repository, the services are derived from the manifests
//! (`service_derivation::resolve`).
//!
//! The document's producer is carrick-cloud
//! (`lambdas/check-or-upload/project_config_action.ts`, C2 of the
//! project-config epic):
//!
//! ```text
//! { "schema": "carrick.project-config/1",
//!   "projects": [ { "id", "version",
//!     "repos": [ { "repo": "owner/name", "github_repo_id", "held", "behind",
//!                  "config": <a carrick.json body> | null } ] } ] }
//! ```
//!
//! Only `repo` and `config` are read. `held` is not: a seed the cloud answered
//! but did not store is `held: false` with its view in `config`, and that view
//! is what the scan reads. Unknown keys are ignored, as the producer asks.
//!
//! The variable's contract for a runner is `docs/reference/runner.md`.

use std::path::Path;

use crate::config::Config;

/// The variable that names the project config document.
pub const CONFIG_ENV: &str = "CARRICK_CONFIG";

/// The only document schema this scanner reads.
pub const SCHEMA: &str = "carrick.project-config/1";

/// This repository's entry in the project config.
#[derive(Debug)]
pub struct Section {
    /// The `owner/name` the entry was found under.
    pub repo: String,
    /// The entry's services, parsed as a `carrick.json` body is.
    pub services: Vec<Config>,
}

/// This repository's section of the document `CARRICK_CONFIG` names.
///
/// `None` when the variable is unset or empty, when this checkout has no
/// `owner/name` (no `--repo` and no origin remote), when the document holds
/// no entry for it, or when its entry's `config` is null: each of those
/// derives. An unreadable document, a schema other than [`SCHEMA`], two
/// entries for this repository, or a `config` that does not parse is an
/// error: the scan was handed a config and cannot read it.
pub fn section_for(root: &Path) -> Result<Option<Section>, String> {
    let Some(path) = std::env::var_os(CONFIG_ENV).filter(|value| !value.is_empty()) else {
        return Ok(None);
    };
    let Some(repo) = crate::utils::repo_full_name(root) else {
        tracing::debug!(
            "{CONFIG_ENV} is set, but {} names no owner/name; deriving its services",
            root.display()
        );
        return Ok(None);
    };
    let path = Path::new(&path);
    let text = std::fs::read_to_string(path)
        .map_err(|e| format!("Could not read {CONFIG_ENV} ({}): {e}", path.display()))?;
    section_in(&text, &repo, &path.display().to_string())
}

/// [`section_for`] over a document already read, for `repo`.
fn section_in(text: &str, repo: &str, label: &str) -> Result<Option<Section>, String> {
    let document: serde_json::Value = serde_json::from_str(text)
        .map_err(|e| format!("{CONFIG_ENV} ({label}) is not JSON: {e}"))?;
    let schema = document.get("schema").and_then(|s| s.as_str());
    if schema != Some(SCHEMA) {
        return Err(format!(
            "{CONFIG_ENV} ({label}) has schema {}, and this scanner reads {SCHEMA}",
            schema.map_or_else(|| "none".to_string(), |s| format!("'{s}'"))
        ));
    }
    let entries: Vec<&serde_json::Value> = document
        .get("projects")
        .and_then(|p| p.as_array())
        .into_iter()
        .flatten()
        .filter_map(|project| project.get("repos").and_then(|r| r.as_array()))
        .flatten()
        .filter(|entry| {
            entry
                .get("repo")
                .and_then(|r| r.as_str())
                .is_some_and(|name| name.eq_ignore_ascii_case(repo))
        })
        .collect();
    let entry = match entries.as_slice() {
        [] => {
            tracing::debug!("{CONFIG_ENV} holds no entry for {repo}; deriving its services");
            return Ok(None);
        }
        [entry] => *entry,
        _ => {
            return Err(format!(
                "{CONFIG_ENV} ({label}) holds {} entries for {repo}, and a repository has one",
                entries.len()
            ));
        }
    };
    let name = entry["repo"].as_str().unwrap_or(repo).to_string();
    let config = match entry.get("config") {
        None | Some(serde_json::Value::Null) => {
            tracing::debug!("{CONFIG_ENV} holds no config for {name}; deriving its services");
            return Ok(None);
        }
        Some(config) => config,
    };
    Section::from_body(&name, config).map(Some)
}

impl Section {
    /// `repo`'s entry from its `config` body, parsed as a `carrick.json`
    /// body is.
    pub fn from_body(repo: &str, config: &serde_json::Value) -> Result<Section, String> {
        let source = format!("{CONFIG_ENV} entry for {repo}");
        let services = Config::load_services_from_str(&config.to_string(), &source)
            .map_err(|e| e.to_string())?;
        Ok(Section {
            repo: repo.to_string(),
            services,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn document(repos: serde_json::Value) -> String {
        json!({
            "schema": SCHEMA,
            "projects": [{ "id": "p1", "version": 3, "repos": repos }],
        })
        .to_string()
    }

    fn names(section: &Section) -> Vec<&str> {
        section
            .services
            .iter()
            .map(|s| s.service_name.as_deref().unwrap_or_default())
            .collect()
    }

    #[test]
    fn the_entry_for_this_repo_is_read_case_insensitively() {
        let text = document(json!([
            { "repo": "acme/api", "github_repo_id": "1", "held": true, "behind": false,
              "config": { "services": [{ "serviceName": "x", "directory": "x" }] } },
            { "repo": "acme/Other", "github_repo_id": "2", "held": true, "behind": false,
              "unknown_key": 1,
              "config": { "services": [
                  { "serviceName": "api", "directory": "services/api",
                    "internalEnvVars": ["ORDERS_URL"] }] } },
        ]));
        let section = section_in(&text, "ACME/other", "doc").unwrap().unwrap();
        assert_eq!(section.repo, "acme/Other");
        assert_eq!(names(&section), ["api"]);
        assert!(section.services[0].internal_env_vars.contains("ORDERS_URL"));
    }

    /// The renderer writes a lone root member as a flat body.
    #[test]
    fn a_flat_body_is_one_root_service() {
        let text = document(json!([
            { "repo": "acme/api", "held": true, "behind": false,
              "config": { "serviceName": "api", "internalEnvVars": ["X_URL"] } },
        ]));
        let section = section_in(&text, "acme/api", "doc").unwrap().unwrap();
        assert_eq!(names(&section), ["api"]);
        assert!(section.services[0].directory.is_none());
    }

    /// A seed the cloud answered but did not store: `held: false` with its
    /// view in `config`, which the scan reads. A refused seed carries a null
    /// `config`, and the scan derives.
    #[test]
    fn an_unheld_view_is_read_and_a_null_config_derives() {
        let view = document(json!([
            { "repo": "acme/api", "held": false, "behind": true,
              "config": { "services": [{ "serviceName": "seeded", "directory": "a" }] } },
        ]));
        let section = section_in(&view, "acme/api", "doc").unwrap().unwrap();
        assert_eq!(names(&section), ["seeded"]);
        let refused = document(json!([
            { "repo": "acme/api", "held": false, "behind": true, "config": null },
        ]));
        assert!(section_in(&refused, "acme/api", "doc").unwrap().is_none());
    }

    #[test]
    fn no_entry_derives() {
        let text = document(json!([{ "repo": "acme/web", "config": {} }]));
        assert!(section_in(&text, "acme/api", "doc").unwrap().is_none());
        let empty = json!({ "schema": SCHEMA, "projects": [] }).to_string();
        assert!(section_in(&empty, "acme/api", "doc").unwrap().is_none());
    }

    #[test]
    fn two_entries_a_wrong_schema_or_a_bad_body_are_errors() {
        let two = json!({
            "schema": SCHEMA,
            "projects": [
                { "id": "p1", "repos": [{ "repo": "acme/api", "config": {} }] },
                { "id": "p2", "repos": [{ "repo": "Acme/API", "config": {} }] },
            ],
        })
        .to_string();
        let error = section_in(&two, "acme/api", "doc").unwrap_err();
        assert!(error.contains("2 entries for acme/api"), "{error}");

        let wrong = json!({ "schema": "carrick.project-config/2", "projects": [] }).to_string();
        let error = section_in(&wrong, "acme/api", "doc").unwrap_err();
        assert!(error.contains("'carrick.project-config/2'"), "{error}");
        let error = section_in("{}", "acme/api", "doc").unwrap_err();
        assert!(error.contains("schema none"), "{error}");
        assert!(section_in("not json", "acme/api", "doc").is_err());

        let bad = document(json!([
            { "repo": "acme/api", "config": { "services": [{ "directory": 7 }] } },
        ]));
        assert!(section_in(&bad, "acme/api", "doc").is_err());
    }
}
