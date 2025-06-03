extern crate swc_common;
extern crate swc_ecma_parser;
use derivative::Derivative;

use std::{
    collections::{HashMap, HashSet},
    ops::Deref,
    path::PathBuf,
};
use swc_common::{SourceMap, sync::Lrc};
use swc_ecma_ast::*;
use swc_ecma_visit::{Visit, VisitWith};

use crate::{
    app_context::AppContext,
    extractor::{CoreExtractor, RouteExtractor},
    router_context::RouterContext,
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

#[derive(Debug, Clone)]
pub enum FunctionNodeType {
    ArrowFunction(Box<ArrowExpr>),
    FunctionDeclaration(Box<FnDecl>),
    FunctionExpression(Box<FnExpr>),
    // Used for deserialization when AST data is not available
    Placeholder,
}

impl Default for FunctionNodeType {
    fn default() -> Self {
        // This is used when deserializing in CI mode where AST is not available
        FunctionNodeType::Placeholder
    }
}

impl serde::Serialize for FunctionNodeType {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        match self {
            FunctionNodeType::ArrowFunction(_) => serializer.serialize_str("ArrowFunction"),
            FunctionNodeType::FunctionDeclaration(_) => serializer.serialize_str("FunctionDeclaration"),
            FunctionNodeType::FunctionExpression(_) => serializer.serialize_str("FunctionExpression"),
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

#[derive(Debug)]
pub enum SymbolKind {
    Named,
    Default,
    Namespace,
}

#[derive(Debug)]
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
    pub router_imports: HashSet<String>,
    pub imported_router_name: Option<String>,
    pub has_express_import: bool,
    pub express_import_name: Option<String>, // The local name for the express import
    // Track Express app instances
    pub express_apps: HashMap<String, AppContext>, // app_name -> AppContext
    // Track routers with their full context
    pub routers: HashMap<String, RouterContext>, // router_name -> RouterContext
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
            router_imports: HashSet::new(),
            imported_router_name,
            routers: HashMap::new(),
            express_import_name: None,
            express_apps: HashMap::new(),
            has_express_import: false,
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
        if tpl.exprs.len() > 0 {
            let mut result = String::new();

            // Iterate through quasis and expressions
            let mut quasi_iter = tpl.quasis.iter();

            // Always start with a quasi (could be empty)
            if let Some(first_quasi) = quasi_iter.next() {
                result.push_str(&first_quasi.raw.to_string());
            }

            // Then alternate between expressions and quasis
            for expr in &tpl.exprs {
                // Try to resolve the expression
                match &**expr {
                    Expr::Ident(ident) => {
                        if let Some(resolved) = self.resolve_variable(&ident.sym.to_string()) {
                            // For now, just handle string literals in template expressions
                            if let Expr::Lit(Lit::Str(str_lit)) = resolved {
                                result.push_str(&str_lit.value.to_string());
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
                    result.push_str(&quasi.raw.to_string());
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

        // Check if the import is from 'express'
        if source == "express" {
            self.has_express_import = true;

            // Track the local name for the express import
            for specifier in &import.specifiers {
                match specifier {
                    ImportSpecifier::Default(default) => {
                        // express is typically imported as default: import express from 'express'
                        let local_name = default.local.sym.to_string();
                        self.express_import_name = Some(local_name);
                    }
                    ImportSpecifier::Named(named) => {
                        // Sometimes specific things are imported from express like: import { Router } from 'express'
                        let local_name = named.local.sym.to_string();
                        if local_name == "Router" {
                            self.router_imports.insert(local_name);
                        }
                    }
                    ImportSpecifier::Namespace(namespace) => {
                        // Handle namespace imports: import * as expressLib from 'express'
                        let local_name = namespace.local.sym.to_string();
                        self.express_import_name = Some(local_name);
                    }
                }
            }
        }

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

                        // Call expression (could be a router/app creation or other call)
                        Expr::Call(call_expr) => {
                            // Your existing router and express app detection logic
                            let var_name_str = var_name.as_str();

                            // Check if this is a router creation
                            if self.is_router_creation(call_expr) {
                                // If this is the 'router' variable and we have an imported name,
                                // use the imported name instead
                                let router_name = if var_name_str == "router"
                                    && self.imported_router_name.is_some()
                                {
                                    self.imported_router_name.as_ref().unwrap().as_str()
                                } else {
                                    var_name_str
                                };

                                let prefixed_router_name = self.prefix_owner_type(router_name);

                                // Add the router to the `routers` map with an initial context
                                self.routers.insert(
                                    prefixed_router_name.clone(),
                                    RouterContext {
                                        name: prefixed_router_name.clone(),
                                    },
                                );

                                if router_name != var_name_str {
                                    println!(
                                        "Detected Router: {} (imported as: {})",
                                        var_name_str, router_name
                                    );
                                } else {
                                    println!("Detected Router: {}", var_name_str);
                                }
                            }
                            // Check if this is an express app creation
                            else if let Some(express_name) = &self.express_import_name {
                                if self.is_express_app_creation(call_expr, express_name) {
                                    let express_app_name = self.prefix_owner_type(var_name_str);
                                    // Add to express_apps map
                                    self.express_apps.insert(
                                        express_app_name.to_owned(),
                                        AppContext {
                                            name: express_app_name,
                                        },
                                    );

                                    println!("Detected Express app: {}", var_name_str);
                                }
                            }

                            // Check if the call contains callback functions
                            self.extract_callbacks_from_call(call_expr, &var_name);
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
        // Check the callee (what's being called)
        if let Callee::Expr(callee_expr) = &call.callee {
            if let Expr::Member(member) = &**callee_expr {
                // Check if the object is a known router or app
                if let Expr::Ident(obj_ident) = &*member.obj {
                    // object identifier being the app in app.get
                    let var_name = self.prefix_owner_type(obj_ident.sym.as_str());

                    // Check if the method is a valid HTTP method (GET, POST, etc.)
                    if let Some(http_method) = self.is_express_route_method(member) {
                        // Process route handler for this router/app
                        self.process_route_handler(&var_name, &http_method, call);
                    }
                    // Check for .use() method for mounting routers or middleware
                    else if self.is_use_method(member) {
                        // Check if this is an app or router
                        if self.express_apps.contains_key(&var_name) {
                            // This is an app.use() call
                            self.process_app_use(&var_name, &call.args);
                        } else {
                            // This is a router.use() call
                            self.process_router_use(&var_name, &call.args);
                        }
                    }
                }
            }
        }

        // Continue visiting children
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

    // New helper method to extract callbacks from call expressions
    fn extract_callbacks_from_call(&mut self, call: &CallExpr, context_name: &str) {
        // Check if this is a routing method call like app.get('/route', function...)
        if let Callee::Expr(callee_expr) = &call.callee {
            if let Expr::Member(member) = &**callee_expr {
                // Check if the method could be a route handler (get, post, etc.)
                if let MemberProp::Ident(method_ident) = &member.prop {
                    let method_name = method_ident.sym.to_string().to_lowercase();

                    if ["get", "post", "put", "delete", "patch"].contains(&method_name.as_str()) {
                        // This is likely a route definition, check for callback in the second arg
                        if call.args.len() >= 2 {
                            self.extract_function_from_arg(
                                &call.args[1].expr,
                                &format!("{}_{}_callback", context_name, method_name),
                            );
                        }
                    }
                }
            }
        }

        // Check each argument for function expressions (callbacks)
        for (i, arg) in call.args.iter().enumerate() {
            self.extract_function_from_arg(&arg.expr, &format!("{}_{}", context_name, i));
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

    fn prefix_owner_type(&self, name: &str) -> String {
        format!("{}:{}", self.repo_prefix, name)
    }

    fn get_owner_type(&self, name: &str) -> String {
        name.split(":")
            .filter(|x| !x.is_empty())
            .last()
            .unwrap()
            .to_string()
    }

    fn is_express_route_method(&self, member: &MemberExpr) -> Option<String> {
        // Get the method name (the property part of the member expression)
        if let MemberProp::Ident(method_ident) = &member.prop {
            let method_name = method_ident.sym.to_string().to_lowercase();

            // Check if it's a standard HTTP method
            if ["get", "post", "put", "delete", "patch"].contains(&method_name.as_str()) {
                return Some(method_name.to_uppercase());
            }
        }
        None
    }

    fn is_express_app_creation(&self, call_expr: &CallExpr, express_name: &str) -> bool {
        match &call_expr.callee {
            // Check for express()
            Callee::Expr(expr) => {
                if let Expr::Ident(ident) = &expr.deref() {
                    if ident.sym == *express_name {
                        return true;
                    }
                }
            }
            _ => {}
        }
        false
    }

    fn is_router_creation(&self, call_expr: &CallExpr) -> bool {
        match &call_expr.callee {
            // Check for express.Router()
            Callee::Expr(expr) => {
                if let Expr::Member(member) = &**expr {
                    if let (Expr::Ident(obj), MemberProp::Ident(prop)) =
                        (&*member.obj, &member.prop)
                    {
                        if obj.sym == "express" && prop.sym == "Router" {
                            return true; // Detected express.Router()
                        }
                    }
                }

                // Check for Router() after import { Router } from 'express'
                if let Expr::Ident(callee) = &expr.deref() {
                    if self.router_imports.contains(&callee.sym.to_string()) {
                        return true; // Detected Router()
                    }
                }
            }
            _ => {}
        }

        false
    }

    fn is_use_method(&self, member: &MemberExpr) -> bool {
        if let MemberProp::Ident(method_ident) = &member.prop {
            if method_ident.sym == *"use" {
                return true;
            }
        }
        false
    }

    fn get_path_prefix_from_use_call(&self, args: &[ExprOrSpread]) -> (String, usize) {
        // Determine if the first argument is a path or a middleware/router
        let (path_prefix, target_arg_idx) = if args.len() >= 2 {
            if let Some(path) = self.extract_string_from_expr(&args[0].expr) {
                (path, 1) // First arg is path, second is router/app
            } else {
                ("/".to_string(), 0) // First arg is middleware or router/app
            }
        } else {
            ("/".to_string(), 0) // Only one arg, must be middleware or router/app
        };

        // Normalize path prefix
        let path_prefix = if !path_prefix.starts_with('/') {
            format!("/{}", path_prefix)
        } else {
            path_prefix
        };

        (path_prefix, target_arg_idx)
    }

    // Process app.use() - mounting routers or other apps
    fn process_app_use(&mut self, app_name: &str, args: &[ExprOrSpread]) {
        if args.is_empty() {
            return;
        }

        let (path_prefix, target_arg_idx) = self.get_path_prefix_from_use_call(&args);

        // Check if the argument at target_arg_idx is a router or middleware reference
        if target_arg_idx < args.len() {
            if let Expr::Ident(target_ident) = &*args[target_arg_idx].expr {
                let target_name = target_ident.sym.as_str();
                let prefixed_target_name = self.prefix_owner_type(target_name);

                // Check if this is an imported symbol
                if let Some(imported) = self.imported_symbols.get(target_name) {
                    println!(
                        "App use: {}({}) -> {} (imported from {})",
                        app_name, path_prefix, target_name, imported.source
                    );

                    // This is the important part - record that we need to
                    // analyze the module this router was imported from
                    // The caller would need to handle this information
                } else {
                    println!("App use: {}({}) -> {}", app_name, path_prefix, target_name);
                }

                // For ALL identifiers used with app.use, assume they could be routers
                // and create the mount relationship
                self.mounts.push(Mount {
                    parent: OwnerType::App(app_name.to_string()),
                    child: OwnerType::Router(prefixed_target_name.clone()),
                    prefix: path_prefix,
                });

                // Add the router to our tracking if it's not already there
                // This is important both for locally defined routers AND imported ones
                if !self.routers.contains_key(&prefixed_target_name) {
                    self.routers.insert(
                        prefixed_target_name,
                        RouterContext {
                            name: target_name.to_string(),
                        },
                    );

                    // If this is an import, log it specially
                    if self.imported_symbols.contains_key(target_name) {
                        println!("Detected use of imported router: {}", target_name);
                    }
                }
            }
        }
    }

    // Process router.use() - mounting other routers
    fn process_router_use(&mut self, parent_router_name: &str, args: &[ExprOrSpread]) {
        if args.is_empty() {
            return;
        }

        let (path_prefix, target_arg_idx) = self.get_path_prefix_from_use_call(&args);

        // Check if the argument at target_arg_idx is a router reference
        if target_arg_idx < args.len() {
            if let Expr::Ident(target_ident) = &*args[target_arg_idx].expr {
                let target_name = target_ident.sym.as_str();
                let prefixed_target_name = self.prefix_owner_type(target_name);

                println!(
                    "Router use: {}({}) -> {}",
                    parent_router_name, path_prefix, target_name
                );

                // Create mount relationship
                self.mounts.push(Mount {
                    parent: OwnerType::Router(parent_router_name.to_string()),
                    child: OwnerType::Router(prefixed_target_name.clone()),
                    prefix: path_prefix,
                });

                // Add the router to our tracking if it's not already there
                if !self.routers.contains_key(&prefixed_target_name) {
                    self.routers.insert(
                        prefixed_target_name,
                        RouterContext {
                            name: target_name.to_string(),
                        },
                    );

                    // If this is an import, log it specially
                    if self.imported_symbols.contains_key(target_name) {
                        println!("Detected use of imported router: {}", target_name);
                    }
                }
            }
        }
    }

    // Process route handlers on routers
    fn process_route_handler(&mut self, var_name: &str, http_method: &str, call: &CallExpr) {
        // If this is the generic "router" variable and we have an imported name
        let effective_name = if var_name == self.prefix_owner_type("router")
            && self.imported_router_name.is_some()
        {
            self.prefix_owner_type(self.imported_router_name.as_ref().unwrap())
        } else {
            var_name.to_string()
        };

        // Find whether this is an app or router
        let owner = match self.express_apps.get(&effective_name) {
            Some(_) => OwnerType::App(effective_name.to_string()),
            None => OwnerType::Router(effective_name.to_string()),
        };

        if let Some(endpoint_data) = self.extract_endpoint(call, http_method) {
            let (route, response_fields, request_fields, handler_name) = endpoint_data;

            if let Some(second_arg) = call.args.get(1) {
                match &*second_arg.expr {
                    Expr::Arrow(arrow) => {
                        // Store arrow function definition
                        let arguments = self.extract_function_arguments_from_pats(&arrow.params);
                        self.function_definitions.insert(
                            handler_name.clone(),
                            FunctionDefinition {
                                name: handler_name.clone(),
                                file_path: self.current_file.clone(),
                                node_type: FunctionNodeType::ArrowFunction(Box::new(arrow.clone())),
                                arguments,
                            },
                        );
                    }
                    Expr::Fn(fn_expr) => {
                        // Store function expression definition
                        let arguments =
                            self.extract_function_arguments_from_params(&fn_expr.function.params);
                        self.function_definitions.insert(
                            handler_name.clone(),
                            FunctionDefinition {
                                name: handler_name.clone(),
                                file_path: self.current_file.clone(),
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

            // Store the endpoint with its initial path and type information
            self.endpoints.push(Endpoint {
                owner,
                route: route.clone(),
                method: http_method.to_string(),
                response: response_fields.clone(),
                request: request_fields.clone(),
                request_type: None,
                response_type: None,
                handler_file: self.current_file.clone(),
                handler_name,
            });

            println!(
                "Detected endpoint: {} {} on {}",
                http_method,
                route,
                self.get_owner_type(&effective_name),
            );
        }
    }
}
