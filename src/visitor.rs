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
    utils::join_path_segments,
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

#[derive(Debug, Clone, PartialEq)]
pub enum OwnerType {
    App(String),
    Router(String),
}

#[derive(Debug)]
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
}

#[derive(Debug)]
pub struct Call {
    pub route: String,
    pub method: String,
    pub response: Json,
    pub request: Option<Json>,
}

#[derive(Debug)]
pub struct DependencyVisitor {
    pub repo_prefix: String,
    pub endpoints: Vec<Endpoint>, // (route, method, response fields, request fields)
    pub calls: Vec<Call>,         // (route, method, expected fields)
    pub mounts: Vec<Mount>,
    pub response_fields: HashMap<String, Json>, // Function name -> expected fields
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
}

impl DependencyVisitor {
    pub fn new(file_path: PathBuf, repo_prefix: &str) -> Self {
        Self {
            repo_prefix: repo_prefix.to_owned(),
            endpoints: Vec::new(),
            calls: Vec::new(),
            mounts: Vec::new(),
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
                        let var_name = ident.id.sym.as_str();
                        let prefixed_var_name = self.prefix_owner_type(var_name);

                        // Check if this is a router creation
                        if self.is_router_creation(call_expr) {
                            // Add the router to the `routers` map with an initial context
                            self.routers.insert(
                                prefixed_var_name.clone(),
                                RouterContext {
                                    name: prefixed_var_name.clone(),
                                },
                            );

                            println!("Detected Router: {}", var_name);
                        }
                        // Check if this is an express app creation
                        else if let Some(express_name) = &self.express_import_name {
                            if self.is_express_app_creation(call_expr, express_name) {
                                let express_app_name = self.prefix_owner_type(var_name);
                                // Add to express_apps map
                                self.express_apps.insert(
                                    express_app_name.to_owned(),
                                    AppContext {
                                        name: express_app_name,
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

        // Check if the argument at target_arg_idx is a router or app reference
        if target_arg_idx < args.len() {
            if let Expr::Ident(target_ident) = &*args[target_arg_idx].expr {
                let target_name = target_ident.sym.as_str();
                let prefixed_target_name = self.prefix_owner_type(target_name);

                println!("App use: {}({}) -> {}", app_name, path_prefix, target_name);
                // Save Mount to track Router relationship to App via Route
                self.mounts.push(Mount {
                    parent: OwnerType::App(app_name.to_string()),
                    child: OwnerType::Router(prefixed_target_name),
                    prefix: path_prefix,
                });
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

                // Save Mount to track Router relationship to App via Route
                self.mounts.push(Mount {
                    parent: OwnerType::Router(parent_router_name.to_string()),
                    child: OwnerType::Router(prefixed_target_name),
                    prefix: path_prefix,
                });
            }
        }
    }

    // Process route handlers on routers
    fn process_route_handler(&mut self, var_name: &str, http_method: &str, call: &CallExpr) {
        // find whether this is an app or router
        let owner = match self.express_apps.get(var_name) {
            Some(_) => OwnerType::App(var_name.to_string()),
            None => OwnerType::Router(var_name.to_string()),
        };
        if let Some(endpoint_data) = self.extract_endpoint(call, http_method) {
            let (route, response_fields, request_fields) = endpoint_data;

            // Store the endpoint with its initial path
            self.endpoints.push(Endpoint {
                owner,
                route: route.clone(),
                method: http_method.to_string(),
                response: response_fields.clone(),
                request: request_fields.clone(),
            });

            println!(
                "Detected endpoint: {} {} on {}",
                http_method,
                route,
                self.get_owner_type(var_name)
            );
        }
    }

    pub fn compute_full_paths_for_endpoint(
        &self,
        endpoint: &Endpoint,
        mounts: &[Mount],
        apps: &HashMap<String, AppContext>,
    ) -> Vec<String> {
        println!("{:?} {:?} {:?}", endpoint, mounts, apps);
        let mut results = Vec::new();

        // For each app, try to find all mounting chains to endpoint.owner
        for app_name in apps.keys() {
            let app_owner = OwnerType::App(app_name.clone());
            let mut stack = Vec::new();
            let mut visited = Vec::new();
            // Each stack entry: (current_owner, prefixes_so_far)
            stack.push((app_owner.clone(), Vec::<String>::new()));

            while let Some((current, mut prefixes)) = stack.pop() {
                if &current == &endpoint.owner {
                    // Found a chain! Build the full path
                    prefixes.push(endpoint.route.clone());
                    let path_refs: Vec<&str> = prefixes.iter().map(|s| s.as_str()).collect();
                    results.push(join_path_segments(&path_refs));
                    continue;
                }
                // Prevent cycles (shouldn't happen in Express, but just in case)
                if visited.contains(&current) {
                    continue;
                }
                visited.push(current.clone());

                // For each mount where parent == current, follow to child
                for mount in mounts.iter().filter(|m| m.parent == current) {
                    let mut new_prefixes = prefixes.clone();
                    new_prefixes.push(mount.prefix.clone());
                    stack.push((mount.child.clone(), new_prefixes));
                }
            }
        }
        results
    }

    pub fn resolve_all_endpoint_paths(&mut self) {
        let mut new_endpoints = Vec::new();
        for endpoint in &self.endpoints {
            let full_paths =
                self.compute_full_paths_for_endpoint(endpoint, &self.mounts, &self.express_apps);

            println!("FULL_PATHS ------> {:?}", full_paths);
            for path in full_paths {
                let mut ep = endpoint.clone();
                ep.route = path;
                new_endpoints.push(ep);
            }
        }
        self.endpoints = new_endpoints;
    }
}
