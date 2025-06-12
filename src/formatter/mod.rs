use crate::analyzer::{ApiAnalysisResult, ApiIssues};
use std::collections::HashMap;

pub struct FormattedOutput {
    pub content: String,
}

impl FormattedOutput {
    pub fn new(result: ApiAnalysisResult) -> Self {
        let content = format_analysis_results(result);
        Self { content }
    }

    pub fn print(&self) {
        println!("{}", self.content);
    }
}

pub fn format_analysis_results(result: ApiAnalysisResult) -> String {
    if result.issues.is_empty() {
        return format_no_issues(&result);
    }

    let categorized_issues = categorize_issues(&result.issues);
    let total_issues = categorized_issues.critical.len()
        + categorized_issues.connectivity.len()
        + categorized_issues.configuration.len();

    let mut output = String::new();

    // Add machine-readable delimiter for GitHub Action
    output.push_str("<!-- CARRICK_OUTPUT_START -->\n");
    output.push_str(&format!("<!-- CARRICK_ISSUE_COUNT:{} -->\n", total_issues));

    // Header
    output.push_str(&format!(
        "### 🪢 CARRICK: API Analysis Results\n\nFound **{} total issues**: **{} critical mismatches**, **{} connectivity issues**, and **{} configuration suggestions**.\n\n<br>\n\n",
        total_issues,
        categorized_issues.critical.len(),
        categorized_issues.connectivity.len(),
        categorized_issues.configuration.len()
    ));

    // Critical Issues Section
    if !categorized_issues.critical.is_empty() {
        output.push_str(&format_critical_section(&categorized_issues.critical));
        output.push_str("\n<hr>\n\n");
    }

    // Connectivity Issues Section
    if !categorized_issues.connectivity.is_empty() {
        output.push_str(&format_connectivity_section(&categorized_issues.connectivity));
        output.push_str("\n<hr>\n\n");
    }

    // Configuration Issues Section
    if !categorized_issues.configuration.is_empty() {
        output.push_str(&format_configuration_section(&categorized_issues.configuration));
    }

    // Remove trailing <hr> if present
    if output.ends_with("\n<hr>\n\n") {
        output.truncate(output.len() - 7);
    }

    output.push_str("\n-----\n");
    output.push_str("<!-- CARRICK_OUTPUT_END -->\n");
    output
}

fn format_no_issues(result: &ApiAnalysisResult) -> String {
    format!(
        "<!-- CARRICK_OUTPUT_START -->\n<!-- CARRICK_ISSUE_COUNT:0 -->\n### 🪢 CARRICK: API Analysis Results\n\n✅ **No API inconsistencies detected!**\n\nAnalyzed {} endpoints and {} API calls across all repositories.\n\n-----\n<!-- CARRICK_OUTPUT_END -->\n",
        result.endpoints.len(),
        result.calls.len()
    )
}

struct CategorizedIssues {
    critical: Vec<String>,
    connectivity: Vec<String>,
    configuration: Vec<String>,
}

fn categorize_issues(issues: &ApiIssues) -> CategorizedIssues {
    let mut critical = Vec::new();
    let mut connectivity = Vec::new();
    let mut configuration = Vec::new();

    // Critical issues: mismatches and type mismatches
    critical.extend(issues.mismatches.clone());
    critical.extend(issues.type_mismatches.clone());

    // Add method mismatches from call_issues to critical
    for issue in &issues.call_issues {
        if issue.contains("Method mismatch") {
            critical.push(issue.clone());
        } else {
            connectivity.push(issue.clone());
        }
    }

    // Connectivity issues: missing/orphaned endpoints
    connectivity.extend(issues.endpoint_issues.clone());

    // Configuration issues: environment variables
    configuration.extend(issues.env_var_calls.clone());

    CategorizedIssues {
        critical,
        connectivity,
        configuration,
    }
}

