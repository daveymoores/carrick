use crate::visitor::Json;
use std::collections::HashMap;
use swc_ecma_ast::*;

pub trait CoreExtractor {
    fn extract_params_from_route(&self, route: &str) -> Vec<String> {
        let param_pattern = regex::Regex::new(r":(\w+)").unwrap();
        let mut params = Vec::new();

        for cap in param_pattern.captures_iter(route) {
            params.push(cap[1].to_string());
        }

        params
    }

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
            Stmt::Try(try_stmt) => {
                // Process statements in try block directly
                for stmt in &try_stmt.block.stmts {
                    self.extract_fetch_from_stmt(stmt, fetch_calls);
                }

                // Process catch block if present
                if let Some(handler) = &try_stmt.handler {
                    for stmt in &handler.body.stmts {
                        self.extract_fetch_from_stmt(stmt, fetch_calls);
                    }
                }

                // Process finally block if present
                if let Some(finalizer) = &try_stmt.finalizer {
                    for stmt in &finalizer.stmts {
                        self.extract_fetch_from_stmt(stmt, fetch_calls);
                    }
                }
            }
            Stmt::Decl(decl) => {
                // Handle variable declarations which might contain fetch calls
                if let Decl::Var(var_decl) = decl {
                    for var in &var_decl.decls {
                        if let Some(init) = &var.init {
                            self.extract_fetch_from_expr(init, fetch_calls);
                        }
                    }
                }
            }
            Stmt::If(if_stmt) => {
                self.extract_fetch_from_expr(&if_stmt.test, fetch_calls);
                self.extract_fetch_from_stmt(&if_stmt.cons, fetch_calls);
                if let Some(alt) = &if_stmt.alt {
                    self.extract_fetch_from_stmt(alt, fetch_calls);
                }
            }
            // Add other statement types as needed
            _ => {}
        }
    }

    fn extract_fetch_from_expr(&self, expr: &Expr, fetch_calls: &mut Vec<(String, String)>) {
        match expr {
            Expr::Call(call) => {
                // Check if this is a fetch call
                if let Callee::Expr(callee_expr) = &call.callee {
                    if let Expr::Ident(ident) = &**callee_expr {
                        if ident.sym == "fetch" {
                            // Handle both string literals and template literals
                            if let Some(route) = self.extract_route_from_call_arg(&call.args) {
                                let method = self
                                    .extract_method_from_call_args(call)
                                    .unwrap_or_else(|| "GET".to_string());
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
            Expr::Await(await_expr) => {
                self.extract_fetch_from_expr(&await_expr.arg, fetch_calls);
            }
            Expr::Assign(assign) => {
                self.extract_fetch_from_expr(&assign.right, fetch_calls);
            }
            Expr::Arrow(arrow) => {
                let nested_calls = self.extract_fetch_calls_from_arrow(arrow);
                fetch_calls.extend(nested_calls);
            }
            Expr::Fn(fn_expr) => {
                let nested_calls = self.extract_fetch_calls_from_function_expr(fn_expr);
                fetch_calls.extend(nested_calls);
            }
            // Add handling for template literals and other expression types
            _ => {}
        }
    }

    // Helper method to extract HTTP method from fetch call arguments
    fn extract_method_from_call_args(&self, call: &CallExpr) -> Option<String> {
        // Need at least 2 arguments to have a method
        if call.args.len() <= 1 {
            return Some("GET".to_string()); // Default is GET
        }

        // Check the second argument (options object)
        match &*call.args[1].expr {
            Expr::Object(obj) => {
                // Look for { method: 'POST' } pattern
                for prop in &obj.props {
                    if let PropOrSpread::Prop(boxed_prop) = prop {
                        if let Prop::KeyValue(kv) = &**boxed_prop {
                            // Check if the key is "method"
                            if let PropName::Ident(key_ident) = &kv.key {
                                if key_ident.sym == "method" {
                                    // Extract the method value
                                    if let Expr::Lit(Lit::Str(str_lit)) = &*kv.value {
                                        return Some(str_lit.value.to_string());
                                    }
                                }
                            }
                        }
                    }
                }

                // No method specified in options
                Some("GET".to_string())
            }
            // Second argument isn't an object
            _ => Some("GET".to_string()),
        }
    }

    // Helper method to extract route from call arguments (supporting template literals)
    fn extract_route_from_call_arg(&self, args: &[ExprOrSpread]) -> Option<String> {
        if args.is_empty() {
            return None;
        }

        match &*args[0].expr {
            // Regular string literal
            Expr::Lit(Lit::Str(str_lit)) => Some(str_lit.value.to_string()),

            // Template literal
            Expr::Tpl(tpl) => {
                let mut route = String::new();
                let mut param_index = 0;

                // Process each part of the template
                for (i, quasi) in tpl.quasis.iter().enumerate() {
                    // Add the raw text part
                    route.push_str(&quasi.raw);

                    // If there's an expression after this quasi
                    if i < tpl.exprs.len() {
                        // Add a parameter placeholder
                        route.push_str(&format!(":param{}", param_index));
                        param_index += 1;
                    }
                }

                Some(route)
            }
            // Add other patterns as needed
            _ => None,
        }
    }

    fn extract_req_body_fields(&self, function_body: &BlockStmt) -> Option<Json> {
        // Examine each statement in the function body
        for stmt in &function_body.stmts {
            // Look for variable declarations that extract from req.body
            if let Stmt::Decl(Decl::Var(var_decl)) = stmt {
                if let Some(json) = self.extract_req_body_from_var_decl(var_decl) {
                    return Some(json);
                }
            }

            // Look for direct req.body usage
            if let Stmt::Expr(expr_stmt) = stmt {
                if let Some(json) = self.extract_req_body_from_expr(&expr_stmt.expr) {
                    return Some(json);
                }
            }

            // Check if statements for validation logic
            if let Stmt::If(if_stmt) = stmt {
                if let Some(json) = self.extract_req_body_from_condition(&if_stmt.test) {
                    return Some(json);
                }
            }
        }

        None
    }

    // Handle destructuring patterns: const { field1, field2 } = req.body
    fn extract_req_body_from_var_decl(&self, var_decl: &VarDecl) -> Option<Json> {
        for decl in &var_decl.decls {
            // Check if initialization is from req.body
            if let Some(init) = &decl.init {
                if let Expr::Member(member) = &**init {
                    if let Expr::Ident(obj) = &*member.obj {
                        if obj.sym == "req" {
                            if let MemberProp::Ident(prop) = &member.prop {
                                if prop.sym == "body" {
                                    // Found req.body assignment

                                    // Extract fields from destructuring pattern
                                    if let Pat::Object(obj_pat) = &decl.name {
                                        let mut fields = HashMap::new();

                                        for prop in &obj_pat.props {
                                            if let ObjectPatProp::Assign(assign_prop) = prop {
                                                let field_name = assign_prop.key.sym.to_string();
                                                fields.insert(field_name, Json::Null); // We don't know types yet
                                            } else if let ObjectPatProp::KeyValue(kv_prop) = prop {
                                                if let PropName::Ident(key) = &kv_prop.key {
                                                    let field_name = key.sym.to_string();
                                                    fields.insert(field_name, Json::Null);
                                                }
                                            }
                                        }

                                        if !fields.is_empty() {
                                            return Some(Json::Object(Box::new(fields)));
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }

        None
    }

    // Handle direct access: if(req.body.field)
    fn extract_req_body_from_expr(&self, expr: &Expr) -> Option<Json> {
        if let Expr::Member(member) = expr {
            if let Expr::Member(inner_member) = &*member.obj {
                if let Expr::Ident(obj) = &*inner_member.obj {
                    if obj.sym == "req" {
                        if let MemberProp::Ident(body_prop) = &inner_member.prop {
                            if body_prop.sym == "body" {
                                // Found req.body.something
                                if let MemberProp::Ident(field_prop) = &member.prop {
                                    let field_name = field_prop.sym.to_string();
                                    let mut fields = HashMap::new();
                                    fields.insert(field_name, Json::Null);
                                    return Some(Json::Object(Box::new(fields)));
                                }
                            }
                        }
                    }
                }
            }
        }

        None
    }

    // Handle validation conditions: if(!req.body.field)
    fn extract_req_body_from_condition(&self, expr: &Expr) -> Option<Json> {
        match expr {
            // Handle negation: if(!req.body.field)
            Expr::Unary(unary) => {
                if unary.op == UnaryOp::Bang {
                    return self.extract_req_body_from_expr(&unary.arg);
                }
            }

            // Handle binary operations: if(req.body.field === undefined)
            Expr::Bin(bin) => {
                if let Some(left_json) = self.extract_req_body_from_expr(&bin.left) {
                    return Some(left_json);
                }
                if let Some(right_json) = self.extract_req_body_from_expr(&bin.right) {
                    return Some(right_json);
                }
            }

            // Direct field access
            _ => return self.extract_req_body_from_expr(expr),
        }

        None
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
