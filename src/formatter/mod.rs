use crate::analyzer::{
    ApiAnalysisResult, ApiIssues, ConflictSeverity, DependencyConflict, OrphanedEndpoint,
};
use std::collections::{BTreeMap, BTreeSet};

/// Shape of the project being analyzed, threaded from the engine so the PR
/// comment frames findings for the right setup: a lone repo, a monorepo
/// (multiple services declared in one carrick.json), or a poly-repo project
/// (peer repos indexed alongside this one).
#[derive(Debug, Clone)]
pub struct Topology {
    /// This repo's name, used to title single-repo comments.
    pub repo_name: String,
    /// Services declared for THIS repo. More than one means a monorepo.
    pub local_service_count: usize,
    /// Other repos indexed for the project (peers).
    pub peer_repo_count: usize,
}

impl Topology {
    fn is_monorepo(&self) -> bool {
        self.local_service_count > 1
    }

    fn has_peers(&self) -> bool {
        self.peer_repo_count > 0
    }

    /// Cross-service matching is only conclusive when there is more than one
    /// service to match across: peer repos, or multiple local services in a
    /// monorepo. A lone single-service repo has no baseline, so its
    /// connectivity findings are framed as informational rather than headline
    /// issues (an endpoint looks "orphaned" only because nothing else is
    /// indexed yet).
    fn has_baseline(&self) -> bool {
        self.has_peers() || self.is_monorepo()
    }

    /// Subtitle appended to the comment header to name the topology.
    fn header_suffix(&self) -> String {
        if self.is_monorepo() {
            format!(" · monorepo ({} services)", self.local_service_count)
        } else if !self.has_peers() {
            format!(" · {}", self.repo_name)
        } else {
            String::new()
        }
    }

    /// Scope noun phrase rendered after "across" in the verdict, so it must
    /// not itself contain "across". A monorepo's service count is already shown
    /// in the header suffix, so once peers exist the scope reports repos.
    fn scope_phrase(&self) -> String {
        if self.has_peers() {
            format!("{} repos", self.peer_repo_count + 1)
        } else if self.is_monorepo() {
            format!("{} services", self.local_service_count)
        } else {
            self.repo_name.clone()
        }
    }
}

/// An endpoint added by the current PR (present now, absent from the repo's
/// previously-indexed state).
#[derive(Debug, Clone)]
pub struct NewEndpoint {
    pub method: String,
    pub path: String,
    pub service: Option<String>,
}

/// What changed in this PR relative to the repo's last-indexed (main) state.
/// `None` outside a PR run or when there is no prior index to diff against.
#[derive(Debug, Clone)]
pub struct PrDelta {
    pub new_endpoints: Vec<NewEndpoint>,
}

impl PrDelta {
    fn is_empty(&self) -> bool {
        self.new_endpoints.is_empty()
    }
}

pub struct FormattedOutput {
    pub content: String,
}

impl FormattedOutput {
    pub fn new(result: ApiAnalysisResult, topology: Topology, pr_delta: Option<PrDelta>) -> Self {
        let content = format_analysis_results(result, &topology, pr_delta.as_ref());
        Self { content }
    }

    pub fn print(&self) {
        println!("{}", self.content);
    }

    /// The comment body to relay to the cloud: the rendered markdown without
    /// the machine-only marker lines (`CARRICK_OUTPUT_START`/`_END` and
    /// `CARRICK_ISSUE_COUNT`). The cloud adds its own idempotency marker, so
    /// these would only be noise in the posted comment. Mirrors the `grep -v`
    /// the GitHub Action previously applied before posting.
    pub fn pr_comment_body(&self) -> String {
        self.content
            .lines()
            .filter(|line| {
                !line.contains("CARRICK_OUTPUT_START")
                    && !line.contains("CARRICK_OUTPUT_END")
                    && !line.contains("CARRICK_ISSUE_COUNT")
            })
            .collect::<Vec<_>>()
            .join("\n")
            .trim()
            .to_string()
    }
}

pub fn format_analysis_results(
    result: ApiAnalysisResult,
    topology: &Topology,
    pr_delta: Option<&PrDelta>,
) -> String {
    if result.issues.is_empty() {
        return format_no_issues(&result, topology, pr_delta);
    }

    let categorized_issues = categorize_issues(&result.issues);
    let has_baseline = topology.has_baseline();
    // Without a baseline (a lone single-service repo) connectivity findings are
    // inconclusive, so they stay out of the headline count and the Action's
    // CARRICK_ISSUE_COUNT — they are still listed below, framed as
    // informational. Configuration suggestions are advisory and never gate CI,
    // so they are excluded from the count as well.
    let connectivity_in_headline = if has_baseline {
        categorized_issues.connectivity_len()
    } else {
        0
    };
    let total_issues = categorized_issues.critical.len()
        + connectivity_in_headline
        + categorized_issues.dependencies.len();

    let mut output = String::new();

    // Machine-readable markers consumed by the GitHub Action (stripped before
    // the comment is posted). The issue count must stay parseable.
    output.push_str("<!-- CARRICK_OUTPUT_START -->\n");
    output.push_str(&format!("<!-- CARRICK_ISSUE_COUNT:{} -->\n", total_issues));

    output.push_str(&format!("## 🪢 Carrick{}\n\n", topology.header_suffix()));

    // Verdict callout. GitHub alert blocks carry severity colour natively, so
    // the comment conveys state without leaning on emoji.
    output.push_str(&format_verdict(
        &categorized_issues,
        total_issues,
        has_baseline,
        topology,
    ));
    output.push_str("\n\n");

    output.push_str(&format_pr_delta(pr_delta));

    output.push_str(&format!(
        "Indexed **{} endpoints** and **{} cross-service calls**.\n\n",
        result.endpoints.len(),
        result.calls.len(),
    ));

    let banner = format_graphql_banner(
        &result.detected_graphql_libraries,
        result.graphql_operations_indexed,
    );
    output.push_str(&banner);

    // Sections, ordered by actionability. Verified runs last as a collapsed
    // positive signal.
    if !categorized_issues.critical.is_empty() {
        output.push_str(&format_critical_section(&categorized_issues.critical));
        output.push_str("\n\n");
    }
    if !categorized_issues.connectivity_is_empty() {
        output.push_str(&format_connectivity_section(
            &categorized_issues.missing,
            &categorized_issues.orphaned,
            has_baseline,
        ));
        output.push_str("\n\n");
    }
    if !categorized_issues.dependencies.is_empty() {
        output.push_str(&format_dependency_section(&categorized_issues.dependencies));
        output.push_str("\n\n");
    }
    if !categorized_issues.configuration.is_empty() {
        output.push_str(&format_configuration_section(
            &categorized_issues.configuration,
        ));
        output.push_str("\n\n");
    }
    if !result.verified_endpoints.is_empty() {
        output.push_str(&format_verified_section(&result.verified_endpoints));
        output.push_str("\n\n");
    }

    output.push_str(&dashboard_footer());
    output.push_str("\n<!-- CARRICK_OUTPUT_END -->\n");
    output
}

