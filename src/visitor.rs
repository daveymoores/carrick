extern crate swc_common;
extern crate swc_ecma_parser;
use std::collections::HashMap;

use swc_ecma_ast::*;
use swc_ecma_visit::{Visit, VisitWith};
extern crate regex;
use regex::Regex;

#[derive(Debug)]
pub struct DependencyVisitor {
    pub endpoints: Vec<(String, String, Vec<String>)>, // (route, method, response fields)
    pub calls: Vec<(String, String, Vec<String>)>,     // (route, method, expected fields)
    pub current_fn: Option<String>,                    // Track current function context
    pub response_fields: HashMap<String, Vec<String>>, // Function name -> expected fields
}

impl DependencyVisitor {
    pub fn new() -> Self {
        Self {
            endpoints: Vec::new(),
            calls: Vec::new(),
            current_fn: None,
            response_fields: HashMap::new(),
        }
    }
}

impl Visit for DependencyVisitor {
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

    // Extract route and response fields from app.get
    fn extract_endpoint(&self, call: &CallExpr) -> Option<(String, Vec<String>)> {
        // Get the route from the first argument
        let route = call.args.get(0)?.expr.as_lit()?.as_str()?.value.to_string();

        let mut response_fields = Vec::new();

        // Look for the callback function in the second argument
        if let Some(Expr::Fn(fn_expr)) = call.args.get(1).map(|arg| &*arg.expr) {
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
        Some((route, response_fields))
    }

    // Extract fields from res.json({ ... })
    fn extract_res_json_fields(&self, call: &CallExpr) -> Option<Vec<String>> {
        let callee = call.callee.as_expr()?;
        let member = callee.as_member()?;
        if member.obj.as_ident().map_or(false, |i| i.sym == "res")
            && member.prop.as_ident().map_or(false, |i| i.sym == "json")
        {
            let arg = call.args.get(0)?;
            let obj = arg.expr.as_object()?;
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
            Some(fields)
        } else {
            None
        }
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
