use std::{collections::HashSet, io, path::PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::Value;

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
                if file_name.ends_with("_types.ts") {
                    let base_name = file_name.trim_end_matches(".ts");
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


