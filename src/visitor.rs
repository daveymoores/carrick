extern crate swc_common;
extern crate swc_ecma_parser;
use std::{
    collections::{HashMap, HashSet},
    path::PathBuf,
};

use serde::Serialize;
use swc_ecma_ast::*;
use swc_ecma_visit::{Visit, VisitWith};

use crate::{
    extractor::{CoreExtractor, RouteExtractor},
    router_context::RouterContext,
};
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
    pub router_imports: HashSet<String>,
    pub routers: HashMap<String, RouterContext>,
    pub has_express_import: bool,
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
            router_imports: HashSet::new(),
            routers: HashMap::new(),
            has_express_import: false,
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

        // Check if the import is from 'express'
        if source == "express" {
            self.has_express_import = true;

            // Check for named imports like { Router } or { Router as MyRouter }
            for specifier in &import.specifiers {
                if let ImportSpecifier::Named(named) = specifier {
                    let local_name = named.local.sym.to_string(); // The name used in the file
                    self.router_imports.insert(local_name); // Track the imported name
                }
            }
        }

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

    fn visit_var_decl(&mut self, var_decl: &VarDecl) {
        for decl in &var_decl.decls {
            if let Some(init) = &decl.init {
                // Check if the initializer is a call expression (e.g., Router() or express.Router())
                if let Expr::Call(call_expr) = &**init {
                    // Check if this is a router creation
                    if self.is_router_creation(call_expr) {
                        // Get the variable name being declared
                        if let Pat::Ident(ident) = &decl.name {
                            let router_name = ident.id.sym.to_string();

                            // Add the router to the `routers` map with an initial context
                            self.routers.insert(
                                router_name.clone(),
                                RouterContext {
                                    prefix: "".to_string(), // No prefix yet
                                    parent: None,           // No parent yet
                                },
                            );
                        }
                    }
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
                    // Check if the object is a known router or app
                    if let Expr::Ident(obj_ident) = &*member.obj {
                        let var_name = obj_ident.sym.to_string();

                        // Extract endpoint data first (immutable borrow)
                        let endpoint_data = self.extract_endpoint(call, &http_method);

                        // Handle routes defined on routers
                        if let Some(router_ctx) = self.routers.get(&var_name) {
                            if let Some((route, response_fields, request_fields)) = endpoint_data {
                                // Resolve the full path using the router context
                                let full_path = router_ctx.resolve_full_path(&route);

                                // Now modify self (mutable borrow)
                                self.endpoints.push((
                                    full_path.clone(),
                                    http_method.clone(),
                                    response_fields,
                                    request_fields,
                                ));

                                println!(
                                    "Detected router endpoint: {} {}",
                                    &http_method, &full_path
                                );
                            }
                        }
                        // Handle routes defined on the main app instance
                        else if var_name == "app" {
                            if let Some((route, response_fields, request_fields)) = endpoint_data {
                                // Now modify self (mutable borrow)
                                self.endpoints.push((
                                    route.clone(),
                                    http_method.clone(),
                                    response_fields,
                                    request_fields,
                                ));

                                println!("Detected app endpoint: {} {}", &http_method, &route);
                            }
                        }
                    }
                }

                // Check for router.use() to handle nested routers
                if self.is_router_use_method(member) {
                    if let Expr::Ident(obj_ident) = &*member.obj {
                        let parent_router_name = obj_ident.sym.to_string();

                        // Process router.use('/prefix', childRouter)
                        self.process_router_use(parent_router_name, &call.args);
                    }
                }
            }
        }

        // Continue visiting children
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

    fn is_router_creation(&self, call_expr: &CallExpr) -> bool {
        match &call_expr.callee {
            // Check for express.Router()
            Callee::Expr(expr) => {
                if let Expr::Member(member) = &**expr {
                    if let (Expr::Ident(obj), MemberProp::Ident(prop)) =
                        (&*member.obj, &member.prop)
                    {
                        if obj.sym == *"express" && prop.sym == *"Router" {
                            return true; // Detected express.Router()
                        }
                    }
                }

                // Check for Router() after import { Router } from 'express'
                if let Expr::Ident(callee) = &**expr {
                    if self.router_imports.contains(&callee.sym.to_string()) {
                        return true; // Detected Router()
                    }
                }
            }
            _ => {}
        }

        false
    }

    fn is_router_use_method(&self, member: &MemberExpr) -> bool {
        if let MemberProp::Ident(method_ident) = &member.prop {
            if method_ident.sym == *"use" {
                return true;
            }
        }
        false
    }

    fn extract_string_from_expr(&self, expr: &Expr) -> Option<String> {
        match expr {
            Expr::Lit(Lit::Str(str_lit)) => Some(str_lit.value.to_string()),
            Expr::Tpl(tpl) if tpl.exprs.is_empty() => {
                // Handle simple template literals with no expressions
                Some(tpl.quasis.iter().map(|q| q.raw.to_string()).collect())
            }
            _ => None,
        }
    }

    fn process_router_use(&mut self, parent_router_name: String, args: &[ExprOrSpread]) {
        // Determine if the first argument is a path or a middleware/router
        let (path_prefix, router_arg_idx) = if args.len() >= 2 {
            if let Some(path) = self.extract_string_from_expr(&args[0].expr) {
                (path, 1) // First arg is path, second is router
            } else {
                ("".to_string(), 0) // First arg is middleware or router
            }
        } else if args.len() == 1 {
            ("".to_string(), 0) // Only one arg, must be middleware or router
        } else {
            return; // Not enough arguments
        };

        // Check if the argument at router_arg_idx is a router reference
        if let Expr::Ident(router_ident) = &*args[router_arg_idx].expr {
            let child_router_name = router_ident.sym.to_string();

            // Extract the parent context (immutable borrow)
            let parent_context = self.routers.get(&parent_router_name).cloned();

            // Remove the child router from the HashMap temporarily (mutable borrow)
            if let Some(mut child_ctx) = self.routers.remove(&child_router_name) {
                // Update the child router's context
                child_ctx.prefix = path_prefix.clone();
                child_ctx.parent = parent_context.map(Box::new);

                // Insert the updated child router back into the HashMap
                self.routers.insert(child_router_name.clone(), child_ctx);

                println!(
                    "Updated router context: {} -> prefix: {}, parent: {}",
                    child_router_name, path_prefix, parent_router_name
                );
            }
        }
    }
}
