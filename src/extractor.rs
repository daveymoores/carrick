use crate::visitor::Json;
use std::collections::HashMap;
use swc_ecma_ast::*;

pub trait CoreExtractor {
    fn extract_json_fields_from_call(&self, expr_stmt: &ExprStmt) -> Option<Json> {
        if let Expr::Call(call) = &*expr_stmt.expr {
            self.extract_res_json_fields(call)
        } else {
            None
        }
    }

    fn extract_json_fields_from_return(&self, expr: &Box<Expr>) -> Option<Json> {
        if let Expr::Call(call) = &**expr {
            self.extract_res_json_fields(call)
        } else {
            None
        }
    }

    fn extract_fields_from_arrow(&self, arrow: &ArrowExpr) -> Json {
        match &*arrow.body {
            // For arrow functions with block bodies: (req, res) => { ... }
            BlockStmtOrExpr::BlockStmt(block) => {
                for stmt in &block.stmts {
                    if let Stmt::Expr(expr_stmt) = stmt {
                        if let Some(json) = self.extract_json_fields_from_call(expr_stmt) {
                            return json;
                        }
                    }

                    // Look for return statements
                    if let Stmt::Return(ret) = stmt {
                        if let Some(expr) = &ret.arg {
                            if let Some(json) = self.extract_json_fields_from_return(expr) {
                                return json;
                            }
                        }
                    }
                }
            }
            // For arrow functions with expression bodies: (req, res) => res.json(...)
            BlockStmtOrExpr::Expr(expr) => {
                if let Some(json) = self.extract_json_fields_from_return(expr) {
                    return json;
                }
            }
        }

        // Default if we couldn't find any response
        Json::Null
    }

    // Extract response fields from a function declaration
    fn extract_fields_from_function_decl(&self, fn_decl: &FnDecl) -> Json {
        // Check if the function has a body
        if let Some(body) = &fn_decl.function.body {
            // Analyze each statement in the function body
            for stmt in &body.stmts {
                match stmt {
                    // For expressions like res.json({...})
                    Stmt::Expr(expr_stmt) => {
                        if let Some(json) = self.extract_json_fields_from_call(expr_stmt) {
                            return json;
                        }
                    }
                    // For return statements like return res.json({...})
                    Stmt::Return(return_stmt) => {
                        if let Some(expr) = &return_stmt.arg {
                            if let Some(json) = self.extract_json_fields_from_return(expr) {
                                return json;
                            }
                        }
                    }
                    // Handle nested blocks like if/else statements
                    Stmt::Block(block) => {
                        for nested_stmt in &block.stmts {
                            if let Stmt::Expr(expr_stmt) = nested_stmt {
                                if let Some(json) = self.extract_json_fields_from_call(expr_stmt) {
                                    return json;
                                }
                            }
                        }
                    }
                    // Other statement types could be handled here if needed
                    _ => {}
                }
            }
        }

        // Default if we couldn't find any response
        Json::Null
    }

    // Extract response fields from a function expression
    fn extract_fields_from_function_expr(&self, fn_expr: &FnExpr) -> Json {
        // Check if the function has a body
        if let Some(body) = &fn_expr.function.body {
            // Analyze each statement in the function body
            for stmt in &body.stmts {
                match stmt {
                    // For expressions like res.json({...})
                    Stmt::Expr(expr_stmt) => {
                        if let Some(json) = self.extract_json_fields_from_call(expr_stmt) {
                            return json;
                        }
                    }

                    // For return statements like return res.json({...})
                    Stmt::Return(return_stmt) => {
                        if let Some(expr) = &return_stmt.arg {
                            if let Some(json) = self.extract_json_fields_from_return(expr) {
                                return json;
                            }
                        }
                    }

                    // Handle nested blocks like if/else statements
                    Stmt::Block(block) => {
                        for nested_stmt in &block.stmts {
                            if let Stmt::Expr(expr_stmt) = nested_stmt {
                                if let Some(json) = self.extract_json_fields_from_call(expr_stmt) {
                                    return json;
                                }
                            }
                        }
                    }

                    // Other statement types could be handled here if needed
                    _ => {}
                }
            }
        }

        // Default if we couldn't find any response
        Json::Null
    }

