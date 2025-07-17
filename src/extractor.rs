use crate::visitor::{ImportedSymbol, Json};
use std::collections::HashMap;
use swc_common::{SourceMap, sync::Lrc};
use swc_ecma_ast::*;

pub trait CoreExtractor {
    fn get_source_map(&self) -> &Lrc<SourceMap>;
    fn resolve_variable(&self, _name: &str) -> Option<&Expr> {
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
                let arg = call.args.first()?;
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
                                        if key_ident.sym == prop_name {
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
        let first_arg = call.args.first()?;
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

                    if let BlockStmtOrExpr::BlockStmt(block) = &*arrow_expr.body {
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