/// The "In this PR" block: endpoints this change added relative to the repo's
/// last-indexed state. Empty (renders nothing) outside a PR run, when there is
/// no prior index to diff against, or when nothing new was added.
fn format_pr_delta(pr_delta: Option<&PrDelta>) -> String {
    let Some(delta) = pr_delta else {
        return String::new();
    };
    if delta.is_empty() {
        return String::new();
    }
    let mut output = String::from("**In this PR**\n\n");
    for ep in &delta.new_endpoints {
        let suffix = ep
            .service
            .as_deref()
            .map(|s| format!(" ({})", s))
            .unwrap_or_default();
        output.push_str(&format!(
            "- New endpoint `{} {}`{}\n",
            code_cell(&ep.method),
            code_cell(&ep.path),
            suffix
        ));
    }
    output.push('\n');
    output
}

/// Build the GitHub alert block that opens the comment. The alert kind sets
/// the colour (CAUTION red, WARNING amber, NOTE blue), so severity reads at a
/// glance without emoji.
fn format_verdict(
    categorized: &CategorizedIssues,
    total_issues: usize,
    has_baseline: bool,
    topology: &Topology,
) -> String {
    let kind = if !categorized.critical.is_empty() {
        "CAUTION"
    } else if total_issues > 0 {
        "WARNING"
    } else {
        "NOTE"
    };

    let mut parts: Vec<String> = Vec::new();
    if !categorized.critical.is_empty() {
        parts.push(format!(
            "**{} contract risk{}**",
            categorized.critical.len(),
            plural(categorized.critical.len())
        ));
    }
    if !categorized.connectivity_is_empty() {
        let noun = if has_baseline {
            "connectivity gap"
        } else {
            "connectivity observation"
        };
        parts.push(format!(
            "{} {}{}",
            categorized.connectivity_len(),
            noun,
            plural(categorized.connectivity_len())
        ));
    }
    if !categorized.dependencies.is_empty() {
        parts.push(format!(
            "{} dependency conflict{}",
            categorized.dependencies.len(),
            plural(categorized.dependencies.len())
        ));
    }
    if !categorized.configuration.is_empty() {
        parts.push(format!(
            "{} configuration suggestion{}",
            categorized.configuration.len(),
            plural(categorized.configuration.len())
        ));
    }

    let summary = if parts.is_empty() {
        "Nothing to flag".to_string()
    } else {
        join_human(&parts)
    };

    // Only explain the informational framing when there are connectivity
    // findings to frame; otherwise the note would reference connectivity that
    // isn't present (e.g. a lone repo with only configuration suggestions).
    let baseline_note = if !has_baseline && !categorized.connectivity_is_empty() {
        " First repo indexed, so connectivity findings are informational.".to_string()
    } else {
        String::new()
    };

    format!(
        "> [!{}]\n> {} across {}.{}",
        kind,
        summary,
        topology.scope_phrase(),
        baseline_note
    )
}

/// Closing line pointing at the dashboard. Deep links are injected cloud-side
/// (the scanner does not know the workspace/project slug), so this stays as
/// plain prose for now.
fn dashboard_footer() -> String {
    "Full analysis is in the Carrick dashboard.\n".to_string()
}

fn plural(n: usize) -> &'static str {
    if n == 1 { "" } else { "s" }
}

/// Join phrases as "a", "a and b", or "a, b and c".
fn join_human(parts: &[String]) -> String {
    match parts.len() {
        0 => String::new(),
        1 => parts[0].clone(),
        2 => format!("{} and {}", parts[0], parts[1]),
        _ => {
            let (last, head) = parts.split_last().unwrap();
            format!("{} and {}", head.join(", "), last)
        }
    }
}

fn format_no_issues(
    result: &ApiAnalysisResult,
    topology: &Topology,
    pr_delta: Option<&PrDelta>,
) -> String {
    let mut output = String::new();
    output.push_str("<!-- CARRICK_OUTPUT_START -->\n<!-- CARRICK_ISSUE_COUNT:0 -->\n");
    output.push_str(&format!("## 🪢 Carrick{}\n\n", topology.header_suffix()));
    output.push_str(&format!(
        "> [!TIP]\n> All cross-service calls match the indexed contracts across {}.\n\n",
        topology.scope_phrase()
    ));
    output.push_str(&format_pr_delta(pr_delta));
    output.push_str(&format!(
        "Indexed **{} endpoints** and **{} cross-service calls**.\n\n",
        result.endpoints.len(),
        result.calls.len(),
    ));
    output.push_str(&format_graphql_banner(
        &result.detected_graphql_libraries,
        result.graphql_operations_indexed,
    ));
    if !result.verified_endpoints.is_empty() {
        output.push_str(&format_verified_section(&result.verified_endpoints));
        output.push_str("\n\n");
    }
    output.push_str(&dashboard_footer());
    output.push_str("<!-- CARRICK_OUTPUT_END -->\n");
    output
}

/// Render a "Verified Endpoints" details block listing every endpoint that
/// at least one consumer call successfully matched. Surfaced so PR comments
/// communicate what's *working*, not just what's broken — without it, a
/// clean cross-repo run produces no positive signal at all.
fn format_verified_section(verified: &[(String, String)]) -> String {
    let mut output = String::new();
    output.push_str(&format!(
        "<details>\n<summary><strong>Verified ({})</strong></summary>\n\n",
        verified.len()
    ));
    output.push_str(
        "> Endpoints with at least one matching consumer call. Types were resolved and compared by the TypeScript compiler pass.\n\n",
    );
    output.push_str("| Method | Path |\n| :--- | :--- |\n");
    for (method, path) in verified {
        output.push_str(&format!("| `{}` | `{}` |\n", method, path));
    }
    output.push_str("\n</details>");
    output
}

/// Render a banner when GraphQL libraries are detected but no operations
/// could be extracted. GraphQL extraction is parse-based (SDL files,
/// `gql` template literals); code-first schemas and Relay compiled
/// artifacts produce nothing statically, so the banner suggests committing
/// an emitted schema instead of staying silent about the coverage gap.
fn format_graphql_banner(graphql_libraries: &[String], operations_indexed: bool) -> String {
    if graphql_libraries.is_empty() || operations_indexed {
        return String::new();
    }
    let mut libs: Vec<&String> = graphql_libraries.iter().collect();
    libs.sort();
    libs.dedup();
    let lib_list = libs
        .iter()
        .map(|s| format!("`{}`", s))
        .collect::<Vec<_>>()
        .join(", ");
    format!(
        "> [!NOTE]\n> **GraphQL detected** ({}), but no schema or operation documents were found. Carrick extracts GraphQL contracts from SDL (`.graphql`/`.gql` files, `gql` template literals). If your schema is code-first (Pothos, TypeGraphQL, Nexus), commit the emitted `schema.graphql` to index it. Relay compiled artifacts and persisted queries are out of scope.\n\n",
        lib_list,
    )
}

