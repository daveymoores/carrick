extern crate swc_common;
extern crate swc_ecma_parser;
use derivative::Derivative;

use std::{
    collections::HashMap,
    path::PathBuf,
};
use swc_common::{SourceMap, sync::Lrc};
use swc_ecma_ast::*;
use swc_ecma_visit::{Visit, VisitWith};

use crate::{
    extractor::{CoreExtractor, RouteExtractor},
};
extern crate regex;

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
    Object(Box<HashMap<String, Json>>),
}

#[derive(Debug, Clone, Default)]
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
pub struct Endpoint {
    pub owner: OwnerType,
    pub route: String,
    pub method: String,
    pub response: Json,
    pub request: Option<Json>,
    pub response_type: Option<TypeReference>,
    pub request_type: Option<TypeReference>,
    #[allow(dead_code)]
    pub handler_file: PathBuf,
    pub handler_name: String,
}

#[derive(Debug, Clone)]
pub struct Call {
    pub route: String,
    pub method: String,
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
    pub imported_name: String,
    pub source: String,
    pub kind: SymbolKind,
}

#[derive(Derivative)]
#[derivative(Debug)]
pub struct DependencyVisitor {
    pub repo_prefix: String,
    pub endpoints: Vec<Endpoint>, // (route, method, response fields, request fields)
    pub calls: Vec<Call>,         // (route, method, expected fields)
    pub mounts: Vec<Mount>,
    pub response_fields: HashMap<String, Json>, // Function name -> expected fields
    // Maps function names to their source modules to find functon_definitions in second pass analysis
    pub current_file: PathBuf,
    // <Route, http_method, handler_name, source>
    pub imported_handlers: Vec<(String, String, String, String)>,
    // Store local function definitions found in this file, which may be
    // referenced from other files via imports
    pub function_definitions: HashMap<String, FunctionDefinition>,
    pub imported_router_name: Option<String>,
    pub exported_variables: HashMap<String, Expr>,
    pub imported_symbols: HashMap<String, ImportedSymbol>,
    pub variable_values: HashMap<String, Expr>,
    #[derivative(Debug = "ignore")]
    pub source_map: Lrc<SourceMap>,
}

impl DependencyVisitor {
    pub fn new(
        file_path: PathBuf,
        repo_prefix: &str,
        imported_router_name: Option<String>,
        cm: Lrc<SourceMap>,
    ) -> Self {
        Self {
            repo_prefix: repo_prefix.to_owned(),
            endpoints: Vec::new(),
            calls: Vec::new(),
            mounts: Vec::new(),
            response_fields: HashMap::new(),
            current_file: file_path,
            imported_handlers: Vec::new(),
            function_definitions: HashMap::new(),
            imported_router_name,
            exported_variables: HashMap::new(),
            imported_symbols: HashMap::new(),
            variable_values: HashMap::new(),
            source_map: cm,
        }
    }
}

impl CoreExtractor for DependencyVisitor {
    fn get_source_map(&self) -> &Lrc<SourceMap> {
        &self.source_map
    }
    fn resolve_variable(&self, name: &str) -> Option<&Expr> {
        // First check if it's a local variable
        if let Some(expr) = self.variable_values.get(name) {
            return Some(expr);
        }

        // If not local, check if it's an imported symbol
        if let Some(imported) = self.imported_symbols.get(name) {
            // Check if we have the exported value from the source module
            if let Some(expr) = self.exported_variables.get(&imported.imported_name) {
                return Some(expr);
            }
        }

        None
    }
}

impl RouteExtractor for DependencyVisitor {
    // Gets the name of the handler used on a router (possibly another router)
    fn get_route_handler_name(&self, expr: &Expr) -> Option<String> {
        match expr {
            Expr::Ident(ident) => Some(ident.sym.to_string()),
            Expr::Member(member) => {
                // Handle cases like module.handler
                if let (Expr::Ident(obj), MemberProp::Ident(prop)) = (&*member.obj, &member.prop) {
                    let obj_name = obj.sym.to_string();
                    let prop_name = prop.sym.to_string();

                    // Check if this is an imported namespace
                    if let Some(imported) = self.imported_symbols.get(&obj_name) {
                        if let SymbolKind::Namespace = imported.kind {
                            // Return the qualified name
                            return Some(format!("{}:{}", imported.source, prop_name));
                        }
                    }
                }
                None
            }
            // Try to resolve the expression to a handler name
            _ => None,
        }
    }

