// This module previously contained an unused CallSiteClassifier struct
// that called /agent/chat with a long system prompt. The struct was never
// instantiated (no callers anywhere in src/), but mount_graph.rs depends
// on the types defined here (CallSiteType, ClassifiedCallSite,
// HandlerArgument). The classifier impl was deleted; the types remain.
//
// If/when these types are decoupled from mount_graph, this file can be
// deleted entirely (filed as follow-up #7 in the migration plan).

use crate::call_site_extractor::CallSite;
use serde::{Deserialize, Serialize};

/// Classification result for a call site with detailed extraction
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClassifiedCallSite {
    #[serde(flatten)]
    pub call_site: CallSite,
    pub classification: CallSiteType,
    pub confidence: f32,
    pub reasoning: String,
    // Mount information (for RouterMounts)
    pub mount_parent: Option<String>,
    pub mount_child: Option<String>,
    pub mount_prefix: Option<String>,
    // Handler information (for HttpEndpoint or Middleware)
    pub handler_name: Option<String>,
    pub handler_args: Vec<HandlerArgument>,
}

/// Handler argument information
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HandlerArgument {
    pub name: String,
    pub arg_type: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum CallSiteType {
    RouterMount,
    Middleware,
    HttpEndpoint,
    DataFetchingCall,
    GraphQLCall,
    Irrelevant,
}
