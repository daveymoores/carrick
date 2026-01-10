pub mod file_analyzer_agent;
pub mod file_orchestrator;
pub mod framework_guidance_agent;
pub mod legacy_types;
pub mod schemas;

// File-centric analysis types (new architecture)
// These are re-exported for external use
#[allow(unused_imports)]
pub use file_analyzer_agent::FileAnalyzerAgent;
#[allow(unused_imports)]
pub use file_orchestrator::{FileCentricAnalysisResult, FileOrchestrator, ProcessingStats};
#[allow(unused_imports)]
pub use framework_guidance_agent::{FrameworkGuidance, FrameworkGuidanceAgent};

// Legacy types for backward compatibility with tests and mount_graph
#[allow(unused_imports)]
pub use legacy_types::{
    AnalysisResults, DataFetchingCall, HttpEndpoint, Middleware, MountRelationship, TriageStats,
};
