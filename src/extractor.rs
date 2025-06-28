use crate::visitor::{Call, ImportedSymbol, Json};
use std::{collections::HashMap, path::PathBuf};
use swc_common::{SourceMap, sync::Lrc};
use swc_ecma_ast::*;

pub trait CoreExtractor {
    fn get_source_map(&self) -> &Lrc<SourceMap>;
    fn resolve_variable(&self, _name: &str) -> Option<&Expr> {
        None
    }

    fn extract_env_var_from_expr(&self, expr: &Expr) -> Option<String> {
        // Check if expression is process.env.X
        if let Expr::Member(member_expr) = expr {
            if let Expr::Member(process_env) = &*member_expr.obj {
                // Check if obj is "process"
                if let Expr::Ident(process) = &*process_env.obj {
                    if process.sym == *"process" {
                        // Check if prop is "env"
                        if let MemberProp::Ident(env) = &process_env.prop {
                            if env.sym == *"env" {
                                // It's process.env, extract the variable name
                                if let MemberProp::Ident(var_name) = &member_expr.prop {
                                    return Some(var_name.sym.to_string());
                                }
                            }
                        }
                    }
                }
            }
        }
        None
    }

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

    fn create_call_from_fetch(&self, call: &CallExpr) -> Option<Call> {
        let route = self.extract_route_from_call_arg(&call.args)?;
        let method = self
            .extract_method_from_call_args(call)
            .unwrap_or_else(|| "GET".to_string());
        let request_body = self.extract_request_body_from_fetch(call);

        Some(Call {
            route,
            method,
            response: Json::Null, // Will be populated later if needed
            request: request_body,
            response_type: None, // Will be populated when we find the type annotation
            request_type: None,  // Could be extracted from fetch body
            call_file: PathBuf::new(), // Will be set by caller
            call_id: None,       // Will be set by caller with unique identifier
            call_number: None,   // Will be set by caller
            common_type_name: None, // Will be set by caller
        })
    }

    fn extract_call_from_expr(&self, expr: &Expr) -> Option<Call> {
        match expr {
            Expr::Await(await_expr) => {
                if let Expr::Call(call) = &*await_expr.arg {
                    if let Callee::Expr(callee_expr) = &call.callee {
                        if let Expr::Ident(ident) = &**callee_expr {
                            if ident.sym == "fetch" {
                                return self.create_call_from_fetch(call);
                            }
                        }
                    }
                }
            }
            Expr::Call(call) => {
                if let Callee::Expr(callee_expr) = &call.callee {
                    if let Expr::Ident(ident) = &**callee_expr {
                        if ident.sym == "fetch" {
                            return self.create_call_from_fetch(call);
                        }
                    }
                }
            }
            _ => {}
        }
        None
    }

    fn is_json_method_call(&self, call: &CallExpr) -> bool {
        if let Callee::Expr(callee_expr) = &call.callee {
            if let Expr::Member(member) = &**callee_expr {
                if let MemberProp::Ident(method) = &member.prop {
                    return method.sym == "json";
                }
            }
        }
        false
    }

    /// Extract fetch calls from arrow function with file context
    fn extract_fetch_calls_from_arrow_with_file(
        &self,
        arrow: &swc_ecma_ast::ArrowExpr,
        file_path: &PathBuf,
    ) -> Vec<crate::visitor::Call> {
        use swc_ecma_ast::*;
        let mut fetch_calls = Vec::new();

        match &*arrow.body {
            BlockStmtOrExpr::BlockStmt(block) => {
                self.extract_calls_from_block_with_file(block, &mut fetch_calls, file_path);
            }
            BlockStmtOrExpr::Expr(expr) => {
                if let Some(mut call) = self.extract_call_from_expr(expr) {
                    call.call_file = file_path.clone();
                    fetch_calls.push(call);
                }
            }
        }

        fetch_calls
    }