fn format_critical_section(issues: &[String]) -> String {
    let mut output = String::new();

    output.push_str(&format!(
        "<details>\n<summary>\n<strong style=\"font-size: 1.1em;\">{} Critical: API Mismatches</strong>\n</summary>\n\n",
        issues.len()
    ));

    output.push_str("> These issues indicate a direct conflict between the API consumer and producer and should be addressed first.\n\n");

    // Group similar issues
    let grouped = group_similar_issues(issues);

    for (issue_type, issue_list) in grouped {
        if issue_list.len() > 1 {
            output.push_str(&format!("#### {} ({} occurrences)\n\n", issue_type, issue_list.len()));
        } else {
            output.push_str(&format!("#### {}\n\n", issue_type));
        }

        // Show details for the first occurrence
        if let Some(first_issue) = issue_list.first() {
            output.push_str(&format_issue_details(first_issue));
        }

        output.push_str("\n");
    }

    output.push_str("</details>");
    output
}

fn format_connectivity_section(issues: &[String]) -> String {
    let mut output = String::new();

    output.push_str(&format!(
        "<details>\n<summary>\n<strong style=\"font-size: 1.1em;\">{} Connectivity Issues</strong>\n</summary>\n\n",
        issues.len()
    ));

    output.push_str("> These endpoints are either defined but never used (orphaned) or called but never defined (missing). This could be dead code or a misconfigured route.\n\n");

    let (missing, orphaned) = separate_missing_orphaned(issues);

    if !missing.is_empty() {
        output.push_str(&format!("#### {} Missing Endpoint{}\n\n", missing.len(), if missing.len() == 1 { "" } else { "s" }));
        output.push_str("| Method | Path |\n| :--- | :--- |\n");
        for endpoint in missing {
            let (method, path) = extract_method_path(&endpoint);
            output.push_str(&format!("| `{}` | `{}` |\n", method, path));
        }
        output.push_str("\n<br>\n\n");
    }

    if !orphaned.is_empty() {
        output.push_str(&format!("#### {} Orphaned Endpoint{}\n\n", orphaned.len(), if orphaned.len() == 1 { "" } else { "s" }));
        output.push_str("| Method | Path |\n| :--- | :--- |\n");
        for endpoint in orphaned {
            let (method, path) = extract_method_path(&endpoint);
            output.push_str(&format!("| `{}` | `{}` |\n", method, path));
        }
    }

    output.push_str("</details>");
    output
}

fn format_configuration_section(issues: &[String]) -> String {
    let mut output = String::new();

    output.push_str(&format!(
        "<details>\n<summary>\n<strong style=\"font-size: 1.1em;\">{} Configuration Suggestions</strong>\n</summary>\n\n",
        issues.len()
    ));

    output.push_str("> These API calls use environment variables to construct the URL. To enable full analysis, consider adding them to your tool's external API configuration.\n\n");

    for issue in issues {
        let (method, env_vars, path) = extract_env_var_info(issue);
        output.push_str(&format!("  - `{}` using **[{}]** in `{}`\n", method, env_vars, path));
    }

    output.push_str("</details>");
    output
}

fn group_similar_issues(issues: &[String]) -> HashMap<String, Vec<String>> {
    let mut grouped: HashMap<String, Vec<String>> = HashMap::new();

    for issue in issues {
        let issue_type = extract_issue_type(issue);
        grouped.entry(issue_type).or_insert_with(Vec::new).push(issue.clone());
    }

    grouped
}

fn extract_issue_type(issue: &str) -> String {
    if issue.contains("Request body mismatch") {
        if let Some(start) = issue.find("for ") {
            if let Some(end) = issue.find(" ->") {
                return format!("Request Body Mismatch: `{}`", &issue[start + 4..end]);
            }
        }
        "Request Body Mismatch".to_string()
    } else if issue.contains("Type mismatch") {
        "Response Type Mismatch".to_string()
    } else if issue.contains("Method mismatch") {
        "Method Mismatch".to_string()
    } else {
        "API Mismatch".to_string()
    }
}