#[derive(Debug, Clone)]
struct EnvVarSuggestionGroup {
    method: String,
    env_var: String,
    path: String,
    count: usize,
    locations: Vec<String>,
}

struct CategorizedIssues {
    critical: Vec<String>,
    /// Consumer calls with no producer (still string-based; consumer-repo
    /// attribution is a follow-up).
    missing: Vec<String>,
    /// Producers with no consumer, carrying their owning service/repo.
    orphaned: Vec<OrphanedEndpoint>,
    configuration: Vec<EnvVarSuggestionGroup>,
    dependencies: Vec<DependencyConflict>,
}

impl CategorizedIssues {
    fn connectivity_len(&self) -> usize {
        self.missing.len() + self.orphaned.len()
    }

    fn connectivity_is_empty(&self) -> bool {
        self.missing.is_empty() && self.orphaned.is_empty()
    }
}

fn categorize_issues(issues: &ApiIssues) -> CategorizedIssues {
    let mut critical = Vec::new();
    let mut missing = Vec::new();

    critical.extend(issues.mismatches.clone());
    critical.extend(issues.type_mismatches.clone());

    for issue in &issues.call_issues {
        if issue.contains("Method mismatch") {
            critical.push(issue.clone());
        } else {
            missing.push(issue.clone());
        }
    }

    CategorizedIssues {
        critical,
        missing,
        orphaned: issues.endpoint_issues.clone(),
        configuration: group_env_var_suggestions(&issues.env_var_calls),
        dependencies: issues.dependency_conflicts.clone(),
    }
}

fn format_critical_section(issues: &[String]) -> String {
    let mut output = String::new();
    output.push_str(&format!(
        "<details>\n<summary><strong>Contract risks ({})</strong></summary>\n\n",
        issues.len()
    ));
    output.push_str(
        "> A consumer call conflicts with the producer it targets in the index. These break the consumer at runtime.\n\n",
    );
    output.push_str("| Endpoint | Issue |\n| :--- | :--- |\n");
    for issue in issues {
        let (endpoint, detail) = summarize_critical(issue);
        output.push_str(&format!("| `{}` | {} |\n", cell(&endpoint), cell(&detail)));
    }
    output.push_str("\n</details>");
    output
}

/// Reduce a critical-issue string to `(endpoint, one-line detail)` for a table
/// row, reusing the structured/generic TypeScript error parsers.
fn summarize_critical(issue: &str) -> (String, String) {
    if issue.contains("Type mismatch on ") {
        let (endpoint, producer, consumer, error) = parse_structured_type_error(issue);
        (
            endpoint,
            format!(
                "producer `{}` vs consumer `{}`: {}",
                producer, consumer, error
            ),
        )
    } else if issue.contains(": Type '") {
        parse_generic_typescript_error(issue)
    } else if issue.contains("Method mismatch") {
        let (method, path) = extract_method_path(issue);
        (
            format!("{} {}", method, path),
            "HTTP method mismatch".to_string(),
        )
    } else {
        ("-".to_string(), issue.to_string())
    }
}

/// Escape a value for a Markdown table cell: no pipes, and no line breaks
/// (CRLF, lone CR, or LF) that would otherwise split the row.
fn cell(value: &str) -> String {
    value
        .replace('|', "\\|")
        .replace("\r\n", " ")
        .replace(['\r', '\n'], " ")
        .trim()
        .to_string()
}

/// Sanitize a value that will be wrapped in inline-code backticks inside a
/// table cell. Beyond `cell()`'s pipe/newline escaping, drop backticks: an
/// HTTP method, route, or service name should never contain one, and a stray
/// backtick (e.g. from a hand-written carrick.json `serviceName`) would break
/// out of the code span and could inject Markdown.
fn code_cell(value: &str) -> String {
    cell(value).replace('`', "")
}

fn format_connectivity_section(
    missing: &[String],
    orphaned: &[OrphanedEndpoint],
    has_baseline: bool,
) -> String {
    let mut output = String::new();

    let heading = if has_baseline {
        "Connectivity"
    } else {
        "Connectivity Observations"
    };
    output.push_str(&format!(
        "<details>\n<summary><strong>{} ({})</strong></summary>\n\n",
        heading,
        missing.len() + orphaned.len()
    ));

    output.push_str("> Orphaned endpoints have no consumer in the indexed services. Missing endpoints have a consumer call but no producer.\n\n");

    if !has_baseline {
        output.push_str("> **First repo indexed for this project.** With nothing else to match against, every endpoint without a same-repo consumer is listed below; most resolve once you connect the repos that call them.\n\n");
    }

    if !missing.is_empty() {
        output.push_str(&format!("**Missing ({})**\n\n", missing.len()));
        output.push_str("| Method | Path |\n| :--- | :--- |\n");
        for issue in missing {
            let (method, path) = extract_method_path(issue);
            output.push_str(&format!(
                "| `{}` | `{}` |\n",
                code_cell(&method),
                code_cell(&path)
            ));
        }
        output.push('\n');
    }

    if !orphaned.is_empty() {
        output.push_str(&format!("**Orphaned ({})**\n\n", orphaned.len()));
        // Show the owning-service column only when at least one orphan is
        // attributed (single-repo runs have none, so the column is dropped).
        if orphaned.iter().any(|o| o.service.is_some()) {
            output.push_str("| Method | Path | Service |\n| :--- | :--- | :--- |\n");
            for o in orphaned {
                // A row can be unattributed even when the column is shown (e.g.
                // a GraphQL orphan alongside an attributed HTTP one); use a dash
                // rather than an empty cell.
                let service = o
                    .service
                    .as_deref()
                    .map(|s| format!("`{}`", code_cell(s)))
                    .unwrap_or_else(|| "-".to_string());
                output.push_str(&format!(
                    "| `{}` | `{}` | {} |\n",
                    code_cell(&o.method),
                    code_cell(&o.path),
                    service
                ));
            }
        } else {
            output.push_str("| Method | Path |\n| :--- | :--- |\n");
            for o in orphaned {
                output.push_str(&format!(
                    "| `{}` | `{}` |\n",
                    code_cell(&o.method),
                    code_cell(&o.path)
                ));
            }
        }
    }

    output.push_str("\n</details>");
    output
}

fn format_configuration_section(issues: &[EnvVarSuggestionGroup]) -> String {
    let mut output = String::new();

    output.push_str(&format!(
        "<details>\n<summary><strong>Configuration suggestions ({})</strong></summary>\n\n",
        issues.len()
    ));

    output.push_str("> These calls use environment variables to construct the URL. Add them to `internalEnvVars` (to validate routes) or `externalEnvVars` (to ignore) in your `carrick.json`.\n\n");

    for issue in issues {
        output.push_str(&format!(
            "  - `{} {}` using **[{}]** ({} call site{})\n",
            issue.method,
            issue.path,
            issue.env_var,
            issue.count,
            if issue.count == 1 { "" } else { "s" }
        ));

        let shown = issue.locations.len().min(3);
        for loc in issue.locations.iter().take(shown) {
            output.push_str(&format!("    - `{}`\n", loc));
        }
        if shown > 0 && issue.count > shown {
            output.push_str(&format!("    - … +{} more\n", issue.count - shown));
        }
    }

    output.push_str("\n</details>");
    output
}