    /// Extract fetch calls from function declaration with file context
    fn extract_fetch_calls_from_function_decl_with_file(
        &self,
        fn_decl: &swc_ecma_ast::FnDecl,
        file_path: &PathBuf,
    ) -> Vec<crate::visitor::Call> {
        let mut fetch_calls = Vec::new();

        if let Some(body) = &fn_decl.function.body {
            self.extract_calls_from_block_with_file(body, &mut fetch_calls, file_path);
        }

        fetch_calls
    }

    /// Extract fetch calls from function expression with file context
    fn extract_fetch_calls_from_function_expr_with_file(
        &self,
        fn_expr: &swc_ecma_ast::FnExpr,
        file_path: &PathBuf,
    ) -> Vec<crate::visitor::Call> {
        let mut fetch_calls = Vec::new();

        if let Some(body) = &fn_expr.function.body {
            self.extract_calls_from_block_with_file(body, &mut fetch_calls, file_path);
        }

        fetch_calls
    }

    fn extract_calls_from_block_with_file(
        &self,
        block: &swc_ecma_ast::BlockStmt,
        calls: &mut Vec<crate::visitor::Call>,
        file_path: &PathBuf,
    ) {
        use swc_ecma_ast::*;
        let mut pending_fetch: Option<crate::visitor::Call> = None;
        let cm = self.get_source_map();

        for stmt in &block.stmts {
            // First, recursively extract calls from nested statements
            self.extract_calls_from_stmt_recursive(stmt, calls, file_path);

            // Then handle the special case of variable declarations with type annotations
            if let Stmt::Decl(Decl::Var(var_decl)) = stmt {
                for decl_item in &var_decl.decls {
                    if let Some(init) = &decl_item.init {
                        if let Some(mut call) = self.extract_call_from_expr(init) {
                            call.call_file = file_path.clone();
                            if let Some(prev_fetch) = pending_fetch.take() {
                                calls.push(prev_fetch);
                            }
                            pending_fetch = Some(call);
                        }
                    }

                    if let Pat::Ident(ident) = &decl_item.name {
                        if let Some(type_ann) = &ident.type_ann {
                            if let Some(init_expr) = &decl_item.init {
                                let is_json_await_or_call = match &**init_expr {
                                    Expr::Await(await_expr) => {
                                        if let Expr::Call(call_expr) = &*await_expr.arg {
                                            self.is_json_method_call(call_expr)
                                        } else {
                                            false
                                        }
                                    }
                                    Expr::Call(call_expr) => self.is_json_method_call(call_expr),
                                    _ => false,
                                };

                                if is_json_await_or_call {
                                    if let Some(ref mut fetch) = pending_fetch {
                                        let alias =
                                            crate::analyzer::Analyzer::generate_common_type_alias_name(
                                                &fetch.route,
                                                &fetch.method,
                                                false,
                                                true, // is_consumer = true (fetch calls are consumers)
                                            );
                                        if let Some(type_ref) =
                                            crate::analyzer::Analyzer::create_type_reference_from_swc(
                                                type_ann, cm, file_path, alias,
                                            )
                                        {
                                            fetch.response_type = Some(type_ref);
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }

        if let Some(fetch) = pending_fetch.take() {
            calls.push(fetch);
        }
    }

    // Add this new helper method to recursively extract calls from all statement types
    fn extract_calls_from_stmt_recursive(
        &self,
        stmt: &swc_ecma_ast::Stmt,
        calls: &mut Vec<crate::visitor::Call>,
        file_path: &PathBuf,
    ) {
        use swc_ecma_ast::*;

        match stmt {
            Stmt::Expr(expr_stmt) => {
                if let Some(mut call) = self.extract_call_from_expr(&expr_stmt.expr) {
                    call.call_file = file_path.clone();
                    calls.push(call);
                }
            }
            Stmt::Return(return_stmt) => {
                if let Some(expr) = &return_stmt.arg {
                    if let Some(mut call) = self.extract_call_from_expr(expr) {
                        call.call_file = file_path.clone();
                        calls.push(call);
                    }
                }
            }
            Stmt::Block(block) => {
                self.extract_calls_from_block_with_file(block, calls, file_path);
            }
            Stmt::Try(try_stmt) => {
                // Process statements in try block
                self.extract_calls_from_block_with_file(&try_stmt.block, calls, file_path);

                // Process catch block if present
                if let Some(handler) = &try_stmt.handler {
                    self.extract_calls_from_block_with_file(&handler.body, calls, file_path);
                }

                // Process finally block if present
                if let Some(finalizer) = &try_stmt.finalizer {
                    self.extract_calls_from_block_with_file(finalizer, calls, file_path);
                }
            }
            Stmt::Decl(Decl::Var(var_decl)) => {
                // Handle variable declarations which might contain fetch calls
                for var in &var_decl.decls {
                    if let Some(init) = &var.init {
                        if let Some(mut call) = self.extract_call_from_expr(init) {
                            call.call_file = file_path.clone();
                            calls.push(call);
                        }
                    }
                }
            }
            Stmt::If(if_stmt) => {
                // Check condition for fetch calls
                if let Some(mut call) = self.extract_call_from_expr(&if_stmt.test) {
                    call.call_file = file_path.clone();
                    calls.push(call);
                }

                // Process consequent
                self.extract_calls_from_stmt_recursive(&if_stmt.cons, calls, file_path);

                // Process alternate if present
                if let Some(alt) = &if_stmt.alt {
                    self.extract_calls_from_stmt_recursive(alt, calls, file_path);
                }
            }
            Stmt::While(while_stmt) => {
                if let Some(mut call) = self.extract_call_from_expr(&while_stmt.test) {
                    call.call_file = file_path.clone();
                    calls.push(call);
                }
                self.extract_calls_from_stmt_recursive(&while_stmt.body, calls, file_path);
            }
            Stmt::For(for_stmt) => {
                if let Some(init) = &for_stmt.init {
                    match init {
                        VarDeclOrExpr::VarDecl(var_decl) => {
                            for var in &var_decl.decls {
                                if let Some(init_expr) = &var.init {
                                    if let Some(mut call) = self.extract_call_from_expr(init_expr) {
                                        call.call_file = file_path.clone();
                                        calls.push(call);
                                    }
                                }
                            }
                        }
                        VarDeclOrExpr::Expr(expr) => {
                            if let Some(mut call) = self.extract_call_from_expr(expr) {
                                call.call_file = file_path.clone();
                                calls.push(call);
                            }
                        }
                    }
                }
                if let Some(test) = &for_stmt.test {
                    if let Some(mut call) = self.extract_call_from_expr(test) {
                        call.call_file = file_path.clone();
                        calls.push(call);
                    }
                }
                if let Some(update) = &for_stmt.update {
                    if let Some(mut call) = self.extract_call_from_expr(update) {
                        call.call_file = file_path.clone();
                        calls.push(call);
                    }
                }
                self.extract_calls_from_stmt_recursive(&for_stmt.body, calls, file_path);
            }
            Stmt::ForIn(for_in_stmt) => {
                if let Some(mut call) = self.extract_call_from_expr(&for_in_stmt.right) {
                    call.call_file = file_path.clone();
                    calls.push(call);
                }
                self.extract_calls_from_stmt_recursive(&for_in_stmt.body, calls, file_path);
            }
            Stmt::ForOf(for_of_stmt) => {
                if let Some(mut call) = self.extract_call_from_expr(&for_of_stmt.right) {
                    call.call_file = file_path.clone();
                    calls.push(call);
                }
                self.extract_calls_from_stmt_recursive(&for_of_stmt.body, calls, file_path);
            }
            Stmt::Switch(switch_stmt) => {
                if let Some(mut call) = self.extract_call_from_expr(&switch_stmt.discriminant) {
                    call.call_file = file_path.clone();
                    calls.push(call);
                }
                for case in &switch_stmt.cases {
                    if let Some(test) = &case.test {
                        if let Some(mut call) = self.extract_call_from_expr(test) {
                            call.call_file = file_path.clone();
                            calls.push(call);
                        }
                    }
                    for stmt in &case.cons {
                        self.extract_calls_from_stmt_recursive(stmt, calls, file_path);
                    }
                }
            }
            // Add other statement types as needed
            _ => {}
        }
    }

    // New function to extract request body from fetch calls
    fn extract_request_body_from_fetch(&self, call: &CallExpr) -> Option<Json> {
        // Fetch calls have format: fetch(url, options)
        if call.args.len() < 2 {
            return None; // No options object
        }

        // Get the options object (second argument)
        match &*call.args[1].expr {
            Expr::Object(obj) => {
                // Look for body property in options
                for prop in &obj.props {
                    if let PropOrSpread::Prop(boxed_prop) = prop {
                        if let Prop::KeyValue(kv) = &**boxed_prop {
                            // Check if the property is "body"
                            if let PropName::Ident(key_ident) = &kv.key {
                                if key_ident.sym == "body" {
                                    // Extract the body value
                                    match &*kv.value {
                                        // Handle JSON.stringify(...) case
                                        Expr::Call(body_call) => {
                                            if let Callee::Expr(callee_expr) = &body_call.callee {
                                                if let Expr::Member(member) = &**callee_expr {
                                                    if let Expr::Ident(obj) = &*member.obj {
                                                        if obj.sym == "JSON" {
                                                            if let MemberProp::Ident(method) =
                                                                &member.prop
                                                            {
                                                                if method.sym == "stringify" {
                                                                    // Get the object being stringified
                                                                    if let Some(arg) =
                                                                        body_call.args.get(0)
                                                                    {
                                                                        // Convert the argument to Json
                                                                        return Some(
                                                                            self.expr_to_json(
                                                                                &arg.expr,
                                                                            ),
                                                                        );
                                                                    }
                                                                }
                                                            }
                                                        }
                                                    }
                                                }
                                            }
                                        }
                                        // Direct object literal case (unlikely but possible)
                                        _ => return Some(self.expr_to_json(&kv.value)),
                                    }
                                }
                            }
                        }
                    }
                }
            }
            _ => {}
        }

        None
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

        let mut route = match &*args[0].expr {
            Expr::Lit(Lit::Str(str_lit)) => Some(str_lit.value.to_string()),
            Expr::Tpl(tpl) => self.process_template(tpl),
            Expr::Ident(ident) => {
                if let Some(resolved) = self.resolve_variable(&ident.sym.to_string()) {
                    match resolved {
                        Expr::Lit(Lit::Str(str_lit)) => Some(str_lit.value.to_string()),
                        Expr::Tpl(tpl) => self.process_template(tpl),
                        _ => None,
                    }
                } else {
                    None
                }
            }
            _ => None,
        }?;

        // Strip query parameters - only keep the path portion
        // This can be handled with type checking and doesn't ultimately affect the route path
        if let Some(query_start) = route.find('?') {
            route = route[..query_start].to_string();
        }

        Some(route)
    }

    fn process_template(&self, tpl: &Tpl) -> Option<String> {
        let mut route = String::new();

        // Concatenate the raw parts of the template with appropriate placeholders
        for (i, quasi) in tpl.quasis.iter().enumerate() {
            // Add the raw text part
            route.push_str(&quasi.raw);

            // If there's an expression after this quasi
            if i < tpl.exprs.len() {
                // Check if it's an environment variable
                if let Some(env_var) = self.extract_env_var_from_expr(&tpl.exprs[i]) {
                    // Use a special format for env vars
                    route.push_str(&format!("ENV_VAR:{}:", env_var));
                }
                // Check if it's a variable reference
                else if let Expr::Ident(ident) = &*tpl.exprs[i] {
                    // For variables, check if they're from environment variables
                    if let Some(resolved) = self.resolve_variable(&ident.sym.to_string()) {
                        if let Some(env_var) = self.extract_env_var_from_expr(resolved) {
                            route.push_str(&format!("ENV_VAR:{}:", env_var));
                        } else {
                            // Regular variable, use parameter placeholder
                            route.push_str(":param");
                        }
                    } else {
                        // Can't resolve, use generic parameter
                        route.push_str(":param");
                    }
                } else {
                    // For other expressions, use generic parameter placeholder
                    route.push_str(":param");
                }
            }
        }

        Some(route)
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

    // Async call extraction methods for Gemini Flash integration
    fn extract_async_calls_from_function(
        &self,
        func: &crate::visitor::FunctionDefinition,
    ) -> Vec<crate::gemini_service::AsyncCallContext> {
        use swc_common::{SourceMapper, Spanned};

        let mut contexts = Vec::new();

        // Get the function source code for LLM analysis
        let source_map = self.get_source_map();

        let (function_source, function_name) = match &func.node_type {
            crate::visitor::FunctionNodeType::ArrowFunction(arrow) => {
                let source = source_map.span_to_snippet(arrow.span).unwrap_or_default();
                (source, "arrow_function".to_string())
            }
            crate::visitor::FunctionNodeType::FunctionDeclaration(decl) => {
                let source = source_map.span_to_snippet(decl.span()).unwrap_or_default();
                let name = decl.ident.sym.to_string();
                (source, name)
            }
            crate::visitor::FunctionNodeType::FunctionExpression(expr) => {
                let source = source_map.span_to_snippet(expr.span()).unwrap_or_default();
                let name = expr
                    .ident
                    .as_ref()
                    .map(|i| i.sym.to_string())
                    .unwrap_or("anonymous".to_string());
                (source, name)
            }
            crate::visitor::FunctionNodeType::Placeholder => {
                // In CI mode, AST is not available, skip extraction
                return contexts;
            }
        };

        // Only create context if we found async patterns in the source
        if function_source.contains("await")
            || function_source.contains(".then")
            || function_source.contains("fetch")
            || function_source.contains("axios")
        {
            contexts.push(crate::gemini_service::AsyncCallContext {
                kind: "function_analysis".to_string(),
                function_source,
                file: func.file_path.to_string_lossy().to_string(),
                line: 1, // We'll let Gemini figure out the specific line
                function_name,
            });
        }

        contexts
    }
}

pub trait RouteExtractor: CoreExtractor {
    fn get_route_handler_name(&self, expr: &Expr) -> Option<String>;
    fn resolve_template_string(&self, tpl: &Tpl) -> Option<String>;
    fn get_imported_symbols(&self) -> &HashMap<String, ImportedSymbol>;
    fn get_response_fields(&self) -> &HashMap<String, Json>;
    fn add_imported_handler(
        &mut self,
        route: String,
        method: String,
        handler: String,
        source: String,
    );

    fn extract_string_from_expr(&self, expr: &Expr) -> Option<String> {
        match expr {
            Expr::Lit(Lit::Str(str_lit)) => Some(str_lit.value.to_string()),
            Expr::Tpl(tpl) => self.resolve_template_string(tpl),
            Expr::Ident(ident) => {
                // Try to resolve the variable
                let var_name = ident.sym.to_string();
                if let Some(resolved_expr) = self.resolve_variable(&var_name) {
                    return self.extract_string_from_expr(resolved_expr);
                }
                None
            }
            // Handle other expression types that could resolve to strings
            // For example, member expressions like config.API_URL
            Expr::Member(member) => self.extract_string_from_member_expr(member),
            _ => None,
        }
    }

    fn extract_string_from_member_expr(&self, member: &MemberExpr) -> Option<String> {
        // For simple cases like obj.prop where obj is a variable
        if let Expr::Ident(obj_ident) = &*member.obj {
            let obj_name = obj_ident.sym.to_string();

            // Try to resolve the object
            if let Some(resolved_obj) = self.resolve_variable(&obj_name) {
                // If it's an object literal, extract the property
                if let Expr::Object(obj_lit) = resolved_obj {
                    if let MemberProp::Ident(prop_ident) = &member.prop {
                        let prop_name = prop_ident.sym.to_string();

                        // Find the property in the object
                        for prop in &obj_lit.props {
                            if let PropOrSpread::Prop(box_prop) = prop {
                                if let Prop::KeyValue(kv) = &**box_prop {
                                    if let PropName::Ident(key_ident) = &kv.key {
                                        if key_ident.sym.to_string() == prop_name {
                                            return self.extract_string_from_expr(&kv.value);
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

    fn extract_endpoint(
        &mut self,
        call: &CallExpr,
        method: &str,
    ) -> Option<(String, Json, Option<Json>, String)> {
        // Get the route from the first argument
        let first_arg = call.args.get(0)?;
        let route = self.extract_string_from_expr(&first_arg.expr)?;

        let mut response_json = Json::Null;
        let mut request_json = None;
        let mut handler_name = String::from("anonymous_handler");

        // Check the second argument (handler)
        if let Some(second_arg) = call.args.get(1) {
            if let Some(name) = self.get_route_handler_name(&second_arg.expr) {
                handler_name = name.clone();

                // Check if this handler is an imported function
                if let Some(symbol) = self.get_imported_symbols().get(&handler_name) {
                    // Track this imported handler usage
                    self.add_imported_handler(
                        route.clone(),
                        method.to_string(),
                        handler_name.clone(),
                        symbol.source.clone(),
                    );

                    // Look up the response fields for this handler
                    if let Some(fields) = self.get_response_fields().get(&handler_name) {
                        response_json = fields.clone();
                    }

                    // We've handled this case, so we can return early
                    return Some((route, response_json, request_json, handler_name));
                }
            }

            match &*second_arg.expr {
                // Arrow function handler
                Expr::Arrow(arrow_expr) => {
                    handler_name = format!(
                        "{}_{}_{}",
                        route.replace('/', "_"),
                        method.to_lowercase(),
                        "arrow"
                    );

                    match &*arrow_expr.body {
                        BlockStmtOrExpr::BlockStmt(block) => {
                            // Extract request body fields using existing method
                            request_json = self.extract_req_body_fields(block);

                            // Extract response fields (using existing logic)
                            for stmt in &block.stmts {
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
                        _ => {}
                    }
                }

                // Regular function handler
                Expr::Fn(fn_expr) => {
                    handler_name = if let Some(ident) = &fn_expr.ident {
                        ident.sym.to_string()
                    } else {
                        format!(
                            "{}_{}_{}",
                            route.replace('/', "_"),
                            method.to_lowercase(),
                            "fn"
                        )
                    };

                    if let Some(body) = &fn_expr.function.body {
                        // Extract request body fields using existing method
                        request_json = self.extract_req_body_fields(body);

                        // Extract response fields (existing logic)
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

                // Imported handler
                Expr::Ident(ident) => {
                    let handler_name = ident.sym.to_string();

                    // Check if this handler is an imported function
                    if let Some(symbol) = self.get_imported_symbols().get(&handler_name) {
                        // Track this imported handler usage
                        self.add_imported_handler(
                            route.clone(),
                            method.to_string(),
                            handler_name.clone(),
                            symbol.source.clone(),
                        );

                        if let Some(fields) = self.get_response_fields().get(&handler_name) {
                            response_json = fields.clone();
                        }
                    }
                }

                _ => {}
            }
        }

        Some((route, response_json, request_json, handler_name))
    }
}
