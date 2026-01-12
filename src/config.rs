use std::{collections::HashSet, io, path::PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Serialize, Deserialize, Default, Clone)]
pub struct Config {
    #[serde(default)]
    #[serde(rename = "serviceName")]
    pub service_name: Option<String>,
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

            // Use first non-None service_name encountered
            if merged_config.service_name.is_none() {
                merged_config.service_name = config.service_name;
            }

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

/// Creates a standard TypeScript configuration for type checking only
pub fn create_standard_tsconfig() -> Value {
    serde_json::json!({
        "compilerOptions": {
            "target": "ES2020",
            "module": "commonjs",
            "strict": true,
            "esModuleInterop": true,
            "skipLibCheck": true,
            "forceConsistentCasingInFileNames": true,
            "resolveJsonModule": true,
            "noEmit": true,
            "baseUrl": ".",
            "paths": {
                "*-types": ["./*_types"]
            }
        },
        "include": [
            "*.ts",
            "**/*.ts"
        ],
        "exclude": [
            "node_modules"
        ]
    })
}

pub fn create_dynamic_tsconfig(output_dir: &std::path::Path) -> Value {
    use std::collections::HashMap;

    let mut paths = HashMap::new();

    // Add the generic pattern
    paths.insert("*-types".to_string(), vec!["./*_types".to_string()]);

    // Scan for actual type files and create specific mappings
    if let Ok(entries) = std::fs::read_dir(output_dir) {
        for entry in entries.flatten() {
            if let Some(file_name) = entry.file_name().to_str() {
                if file_name.ends_with("_types.ts") || file_name.ends_with("_types.d.ts") {
                    let base_name = file_name.trim_end_matches(".d.ts").trim_end_matches(".ts");
                    let module_name = base_name.replace("_", "-");
                    paths.insert(module_name, vec![format!("./{}", base_name)]);
                }
            }
        }
    }

    serde_json::json!({
        "compilerOptions": {
            "target": "ES2020",
            "module": "commonjs",
            "strict": true,
            "esModuleInterop": true,
            "skipLibCheck": true,
            "forceConsistentCasingInFileNames": true,
            "resolveJsonModule": true,
            "noEmit": true,
            "baseUrl": ".",
            "paths": paths
        },
        "include": [
            "*.ts",
            "**/*.ts"
        ],
        "exclude": [
            "node_modules"
        ]
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_config_with_service_name() {
        let json = r#"{
            "serviceName": "order-service",
            "internalEnvVars": ["USER_SERVICE_URL"],
            "externalEnvVars": ["STRIPE_API"]
        }"#;

        let config: Config = serde_json::from_str(json).unwrap();
        assert_eq!(config.service_name, Some("order-service".to_string()));
        assert!(config.internal_env_vars.contains("USER_SERVICE_URL"));
        assert!(config.external_env_vars.contains("STRIPE_API"));
    }

    #[test]
    fn test_is_internal_call() {
        let config = Config {
            service_name: None,
            internal_env_vars: ["USER_SERVICE_URL".to_string()].into_iter().collect(),
            internal_domains: ["https://api.internal.com".to_string()]
                .into_iter()
                .collect(),
            external_env_vars: HashSet::new(),
            external_domains: HashSet::new(),
        };

        // ENV_VAR pattern matching
        assert!(config.is_internal_call("ENV_VAR:USER_SERVICE_URL:/users"));
        assert!(!config.is_internal_call("ENV_VAR:UNKNOWN_URL:/users"));

        // Domain matching (route must start with domain)
        assert!(config.is_internal_call("https://api.internal.com/users"));
        assert!(!config.is_internal_call("https://unknown.com/users"));
    }

    #[test]
    fn test_is_external_call() {
        let config = Config {
            service_name: None,
            internal_env_vars: HashSet::new(),
            internal_domains: HashSet::new(),
            external_env_vars: ["STRIPE_API".to_string()].into_iter().collect(),
            external_domains: ["https://api.stripe.com".to_string()].into_iter().collect(),
        };

        // ENV_VAR pattern matching
        assert!(config.is_external_call("ENV_VAR:STRIPE_API:/charges"));
        assert!(!config.is_external_call("ENV_VAR:UNKNOWN_URL:/users"));

        // Domain matching (route must start with domain)
        assert!(config.is_external_call("https://api.stripe.com/charges"));
        assert!(!config.is_external_call("https://unknown.com/users"));
    }
}
