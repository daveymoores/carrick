use crate::{gemini_service::GeminiService, packages::Packages, visitor::ImportedSymbol};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Result of framework and library detection
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DetectionResult {
    pub frameworks: Vec<String>,
    pub data_fetchers: Vec<String>,
    pub notes: String,
}

/// Input data for LLM-based framework detection
#[derive(Debug, Serialize)]
struct FrameworkDetectionInput {
    package_json: PackageJsonSummary,
    imports: Vec<String>,
}

/// Simplified package.json summary for LLM analysis
#[derive(Debug, Serialize)]
struct PackageJsonSummary {
    dependencies: HashMap<String, String>,
    dev_dependencies: HashMap<String, String>,
}

/// Framework detector that combines package.json analysis with LLM classification
pub struct FrameworkDetector {
    gemini_service: GeminiService,
}

impl FrameworkDetector {
    pub fn new(gemini_service: GeminiService) -> Self {
        Self { gemini_service }
    }

    /// Main detection function that combines package.json and import analysis
    pub async fn detect_frameworks_and_libraries(
        &self,
        packages: &Packages,
        imported_symbols: &HashMap<String, ImportedSymbol>,
    ) -> Result<DetectionResult, Box<dyn std::error::Error>> {
        // Extract package.json data
        let package_summary = self.extract_package_summary(packages);
        
        // Extract import statements
        let import_statements = self.extract_import_statements(imported_symbols);
        
        // Prepare input for LLM
        let input = FrameworkDetectionInput {
            package_json: package_summary,
            imports: import_statements,
        };
        
        // Call LLM for classification
        let result = self.classify_with_llm(input).await?;
        
        Ok(result)
    }

    /// Extract relevant package.json information
    fn extract_package_summary(&self, packages: &Packages) -> PackageJsonSummary {
        let mut all_dependencies = HashMap::new();
        let mut all_dev_dependencies = HashMap::new();

        for package_json in &packages.package_jsons {
            // Merge dependencies
            for (name, version) in &package_json.dependencies {
                all_dependencies.insert(name.clone(), version.clone());
            }
            
            // Merge dev dependencies  
            for (name, version) in &package_json.dev_dependencies {
                all_dev_dependencies.insert(name.clone(), version.clone());
            }
        }

        PackageJsonSummary {
            dependencies: all_dependencies,
            dev_dependencies: all_dev_dependencies,
        }
    }

    /// Convert imported symbols to import statement strings for LLM analysis
    fn extract_import_statements(&self, imported_symbols: &HashMap<String, ImportedSymbol>) -> Vec<String> {
        let mut import_statements = Vec::new();
        let mut source_to_symbols: HashMap<String, Vec<&ImportedSymbol>> = HashMap::new();

        // Group symbols by source
        for symbol in imported_symbols.values() {
            source_to_symbols
                .entry(symbol.source.clone())
                .or_default()
                .push(symbol);
        }

        // Convert to import statement strings
        for (source, symbols) in source_to_symbols {
            let mut statement = String::new();
            
            let default_imports: Vec<_> = symbols.iter()
                .filter(|s| matches!(s.kind, crate::visitor::SymbolKind::Default))
                .collect();
                
            let named_imports: Vec<_> = symbols.iter()
                .filter(|s| matches!(s.kind, crate::visitor::SymbolKind::Named))
                .collect();
                
            let namespace_imports: Vec<_> = symbols.iter()
                .filter(|s| matches!(s.kind, crate::visitor::SymbolKind::Namespace))
                .collect();

            if !default_imports.is_empty() {
                statement.push_str(&format!("import {} from '{}';", default_imports[0].local_name, source));
            } else if !named_imports.is_empty() {
                let named_list: Vec<_> = named_imports.iter()
                    .map(|s| s.local_name.as_str())
                    .collect();
                statement.push_str(&format!("import {{ {} }} from '{}';", named_list.join(", "), source));
            } else if !namespace_imports.is_empty() {
                statement.push_str(&format!("import * as {} from '{}';", namespace_imports[0].local_name, source));
            }
            
            if !statement.is_empty() {
                import_statements.push(statement);
            }
        }

        import_statements
    }

    /// Use LLM to classify frameworks and data-fetching libraries
    async fn classify_with_llm(&self, input: FrameworkDetectionInput) -> Result<DetectionResult, Box<dyn std::error::Error>> {
        let prompt = self.build_classification_prompt(&input);
        
        let response = self.gemini_service.analyze_code(
            &prompt,
            "You are analyzing a Node.js/TypeScript project to detect HTTP frameworks and data-fetching libraries."
        ).await?;

        // Parse the JSON response
        let detection_result: DetectionResult = serde_json::from_str(&response)
            .map_err(|e| format!("Failed to parse LLM response as JSON: {}", e))?;

        Ok(detection_result)
    }

    /// Build the prompt for LLM classification
    fn build_classification_prompt(&self, input: &FrameworkDetectionInput) -> String {
        let input_json = serde_json::to_string_pretty(input).unwrap_or_default();
        
        format!(r#"
You are analyzing a Node.js / TypeScript project to detect which HTTP frameworks and data-fetching libraries are used.

Input:
1. `package.json` dependencies and devDependencies.
2. List of import statements found in source files.

Task:
- Identify all frameworks/libraries used for HTTP routing, e.g., express, koa, fastify, hapi, nestjs.
- Identify all libraries used for data fetching or HTTP clients, e.g., axios, node-fetch, got, superagent, graphql-request.
- Return a JSON object with:

{{
  "frameworks": ["express", "koa", ...],      // list of HTTP frameworks detected
  "data_fetchers": ["axios", "graphql-request", ...],  // list of data-fetching libraries
  "notes": "<optional comments about version or partial usage>"
}}

Example Input:
{{
  "package_json": {{
    "dependencies": {{"express": "^4.18.0", "axios": "^1.7.0"}},
    "devDependencies": {{"typescript": "^5.3.0"}}
  }},
  "imports": [
    "import express from 'express';",
    "const Router = require('express').Router",
    "import axios from 'axios';"
  ]
}}

Expected Output:
{{
  "frameworks": ["express"],
  "data_fetchers": ["axios"],
  "notes": "express is a direct dependency and imported in multiple modules; axios is imported in src/api.ts"
}}

Instructions:
- Include only libraries that affect routing or data-fetching.
- If a library is listed in package.json but not imported, you can still include it but note it in `notes`.
- Output valid JSON only.

Actual Input:
{}

Respond with valid JSON only:
"#, input_json)
    }
}

