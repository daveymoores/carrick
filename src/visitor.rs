extern crate swc_common;
extern crate swc_ecma_parser;
use std::{
    collections::{HashMap, HashSet},
    ops::Deref,
    path::PathBuf,
};

use serde::Serialize;
use swc_ecma_ast::*;
use swc_ecma_visit::{Visit, VisitWith};

use crate::{
    app_context::AppContext,
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
    pub has_express_import: bool,
    pub express_import_name: Option<String>, // The local name for the express import
    // Track Express app instances
    pub express_apps: HashMap<String, AppContext>, // app_name -> AppContext
    // Track routers with their full context
    pub routers: HashMap<String, RouterContext>, // router_name -> RouterContext
    pub endpoint_owners: HashMap<(String, String), String>, // (path, method) -> owner_name
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
            express_import_name: None,
            express_apps: HashMap::new(),
            has_express_import: false,
            endpoint_owners: HashMap::new(),
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
                // Check if the initializer is a call expression
                if let Expr::Call(call_expr) = &**init {
                    if let Pat::Ident(ident) = &decl.name {
                        let var_name = ident.id.sym.to_string().clone();

                        // Check if this is a router creation
                        if self.is_router_creation(call_expr) {
                            // Add the router to the `routers` map with an initial context
                            self.routers.insert(
                                var_name.clone(),
                                RouterContext {
                                    name: var_name.clone(),
                                    prefix: "".to_string(),
                                    parent_app: None,
                                    parent_router: None,
                                },
                            );

                            println!("Detected Router: {}", var_name);
                        }
                        // Check if this is an express app creation
                        else if let Some(express_name) = &self.express_import_name {
                            if self.is_express_app_creation(call_expr, express_name) {
                                // Add to express_apps map
                                self.express_apps.insert(
                                    var_name.clone(),
                                    AppContext {
                                        name: var_name.clone(),
                                        mount_path: "".to_string(),
                                        parent_app: None,
                                    },
                                );

                                println!("Detected Express app: {}", var_name);
                            }
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
                // Check if the object is a known router or app
                if let Expr::Ident(obj_ident) = &*member.obj {
                    // object identifier being the app in app.get
                    let var_name = obj_ident.sym.to_string();

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

    // Process app.use() - mounting routers or other apps
    fn process_app_use(&mut self, app_name: &str, args: &[ExprOrSpread]) {
        if args.is_empty() {
            return;
        }

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

        // Check if the argument at target_arg_idx is a router or app reference
        if target_arg_idx < args.len() {
            if let Expr::Ident(target_ident) = &*args[target_arg_idx].expr {
                let target_name = target_ident.sym.to_string();

                println!("App use: {}({}) -> {}", app_name, path_prefix, target_name);

                // Get the parent app context
                let parent_app = self.express_apps.get(app_name).cloned().map(Box::new);

                // Check if we're mounting another app
                if let Some(mounted_app) = self.express_apps.get_mut(&target_name) {
                    // Update the mounted app's context
                    mounted_app.mount_path = path_prefix;
                    mounted_app.parent_app = parent_app;
                    self.update_endpoints_for_owner(&target_name);

                    println!(
                        "Updated app context: {} with parent: {}",
                        target_name, app_name
                    );
                }
                // Check if we're mounting a router
                else if let Some(router) = self.routers.get_mut(&target_name) {
                    // Update router's context
                    router.prefix = path_prefix;
                    router.parent_app = parent_app;
                    self.update_endpoints_for_owner(&target_name);

                    println!(
                        "Updated router: {} mounted on app: {}",
                        target_name, app_name
                    );
                }
            }
        }
    }

    // Process router.use() - mounting other routers
    fn process_router_use(&mut self, parent_router_name: &str, args: &[ExprOrSpread]) {
        if args.is_empty() {
            return;
        }

        // Determine if the first argument is a path or a middleware/router
        let (path_prefix, target_arg_idx) = if args.len() >= 2 {
            if let Some(path) = self.extract_string_from_expr(&args[0].expr) {
                (path, 1) // First arg is path, second is router
            } else {
                ("/".to_string(), 0) // First arg is middleware or router
            }
        } else {
            ("/".to_string(), 0) // Only one arg, must be middleware or router
        };

        // Normalize path prefix
        let path_prefix = if !path_prefix.starts_with('/') {
            format!("/{}", path_prefix)
        } else {
            path_prefix
        };

        // Check if the argument at target_arg_idx is a router reference
        if target_arg_idx < args.len() {
            if let Expr::Ident(target_ident) = &*args[target_arg_idx].expr {
                let target_name = target_ident.sym.to_string();

                println!(
                    "Router use: {}({}) -> {}",
                    parent_router_name, path_prefix, target_name
                );

                // First, get and clone the parent router (without boxing yet)
                if let Some(parent_router) = self.routers.get(parent_router_name).cloned() {
                    // Get any parent app from the parent router
                    let parent_app = parent_router.parent_app.clone();

                    // Now box the parent router for the parent_router field
                    let boxed_parent = Some(Box::new(parent_router));

                    // Check if we're mounting a router
                    if let Some(mounted_router) = self.routers.get_mut(&target_name) {
                        // Update the mounted router's context
                        mounted_router.prefix = path_prefix;
                        mounted_router.parent_router = boxed_parent;

                        // Set parent app if available
                        if parent_app.is_some() {
                            mounted_router.parent_app = parent_app;
                        }

                        self.update_endpoints_for_owner(&target_name);

                        println!(
                            "Updated router: {} with parent router: {}",
                            target_name, parent_router_name
                        );
                    }
                }
            }
        }
    }

    fn update_endpoints_for_owner(&mut self, owner_name: &str) {
        // Find all endpoints owned by this router/app
        let mut endpoints_to_update = Vec::new();

        for ((old_path, method), owner) in &self.endpoint_owners {
            if owner == owner_name {
                endpoints_to_update.push((old_path.clone(), method.clone()));
            }
        }

        // Update each endpoint with the new full path
        for (old_path, method) in endpoints_to_update {
            // Find the endpoint in our endpoints vector
            let position = self
                .endpoints
                .iter()
                .position(|(path, m, _, _)| path == &old_path && m == &method);

            if let Some(pos) = position {
                let (_, method, response, request) = self.endpoints[pos].clone();

                // Resolve the full path using the updated router/app context
                let new_full_path = if let Some(router) = self.routers.get(owner_name) {
                    router.resolve_full_path(&old_path)
                } else if let Some(app) = self.express_apps.get(owner_name) {
                    app.resolve_full_path(&old_path)
                } else {
                    old_path.clone()
                };

                // Update the endpoint
                self.endpoints[pos] = (new_full_path.clone(), method.clone(), response, request);

                // Update the ownership tracker to use the new path
                self.endpoint_owners.remove(&(old_path, method.clone()));
                self.endpoint_owners
                    .insert((new_full_path.clone(), method), owner_name.to_string());
            }
        }
    }

    // Process route handlers on routers
    fn process_route_handler(&mut self, var_name: &str, http_method: &str, call: &CallExpr) {
        if let Some(endpoint_data) = self.extract_endpoint(call, http_method) {
            let (route, response_fields, request_fields) = endpoint_data;

            // Store the endpoint with its initial path
            self.endpoints.push((
                route.clone(),
                http_method.to_string(),
                response_fields.clone(),
                request_fields.clone(),
            ));

            // Record ownership
            self.endpoint_owners.insert(
                (route.clone(), http_method.to_string()),
                var_name.to_string(),
            );

            println!(
                "Detected endpoint: {} {} on {}",
                http_method, route, var_name
            );
        }
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
}
