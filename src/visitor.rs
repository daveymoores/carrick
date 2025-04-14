extern crate swc_common;
extern crate swc_ecma_parser;
use std::{collections::HashMap, path::PathBuf};

use swc_ecma_ast::*;
use swc_ecma_visit::{Visit, VisitWith};
extern crate regex;
use regex::Regex;

#[derive(Debug, Clone)]
pub enum FunctionNodeType {
    ArrowFunction(Box<ArrowExpr>),
    FunctionDeclaration(Box<FnDecl>),
    FunctionExpression(Box<FnExpr>),
}

#[derive(Debug, Clone)]
pub struct FunctionDefinition {
    pub name: String,
    pub file_path: PathBuf,
    pub node_type: FunctionNodeType,
    pub analyzed: bool,
}

#[derive(Debug)]
pub struct DependencyVisitor {
    pub endpoints: Vec<(String, String, Vec<String>)>, // (route, method, response fields)
    pub calls: Vec<(String, String, Vec<String>)>,     // (route, method, expected fields)
    pub current_fn: Option<String>,                    // Track current function context
    pub response_fields: HashMap<String, Vec<String>>, // Function name -> expected fields
    pub imported_functions: HashMap<String, String>,
    pub current_file: PathBuf,
    pub imported_handlers: Vec<(String, String, String)>,
    // Store function definitions for second pass analysis
    pub function_definitions: HashMap<String, FunctionDefinition>,
}

impl DependencyVisitor {
    pub fn new(file_path: PathBuf) -> Self {
        Self {
            endpoints: Vec::new(),
            calls: Vec::new(),
            current_fn: None,
            response_fields: HashMap::new(),
            imported_functions: HashMap::new(),
            current_file: file_path,
            imported_handlers: Vec::new(),
            function_definitions: HashMap::new(),
        }
    }
}

impl Visit for DependencyVisitor {
    // Track ES Module exports
    fn visit_export_decl(&mut self, export: &ExportDecl) {
        println!("Found export declaration");

        // Handle different export types
        match &export.decl {
            // For "export const x = ..."
            Decl::Var(var_decl) => {
                for decl in &var_decl.decls {
                    if let Pat::Ident(ident) = &decl.name {
                        let exported_name = ident.id.sym.to_string();
                        println!("  - Exported variable: {}", exported_name);

                        // Check what's being exported
                        if let Some(init) = &decl.init {
                            match &**init {
                                Expr::Arrow(arrow) => {
                                    println!("    (Arrow function export)");

                                    // Store the function definition for later analysis
                                    self.function_definitions.insert(
                                        exported_name.clone(),
                                        FunctionDefinition {
                                            name: exported_name.clone(),
                                            file_path: self.current_file.clone(),
                                            node_type: FunctionNodeType::ArrowFunction(Box::new(
                                                arrow.clone(),
                                            )),
                                            analyzed: false,
                                        },
                                    );

                                    // You can still extract fields here if you want
                                    let fields = self.extract_fields_from_arrow(arrow);
                                    println!("    Response fields: {:?}", fields);
                                    self.response_fields.insert(exported_name.clone(), fields);
                                }
                                // Regular function export
                                Expr::Fn(_) => {
                                    println!("    (Function export)");
                                    // Track this function as defined in this file
                                    self.response_fields
                                        .insert(exported_name.clone(), Vec::new());
                                }
                                _ => {
                                    println!("    (Other export type)");
                                }
                            }
                        }
                    }
                }
            }
            // For "export function x() {...}"
            Decl::Fn(fn_decl) => {
                let exported_name = fn_decl.ident.sym.to_string();
                println!("  - Exported function: {}", exported_name);

                // Track this function as defined in this file
                self.response_fields
                    .insert(exported_name.clone(), Vec::new());
            }
            _ => {}
        }
    }

