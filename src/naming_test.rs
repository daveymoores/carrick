#[cfg(test)]
mod tests {
    use super::super::analyzer::Analyzer;
    use super::super::config::Config;
    use super::super::visitor::{Call, Json};
    use std::path::PathBuf;
    use swc_common::{SourceMap, sync::Lrc};

    #[test]
    fn test_naming_strategy_for_multiple_fetch_calls() {
        let config = Config::default();
        let source_map = Lrc::new(SourceMap::new(swc_common::FilePathMapping::empty()));
        let mut analyzer = Analyzer::new(config, source_map);

        // Create three fetch calls to the same endpoint
        let route = "/api/comments";
        let method = "GET";
        
        let mut calls = vec![
            Call {
                route: route.to_string(),
                method: method.to_string(),
                response: Json::Null,
                request: None,
                response_type: None,
                request_type: None,
                call_file: PathBuf::from("file1.ts"),
                call_id: None,
                call_number: None,
                common_type_name: None,
            },
            Call {
                route: route.to_string(),
                method: method.to_string(),
                response: Json::Null,
                request: None,
                response_type: None,
                request_type: None,
                call_file: PathBuf::from("file2.ts"),
                call_id: None,
                call_number: None,
                common_type_name: None,
            },
            Call {
                route: route.to_string(),
                method: method.to_string(),
                response: Json::Null,
                request: None,
                response_type: None,
                request_type: None,
                call_file: PathBuf::from("file3.ts"),
                call_id: None,
                call_number: None,
                common_type_name: None,
            },
        ];

        // Process the calls
        let processed_calls = analyzer.process_fetch_calls(calls);

        // Verify naming
        assert_eq!(processed_calls.len(), 3);
        
        // All calls should have the same common type name for comparison
        let expected_common_name = "GetApiCommentsResponse";
        for call in &processed_calls {
            assert_eq!(call.common_type_name.as_ref().unwrap(), expected_common_name);
        }

        // Each call should have unique call IDs
        let call_ids: Vec<String> = processed_calls.iter()
            .map(|c| c.call_id.as_ref().unwrap().clone())
            .collect();
        
        assert_eq!(call_ids[0], "GetApiCommentsResponseCall1");
        assert_eq!(call_ids[1], "GetApiCommentsResponseCall2");
        assert_eq!(call_ids[2], "GetApiCommentsResponseCall3");

        // Each call should have sequential call numbers
        assert_eq!(processed_calls[0].call_number.unwrap(), 1);
        assert_eq!(processed_calls[1].call_number.unwrap(), 2);
        assert_eq!(processed_calls[2].call_number.unwrap(), 3);
    }

    #[test]
    fn test_common_vs_unique_naming() {
        let route = "/users/:id/profile";
        let method = "GET";

        // Common name for type comparison (same for producer and all consumers)
        let common_name = Analyzer::generate_common_type_alias_name(route, method, false);
        assert_eq!(common_name, "GetUsersByIdProfileResponse");

        // Unique names for call tracking
        let unique_name_1 = Analyzer::generate_unique_call_alias_name(route, method, false, 1);
        let unique_name_2 = Analyzer::generate_unique_call_alias_name(route, method, false, 2);
        
        assert_eq!(unique_name_1, "GetUsersByIdProfileResponseCall1");
        assert_eq!(unique_name_2, "GetUsersByIdProfileResponseCall2");
        
        // Verify they're different for tracking but share common base for comparison
        assert_ne!(unique_name_1, unique_name_2);
        assert!(unique_name_1.starts_with(&common_name));
        assert!(unique_name_2.starts_with(&common_name));
    }

    #[test]
    fn test_example_naming_output() {
        let example = Analyzer::example_naming_strategy();
        
        // Verify the example contains expected content
        assert!(example.contains("GetApiCommentsResponse"));
        assert!(example.contains("GetApiCommentsResponseCall1"));
        assert!(example.contains("GetApiCommentsResponseCall2"));
        assert!(example.contains("GetApiCommentsResponseCall3"));
        assert!(example.contains("Common Interface Name"));
        assert!(example.contains("Call Tracking Names"));
    }

    #[test] 
    fn test_different_routes_get_different_names() {
        // Test different routes generate different names
        let name1 = Analyzer::generate_common_type_alias_name("/api/users", "GET", false);
        let name2 = Analyzer::generate_common_type_alias_name("/api/comments", "GET", false);
        let name3 = Analyzer::generate_common_type_alias_name("/api/orders", "POST", false);
        
        assert_eq!(name1, "GetApiUsersResponse");
        assert_eq!(name2, "GetApiCommentsResponse");
        assert_eq!(name3, "PostApiOrdersResponse");
        
        // All should be unique
        assert_ne!(name1, name2);
        assert_ne!(name2, name3);
        assert_ne!(name1, name3);
    }
}