fn format_dependency_section(conflicts: &[DependencyConflict]) -> String {
    let mut output = String::new();

    // Group conflicts by severity
    let mut critical = Vec::new();
    let mut warning = Vec::new();
    let mut info = Vec::new();

    for conflict in conflicts {
        match conflict.severity {
            ConflictSeverity::Critical => critical.push(conflict),
            ConflictSeverity::Warning => warning.push(conflict),
            ConflictSeverity::Info => info.push(conflict),
        }
    }

    output.push_str(&format!(
        "<details>\n<summary><strong>Dependency conflicts ({})</strong></summary>\n\n",
        conflicts.len()
    ));

    output.push_str("> Packages with different versions across services in the index.\n\n");

    // Critical conflicts (major version differences)
    if !critical.is_empty() {
        output.push_str(&format!(
            "### Critical Conflicts ({}) - Major Version Differences\n\n",
            critical.len()
        ));
        for conflict in &critical {
            output.push_str(&format!("#### {}\n\n", conflict.package_name));
            output.push_str("| Repository | Version | Source |\n| :--- | :--- | :--- |\n");

            for repo_info in &conflict.repos {
                output.push_str(&format!(
                    "| `{}` | `{}` | `{}` |\n",
                    repo_info.repo_name,
                    repo_info.version,
                    repo_info.source_path.display()
                ));
            }
            output.push('\n');
        }
    }

    // Warning conflicts (minor version differences)
    if !warning.is_empty() {
        output.push_str(&format!(
            "### Warning Conflicts ({}) - Minor Version Differences\n\n",
            warning.len()
        ));
        for conflict in &warning {
            output.push_str(&format!("#### {}\n\n", conflict.package_name));
            output.push_str("| Repository | Version | Source |\n| :--- | :--- | :--- |\n");

            for repo_info in &conflict.repos {
                output.push_str(&format!(
                    "| `{}` | `{}` | `{}` |\n",
                    repo_info.repo_name,
                    repo_info.version,
                    repo_info.source_path.display()
                ));
            }
            output.push('\n');
        }
    }

    // Info conflicts (patch version differences)
    if !info.is_empty() {
        output.push_str(&format!(
            "### Info Conflicts ({}) - Patch Version Differences\n\n",
            info.len()
        ));
        for conflict in &info {
            output.push_str(&format!("#### {}\n\n", conflict.package_name));
            output.push_str("| Repository | Version | Source |\n| :--- | :--- | :--- |\n");

            for repo_info in &conflict.repos {
                output.push_str(&format!(
                    "| `{}` | `{}` | `{}` |\n",
                    repo_info.repo_name,
                    repo_info.version,
                    repo_info.source_path.display()
                ));
            }
            output.push('\n');
        }
    }

    output.push_str("</details>");
    output
}

fn parse_generic_typescript_error(issue: &str) -> (String, String) {
    // Parse any TypeScript error like: "GET /users/:param: Type '...' error message"

    let endpoint = {
        let methods = ["GET ", "POST ", "PUT ", "DELETE ", "PATCH "];
        let mut found_endpoint = "Unknown".to_string();

        for method in &methods {
            if let Some(start) = issue.find(method)
                && let Some(end) = issue.find(": Type '")
            {
                found_endpoint = issue[start..end].to_string();
                break;
            }
        }
        found_endpoint
    };

    let error_message = if let Some(start) = issue.find(": ") {
        let remaining = &issue[start + 2..];
        if remaining.len() > 200 {
            format!("{}...", &remaining[..200])
        } else {
            remaining.to_string()
        }
    } else {
        issue.to_string()
    };

    (endpoint, error_message)
}

fn parse_structured_type_error(issue: &str) -> (String, String, String, String) {
    // Parse errors like: "Type mismatch on GET /users/:param: Producer (SomeType) incompatible with Consumer (AnotherType) - Error details"

    let endpoint = if let Some(start) = issue.find("Type mismatch on ") {
        if let Some(end) = issue.find(": Producer") {
            issue[start + 17..end].to_string()
        } else {
            "Unknown".to_string()
        }
    } else {
        "Unknown".to_string()
    };

    let producer = if let Some(start) = issue.find("Producer (") {
        if let Some(end) = issue.find(") incompatible") {
            issue[start + 10..end].to_string()
        } else {
            "Unknown".to_string()
        }
    } else {
        "Unknown".to_string()
    };

    let consumer = if let Some(start) = issue.find("Consumer (") {
        if let Some(end) = issue.find(") - ") {
            issue[start + 10..end].to_string()
        } else {
            "Unknown".to_string()
        }
    } else {
        "Unknown".to_string()
    };

    let error = if let Some(start) = issue.find(") - ") {
        let remaining = &issue[start + 4..];
        if remaining.len() > 150 {
            format!("{}...", &remaining[..150])
        } else {
            remaining.to_string()
        }
    } else {
        "Type compatibility issue".to_string()
    };

    (endpoint, producer, consumer, error)
}

fn extract_method_path(issue: &str) -> (String, String) {
    // Handle "Orphaned endpoint: METHOD PATH in FILE"
    if let Some(rest) = issue.strip_prefix("Orphaned endpoint: ") {
        let parts: Vec<&str> = rest.splitn(3, ' ').collect();
        if parts.len() >= 2 {
            let method = parts[0];
            let path = parts[1];
            return (method.to_string(), path.to_string());
        }
    }

    // Handle "Missing endpoint for METHOD PATH (normalized: ...)"
    if let Some(for_pos) = issue.find(" for ") {
        let method_path = &issue[for_pos + 5..];
        let parts: Vec<&str> = method_path.splitn(3, ' ').collect();
        if parts.len() >= 2 {
            let path = parts[1].split(" (").next().unwrap_or(parts[1]);
            return (parts[0].to_string(), path.to_string());
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
    // Parse issues in two formats:
    // New: "Unclassified env var: GET /orders using [ORDER_SERVICE_URL] - add to internalEnvVars..."
    // Old: "Environment variable endpoint: GET using env vars [API_URL] in ENV_VAR:API_URL:/users"

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
        } else {
            "UNKNOWN"
        }
    } else {
        "UNKNOWN"
    };

    // Try new format first: "Unclassified env var: GET /path using [ENV_VAR]"
    // Path is between the method and " using"
    let path = if issue.starts_with("Unclassified env var:") {
        // Format: "Unclassified env var: GET /orders using [ORDER_SERVICE_URL] - ..."
        if let Some(using_pos) = issue.find(" using") {
            // Find the method position and extract path after it
            let after_method = if let Some(pos) = issue.find("GET ") {
                &issue[pos + 4..using_pos]
            } else if let Some(pos) = issue.find("POST ") {
                &issue[pos + 5..using_pos]
            } else if let Some(pos) = issue.find("PUT ") {
                &issue[pos + 4..using_pos]
            } else if let Some(pos) = issue.find("DELETE ") {
                &issue[pos + 7..using_pos]
            } else {
                "UNKNOWN"
            };
            after_method.trim()
        } else {
            "UNKNOWN"
        }
    } else if let Some(start) = issue.find("ENV_VAR:") {
        // Old format: "ENV_VAR:UNKNOWN_API:/data"
        let env_var_section = &issue[start..];
        let parts: Vec<&str> = env_var_section.splitn(3, ':').collect();
        if parts.len() >= 3 {
            parts[2]
        } else {
            "UNKNOWN"
        }
    } else {
        "UNKNOWN"
    };

    (method.to_string(), env_vars.to_string(), path.to_string())
}

