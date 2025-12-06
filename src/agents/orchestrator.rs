use crate::{
    call_site_extractor::{ArgumentType, CallSite},
    framework_detector::DetectionResult,
    gemini_service::GeminiService,
};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

use super::{
    consumer_agent::{ConsumerAgent, DataFetchingCall},
    endpoint_agent::{EndpointAgent, HttpEndpoint},
    middleware_agent::{Middleware, MiddlewareAgent},
    mount_agent::{MountAgent, MountRelationship},
    triage_agent::{TriageAgent, TriageClassification, TriageResult},
};

/// Complete analysis results from all specialized agents
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AnalysisResults {
    pub endpoints: Vec<HttpEndpoint>,
    pub data_fetching_calls: Vec<DataFetchingCall>,
    pub middleware: Vec<Middleware>,
    pub mount_relationships: Vec<MountRelationship>,
    pub triage_stats: TriageStats,
}

/// Statistics from the triage process
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct TriageStats {
    pub total_call_sites: usize,
    pub endpoints_count: usize,
    pub data_fetching_count: usize,
    pub middleware_count: usize,
    pub router_mount_count: usize,
    pub irrelevant_count: usize,
}

/// Orchestrator that implements the Classify-Then-Dispatch pattern
pub struct CallSiteOrchestrator {
    triage_agent: TriageAgent,
    endpoint_agent: EndpointAgent,
    consumer_agent: ConsumerAgent,
    middleware_agent: MiddlewareAgent,
    mount_agent: MountAgent,
}

impl CallSiteOrchestrator {
    pub fn new(gemini_service: GeminiService) -> Self {
        let triage_agent = TriageAgent::new(gemini_service.clone());
        let endpoint_agent = EndpointAgent::new(gemini_service.clone());
        let consumer_agent = ConsumerAgent::new(gemini_service.clone());
        let middleware_agent = MiddlewareAgent::new(gemini_service.clone());
        let mount_agent = MountAgent::new(gemini_service.clone());

        Self {
            triage_agent,
            endpoint_agent,
            consumer_agent,
            middleware_agent,
            mount_agent,
        }
    }

    /// Perform complete call site analysis using the Classify-Then-Dispatch pattern
    pub async fn analyze_call_sites(
        &self,
        call_sites: &[CallSite],
        framework_detection: &DetectionResult,
    ) -> Result<AnalysisResults, Box<dyn std::error::Error>> {
        if call_sites.is_empty() {
            return Ok(AnalysisResults {
                endpoints: Vec::new(),
                data_fetching_calls: Vec::new(),
                middleware: Vec::new(),
                mount_relationships: Vec::new(),
                triage_stats: TriageStats {
                    total_call_sites: 0,
                    endpoints_count: 0,
                    data_fetching_count: 0,
                    middleware_count: 0,
                    router_mount_count: 0,
                    irrelevant_count: 0,
                },
            });
        }

        println!("=== ORCHESTRATOR: Starting Classify-Then-Dispatch Analysis ===");
        println!("Total call sites to analyze: {}", call_sites.len());

        // Step 1: Triage - Classify all call sites into broad categories
        let triage_results = self
            .triage_agent
            .classify_call_sites(call_sites, framework_detection)
            .await?;

        // Step 2: Dispatch - Group call sites by classification and create lookup map
        let (grouped_call_sites, triage_stats) =
            self.dispatch_call_sites(call_sites, &triage_results)?;

        println!("=== ORCHESTRATOR: Dispatching to Specialist Agents ===");
        println!("Endpoints: {}", grouped_call_sites.endpoints.len());
        println!("Data fetching: {}", grouped_call_sites.data_fetching.len());
        println!("Middleware: {}", grouped_call_sites.middleware.len());
        println!("Router mounts: {}", grouped_call_sites.router_mounts.len());
        println!("Irrelevant: {}", triage_stats.irrelevant_count);

        // Step 3: Run specialist agents in parallel on their respective call sites
        let (mut endpoints_result, data_fetching_result, middleware_result, mount_result) = tokio::try_join!(
            self.endpoint_agent
                .detect_endpoints(&grouped_call_sites.endpoints, framework_detection),
            self.consumer_agent
                .detect_data_fetching_calls(&grouped_call_sites.data_fetching, framework_detection),
            self.middleware_agent
                .detect_middleware(&grouped_call_sites.middleware, framework_detection),
            self.mount_agent
                .detect_mounts(&grouped_call_sites.router_mounts, framework_detection),
        )?;

        // Step 4: Enrich endpoints with type info from call sites (SWC-extracted, reliable)
        self.enrich_endpoints_with_type_info(&mut endpoints_result, &grouped_call_sites.endpoints);

        // Step 5: Enrich data fetching calls with type info from call sites
        let mut data_fetching_result = data_fetching_result;
        self.enrich_data_fetching_calls_with_type_info(
            &mut data_fetching_result,
            &grouped_call_sites.data_fetching,
        );

        println!("=== ORCHESTRATOR: Analysis Complete ===");
        println!("Extracted {} endpoints", endpoints_result.len());
        println!(
            "Extracted {} data fetching calls",
            data_fetching_result.len()
        );
        println!(
            "Extracted {} middleware registrations",
            middleware_result.len()
        );
        println!("Extracted {} mount relationships", mount_result.len());

        Ok(AnalysisResults {
            endpoints: endpoints_result,
            data_fetching_calls: data_fetching_result,
            middleware: middleware_result,
            mount_relationships: mount_result,
            triage_stats,
        })
    }

