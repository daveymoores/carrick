use crate::packages::Packages;
use crate::services::type_sidecar::{WrapperRule, WrapperUnwrapKind, WrapperUnwrapRule};

pub fn wrapper_rules_for_packages(packages: &Packages) -> Vec<WrapperRule> {
    let dependencies = packages.get_dependencies();
    let mut rules = Vec::new();

    if dependencies.contains_key("axios") {
        rules.push(WrapperRule {
            package: "axios".to_string(),
            type_name: "AxiosResponse".to_string(),
            unwrap: WrapperUnwrapRule {
                kind: WrapperUnwrapKind::Property,
                property: Some("data".to_string()),
                index: None,
            },
        });
    }

    rules
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::packages::PackageInfo;
    use std::path::PathBuf;

    #[test]
    fn returns_rules_for_known_dependencies() {
        let mut packages = Packages::default();
        packages.merged_dependencies.insert(
            "axios".to_string(),
            PackageInfo {
                name: "axios".to_string(),
                version: "1.7.0".to_string(),
                source_path: PathBuf::from("package.json"),
            },
        );

        let rules = wrapper_rules_for_packages(&packages);
        assert_eq!(rules.len(), 1);
        assert_eq!(rules[0].package, "axios");
        assert_eq!(rules[0].type_name, "AxiosResponse");
        assert_eq!(rules[0].unwrap.kind, WrapperUnwrapKind::Property);
        assert_eq!(rules[0].unwrap.property.as_deref(), Some("data"));
    }

    #[test]
    fn returns_empty_for_unknown_dependencies() {
        let packages = Packages::default();
        let rules = wrapper_rules_for_packages(&packages);
        assert!(rules.is_empty());
    }
}
