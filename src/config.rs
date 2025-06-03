use std::{collections::HashSet, io, path::PathBuf};

use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize, Default, Clone)]
pub struct Config {
    #[serde(default)]
    #[serde(rename = "internalEnvVars")]
    pub internal_env_vars: HashSet<String>,
    #[serde(default)]
    #[serde(rename = "internalDomains")]
    pub internal_domains: HashSet<String>,
    #[serde(default)]
    #[serde(rename = "externalEnvVars")]
    pub external_env_vars: HashSet<String>,
    #[serde(default)]
    #[serde(rename = "externalDomains")]
    pub external_domains: HashSet<String>,
}

impl Config {
    // Get vec of filepaths and create HashSet from json
    pub fn new(file_paths: Vec<PathBuf>) -> Result<Self, std::io::Error> {
        let mut merged_config = Config::default();
        for path in file_paths.iter() {
            let config_content = std::fs::read_to_string(path)?;
            let config: Config = serde_json::from_str(&config_content)
                .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;

            merged_config
                .internal_env_vars
                .extend(config.internal_env_vars);
            merged_config
                .internal_domains
                .extend(config.internal_domains);
            merged_config
                .external_env_vars
                .extend(config.external_env_vars);
            merged_config
                .external_domains
                .extend(config.external_domains);
        }

        Ok(merged_config)
    }

    pub fn is_internal_call(&self, route: &str) -> bool {
        // Check if route starts with any internal env var
        if route.starts_with("ENV_VAR:") {
            let parts: Vec<&str> = route.split(':').collect();
            if parts.len() >= 2 {
                let env_var = parts[1];
                return self.internal_env_vars.iter().any(|var| var == env_var);
            }
        }

        // Check if route starts with any internal domain
        self.internal_domains
            .iter()
            .any(|domain| route.starts_with(domain))
    }

    pub fn is_external_call(&self, route: &str) -> bool {
        // Check if route starts with any external env var
        if route.starts_with("ENV_VAR:") {
            let parts: Vec<&str> = route.split(':').collect();
            if parts.len() >= 2 {
                let env_var = parts[1];
                return self.external_env_vars.iter().any(|var| var == env_var);
            }
        }

        // Check if route starts with any external domain
        self.external_domains
            .iter()
            .any(|domain| route.starts_with(domain))
    }
}