fn extract_env_var_location(issue: &str) -> Option<String> {
    let marker = "(from ";
    let start = issue.find(marker)?;
    let rest = &issue[start + marker.len()..];
    let end = rest.find(')')?;
    Some(rest[..end].to_string())
}

fn group_env_var_suggestions(issues: &[String]) -> Vec<EnvVarSuggestionGroup> {
    #[derive(Default)]
    struct Acc {
        raw_count: usize,
        locations: BTreeSet<String>,
    }

    let mut grouped: BTreeMap<(String, String, String), Acc> = BTreeMap::new();

    for issue in issues {
        let (method, env_var, path) = extract_env_var_info(issue);
        let location = extract_env_var_location(issue);

        let acc = grouped.entry((env_var, method, path)).or_default();
        acc.raw_count += 1;
        if let Some(loc) = location {
            acc.locations.insert(loc);
        }
    }

    grouped
        .into_iter()
        .map(|((env_var, method, path), acc)| {
            let locations: Vec<String> = acc.locations.into_iter().collect();
            let count = if locations.is_empty() {
                acc.raw_count
            } else {
                locations.len()
            };

            EnvVarSuggestionGroup {
                method,
                env_var,
                path,
                count,
                locations,
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::analyzer::{ApiAnalysisResult, ApiIssues};

    /// A poly-repo topology with peers, so connectivity findings are
    /// conclusive (has_baseline == true).
    fn topology_baseline() -> Topology {
        Topology {
            repo_name: "api-server".to_string(),
            local_service_count: 1,
            peer_repo_count: 2,
        }
    }

    /// A lone single-service repo with no peers (has_baseline == false): the
    /// first-repo-indexed framing.
    fn topology_first_repo() -> Topology {
        Topology {
            repo_name: "api-server".to_string(),
            local_service_count: 1,
            peer_repo_count: 0,
        }
    }

    #[test]
    fn test_typescript_error_formatting() {
        let type_mismatches = vec![
            "GET /users/:param/comments: Type '{ userId: number; comments: Comment[]; }' is missing the following properties from type 'Comment[]': length, pop, push, concat, and 29 more.".to_string(),
            "GET /users/:param: Type '{ commentsByUser: Comment[]; }' is missing the following properties from type 'User': id, name, role".to_string(),
        ];

        let issues = ApiIssues {
            call_issues: vec![],
            endpoint_issues: vec![],
            env_var_calls: vec![],
            mismatches: vec![],
            type_mismatches,
            dependency_conflicts: vec![],
        };

        let result = ApiAnalysisResult {
            endpoints: vec![],
            calls: vec![],
            issues,
            verified_endpoints: vec![],
            detected_graphql_libraries: vec![],
            graphql_operations_indexed: false,
        };

        let output = format_analysis_results(result, &topology_baseline(), None);

        // The endpoints and the raw compiler error are surfaced as table rows.
        assert!(output.contains("GET /users/:param/comments"));
        assert!(output.contains("GET /users/:param"));
        assert!(output.contains("Type '{ userId: number; comments: Comment[]; }'"));
        assert!(output.contains("Type '{ commentsByUser: Comment[]; }'"));
    }

    #[test]
    fn test_structured_type_error_formatting() {
        let type_mismatches = vec![
            "Type mismatch on GET /api/users: Producer (UserResponse) incompatible with Consumer (User[]) - Property 'role' is missing".to_string(),
        ];

        let issues = ApiIssues {
            call_issues: vec![],
            endpoint_issues: vec![],
            env_var_calls: vec![],
            mismatches: vec![],
            type_mismatches,
            dependency_conflicts: vec![],
        };

        let result = ApiAnalysisResult {
            endpoints: vec![],
            calls: vec![],
            issues,
            verified_endpoints: vec![],
            detected_graphql_libraries: vec![],
            graphql_operations_indexed: false,
        };

        let output = format_analysis_results(result, &topology_baseline(), None);

        // The endpoint, both type names, and the error are surfaced.
        assert!(output.contains("GET /api/users"));
        assert!(output.contains("UserResponse"));
        assert!(output.contains("User[]"));
        assert!(output.contains("Property 'role' is missing"));
    }

    #[test]
    fn test_graphql_banner_renders_when_libraries_detected() {
        let issues = ApiIssues {
            call_issues: vec![],
            endpoint_issues: vec![],
            env_var_calls: vec![],
            mismatches: vec![],
            type_mismatches: vec![],
            dependency_conflicts: vec![],
        };
        let result = ApiAnalysisResult {
            endpoints: vec![],
            calls: vec![],
            issues,
            verified_endpoints: vec![],
            detected_graphql_libraries: vec![
                "graphql-request".to_string(),
                "@apollo/client".to_string(),
            ],
            graphql_operations_indexed: false,
        };
        let output = format_analysis_results(result, &topology_baseline(), None);
        assert!(output.contains("GraphQL detected"));
        assert!(output.contains("graphql-request"));
        assert!(output.contains("@apollo/client"));
        assert!(output.contains("no schema or operation documents were found"));
    }

    #[test]
    fn test_graphql_banner_absent_when_operations_indexed() {
        let issues = ApiIssues {
            call_issues: vec![],
            endpoint_issues: vec![],
            env_var_calls: vec![],
            mismatches: vec![],
            type_mismatches: vec![],
            dependency_conflicts: vec![],
        };
        let result = ApiAnalysisResult {
            endpoints: vec![],
            calls: vec![],
            issues,
            verified_endpoints: vec![],
            detected_graphql_libraries: vec!["@apollo/client".to_string()],
            graphql_operations_indexed: true,
        };
        let output = format_analysis_results(result, &topology_baseline(), None);
        assert!(!output.contains("GraphQL detected"));
    }

    #[test]
    fn test_graphql_banner_absent_when_no_libraries() {
        let issues = ApiIssues {
            call_issues: vec![],
            endpoint_issues: vec![],
            env_var_calls: vec![],
            mismatches: vec![],
            type_mismatches: vec![],
            dependency_conflicts: vec![],
        };
        let result = ApiAnalysisResult {
            endpoints: vec![],
            calls: vec![],
            issues,
            verified_endpoints: vec![],
            detected_graphql_libraries: vec![],
            graphql_operations_indexed: false,
        };
        let output = format_analysis_results(result, &topology_baseline(), None);
        assert!(!output.contains("GraphQL detected"));
    }

    #[test]
    fn test_no_issues_output() {
        let issues = ApiIssues {
            call_issues: vec![],
            endpoint_issues: vec![],
            env_var_calls: vec![],
            mismatches: vec![],
            type_mismatches: vec![],
            dependency_conflicts: vec![],
        };

        let result = ApiAnalysisResult {
            endpoints: vec![],
            calls: vec![],
            issues,
            verified_endpoints: vec![],
            detected_graphql_libraries: vec![],
            graphql_operations_indexed: false,
        };

        let output = format_analysis_results(result, &topology_baseline(), None);

        // Check that no issues message is displayed
        assert!(output.contains("All cross-service calls match the indexed contracts"));
        assert!(output.contains("CARRICK_ISSUE_COUNT:0"));
    }

    #[test]
    fn test_pr_comment_body_strips_machine_markers() {
        let issues = ApiIssues {
            call_issues: vec![],
            endpoint_issues: vec![],
            env_var_calls: vec![],
            mismatches: vec![],
            type_mismatches: vec![],
            dependency_conflicts: vec![],
        };
        let result = ApiAnalysisResult {
            endpoints: vec![],
            calls: vec![],
            issues,
            verified_endpoints: vec![],
            detected_graphql_libraries: vec![],
            graphql_operations_indexed: false,
        };

        let formatted = FormattedOutput::new(result, topology_baseline(), None);
        let body = formatted.pr_comment_body();

        // The marker lines the old Action stripped before posting must not
        // leak into the comment the cloud relays.
        assert!(!body.contains("CARRICK_OUTPUT_START"));
        assert!(!body.contains("CARRICK_OUTPUT_END"));
        assert!(!body.contains("CARRICK_ISSUE_COUNT"));
        // The human-facing content is preserved.
        assert!(body.contains("All cross-service calls match the indexed contracts"));
        // No leading/trailing blank lines left behind by the stripped markers.
        assert_eq!(body, body.trim());
    }

    #[test]
    fn test_verified_section_renders_when_matches_present() {
        let issues = ApiIssues {
            call_issues: vec![],
            endpoint_issues: vec![],
            env_var_calls: vec![],
            mismatches: vec![],
            type_mismatches: vec![],
            dependency_conflicts: vec![],
        };
        let result = ApiAnalysisResult {
            endpoints: vec![],
            calls: vec![],
            issues,
            verified_endpoints: vec![
                ("GET".to_string(), "/api/users".to_string()),
                ("POST".to_string(), "/api/orders".to_string()),
            ],
            detected_graphql_libraries: vec![],
            graphql_operations_indexed: false,
        };

        let output = format_analysis_results(result, &topology_baseline(), None);

        assert!(output.contains("Verified (2)"));
        assert!(output.contains("`GET` | `/api/users`"));
        assert!(output.contains("`POST` | `/api/orders`"));
    }

    #[test]
    fn test_verified_section_singular_label() {
        let issues = ApiIssues {
            call_issues: vec![],
            endpoint_issues: vec![],
            env_var_calls: vec![],
            mismatches: vec![],
            type_mismatches: vec![],
            dependency_conflicts: vec![],
        };
        let result = ApiAnalysisResult {
            endpoints: vec![],
            calls: vec![],
            issues,
            verified_endpoints: vec![("GET".to_string(), "/api/users".to_string())],
            detected_graphql_libraries: vec![],
            graphql_operations_indexed: false,
        };

        let output = format_analysis_results(result, &topology_baseline(), None);

        assert!(output.contains("Verified (1)"));
    }

    #[test]
    fn test_verified_section_renders_on_clean_run() {
        // Even when there are zero issues, a clean run with verified
        // matches must surface them — that's the whole point of the
        // section. Pre-fix, `format_no_issues` ignored verified entirely.
        let issues = ApiIssues {
            call_issues: vec![],
            endpoint_issues: vec![],
            env_var_calls: vec![],
            mismatches: vec![],
            type_mismatches: vec![],
            dependency_conflicts: vec![],
        };
        let result = ApiAnalysisResult {
            endpoints: vec![],
            calls: vec![],
            issues,
            verified_endpoints: vec![("GET".to_string(), "/api/users".to_string())],
            detected_graphql_libraries: vec![],
            graphql_operations_indexed: false,
        };

        let output = format_analysis_results(result, &topology_baseline(), None);

        assert!(output.contains("All cross-service calls match the indexed contracts"));
        assert!(output.contains("Verified (1)"));
    }

    #[test]
    fn test_extract_env_var_info_new_format() {
        // Test the new format: "Unclassified env var: GET /orders using [ORDER_SERVICE_URL] - add to..."
        let issue = "Unclassified env var: GET /orders using [ORDER_SERVICE_URL] - add to internalEnvVars or externalEnvVars in carrick.json";
        let (method, env_var, path) = extract_env_var_info(issue);

        assert_eq!(method, "GET");
        assert_eq!(env_var, "ORDER_SERVICE_URL");
        assert_eq!(path, "/orders");
    }

    #[test]
    fn test_extract_env_var_info_new_format_with_params() {
        let issue = "Unclassified env var: POST /users/:id/comments using [USER_API] - add to internalEnvVars or externalEnvVars in carrick.json";
        let (method, env_var, path) = extract_env_var_info(issue);

        assert_eq!(method, "POST");
        assert_eq!(env_var, "USER_API");
        assert_eq!(path, "/users/:id/comments");
    }

    #[test]
    fn test_extract_env_var_info_old_format() {
        // Test the old format: "Environment variable endpoint: GET using env vars [API_URL] in ENV_VAR:API_URL:/users"
        let issue =
            "Environment variable endpoint: GET using env vars [API_URL] in ENV_VAR:API_URL:/users";
        let (method, env_var, path) = extract_env_var_info(issue);

        assert_eq!(method, "GET");
        assert_eq!(env_var, "API_URL");
        assert_eq!(path, "/users");
    }

    #[test]
    fn test_extract_env_var_info_old_format_complex_path() {
        let issue = "Environment variable endpoint: DELETE using env vars [ORDER_SERVICE] in ENV_VAR:ORDER_SERVICE:/orders/123/items";
        let (method, env_var, path) = extract_env_var_info(issue);

        assert_eq!(method, "DELETE");
        assert_eq!(env_var, "ORDER_SERVICE");
        assert_eq!(path, "/orders/123/items");
    }

    #[test]
    fn test_extract_method_path_orphaned_endpoint() {
        let issue = "Orphaned endpoint: GET /api/users in src/routes.ts:42";
        let (method, path) = extract_method_path(issue);
        assert_eq!(method, "GET");
        assert_eq!(path, "/api/users");
    }

    #[test]
    fn test_extract_method_path_missing_endpoint() {
        let issue = "Missing endpoint for POST /api/orders (normalized: /api/orders) (called from src/client.ts)";
        let (method, path) = extract_method_path(issue);
        assert_eq!(method, "POST");
        assert_eq!(path, "/api/orders");
    }

    #[test]
    fn test_extract_method_path_no_unknown() {
        // Previously this would return UNKNOWN/UNKNOWN
        let issue = "Orphaned endpoint: DELETE /items/:id in src/items.ts:10";
        let (method, path) = extract_method_path(issue);
        assert_eq!(method, "DELETE");
        assert_eq!(path, "/items/:id");
    }

    #[test]
    fn test_no_baseline_excludes_connectivity_from_headline_count() {
        // First repo indexed (has_cross_repo_baseline = false): connectivity
        // findings are inconclusive (no peers to match against) so they must be
        // kept OUT of the headline CARRICK_ISSUE_COUNT, yet the section must
        // still render — framed as informational "Observations". This covers
        // the previously-untested `false` branch of `has_cross_repo_baseline`.
        let issues = ApiIssues {
            call_issues: vec![],
            endpoint_issues: vec![
                OrphanedEndpoint {
                    method: "GET".to_string(),
                    path: "/api/users".to_string(),
                    service: None,
                },
                OrphanedEndpoint {
                    method: "PUT".to_string(),
                    path: "/api/sessions".to_string(),
                    service: None,
                },
            ],
            env_var_calls: vec![],
            mismatches: vec![],
            type_mismatches: vec![],
            dependency_conflicts: vec![],
        };
        let result = ApiAnalysisResult {
            endpoints: vec![],
            calls: vec![],
            issues,
            verified_endpoints: vec![],
            detected_graphql_libraries: vec![],
            graphql_operations_indexed: false,
        };

        let output = format_analysis_results(result, &topology_first_repo(), None);

        // Headline count excludes the two connectivity findings → zero issues.
        assert!(
            output.contains("<!-- CARRICK_ISSUE_COUNT:0 -->"),
            "first-run connectivity findings must not inflate the headline count, got:\n{}",
            output
        );
        // But the connectivity section still renders, framed as observations.
        assert!(
            output.contains("Connectivity Observations"),
            "connectivity section must still render as informational observations, got:\n{}",
            output
        );
        assert!(
            output.contains("First repo indexed"),
            "first-run observations must carry the informational framing, got:\n{}",
            output
        );
        // The orphaned endpoints are rendered as Method/Path table rows.
        assert!(
            output.contains("`/api/users`"),
            "the orphaned endpoints must still be listed, got:\n{}",
            output
        );
        assert!(
            output.contains("`/api/sessions`"),
            "the orphaned endpoints must still be listed, got:\n{}",
            output
        );
        // Headline phrasing reflects the informational framing, not "gaps".
        assert!(
            output.contains("connectivity observations"),
            "headline should frame first-run connectivity as observations, got:\n{}",
            output
        );
    }

    fn empty_issues() -> ApiIssues {
        ApiIssues {
            call_issues: vec![],
            endpoint_issues: vec![],
            env_var_calls: vec![],
            mismatches: vec![],
            type_mismatches: vec![],
            dependency_conflicts: vec![],
        }
    }

    fn result_with(issues: ApiIssues) -> ApiAnalysisResult {
        ApiAnalysisResult {
            endpoints: vec![],
            calls: vec![],
            issues,
            verified_endpoints: vec![],
            detected_graphql_libraries: vec![],
            graphql_operations_indexed: false,
        }
    }

    #[test]
    fn test_monorepo_header_and_scope() {
        let topology = Topology {
            repo_name: "platform".to_string(),
            local_service_count: 3,
            peer_repo_count: 0,
        };
        // A monorepo has a cross-service baseline even with no peers, so a
        // type mismatch headlines as a contract risk.
        let mut issues = empty_issues();
        issues.type_mismatches = vec![
            "Type mismatch on GET /api/users: Producer (UserResponse) incompatible with Consumer (User[]) - Property 'role' is missing".to_string(),
        ];
        let output = format_analysis_results(result_with(issues), &topology, None);

        assert!(output.contains("## 🪢 Carrick · monorepo (3 services)"));
        assert!(output.contains("> [!CAUTION]"));
        assert!(output.contains("contract risk"));
        assert!(output.contains("3 services"));
    }

    #[test]
    fn test_single_repo_clean_header() {
        let topology = Topology {
            repo_name: "api-server".to_string(),
            local_service_count: 1,
            peer_repo_count: 0,
        };
        let output = format_analysis_results(result_with(empty_issues()), &topology, None);

        assert!(output.contains("## 🪢 Carrick · api-server"));
        assert!(output.contains("> [!TIP]"));
        assert!(output.contains("across api-server"));
    }

    #[test]
    fn test_monorepo_with_peers_scope_has_no_double_across() {
        // A monorepo that also has peer repos is the one topology where the
        // scope phrase and the verdict's "across" could collide. The service
        // count lives in the header suffix; the scope reports repos.
        let topology = Topology {
            repo_name: "platform".to_string(),
            local_service_count: 3,
            peer_repo_count: 2,
        };
        let mut issues = empty_issues();
        issues.dependency_conflicts = vec![DependencyConflict {
            package_name: "zod".to_string(),
            repos: vec![crate::analyzer::RepoPackageInfo {
                repo_name: "billing".to_string(),
                version: "^3.22".to_string(),
                source_path: std::path::PathBuf::from("package.json"),
            }],
            severity: ConflictSeverity::Warning,
        }];
        let output = format_analysis_results(result_with(issues), &topology, None);

        assert!(output.contains("## 🪢 Carrick · monorepo (3 services)"));
        assert!(output.contains("across 3 repos"));
        assert!(
            !output.contains("services across"),
            "the verdict must not double 'across', got:\n{}",
            output
        );
    }

    #[test]
    fn test_verdict_warning_on_dependencies_only() {
        let mut issues = empty_issues();
        issues.dependency_conflicts = vec![DependencyConflict {
            package_name: "zod".to_string(),
            repos: vec![crate::analyzer::RepoPackageInfo {
                repo_name: "billing".to_string(),
                version: "^3.22".to_string(),
                source_path: std::path::PathBuf::from("package.json"),
            }],
            severity: ConflictSeverity::Warning,
        }];
        let output = format_analysis_results(result_with(issues), &topology_baseline(), None);

        // No contract risks, but a counted issue → amber, not red.
        assert!(output.contains("> [!WARNING]"));
        assert!(!output.contains("> [!CAUTION]"));
        assert!(output.contains("dependency conflict"));
    }

    #[test]
    fn test_no_baseline_without_connectivity_omits_baseline_note() {
        // A lone repo with only a configuration suggestion (no connectivity
        // findings) must not claim "connectivity findings are informational".
        let mut issues = empty_issues();
        issues.env_var_calls = vec![
            "Unclassified env var: GET /orders using [ORDER_SERVICE_URL] (from src/orders.ts) - add to internalEnvVars or externalEnvVars in carrick.json".to_string(),
        ];
        let output = format_analysis_results(result_with(issues), &topology_first_repo(), None);

        assert!(output.contains("configuration suggestion"));
        assert!(
            !output.contains("First repo indexed"),
            "no connectivity findings means no baseline note, got:\n{}",
            output
        );
    }

    #[test]
    fn test_no_decorative_emoji_in_output() {
        // Severity is carried by GitHub alert blocks, not emoji. Only the 🪢
        // brand mark in the header is allowed.
        let mut issues = empty_issues();
        issues.type_mismatches = vec![
            "Type mismatch on GET /api/users: Producer (UserResponse) incompatible with Consumer (User[]) - Property 'role' is missing".to_string(),
        ];
        issues.endpoint_issues = vec![OrphanedEndpoint {
            method: "GET".to_string(),
            path: "/legacy/ping".to_string(),
            service: Some("billing".to_string()),
        }];
        let output = format_analysis_results(result_with(issues), &topology_baseline(), None);

        for banned in ["✅", "ℹ️", "⚠️", "❌", "🔁"] {
            assert!(
                !output.contains(banned),
                "decorative emoji {} must not appear, got:\n{}",
                banned,
                output
            );
        }
        // The brand mark is intentionally retained.
        assert!(output.contains("🪢"));
    }

    #[test]
    fn test_orphaned_endpoints_show_service_when_attributed() {
        // When orphans carry an owning service, the connectivity table gains a
        // Service column naming where each lives.
        let mut issues = empty_issues();
        issues.endpoint_issues = vec![
            OrphanedEndpoint {
                method: "GET".to_string(),
                path: "/users".to_string(),
                service: Some("auth".to_string()),
            },
            OrphanedEndpoint {
                method: "POST".to_string(),
                path: "/charges".to_string(),
                service: Some("billing".to_string()),
            },
        ];
        let output = format_analysis_results(result_with(issues), &topology_baseline(), None);

        assert!(output.contains("| Method | Path | Service |"));
        assert!(output.contains("| `GET` | `/users` | `auth` |"));
        assert!(output.contains("| `POST` | `/charges` | `billing` |"));
    }

    #[test]
    fn test_orphaned_endpoints_omit_service_column_when_unattributed() {
        // A lone repo has no service attribution, so the column is dropped
        // rather than rendering an empty cell per row.
        let mut issues = empty_issues();
        issues.endpoint_issues = vec![OrphanedEndpoint {
            method: "GET".to_string(),
            path: "/legacy/ping".to_string(),
            service: None,
        }];
        let output = format_analysis_results(result_with(issues), &topology_first_repo(), None);

        assert!(!output.contains("Service |"));
        assert!(output.contains("| `GET` | `/legacy/ping` |"));
    }

    #[test]
    fn test_cell_escapes_pipes_and_collapses_line_breaks() {
        assert_eq!(cell("a|b"), "a\\|b");
        assert_eq!(cell("a\r\nb"), "a b");
        assert_eq!(cell("a\rb"), "a b");
        assert_eq!(cell("a\nb"), "a b");
        // code_cell additionally drops backticks for inline-code cells.
        assert_eq!(code_cell("x`y|z"), "xy\\|z");
    }

    #[test]
    fn test_orphaned_mixed_attribution_uses_dash_and_escapes_cells() {
        // When the Service column is shown, an unattributed row gets a dash, and
        // any pipe in a value is escaped so it can't break the table.
        let mut issues = empty_issues();
        issues.endpoint_issues = vec![
            OrphanedEndpoint {
                method: "GET".to_string(),
                path: "/users".to_string(),
                // Backtick must be stripped so it can't break the code span.
                service: Some("au`th".to_string()),
            },
            OrphanedEndpoint {
                method: "QUERY".to_string(),
                path: "weird|field".to_string(),
                service: None,
            },
        ];
        let output = format_analysis_results(result_with(issues), &topology_baseline(), None);

        assert!(output.contains("| `GET` | `/users` | `auth` |"));
        // Unattributed row → dash, and the pipe in the path is escaped.
        assert!(output.contains("| `QUERY` | `weird\\|field` | - |"));
    }

    #[test]
    fn test_pr_delta_strip_lists_new_endpoints() {
        let delta = PrDelta {
            new_endpoints: vec![
                NewEndpoint {
                    method: "POST".to_string(),
                    path: "/v2/invoices".to_string(),
                    service: Some("billing".to_string()),
                },
                NewEndpoint {
                    method: "GET".to_string(),
                    path: "/charges".to_string(),
                    service: None,
                },
            ],
        };
        let mut issues = empty_issues();
        issues.dependency_conflicts = vec![DependencyConflict {
            package_name: "zod".to_string(),
            repos: vec![crate::analyzer::RepoPackageInfo {
                repo_name: "billing".to_string(),
                version: "^3.22".to_string(),
                source_path: std::path::PathBuf::from("package.json"),
            }],
            severity: ConflictSeverity::Warning,
        }];
        let output =
            format_analysis_results(result_with(issues), &topology_baseline(), Some(&delta));

        assert!(output.contains("**In this PR**"));
        assert!(output.contains("- New endpoint `POST /v2/invoices` (billing)"));
        assert!(output.contains("- New endpoint `GET /charges`"));
        // The strip sits above the findings sections.
        let strip_at = output.find("**In this PR**").unwrap();
        let deps_at = output.find("Dependency conflicts").unwrap();
        assert!(strip_at < deps_at, "the strip must precede the sections");
    }

    #[test]
    fn test_pr_delta_absent_renders_no_strip() {
        let output =
            format_analysis_results(result_with(empty_issues()), &topology_baseline(), None);
        assert!(!output.contains("In this PR"));
    }

    #[test]
    fn test_pr_delta_strip_renders_on_clean_run() {
        // A PR can add an endpoint without introducing any issue, so the strip
        // must also appear on the clean (TIP) path.
        let delta = PrDelta {
            new_endpoints: vec![NewEndpoint {
                method: "GET".to_string(),
                path: "/health".to_string(),
                service: None,
            }],
        };
        let output = format_analysis_results(
            result_with(empty_issues()),
            &topology_baseline(),
            Some(&delta),
        );

        assert!(output.contains("> [!TIP]"));
        assert!(output.contains("**In this PR**"));
        assert!(output.contains("- New endpoint `GET /health`"));
    }
}
