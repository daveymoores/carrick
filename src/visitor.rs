extern crate swc_common;
extern crate swc_ecma_parser;
use std::{collections::HashMap, path::PathBuf};

use serde::Serialize;
use swc_ecma_ast::*;
use swc_ecma_visit::{Visit, VisitWith};

use crate::extractor::{CoreExtractor, RouteExtractor};
extern crate regex;

#[derive(Debug, Clone, Serialize, PartialEq)]
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
}

#[derive(Debug, Clone)]
pub struct FunctionDefinition {
    #[allow(dead_code)]
    pub name: String,
    #[allow(dead_code)]
    pub file_path: PathBuf,
    pub node_type: FunctionNodeType,
}

#[derive(Debug)]
pub struct DependencyVisitor {
    pub endpoints: Vec<(String, String, Json, Option<Json>)>, // (route, method, response fields, request fields)
    pub calls: Vec<(String, String, Json, Option<Json>)>,     // (route, method, expected fields)
    pub response_fields: HashMap<String, Json>,               // Function name -> expected fields
    // Maps function names to their source modules to find functon_definitions in second pass analysis
    pub imported_functions: HashMap<String, String>,
    pub current_file: PathBuf,
    // <Route, http_method, handler_name, source>
    pub imported_handlers: Vec<(String, String, String, String)>,
    // Store local function definitions found in this file, which may be
    // referenced from other files via imports
    pub function_definitions: HashMap<String, FunctionDefinition>,
}

impl DependencyVisitor {
    pub fn new(file_path: PathBuf) -> Self {
        Self {
            endpoints: Vec::new(),
            calls: Vec::new(),
            response_fields: HashMap::new(),
            imported_functions: HashMap::new(),
            current_file: file_path,
            imported_handlers: Vec::new(),
            function_definitions: HashMap::new(),
        }
    }
}

impl CoreExtractor for DependencyVisitor {}

impl RouteExtractor for DependencyVisitor {
    fn get_imported_functions(&self) -> &HashMap<String, String> {
        &self.imported_functions
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
    // Track ES Module exports
    fn visit_export_decl(&mut self, export: &ExportDecl) {
        // Handle different export types
        match &export.decl {
            // For "export const x = ..."
            Decl::Var(var_decl) => {
                for decl in &var_decl.decls {
                    if let Pat::Ident(ident) = &decl.name {
                        let exported_name = ident.id.sym.to_string();

                        // Check what's being exported
                        if let Some(init) = &decl.init {
                            match &**init {
                                Expr::Arrow(arrow) => {
                                    // Store the function definition for later analysis
                                    self.function_definitions.insert(
                                        exported_name.clone(),
                                        FunctionDefinition {
                                            name: exported_name.clone(),
                                            file_path: self.current_file.clone(),
                                            node_type: FunctionNodeType::ArrowFunction(Box::new(
                                                arrow.clone(),
                                            )),
                                        },
                                    );

                                    // You can still extract fields here if you want
                                    let fields = self.extract_fields_from_arrow(arrow);
                                    self.response_fields.insert(exported_name.clone(), fields);
                                }
                                // Regular function export: export const handler = function() {...}
                                Expr::Fn(fn_expr) => {
                                    // Store the function definition for later analysis
                                    self.function_definitions.insert(
                                        exported_name.clone(),
                                        FunctionDefinition {
                                            name: exported_name.clone(),
                                            file_path: self.current_file.clone(),
                                            node_type: FunctionNodeType::FunctionExpression(
                                                Box::new(fn_expr.clone()),
                                            ),
                                        },
                                    );

                                    // Extract response fields from the function
                                    let fields = self.extract_fields_from_function_expr(fn_expr);
                                    self.response_fields.insert(exported_name.clone(), fields);
                                }
                                // Other export type
                                _ => {}
                            }
                        }
                    }
                }
            }
            // For "export function x() {...}"
            Decl::Fn(fn_decl) => {
                let exported_name = fn_decl.ident.sym.to_string();

                self.function_definitions.insert(
                    exported_name.clone(),
                    FunctionDefinition {
                        name: exported_name.clone(),
                        file_path: self.current_file.clone(),
                        node_type: FunctionNodeType::FunctionDeclaration(Box::new(fn_decl.clone())),
                    },
                );

                // Extract fields
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
                    // Track this imported function
                    self.imported_functions.insert(local_name, source.clone());
                }
                ImportSpecifier::Default(default) => {
                    // Handle default imports: import func from './module'
                    let local_name = default.local.sym.to_string();
                    // Track this imported function
                    self.imported_functions.insert(local_name, source.clone());
                }
                ImportSpecifier::Namespace(namespace) => {
                    // Handle namespace imports: import * as mod from './module'
                    let _local_name = namespace.local.sym.to_string();
                    // Not tracking these directly as they're not individual functions
                }
            }
        }
    }

    fn visit_call_expr(&mut self, call: &CallExpr) {
        // Check the callee (what's being called)
        if let Callee::Expr(callee_expr) = &call.callee {
            if let Expr::Member(member) = &**callee_expr {
                // Check if the method is a valid HTTP method (GET, POST, etc.)
                if let Some(http_method) = self.is_express_route_method(member) {
                    // Check if it looks like an Express app or router
                    // This is still simple but better than checking just for "app"
                    if let Expr::Ident(obj_ident) = &*member.obj {
                        let var_name = obj_ident.sym.to_string();

                        // Accept common Express variable names
                        if ["app", "router", "route", "apiRouter", "r"].contains(&var_name.as_str())
                        {
                            if let Some((route, response_fields, request_fields)) =
                                self.extract_endpoint(call, &http_method)
                            {
                                self.endpoints.push((
                                    route,
                                    http_method,
                                    response_fields,
                                    request_fields,
                                ));
                            }
                        }
                    }
                }
            }
        }
        call.visit_children_with(self);
    }
}

impl DependencyVisitor {
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
}