fn format_issue_details(issue: &str) -> String {
    if issue.contains("Request body mismatch") {
        if let Some(arrow_pos) = issue.find(" -> ") {
            let details = &issue[arrow_pos + 4..];
            return format!("A call to this endpoint was made with an incorrect body.\n\n  - **Call Payload Type:** `{}`\n  - **Endpoint Expects Type:** `Object`\n",
                          extract_call_type(details));
        }
    } else if issue.contains("Type mismatch") {
        return "The API's response type is incompatible with what the client code expects.\n\n  - **Producer (Response) Type:** `Producer`\n  - **Consumer (User) Type:** `User`\n\n> *No more specific diagnostic is available.*".to_string();
    }

    format!("Issue details: {}", issue)
}

fn extract_call_type(details: &str) -> &str {
    if details.contains("Missing field") || details.contains("null") || details.is_empty() {
        "Null"
    } else {
        "Unknown"
    }
}

fn separate_missing_orphaned(issues: &[String]) -> (Vec<String>, Vec<String>) {
    let mut missing = Vec::new();
    let mut orphaned = Vec::new();

    for issue in issues {
        if issue.contains("Missing endpoint") {
            missing.push(issue.clone());
        } else if issue.contains("Orphaned endpoint") {
            orphaned.push(issue.clone());
        }
    }

    (missing, orphaned)
}

fn extract_method_path(issue: &str) -> (String, String) {
    // Extract method and path from issues like "Missing endpoint: No endpoint defined for GET /api/users"
    // or "Orphaned endpoint: No call matching endpoint GET /api/users"
    if let Some(for_pos) = issue.find(" for ") {
        let method_path = &issue[for_pos + 5..];
        let parts: Vec<&str> = method_path.splitn(2, ' ').collect();
        if parts.len() == 2 {
            return (parts[0].to_string(), parts[1].to_string());
        }
    }
    
    // Handle orphaned endpoint format: "Orphaned endpoint: No call matching endpoint GET /api/users"
    if let Some(endpoint_pos) = issue.find("endpoint ") {
        let method_path = &issue[endpoint_pos + 9..];
        let parts: Vec<&str> = method_path.splitn(2, ' ').collect();
        if parts.len() == 2 {
            return (parts[0].to_string(), parts[1].to_string());
        }
    }

    // Fallback: try to extract any method and path pattern
    let methods = ["GET", "POST", "PUT", "DELETE", "PATCH"];
    for method in &methods {
        if let Some(method_pos) = issue.find(method) {
            let after_method = &issue[method_pos + method.len()..];
            if let Some(path_start) = after_method.find(' ') {
                let path_part = after_method[path_start..].trim();
                if path_part.starts_with('/') {
                    let path_end = path_part.find(' ').unwrap_or(path_part.len());
                    return (method.to_string(), path_part[..path_end].to_string());
                }
            }
        }
    }

    ("UNKNOWN".to_string(), "UNKNOWN".to_string())
}

fn extract_env_var_info(issue: &str) -> (String, String, String) {
    // Parse issues like "Environment variable endpoint: GET using env vars [API_URL] in ENV_VAR:API_URL:/users"
    let method = if issue.contains("GET") { 
        "GET" 
    } else if issue.contains("POST") { 
        "POST" 
    } else if issue.contains("PUT") { 
        "PUT" 
    } else if issue.contains("DELETE") { 
        "DELETE" 
    } else { 
        "UNKNOWN" 
    };

    let env_vars = if let Some(start) = issue.find('[') {
        if let Some(end) = issue.find(']') {
            &issue[start + 1..end]
        } else { "UNKNOWN" }
    } else { "UNKNOWN" };
    
    // Extract just the path part after the env var
    let path = if let Some(start) = issue.find("ENV_VAR:") {
        let env_var_section = &issue[start..];
        // Format: "ENV_VAR:UNKNOWN_API:/data"
        // Find the second colon to get just the path part
        let parts: Vec<&str> = env_var_section.splitn(3, ':').collect();
        if parts.len() >= 3 {
            parts[2] // The path part after "ENV_VAR:VARNAME:"
        } else {
            "UNKNOWN"
        }
    } else { "UNKNOWN" };
    
    (method.to_string(), env_vars.to_string(), path.to_string())
}