    // Track ES Module imports
    fn visit_import_decl(&mut self, import: &ImportDecl) {
        let source = import.src.value.to_string();
        println!("Found import from: {}", source);

        for specifier in &import.specifiers {
            match specifier {
                ImportSpecifier::Named(named) => {
                    // Handle named imports: import { func } from './module'
                    let local_name = named.local.sym.to_string();
                    println!("  - Named import: {}", local_name);

                    // Track this imported function
                    self.imported_functions.insert(local_name, source.clone());
                }
                ImportSpecifier::Default(default) => {
                    // Handle default imports: import func from './module'
                    let local_name = default.local.sym.to_string();
                    println!("  - Default import: {}", local_name);

                    // Track this imported function
                    self.imported_functions.insert(local_name, source.clone());
                }
                ImportSpecifier::Namespace(namespace) => {
                    // Handle namespace imports: import * as mod from './module'
                    let local_name = namespace.local.sym.to_string();
                    println!("  - Namespace import: {}", local_name);

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
                            if let Some((route, response_fields)) = self.extract_endpoint(call) {
                                self.endpoints.push((route, http_method, response_fields));
                            }
                        }
                    }
                }
            } else if let Expr::Ident(ident) = &**callee_expr {
                // Your existing fetch detection
                if ident.sym == "fetch" {
                    if let (Some(route), Some(method)) = self.extract_fetch_route(call) {
                        self.calls.push((route, method, Vec::new()));
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

    fn extract_fields_from_arrow(&self, arrow: &ArrowExpr) -> Vec<String> {
        let mut fields = Vec::new();

        match &*arrow.body {
            // For arrow functions with block bodies: (req, res) => { ... }
            BlockStmtOrExpr::BlockStmt(block) => {
                for stmt in &block.stmts {
                    if let Stmt::Expr(expr_stmt) = stmt {
                        if let Expr::Call(call) = &*expr_stmt.expr {
                            if let Some(extracted_fields) = self.extract_res_json_fields(call) {
                                fields.extend(extracted_fields);
                            }
                        }
                    }
                }
            }
            // For arrow functions with expression bodies: (req, res) => res.json(...)
            BlockStmtOrExpr::Expr(expr) => {
                if let Expr::Call(call) = &**expr {
                    if let Some(extracted_fields) = self.extract_res_json_fields(call) {
                        fields.extend(extracted_fields);
                    }
                }
            }
        }

        fields
    }

    // Extract response fields from a function declaration
    fn extract_fields_from_function_decl(&self, fn_decl: &FnDecl) -> Vec<String> {
        let mut fields = Vec::new();

        // Check if the function has a body
        if let Some(body) = &fn_decl.function.body {
            // Analyze each statement in the function body
            for stmt in &body.stmts {
                match stmt {
                    // For expressions like res.json({...})
                    Stmt::Expr(expr_stmt) => {
                        if let Expr::Call(call) = &*expr_stmt.expr {
                            if let Some(extracted_fields) = self.extract_res_json_fields(call) {
                                fields.extend(extracted_fields);
                            }
                        }
                    }
                    // For return statements like return res.json({...})
                    Stmt::Return(return_stmt) => {
                        if let Some(expr) = &return_stmt.arg {
                            if let Expr::Call(call) = &**expr {
                                if let Some(extracted_fields) = self.extract_res_json_fields(call) {
                                    fields.extend(extracted_fields);
                                }
                            }
                        }
                    }
                    // Handle nested blocks like if/else statements
                    Stmt::Block(block) => {
                        for nested_stmt in &block.stmts {
                            if let Stmt::Expr(expr_stmt) = nested_stmt {
                                if let Expr::Call(call) = &*expr_stmt.expr {
                                    if let Some(extracted_fields) =
                                        self.extract_res_json_fields(call)
                                    {
                                        fields.extend(extracted_fields);
                                    }
                                }
                            }
                        }
                    }
                    // Other statement types could be handled here if needed
                    _ => {}
                }
            }
        }

        fields
    }

    // Extract response fields from a function expression
    fn extract_fields_from_function_expr(&self, fn_expr: &FnExpr) -> Vec<String> {
        let mut fields = Vec::new();

        // Check if the function has a body
        if let Some(body) = &fn_expr.function.body {
            // Analyze each statement in the function body
            for stmt in &body.stmts {
                match stmt {
                    // For expressions like res.json({...})
                    Stmt::Expr(expr_stmt) => {
                        if let Expr::Call(call) = &*expr_stmt.expr {
                            if let Some(extracted_fields) = self.extract_res_json_fields(call) {
                                fields.extend(extracted_fields);
                            }
                        }
                    }
                    // For return statements like return res.json({...})
                    Stmt::Return(return_stmt) => {
                        if let Some(expr) = &return_stmt.arg {
                            if let Expr::Call(call) = &**expr {
                                if let Some(extracted_fields) = self.extract_res_json_fields(call) {
                                    fields.extend(extracted_fields);
                                }
                            }
                        }
                    }
                    // Handle nested blocks like if/else statements
                    Stmt::Block(block) => {
                        for nested_stmt in &block.stmts {
                            if let Stmt::Expr(expr_stmt) = nested_stmt {
                                if let Expr::Call(call) = &*expr_stmt.expr {
                                    if let Some(extracted_fields) =
                                        self.extract_res_json_fields(call)
                                    {
                                        fields.extend(extracted_fields);
                                    }
                                }
                            }
                        }
                    }
                    // Other statement types could be handled here if needed
                    _ => {}
                }
            }
        }

        fields
    }

    // Extract route and handler information from route definitions
    fn extract_endpoint(&mut self, call: &CallExpr) -> Option<(String, Vec<String>)> {
        // Get the route from the first argument
        let route = call.args.get(0)?.expr.as_lit()?.as_str()?.value.to_string();

        let mut response_fields = Vec::new();

        // Check the second argument (handler)
        if let Some(second_arg) = call.args.get(1) {
            match &*second_arg.expr {
                // Case 1: Inline function handler (as before)
                Expr::Fn(fn_expr) => {
                    if let Some(body) = &fn_expr.function.body {
                        for stmt in &body.stmts {
                            if let Stmt::Expr(expr_stmt) = stmt {
                                if let Expr::Call(call) = &*expr_stmt.expr {
                                    if let Some(fields) = self.extract_res_json_fields(call) {
                                        response_fields.extend(fields);
                                    }
                                }
                            }
                        }
                    }
                }

                // Case 2: Imported function handler
                Expr::Ident(ident) => {
                    let handler_name = ident.sym.to_string();

                    // Check if this handler is an imported function
                    if let Some(source) = self.imported_functions.get(&handler_name) {
                        println!(
                            "FOUND IMPORTED HANDLER: Route '{}' uses imported function '{}' from '{}'",
                            route, handler_name, source
                        );

                        // Track this imported handler usage
                        self.imported_handlers.push((
                            route.clone(),
                            handler_name.clone(),
                            source.clone(),
                        ));
                    }
                }

                _ => {
                    // Other handler types (arrow functions, etc.)
                }
            }
        }

        Some((route, response_fields))
    }

    // Extract fields from res.json({ ... })
    fn extract_res_json_fields(&self, call: &CallExpr) -> Option<Vec<String>> {
        let callee = call.callee.as_expr()?;
        let member = callee.as_member()?;

        // Check if it's a res.json call
        if member.obj.as_ident().map_or(false, |i| i.sym == "res")
            && member.prop.as_ident().map_or(false, |i| i.sym == "json")
        {
            let arg = call.args.get(0)?;

            // Case 1: Object response - like res.json({ field1: value, field2: value })
            if let Some(obj) = arg.expr.as_object() {
                let fields = obj
                    .props
                    .iter()
                    .filter_map(|prop| {
                        prop.as_prop()?
                            .as_key_value()?
                            .key
                            .as_ident()
                            .map(|key| key.sym.to_string())
                    })
                    .collect();
                return Some(fields);
            }

            // Case 2: Array response - like res.json([{ id: 1, name: 'Product' }])
            if let Some(arr) = arg.expr.as_array() {
                // For arrays, try to extract fields from the first item if it's an object
                if let Some(first_elem) = arr.elems.first() {
                    if let Some(elem) = first_elem {
                        if let Some(obj) = elem.expr.as_object() {
                            let fields: Vec<String> = obj
                                .props
                                .iter()
                                .filter_map(|prop| {
                                    prop.as_prop()?
                                        .as_key_value()?
                                        .key
                                        .as_ident()
                                        .map(|key| key.sym.to_string())
                                })
                                .collect();

                            // Add a special marker to indicate it's an array of objects
                            let mut result = vec!["[array_of_objects]".to_string()];
                            result.extend(fields);
                            return Some(result);
                        }
                    }
                }
                // For empty arrays or arrays of non-objects
                return Some(vec!["[array]".to_string()]);
            }

            // Case 3: Other response types (might want to handle primitives, etc.)
            return Some(vec!["[unknown]".to_string()]);
        }

        None
    }

    // Extract route from fetch call
    fn extract_fetch_route(&self, call: &CallExpr) -> (Option<String>, Option<String>) {
        let route = match call.args.get(0) {
            Some(arg) => match &*arg.expr {
                Expr::Lit(lit) => match lit {
                    Lit::Str(str_lit) => Some(str_lit.value.to_string()),
                    _ => None,
                },
                _ => None,
            },
            _ => None,
        };

        // Extract method from second argument (if it exists)
        let method = if call.args.len() > 1 {
            match &*call.args[1].expr {
                Expr::Object(obj) => {
                    // Look for { method: 'POST' } pattern
                    for prop in &obj.props {
                        if let PropOrSpread::Prop(boxed_prop) = prop {
                            if let Prop::KeyValue(kv) = &**boxed_prop {
                                // Check if the key is "method"
                                if let PropName::Ident(key_ident) = &kv.key {
                                    if key_ident.sym.to_string() == "method" {
                                        // Extract the method value
                                        if let Expr::Lit(lit) = &*kv.value {
                                            if let Lit::Str(str_lit) = lit {
                                                return (route, Some(str_lit.value.to_string()));
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                    None
                }
                _ => None,
            }
        } else {
            // No second argument, default to GET
            Some("GET".to_string())
        };

        (route, method)
    }

    fn normalize_route(&self, route: &str) -> String {
        let mut normalized = route.to_string();

        // Remove trailing slashes
        while normalized.ends_with('/') && normalized.len() > 1 {
            normalized.pop();
        }

        // Ensure leading slash
        if !normalized.starts_with('/') {
            normalized = format!("/{}", normalized);
        }

        normalized
    }

    pub fn print_imported_handler_summary(&self) {
        println!("\nImported Handler Usage:");
        println!("----------------------");
        if self.imported_handlers.is_empty() {
            println!("No routes using imported handlers.");
            return;
        }

        for (route, handler, source) in &self.imported_handlers {
            println!("Route: {} uses handler {} from {}", route, handler, source);
        }
    }

    pub fn print_function_definitions(&self) {
        println!("\nStored Function Definitions:");
        println!("--------------------------");
        if self.function_definitions.is_empty() {
            println!("No function definitions stored.");
            return;
        }

        for (name, def) in &self.function_definitions {
            println!("Function: {} in {}", name, def.file_path.display());
        }
    }

    // This function analyzes the function definitions and returns a HashMap of route fields.
    // TODO it should probably be refactored into its own function.
    pub fn analyze_function_definitions(
        &self,
        imported_handlers: &[(String, String, String)],
        function_definitions: &HashMap<String, FunctionDefinition>,
    ) -> HashMap<String, Vec<String>> {
        let mut route_fields = HashMap::new();

        for (route, handler_name, _) in imported_handlers {
            if let Some(func_def) = function_definitions.get(handler_name) {
                let fields = match &func_def.node_type {
                    FunctionNodeType::ArrowFunction(arrow) => self.extract_fields_from_arrow(arrow),
                    FunctionNodeType::FunctionDeclaration(decl) => {
                        self.extract_fields_from_function_decl(decl)
                    }
                    FunctionNodeType::FunctionExpression(expr) => {
                        self.extract_fields_from_function_expr(expr)
                    }
                };

                route_fields.insert(route.clone(), fields);
            }
        }

        route_fields
    }

    pub fn analyze_matches(&self) -> Vec<String> {
        let mut issues = Vec::new();

        // Create a map of endpoints for efficient lookups
        let mut endpoint_map: HashMap<String, Vec<String>> = HashMap::new();
        for (route, method, _) in &self.endpoints {
            let normalized_route = self.normalize_route(route);
            endpoint_map
                .entry(normalized_route)
                .or_insert_with(Vec::new)
                .push(method.clone());
        }

        // Check each call against endpoints
        for (route, method, _) in &self.calls {
            let normalized_route = self.normalize_route(route);

            // Check if route exists
            match endpoint_map.get(&normalized_route) {
                Some(allowed_methods) => {
                    // Check if method is allowed
                    if !allowed_methods.contains(method) {
                        issues.push(format!(
                                "Method mismatch: {} {} is called but endpoint only supports methods: {:?}",
                                method, route, allowed_methods
                            ));
                    }
                }
                None => {
                    // Try to find if it's a sub-route or has a parent
                    let mut found = false;

                    // Check for API base paths (simple approach)
                    for (endpoint_route, _, _) in &self.endpoints {
                        let norm_endpoint = self.normalize_route(endpoint_route);
                        if normalized_route.starts_with(&norm_endpoint) {
                            found = true;
                            break;
                        }

                        // Check for route parameters like '/users/:id'
                        if endpoint_route.contains(':') {
                            // Convert route with params to regex pattern
                            // Replace :param with a regex capture group that matches everything except slashes
                            let pattern = norm_endpoint
                                .split('/')
                                .map(|segment| {
                                    if segment.starts_with(':') {
                                        "([^/]+)".to_string() // Match any character except /
                                    } else {
                                        regex::escape(segment) // Escape other segments for regex safety
                                    }
                                })
                                .collect::<Vec<String>>()
                                .join("/");

                            // Ensure pattern matches the whole path
                            let re_str = format!("^{}$", pattern);

                            // Create regex and check for match
                            if let Ok(regex) = Regex::new(&re_str) {
                                if regex.is_match(&normalized_route) {
                                    found = true;
                                    break;
                                }
                            }
                        }
                    }

                    if !found {
                        issues.push(format!(
                            "Missing endpoint: No endpoint defined for {} {}",
                            method, route
                        ));
                    }
                }
            }
        }

        issues
    }
}
