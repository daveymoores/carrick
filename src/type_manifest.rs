use crate::cloud_storage::{ManifestRole, ManifestTypeKind};

pub fn normalize_manifest_method(method: &str) -> String {
    let trimmed = method.trim();
    if trimmed.is_empty() {
        "UNKNOWN".to_string()
    } else {
        trimmed.to_uppercase()
    }
}

pub fn build_manifest_type_alias(
    method: &str,
    path: &str,
    role: ManifestRole,
    type_kind: ManifestTypeKind,
) -> String {
    let role_label = match role {
        ManifestRole::Producer => "producer",
        ManifestRole::Consumer => "consumer",
    };
    let type_label = match type_kind {
        ManifestTypeKind::Request => "Request",
        ManifestTypeKind::Response => "Response",
    };

    let key = format!("{}|{}|{}|{}", method, path, role_label, type_label);
    let hash = fnv1a_hash(&key);

    format!("Endpoint_{:016x}_{}", hash, type_label)
}

pub fn build_call_site_id(file_path: &str, line_number: u32, method: &str, path: &str) -> String {
    let key = format!("{}|{}|{}|{}", file_path, line_number, method, path);
    format!("{:016x}", fnv1a_hash(&key))
}

pub fn build_manifest_type_alias_with_call_id(
    method: &str,
    path: &str,
    role: ManifestRole,
    type_kind: ManifestTypeKind,
    call_id: Option<&str>,
) -> String {
    let base = build_manifest_type_alias(method, path, role, type_kind);
    match call_id {
        Some(id) if !id.trim().is_empty() => format!("{}_Call{}", base, id.trim()),
        _ => base,
    }
}

pub fn parse_file_location(location: &str) -> (String, u32) {
    let segments: Vec<&str> = location.split(':').collect();
    if segments.len() < 2 {
        return (location.to_string(), 1);
    }

    let mut line_number = None;
    let mut cut_index = segments.len();

    if let Ok(last_num) = segments[segments.len() - 1].parse::<u32>() {
        if let Ok(second_last_num) = segments[segments.len() - 2].parse::<u32>() {
            line_number = Some(second_last_num);
            cut_index = segments.len().saturating_sub(2);
        } else {
            line_number = Some(last_num);
            cut_index = segments.len().saturating_sub(1);
        }
    }

    let file_path = if cut_index < segments.len() {
        segments[..cut_index].join(":")
    } else {
        location.to_string()
    };

    let line_number = match line_number {
        Some(0) | None => 1,
        Some(value) => value,
    };

    (file_path, line_number)
}

fn fnv1a_hash(input: &str) -> u64 {
    const OFFSET_BASIS: u64 = 0xcbf29ce484222325;
    const FNV_PRIME: u64 = 0x100000001b3;

    let mut hash = OFFSET_BASIS;
    for byte in input.as_bytes() {
        hash ^= *byte as u64;
        hash = hash.wrapping_mul(FNV_PRIME);
    }
    hash
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_build_manifest_type_alias_with_call_id() {
        let base = build_manifest_type_alias(
            "GET",
            "/users",
            ManifestRole::Consumer,
            ManifestTypeKind::Response,
        );
        let call_id = build_call_site_id("src/service.ts", 12, "GET", "/users");
        let with_call = build_manifest_type_alias_with_call_id(
            "GET",
            "/users",
            ManifestRole::Consumer,
            ManifestTypeKind::Response,
            Some(&call_id),
        );

        assert_ne!(base, with_call);
        assert!(with_call.contains("_Call"));
    }
}