    fn resolve_template_string(&self, tpl: &Tpl) -> Option<String> {
        // Only handle simple cases for now
        if !tpl.exprs.is_empty() {
            let mut result = String::new();

            // Iterate through quasis and expressions
            let mut quasi_iter = tpl.quasis.iter();

            // Always start with a quasi (could be empty)
            if let Some(first_quasi) = quasi_iter.next() {
                result.push_str(first_quasi.raw.as_ref());
            }

            // Then alternate between expressions and quasis
            for expr in &tpl.exprs {
                // Try to resolve the expression
                match &**expr {
                    Expr::Ident(ident) => {
                        if let Some(resolved) = self.resolve_variable(ident.sym.as_ref()) {
                            // For now, just handle string literals in template expressions
                            if let Expr::Lit(Lit::Str(str_lit)) = resolved {
                                result.push_str(str_lit.value.as_ref());
                            } else {
                                // Can't fully resolve - return None
                                return None;
                            }
                        } else {
                            // Can't resolve this variable
                            return None;
                        }
                    }
                    // Add more expression types as needed
                    _ => return None,
                }

                // Add the next quasi if available
                if let Some(quasi) = quasi_iter.next() {
                    result.push_str(quasi.raw.as_ref());
                }
            }

            return Some(result);
        }

        // Simple template with no expressions
        Some(tpl.quasis.iter().map(|q| q.raw.to_string()).collect())
    }

    fn get_imported_symbols(&self) -> &HashMap<String, ImportedSymbol> {
        &self.imported_symbols
    }

    fn get_response_fields(&self) -> &HashMap<String, Json> {
        &self.response_fields
    }

    fn add_imported_handler(
        &mut self,
        route: String,
        method: String,
        handler: String,
        source: String,
    ) {
        self.imported_handlers
            .push((route, method, handler, source));
    }
}

impl Visit for DependencyVisitor {
    fn visit_fn_decl(&mut self, fn_decl: &FnDecl) {
        // Get the function name
        let fn_name = fn_decl.ident.sym.to_string();

        // Extract arguments
        let arguments = self.extract_function_arguments_from_params(&fn_decl.function.params);

        // Store in function_definitions
        self.function_definitions.insert(
            fn_name.clone(),
            FunctionDefinition {
                name: fn_name.clone(),
                file_path: self.current_file.clone(),
                node_type: FunctionNodeType::FunctionDeclaration(Box::new(fn_decl.clone())),
                arguments,
            },
        );

        // Extract fields as you do for exported functions
        let fields = self.extract_fields_from_function_decl(fn_decl);
        self.response_fields.insert(fn_name.clone(), fields);

        // Visit the function body
        fn_decl.function.visit_children_with(self);
    }

    // Track ES Module exports
    fn visit_export_decl(&mut self, export: &ExportDecl) {
        match &export.decl {
            // For "export const x = ..."
            Decl::Var(var_decl) => {
                for decl in &var_decl.decls {
                    if let Pat::Ident(ident) = &decl.name {
                        let exported_name = ident.id.sym.to_string();
                        if let Some(init) = &decl.init {
                            // Always store the initializer for variable exports
                            self.exported_variables
                                .insert(exported_name.clone(), *init.clone());

                            // If it's a function, also store in function_definitions/response_fields
                            match &**init {
                                Expr::Arrow(arrow) => {
                                    let arguments =
                                        self.extract_function_arguments_from_pats(&arrow.params);
                                    self.function_definitions.insert(
                                        exported_name.clone(),
                                        FunctionDefinition {
                                            name: exported_name.clone(),
                                            file_path: self.current_file.clone(),
                                            node_type: FunctionNodeType::ArrowFunction(Box::new(
                                                arrow.clone(),
                                            )),
                                            arguments,
                                        },
                                    );
                                    let fields = self.extract_fields_from_arrow(arrow);
                                    self.response_fields.insert(exported_name.clone(), fields);
                                }
                                // Regular function export: export const handler = function() {...}
                                Expr::Fn(fn_expr) => {
                                    let arguments = self.extract_function_arguments_from_params(
                                        &fn_expr.function.params,
                                    );
                                    self.function_definitions.insert(
                                        exported_name.clone(),
                                        FunctionDefinition {
                                            name: exported_name.clone(),
                                            file_path: self.current_file.clone(),
                                            node_type: FunctionNodeType::FunctionExpression(
                                                Box::new(fn_expr.clone()),
                                            ),
                                            arguments,
                                        },
                                    );
                                    let fields = self.extract_fields_from_function_expr(fn_expr);
                                    self.response_fields.insert(exported_name.clone(), fields);
                                }
                                _ => {}
                            }
                        }
                    }
                }
            }
            // For "export function x() {...}"
            Decl::Fn(fn_decl) => {
                let arguments =
                    self.extract_function_arguments_from_params(&fn_decl.function.params);
                let exported_name = fn_decl.ident.sym.to_string();
                self.function_definitions.insert(
                    exported_name.clone(),
                    FunctionDefinition {
                        name: exported_name.clone(),
                        file_path: self.current_file.clone(),
                        node_type: FunctionNodeType::FunctionDeclaration(Box::new(fn_decl.clone())),
                        arguments,
                    },
                );
                let fields = self.extract_fields_from_function_decl(fn_decl);
                self.response_fields.insert(exported_name.clone(), fields);
            }
            _ => {}
        }
    }

