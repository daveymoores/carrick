extern crate swc_common;
extern crate swc_ecma_parser;

use std::{collections::HashMap, path::PathBuf};
use swc_ecma_ast::*;
use swc_ecma_visit::Visit;

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct FunctionArgument {
    #[allow(dead_code)]
    pub name: String,
    #[serde(skip)]
    pub type_ann: Option<TsTypeAnn>, // swc_ecma_ast::TsTypeAnn
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct TypeReference {
    pub file_path: PathBuf,
    #[allow(dead_code)]
    #[serde(skip)]
    pub type_ann: Option<Box<TsType>>,
    pub start_position: usize,
    pub composite_type_string: String,
    pub alias: String,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum Json {
    Null,
    Boolean(bool),
    Number(f64),
    String(String),
    Array(Vec<Json>),
    Object(HashMap<String, Json>),
}

#[derive(Debug, Clone, Default)]
#[allow(dead_code)]
pub enum FunctionNodeType {
    ArrowFunction(Box<ArrowExpr>),
    FunctionDeclaration(Box<FnDecl>),
    FunctionExpression(Box<FnExpr>),
    // Used for deserialization when AST data is not available
    #[default]
    Placeholder,
}

impl serde::Serialize for FunctionNodeType {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        match self {
            FunctionNodeType::ArrowFunction(_) => serializer.serialize_str("ArrowFunction"),
            FunctionNodeType::FunctionDeclaration(_) => {
                serializer.serialize_str("FunctionDeclaration")
            }
            FunctionNodeType::FunctionExpression(_) => {
                serializer.serialize_str("FunctionExpression")
            }
            FunctionNodeType::Placeholder => serializer.serialize_str("Placeholder"),
        }
    }
}

impl<'de> serde::Deserialize<'de> for FunctionNodeType {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let s = String::deserialize(deserializer)?;
        match s.as_str() {
            "ArrowFunction" => Ok(FunctionNodeType::Placeholder),
            "FunctionDeclaration" => Ok(FunctionNodeType::Placeholder),
            "FunctionExpression" => Ok(FunctionNodeType::Placeholder),
            "Placeholder" => Ok(FunctionNodeType::Placeholder),
            _ => Ok(FunctionNodeType::Placeholder),
        }
    }
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct FunctionDefinition {
    #[allow(dead_code)]
    pub name: String,
    #[allow(dead_code)]
    pub file_path: PathBuf,
    pub node_type: FunctionNodeType,
    pub arguments: Vec<FunctionArgument>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum OwnerType {
    App(String),
    Router(String),
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Mount {
    pub parent: OwnerType, // App or Router doing the .use
    pub child: OwnerType,  // Router being mounted
    pub prefix: String,    // Path prefix for this mount
}

#[derive(Debug, Clone)]
pub struct Call {
    pub route: String,
    pub method: String,
    #[allow(dead_code)]
    pub response: Json,
    pub request: Option<Json>,
    pub response_type: Option<TypeReference>,
    pub request_type: Option<TypeReference>,
    pub call_file: PathBuf,
    pub call_id: Option<String>, // Unique identifier for this specific call instance
    pub call_number: Option<u32>, // Sequential number for this route+method combination
    pub common_type_name: Option<String>, // Common interface name for type comparison
}

#[derive(Debug, Clone)]
pub enum SymbolKind {
    Named,
    Default,
    Namespace,
}

#[derive(Debug, Clone)]
pub struct ImportedSymbol {
    #[allow(dead_code)]
    pub local_name: String,
    #[allow(dead_code)]
    pub imported_name: String,
    pub source: String,
    pub kind: SymbolKind,
}

/// Lightweight extractor focused only on import symbols.
/// Used by the multi-agent pipeline for import resolution.
#[derive(Debug)]
pub struct ImportSymbolExtractor {
    pub imported_symbols: HashMap<String, ImportedSymbol>,
}

impl ImportSymbolExtractor {
    pub fn new() -> Self {
        Self {
            imported_symbols: HashMap::new(),
        }
    }
}

impl Default for ImportSymbolExtractor {
    fn default() -> Self {
        Self::new()
    }
}

impl Visit for ImportSymbolExtractor {
    fn visit_import_decl(&mut self, import: &ImportDecl) {
        let source = import.src.value.to_string();

        for specifier in &import.specifiers {
            match specifier {
                ImportSpecifier::Named(named) => {
                    let local_name = named.local.sym.to_string();
                    self.imported_symbols.insert(
                        local_name.to_string(),
                        ImportedSymbol {
                            local_name: local_name.clone(),
                            imported_name: local_name,
                            source: source.clone(),
                            kind: SymbolKind::Named,
                        },
                    );
                }
                ImportSpecifier::Default(default) => {
                    let local_name = default.local.sym.to_string();
                    self.imported_symbols.insert(
                        local_name.to_string(),
                        ImportedSymbol {
                            local_name: local_name.clone(),
                            imported_name: local_name,
                            source: source.clone(),
                            kind: SymbolKind::Default,
                        },
                    );
                }
                ImportSpecifier::Namespace(namespace) => {
                    let local_name = namespace.local.sym.to_string();
                    self.imported_symbols.insert(
                        local_name.to_string(),
                        ImportedSymbol {
                            local_name: local_name.clone(),
                            imported_name: local_name,
                            source: source.clone(),
                            kind: SymbolKind::Namespace,
                        },
                    );
                }
            }
        }
    }
}