    /// Dispatch triaged call sites to appropriate groups
    fn dispatch_call_sites(
        &self,
        call_sites: &[CallSite],
        triage_results: &[TriageResult],
    ) -> Result<(GroupedCallSites, TriageStats), Box<dyn std::error::Error>> {
        // Create lookup map from location to call site
        let mut location_to_call_site: HashMap<String, &CallSite> = HashMap::new();
        for call_site in call_sites {
            location_to_call_site.insert(call_site.location.clone(), call_site);
        }

        // Group call sites by triage classification
        let mut grouped = GroupedCallSites {
            endpoints: Vec::new(),
            data_fetching: Vec::new(),
            middleware: Vec::new(),
            router_mounts: Vec::new(),
        };

        let mut stats = TriageStats {
            total_call_sites: call_sites.len(),
            endpoints_count: 0,
            data_fetching_count: 0,
            middleware_count: 0,
            router_mount_count: 0,
            irrelevant_count: 0,
        };

        for triage_result in triage_results {
            let call_site = location_to_call_site
                .get(&triage_result.location)
                .ok_or_else(|| {
                    format!(
                        "Triage result location '{}' not found in original call sites",
                        triage_result.location
                    )
                })?;

            match triage_result.classification {
                TriageClassification::HttpEndpoint => {
                    grouped.endpoints.push((*call_site).clone());
                    stats.endpoints_count += 1;
                }
                TriageClassification::DataFetchingCall => {
                    grouped.data_fetching.push((*call_site).clone());
                    stats.data_fetching_count += 1;
                }
                TriageClassification::Middleware => {
                    grouped.middleware.push((*call_site).clone());
                    stats.middleware_count += 1;
                }
                TriageClassification::RouterMount => {
                    grouped.router_mounts.push((*call_site).clone());
                    stats.router_mount_count += 1;
                }
                TriageClassification::Irrelevant => {
                    stats.irrelevant_count += 1;
                    // Don't add to any group - these are filtered out
                }
            }
        }

        // Validate that we accounted for all call sites
        let total_classified = stats.endpoints_count
            + stats.data_fetching_count
            + stats.middleware_count
            + stats.router_mount_count
            + stats.irrelevant_count;
        if total_classified != stats.total_call_sites {
            return Err(format!(
                "Triage classification mismatch: {} call sites input, {} classified",
                stats.total_call_sites, total_classified
            )
            .into());
        }

        Ok((grouped, stats))
    }
}

/// Internal structure for grouping call sites by classification
struct GroupedCallSites {
    endpoints: Vec<CallSite>,
    data_fetching: Vec<CallSite>,
    middleware: Vec<CallSite>,
    router_mounts: Vec<CallSite>,
}

