use semver::Version;
use serde::{Deserialize, Serialize};
use std::{collections::HashMap, io, path::PathBuf};

#[derive(Debug, Serialize)]
pub struct PackagesDataForTypeScript {
    pub name: String,
    pub dependencies: HashMap<String, String>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct PackageJson {
    pub name: Option<String>,
    pub version: Option<String>,
    #[serde(default)]
    pub dependencies: HashMap<String, String>,
    #[serde(default)]
    #[serde(rename = "devDependencies")]
    pub dev_dependencies: HashMap<String, String>,
    #[serde(default)]
    #[serde(rename = "peerDependencies")]
    pub peer_dependencies: HashMap<String, String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PackageInfo {
    pub name: String,
    pub version: String,
    pub source_path: PathBuf,
}

#[derive(Debug, Default, Clone, serde::Serialize, serde::Deserialize)]
pub struct Packages {
    package_jsons: Vec<PackageJson>,
    source_paths: Vec<PathBuf>,
    merged_dependencies: HashMap<String, PackageInfo>,
}

impl Packages {
    pub fn new(package_json_paths: Vec<PathBuf>) -> Result<Self, io::Error> {
        let mut packages = Packages::default();

        for path in package_json_paths {
            let content = std::fs::read_to_string(&path)?;
            let package_json: PackageJson = serde_json::from_str(&content).map_err(|e| {
                io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!("Failed to parse {}: {}", path.display(), e),
                )
            })?;

            packages.package_jsons.push(package_json);
            packages.source_paths.push(path);
        }

        packages.resolve_dependencies();
        Ok(packages)
    }

    /// Resolves dependencies across all package.json files, choosing the highest version for conflicts
    fn resolve_dependencies(&mut self) {
        for (idx, package_json) in self.package_jsons.iter().enumerate() {
            let source_path = &self.source_paths[idx];

            // Process all dependency types
            let all_deps = [
                &package_json.dependencies,
                &package_json.dev_dependencies,
                &package_json.peer_dependencies,
            ];

            for deps in all_deps {
                for (name, version_spec) in deps {
                    let clean_version = self.clean_version_spec(version_spec);

                    match self.merged_dependencies.get(name) {
                        Some(existing) => {
                            // Compare versions and keep the higher one
                            if self.should_update_version(&existing.version, &clean_version) {
                                self.merged_dependencies.insert(
                                    name.clone(),
                                    PackageInfo {
                                        name: name.clone(),
                                        version: clean_version,
                                        source_path: source_path.clone(),
                                    },
                                );
                            }
                        }
                        None => {
                            self.merged_dependencies.insert(
                                name.clone(),
                                PackageInfo {
                                    name: name.clone(),
                                    version: clean_version,
                                    source_path: source_path.clone(),
                                },
                            );
                        }
                    }
                }
            }
        }
    }

    /// Cleans version specifications to extract actual version numbers
    fn clean_version_spec(&self, version_spec: &str) -> String {
        // Remove common prefixes like ^, ~, >=, etc.
        let cleaned = version_spec
            .trim_start_matches('^')
            .trim_start_matches('~')
            .trim_start_matches(">=")
            .trim_start_matches("<=")
            .trim_start_matches('>')
            .trim_start_matches('<')
            .trim_start_matches('=');

        // Handle ranges like "1.0.0 - 2.0.0" by taking the higher version
        if let Some(_dash_pos) = cleaned.find(" - ") {
            let versions: Vec<&str> = cleaned.split(" - ").collect();
            if versions.len() == 2 {
                return versions[1].trim().to_string();
            }
        }

        // Handle "|| " separated versions by taking the first valid one
        if let Some(_or_pos) = cleaned.find(" || ") {
            let versions: Vec<&str> = cleaned.split(" || ").collect();
            if !versions.is_empty() {
                return versions[0].trim().to_string();
            }
        }

        cleaned.trim().to_string()
    }

    /// Determines if we should update to a new version (chooses higher version)
    fn should_update_version(&self, existing: &str, new: &str) -> bool {
        match (Version::parse(existing), Version::parse(new)) {
            (Ok(existing_ver), Ok(new_ver)) => new_ver > existing_ver,
            (Ok(_), Err(_)) => false, // Keep existing if new is invalid
            (Err(_), Ok(_)) => true,  // Use new if existing is invalid
            (Err(_), Err(_)) => {
                // Fallback to string comparison if both are invalid semver
                new > existing
            }
        }
    }

    /// Gets all merged dependencies
    pub fn get_dependencies(&self) -> &HashMap<String, PackageInfo> {
        &self.merged_dependencies
    }
}