    // Extract JSON structure from res.json(...)
    fn extract_res_json_fields(&self, call: &CallExpr) -> Option<Json> {
        let callee = call.callee.as_expr()?;
        let member = callee.as_member()?;

        if let Some(ident) = member.obj.as_ident() {
            if ident.sym == "res" || ident.sym == "json" {
                let arg = call.args.get(0)?;
                // Extract the JSON structure from the argument
                return Some(self.expr_to_json(&arg.expr));
            }
        }

        None
    }

    // Convert an expression to a Json value
    fn expr_to_json(&self, expr: &Expr) -> Json {
        match expr {
            // Handle literals
            Expr::Lit(lit) => match lit {
                Lit::Str(str_lit) => Json::String(str_lit.value.to_string()),
                Lit::Num(num) => Json::Number(num.value),
                Lit::Bool(b) => Json::Boolean(b.value),
                Lit::Null(_) => Json::Null,
                _ => Json::Null, // Other literals
            },

            // Handle arrays
            Expr::Array(arr) => {
                let values: Vec<Json> = arr
                    .elems
                    .iter()
                    .filter_map(|elem| elem.as_ref().map(|e| self.expr_to_json(&e.expr)))
                    .collect();
                Json::Array(values)
            }

            // Handle objects
            Expr::Object(obj) => {
                let mut map = HashMap::new();

                for prop in &obj.props {
                    if let PropOrSpread::Prop(boxed_prop) = prop {
                        if let Prop::KeyValue(kv) = &**boxed_prop {
                            // Extract key
                            let key = match &kv.key {
                                PropName::Ident(ident) => ident.sym.to_string(),
                                PropName::Str(str) => str.value.to_string(),
                                _ => continue, // Skip computed keys
                            };

                            // Extract value
                            let value = self.expr_to_json(&kv.value);

                            map.insert(key, value);
                        }
                    }
                }

                Json::Object(Box::new(map))
            }

            // Other expressions (function calls, identifiers, etc.) - treat as null for now
            _ => Json::Null,
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

    // Extract fetch calls from an arrow function
    fn extract_fetch_calls_from_arrow(&self, arrow: &ArrowExpr) -> Vec<(String, String)> {
        let mut fetch_calls = Vec::new();

        match &*arrow.body {
            // For arrow functions with block bodies: (req, res) => { ... }
            BlockStmtOrExpr::BlockStmt(block) => {
                for stmt in &block.stmts {
                    self.extract_fetch_from_stmt(stmt, &mut fetch_calls);
                }
            }
            // For arrow functions with expression bodies: (req, res) => fetch(...)
            BlockStmtOrExpr::Expr(expr) => {
                self.extract_fetch_from_expr(expr, &mut fetch_calls);
            }
        }

        fetch_calls
    }

    // Extract fetch calls from a function declaration
    fn extract_fetch_calls_from_function_decl(&self, fn_decl: &FnDecl) -> Vec<(String, String)> {
        let mut fetch_calls = Vec::new();

        if let Some(body) = &fn_decl.function.body {
            for stmt in &body.stmts {
                self.extract_fetch_from_stmt(stmt, &mut fetch_calls);
            }
        }

        fetch_calls
    }

    // Extract fetch calls from a function expression
    fn extract_fetch_calls_from_function_expr(&self, fn_expr: &FnExpr) -> Vec<(String, String)> {
        let mut fetch_calls = Vec::new();

        if let Some(body) = &fn_expr.function.body {
            for stmt in &body.stmts {
                self.extract_fetch_from_stmt(stmt, &mut fetch_calls);
            }
        }

        fetch_calls
    }

    // Helper method to extract fetch calls from a statement
    fn extract_fetch_from_stmt(&self, stmt: &Stmt, fetch_calls: &mut Vec<(String, String)>) {
        match stmt {
            Stmt::Expr(expr_stmt) => {
                self.extract_fetch_from_expr(&expr_stmt.expr, fetch_calls);
            }
            Stmt::Return(return_stmt) => {
                if let Some(expr) = &return_stmt.arg {
                    self.extract_fetch_from_expr(expr, fetch_calls);
                }
            }
            Stmt::Block(block) => {
                for nested_stmt in &block.stmts {
                    self.extract_fetch_from_stmt(nested_stmt, fetch_calls);
                }
            }
            Stmt::If(if_stmt) => {
                self.extract_fetch_from_expr(&if_stmt.test, fetch_calls);
                self.extract_fetch_from_stmt(&*if_stmt.cons, fetch_calls);
                if let Some(alt) = &if_stmt.alt {
                    self.extract_fetch_from_stmt(&**alt, fetch_calls);
                }
            }
            Stmt::Try(try_stmt) => {
                self.extract_fetch_from_stmt(&Stmt::Block(try_stmt.block.clone()), fetch_calls);
                if let Some(handler) = &try_stmt.handler {
                    self.extract_fetch_from_stmt(&Stmt::Block(handler.body.clone()), fetch_calls);
                }
                if let Some(finalizer) = &try_stmt.finalizer {
                    self.extract_fetch_from_stmt(&Stmt::Block(finalizer.clone()), fetch_calls);
                }
            }
            // Add other statement types as needed
            _ => {}
        }
    }

    // Helper method to extract fetch calls from an expression
    fn extract_fetch_from_expr(&self, expr: &Expr, fetch_calls: &mut Vec<(String, String)>) {
        match expr {
            Expr::Call(call) => {
                // Check if this is a fetch call
                if let Callee::Expr(callee_expr) = &call.callee {
                    if let Expr::Ident(ident) = &**callee_expr {
                        if ident.sym == "fetch" {
                            if let (Some(route), Some(method)) = self.extract_fetch_route(call) {
                                fetch_calls.push((route, method));
                            }
                        }
                    }
                }

                // Check args for nested fetch calls
                for arg in &call.args {
                    self.extract_fetch_from_expr(&arg.expr, fetch_calls);
                }
            }
            Expr::Arrow(arrow) => {
                let nested_calls = self.extract_fetch_calls_from_arrow(arrow);
                fetch_calls.extend(nested_calls);
            }
            Expr::Fn(fn_expr) => {
                let nested_calls = self.extract_fetch_calls_from_function_expr(fn_expr);
                fetch_calls.extend(nested_calls);
            }
            Expr::Await(await_expr) => {
                self.extract_fetch_from_expr(&await_expr.arg, fetch_calls);
            }
            // Add other expression types as needed
            _ => {}
        }
    }
}

pub trait RouteExtractor: CoreExtractor {
    fn get_imported_functions(&self) -> &HashMap<String, String>;
    fn get_response_fields(&self) -> &HashMap<String, Json>;
    fn add_imported_handler(&mut self, route: String, handler: String, source: String);

    // Extract route and handler information from route definitions
    fn extract_endpoint(&mut self, call: &CallExpr) -> Option<(String, Json)> {
        // Get the route from the first argument
        let route = call.args.get(0)?.expr.as_lit()?.as_str()?.value.to_string();

        let mut response_json = Json::Null;

        // Check the second argument (handler)
        if let Some(second_arg) = call.args.get(1) {
            match &*second_arg.expr {
                // Case 1: Inline function handler
                Expr::Fn(fn_expr) => {
                    if let Some(body) = &fn_expr.function.body {
                        for stmt in &body.stmts {
                            if let Stmt::Expr(expr_stmt) = stmt {
                                if let Expr::Call(call) = &*expr_stmt.expr {
                                    if let Some(json) = self.extract_res_json_fields(call) {
                                        response_json = json;
                                        break;
                                    }
                                }
                            }
                        }
                    }
                }

                Expr::Ident(ident) => {
                    let handler_name = ident.sym.to_string();

                    // Use the helper methods instead of direct field access
                    if let Some(source) = self.get_imported_functions().get(&handler_name) {
                        self.add_imported_handler(
                            route.clone(),
                            handler_name.clone(),
                            source.clone(),
                        );

                        if let Some(fields) = self.get_response_fields().get(&handler_name) {
                            response_json = fields.clone();
                        }
                    }
                }

                _ => {
                    // Other handler types (arrow functions, etc.)
                }
            }
        }

        Some((route, response_json))
    }
}