impl CallSiteOrchestrator {
    /// Enrich endpoints with type information extracted from call sites
    /// This uses the SWC-extracted byte offsets which are more reliable than LLM guesses
    fn enrich_endpoints_with_type_info(
        &self,
        endpoints: &mut [HttpEndpoint],
        endpoint_call_sites: &[CallSite],
    ) {
        // Create lookup from location to call site
        let location_to_call_site: HashMap<String, &CallSite> = endpoint_call_sites
            .iter()
            .map(|cs| (cs.location.clone(), cs))
            .collect();

        for endpoint in endpoints.iter_mut() {
            // Skip if already has type info (from LLM or other source)
            if endpoint.response_type_string.is_some() {
                continue;
            }

            // Find the matching call site by location
            if let Some(call_site) = location_to_call_site.get(&endpoint.location) {
                // Look for handler argument (typically the last argument that's a function)
                // For Express: app.get("/path", handler) - handler is arg[1]
                // For routes with middleware: app.get("/path", middleware, handler) - handler is last

                let handler_arg = call_site.args.iter().rev().find(|arg| {
                    matches!(
                        arg.arg_type,
                        ArgumentType::ArrowFunction | ArgumentType::FunctionExpression
                    )
                });

                if let Some(arg) = handler_arg {
                    if let Some(param_types) = &arg.handler_param_types {
                        // Look for Response type (typically the second parameter: res: Response<T>)
                        // Common patterns: res, response, reply (Fastify)
                        let response_param = param_types.iter().find(|p| {
                            let name_lower = p.param_name.to_lowercase();
                            name_lower == "res"
                                || name_lower == "response"
                                || name_lower == "reply"
                                || p.type_string.contains("Response<")
                        });

                        if let Some(res_type) = response_param {
                            // Extract file path from location (format: "path/to/file.ts:line:col")
                            let file_path = endpoint
                                .location
                                .split(':')
                                .next()
                                .unwrap_or("")
                                .to_string();

                            endpoint.response_type_file = Some(file_path);
                            endpoint.response_type_position = Some(res_type.utf16_offset);
                            endpoint.response_type_string = Some(res_type.type_string.clone());

                            println!(
                                "  Enriched endpoint {} {} with response type: {} (pos: {})",
                                endpoint.method,
                                endpoint.path,
                                res_type.type_string,
                                res_type.utf16_offset
                            );
                        }

                        // Also look for Request type (typically the first parameter: req: Request<T>)
                        // We don't have request_type fields on HttpEndpoint yet, but we could add them
                    }
                }
            }
        }
    }

    /// Enrich data fetching calls with type information extracted from call sites
    /// This uses the SWC-extracted result types which are more reliable than LLM guesses
    /// Also uses correlated fetch info for .json() calls to get URL/method from the original fetch()
    fn enrich_data_fetching_calls_with_type_info(
        &self,
        calls: &mut [DataFetchingCall],
        call_sites: &[CallSite],
    ) {
        // Create lookup from location to call site
        let location_to_call_site: HashMap<String, &CallSite> = call_sites
            .iter()
            .map(|cs| (cs.location.clone(), cs))
            .collect();

        for call in calls.iter_mut() {
            // Skip if already has type info
            if call.expected_type_string.is_some() {
                continue;
            }

            // Find matching call site by location
            if let Some(call_site) = location_to_call_site.get(&call.location) {
                if let Some(result_type) = &call_site.result_type {
                    // Extract file path from location (format: "path/to/file.ts:line:col")
                    let file_path = call.location.split(':').next().unwrap_or("").to_string();

                    call.expected_type_file = Some(file_path);
                    call.expected_type_position = Some(result_type.utf16_offset);
                    call.expected_type_string = Some(result_type.type_string.clone());

                    // If this call doesn't have URL/method but has correlated fetch info,
                    // use the correlated fetch's URL/method (for .json() calls)
                    if call.url.is_none() || call.method.is_none() {
                        if let Some(fetch_info) = &call_site.correlated_fetch {
                            if call.url.is_none() {
                                call.url = fetch_info.url.clone();
                            }
                            if call.method.is_none() {
                                call.method = Some(fetch_info.method.clone());
                            }
                            println!(
                                "  Correlated .json() call with fetch: url={:?}, method={}",
                                fetch_info.url, fetch_info.method
                            );
                        }
                    }

                    println!(
                        "  Enriched call {} {} with expected type: {} (pos: {})",
                        call.method.as_deref().unwrap_or("?"),
                        call.url.as_deref().unwrap_or("?"),
                        result_type.type_string,
                        result_type.utf16_offset
                    );
                }
            }
        }
    }
}