    // Track ES Module imports
    fn visit_import_decl(&mut self, import: &ImportDecl) {
        let source = import.src.value.to_string();

        for specifier in &import.specifiers {
            match specifier {
                ImportSpecifier::Named(named) => {
                    // Handle named imports: import { func } from './module'
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
                    // Handle default imports: import func from './module'
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
                    // Handle namespace imports: import * as mod from './module'
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

    fn visit_var_decl(&mut self, var_decl: &VarDecl) {
        for decl in &var_decl.decls {
            if let Some(init) = &decl.init {
                if let Pat::Ident(ident) = &decl.name {
                    let var_name = ident.id.sym.to_string();

                    // Store variable value for later reference
                    self.variable_values.insert(var_name.clone(), *init.clone());

                    // Now check different expression types
                    match &**init {
                        // Arrow function
                        Expr::Arrow(arrow) => {
                            let arguments =
                                self.extract_function_arguments_from_pats(&arrow.params);
                            self.function_definitions.insert(
                                var_name.clone(),
                                FunctionDefinition {
                                    name: var_name.clone(),
                                    file_path: self.current_file.clone(),
                                    node_type: FunctionNodeType::ArrowFunction(Box::new(
                                        arrow.clone(),
                                    )),
                                    arguments,
                                },
                            );
                            let fields = self.extract_fields_from_arrow(arrow);
                            self.response_fields.insert(var_name.clone(), fields);
                        }

                        // Function expression
                        Expr::Fn(fn_expr) => {
                            let arguments = self
                                .extract_function_arguments_from_params(&fn_expr.function.params);
                            self.function_definitions.insert(
                                var_name.clone(),
                                FunctionDefinition {
                                    name: var_name.clone(),
                                    file_path: self.current_file.clone(),
                                    node_type: FunctionNodeType::FunctionExpression(Box::new(
                                        fn_expr.clone(),
                                    )),
                                    arguments,
                                },
                            );
                            let fields = self.extract_fields_from_function_expr(fn_expr);
                            self.response_fields.insert(var_name.clone(), fields);
                        }

                        // Call expression (could contain callback functions)
                        Expr::Call(call_expr) => {
                            // Extract any function definitions from this call
                            self.extract_functions_from_call_args(call_expr);
                        }

                        // Other expression types don't need special handling for function detection
                        _ => {}
                    }
                }
            }
        }

        // Make sure to visit children to catch nested definitions
        var_decl.visit_children_with(self);
    }

    fn visit_call_expr(&mut self, call: &CallExpr) {
        // Extract all function definitions from arguments (including anonymous functions)
        self.extract_functions_from_call_args(call);
        
        // Continue visiting children to catch nested calls
        call.visit_children_with(self);
    }
}

impl DependencyVisitor {
    fn extract_function_arguments_from_pats(&self, params: &[Pat]) -> Vec<FunctionArgument> {
        params
            .iter()
            .filter_map(|pat| match pat {
                Pat::Ident(ident) => Some(FunctionArgument {
                    name: ident.id.sym.to_string(),
                    type_ann: ident.type_ann.clone().map(|b| *b),
                }),
                _ => None,
            })
            .collect()
    }

    fn extract_function_arguments_from_params(&self, params: &[Param]) -> Vec<FunctionArgument> {
        params
            .iter()
            .filter_map(|param| match &param.pat {
                Pat::Ident(ident) => Some(FunctionArgument {
                    name: ident.id.sym.to_string(),
                    type_ann: ident.type_ann.clone().map(|b| *b),
                }),
                _ => None,
            })
            .collect()
    }

    // Extract all function definitions from call arguments (framework-agnostic)
    fn extract_functions_from_call_args(&mut self, call: &CallExpr) {
        // Generate a unique context name for this call site
        let call_context = self.generate_call_context_name(call);
        
        // Check each argument for function expressions (callbacks, handlers, etc.)
        for (i, arg) in call.args.iter().enumerate() {
            self.extract_function_from_arg(&arg.expr, &format!("{}_{}", call_context, i));
        }
    }
    
    // Generate a context name for a call site based on its location and structure
    fn generate_call_context_name(&self, call: &CallExpr) -> String {
        match &call.callee {
            Callee::Expr(callee_expr) => {
                match &**callee_expr {
                    Expr::Member(member) => {
                        // For member calls like obj.method(), use obj_method format
                        let obj_name = self.extract_object_name(&member.obj).unwrap_or("unknown".to_string());
                        let method_name = self.extract_property_name(&member.prop).unwrap_or("unknown".to_string());
                        format!("{}_{}", obj_name, method_name)
                    }
                    Expr::Ident(ident) => {
                        // For direct function calls like func(), use the function name
                        ident.sym.to_string()
                    }
                    _ => "unknown_call".to_string(),
                }
            }
            _ => "unknown_call".to_string(),
        }
    }
    
    // Extract object name from an expression
    fn extract_object_name(&self, expr: &Expr) -> Option<String> {
        match expr {
            Expr::Ident(ident) => Some(ident.sym.to_string()),
            Expr::Member(member) => {
                // For nested member access like app.router.use, get the full path
                let obj = self.extract_object_name(&member.obj).unwrap_or("unknown".to_string());
                let prop = self.extract_property_name(&member.prop).unwrap_or("unknown".to_string());
                Some(format!("{}.{}", obj, prop))
            }
            _ => None,
        }
    }
    
    // Extract property name from a member property
    fn extract_property_name(&self, prop: &MemberProp) -> Option<String> {
        match prop {
            MemberProp::Ident(ident) => Some(ident.sym.to_string()),
            MemberProp::Computed(computed) => {
                // For computed properties like obj["method"], try to extract the string
                self.extract_string_from_expr(&computed.expr)
            }
            _ => None,
        }
    }

    // Helper to extract function from an expression argument
    fn extract_function_from_arg(&mut self, expr: &Expr, default_name: &str) {
        match expr {
            Expr::Arrow(arrow) => {
                let arguments = self.extract_function_arguments_from_pats(&arrow.params);
                self.function_definitions.insert(
                    default_name.to_string(),
                    FunctionDefinition {
                        name: default_name.to_string(),
                        file_path: self.current_file.clone(),
                        node_type: FunctionNodeType::ArrowFunction(Box::new(arrow.clone())),
                        arguments,
                    },
                );
            }
            Expr::Fn(fn_expr) => {
                let arguments =
                    self.extract_function_arguments_from_params(&fn_expr.function.params);
                let fn_name = if let Some(ident) = &fn_expr.ident {
                    ident.sym.to_string()
                } else {
                    default_name.to_string()
                };

                self.function_definitions.insert(
                    fn_name.clone(),
                    FunctionDefinition {
                        name: fn_name.clone(),
                        file_path: self.current_file.clone(),
                        node_type: FunctionNodeType::FunctionExpression(Box::new(fn_expr.clone())),
                        arguments,
                    },
                );
            }
            _ => {}
        }
    }


    // Extract string literal from expressions like "string", `template`, etc.
    fn extract_string_from_expr(&self, expr: &Expr) -> Option<String> {
        match expr {
            Expr::Lit(Lit::Str(s)) => Some(s.value.to_string()),
            Expr::Tpl(tpl) if tpl.quasis.len() == 1 && tpl.exprs.is_empty() => {
                // Simple template literal with no expressions
                Some(tpl.quasis[0].raw.to_string())
            }
            _ => None,
        }
    }
}
