use crate::analyzer::Analyzer;
use crate::cloud_storage::CloudRepoData;
use crate::config::Config;
use crate::visitor::DependencyVisitor;
use swc_common::{SourceMap, sync::Lrc};

pub struct AnalyzerBuilder {
    config: Config,
    cm: Lrc<SourceMap>,
    skip_type_resolution: bool,
}

impl AnalyzerBuilder {
    pub fn new(config: Config, cm: Lrc<SourceMap>) -> Self {
        Self {
            config,
            cm,
            skip_type_resolution: false,
        }
    }

    pub fn new_for_cross_repo(config: Config, cm: Lrc<SourceMap>) -> Self {
        Self {
            config,
            cm,
            skip_type_resolution: true,
        }
    }

    /// Build analyzer from visitor data (used by analyze_current_repo)
    pub async fn build_from_visitors(
        &self,
        visitors: Vec<DependencyVisitor>,
    ) -> Result<Analyzer, Box<dyn std::error::Error>> {
        let mut analyzer = Analyzer::new(self.config.clone(), self.cm.clone());

        // Add visitor data to analyzer
        for visitor in visitors {
            analyzer.add_visitor_data(visitor);
        }

        self.finalize_analyzer(analyzer).await
    }

    /// Build analyzer from CloudRepoData (used by build_cross_repo_analyzer)
    pub async fn build_from_repo_data(
        &self,
        all_repo_data: Vec<CloudRepoData>,
    ) -> Result<Analyzer, Box<dyn std::error::Error>> {
        let mut analyzer = Analyzer::new(self.config.clone(), self.cm.clone());

        // Populate analyzer with data from all repos
        for repo_data in all_repo_data {
            analyzer.endpoints.extend(repo_data.endpoints);
            analyzer.calls.extend(repo_data.calls);
            analyzer.mounts.extend(repo_data.mounts);
            analyzer.apps.extend(repo_data.apps);
            analyzer
                .imported_handlers
                .extend(repo_data.imported_handlers);
            analyzer
                .function_definitions
                .extend(repo_data.function_definitions);
        }

        self.finalize_analyzer(analyzer).await
    }

    /// Common analyzer finalization steps (eliminates duplication)
    async fn finalize_analyzer(
        &self,
        mut analyzer: Analyzer,
    ) -> Result<Analyzer, Box<dyn std::error::Error>> {
        // Skip path resolution in cross-repo mode - paths are already resolved
        if !self.skip_type_resolution {
            // Resolve endpoint paths and types
            let endpoints = analyzer.resolve_all_endpoint_paths(
                &analyzer.endpoints.clone(),
                &analyzer.mounts.clone(),
                &analyzer.apps.clone(),
            );
            analyzer.endpoints = endpoints;
        }

        // Build the router
        analyzer.build_endpoint_router();

        // Resolve imported handler route fields
        let (response_fields, request_fields) = analyzer.resolve_imported_handler_route_fields(
            &analyzer.imported_handlers.clone(),
            &analyzer.function_definitions.clone(),
        );

        // Update endpoints and resolve types (skip type resolution in cross-repo mode)
        analyzer.update_endpoints_with_resolved_fields(response_fields, request_fields);

        if !self.skip_type_resolution {
            analyzer.resolve_types_for_endpoints(self.cm.clone());
            // Only analyze functions for fetch calls when we have real AST data
            analyzer.analyze_functions_for_fetch_calls().await;
        }

        Ok(analyzer)
    }
}
