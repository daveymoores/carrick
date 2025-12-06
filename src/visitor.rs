extern crate swc_common;
extern crate swc_ecma_parser;

use std::{collections::HashMap, path::PathBuf};
use swc_ecma_ast::*;
use swc_ecma_visit::{Visit, VisitWith};

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

/// Extractor for function definitions with type annotations.
/// Used to extract handler functions for type resolution in the multi-agent pipeline.
#[derive(Debug)]
pub struct FunctionDefinitionExtractor {
    pub function_definitions: HashMap<String, FunctionDefinition>,
    current_file_path: PathBuf,
}

impl FunctionDefinitionExtractor {
    pub fn new(file_path: PathBuf) -> Self {
        Self {
            function_definitions: HashMap::new(),
            current_file_path: file_path,
        }
    }

    /// Extract function arguments with their type annotations
    fn extract_arguments(&self, params: &[Param]) -> Vec<FunctionArgument> {
        params
            .iter()
            .map(|param| {
                let name = match &param.pat {
                    Pat::Ident(ident) => ident.id.sym.to_string(),
                    Pat::Rest(rest) => match &*rest.arg {
                        Pat::Ident(ident) => format!("...{}", ident.id.sym),
                        _ => "...rest".to_string(),
                    },
                    _ => "param".to_string(),
                };

                // Get type annotation if present
                let type_ann = match &param.pat {
                    Pat::Ident(ident) => ident.type_ann.as_ref().map(|t| *t.clone()),
                    _ => None,
                };

                FunctionArgument { name, type_ann }
            })
            .collect()
    }

    /// Extract arguments from arrow function parameters
    fn extract_arrow_arguments(&self, params: &[Pat]) -> Vec<FunctionArgument> {
        params
            .iter()
            .map(|pat| {
                let (name, type_ann) = match pat {
                    Pat::Ident(ident) => (
                        ident.id.sym.to_string(),
                        ident.type_ann.as_ref().map(|t| *t.clone()),
                    ),
                    Pat::Rest(rest) => {
                        let rest_name = match &*rest.arg {
                            Pat::Ident(ident) => format!("...{}", ident.id.sym),
                            _ => "...rest".to_string(),
                        };
                        (rest_name, rest.type_ann.as_ref().map(|t| *t.clone()))
                    }
                    _ => ("param".to_string(), None),
                };

                FunctionArgument { name, type_ann }
            })
            .collect()
    }
}

impl Visit for FunctionDefinitionExtractor {
    fn visit_fn_decl(&mut self, fn_decl: &FnDecl) {
        let name = fn_decl.ident.sym.to_string();
        let arguments = self.extract_arguments(&fn_decl.function.params);

        self.function_definitions.insert(
            name.clone(),
            FunctionDefinition {
                name,
                file_path: self.current_file_path.clone(),
                node_type: FunctionNodeType::FunctionDeclaration(Box::new(fn_decl.clone())),
                arguments,
            },
        );

        // Continue visiting child nodes
        fn_decl.visit_children_with(self);
    }

    fn visit_var_declarator(&mut self, var_decl: &VarDeclarator) {
        // Handle: const myHandler = (req, res) => { ... }
        // Or: const myHandler = function(req, res) { ... }
        if let Pat::Ident(ident) = &var_decl.name {
            let name = ident.id.sym.to_string();

            if let Some(init) = &var_decl.init {
                match &**init {
                    Expr::Arrow(arrow) => {
                        let arguments = self.extract_arrow_arguments(&arrow.params);
                        self.function_definitions.insert(
                            name.clone(),
                            FunctionDefinition {
                                name,
                                file_path: self.current_file_path.clone(),
                                node_type: FunctionNodeType::ArrowFunction(Box::new(arrow.clone())),
                                arguments,
                            },
                        );
                    }
                    Expr::Fn(fn_expr) => {
                        let arguments = self.extract_arguments(&fn_expr.function.params);
                        self.function_definitions.insert(
                            name.clone(),
                            FunctionDefinition {
                                name,
                                file_path: self.current_file_path.clone(),
                                node_type: FunctionNodeType::FunctionExpression(Box::new(
                                    fn_expr.clone(),
                                )),
                                arguments,
                            },
                        );
                    }
                    _ => {}
                }
            }
        }

        // Continue visiting child nodes
        var_decl.visit_children_with(self);
    }
}
