use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;
use swc_common::{SourceMap, sync::Lrc};
use swc_ecma_ast::*;
use swc_ecma_visit::{Visit, VisitWith};

/// Represents a potential call site that could be an endpoint or mount
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CallSite {
    pub callee_object: String,
    pub callee_property: String,
    pub args: Vec<CallArgument>,
    pub definition: Option<String>,
    pub location: String,
}

/// Represents an argument to a call site
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CallArgument {
    pub arg_type: ArgumentType,
    pub value: Option<String>,
    pub resolved_value: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ArgumentType {
    StringLiteral,
    Identifier,
    FunctionExpression,
    ArrowFunction,
    ObjectLiteral,
    ArrayLiteral,
    TemplateLiteral,
    Other,
}

/// Framework-agnostic visitor that extracts ALL member call expressions
pub struct CallSiteExtractor {
    pub call_sites: Vec<CallSite>,
    pub variable_definitions: HashMap<String, String>,
    pub argument_values: HashMap<String, String>,
    current_file: PathBuf,
    source_map: Lrc<SourceMap>,
}

impl CallSiteExtractor {
    pub fn new(file_path: PathBuf, source_map: Lrc<SourceMap>) -> Self {
        Self {
            call_sites: Vec::new(),
            variable_definitions: HashMap::new(),
            argument_values: HashMap::new(),
            current_file: file_path,
            source_map,
        }
    }

    fn get_line_and_column(&self, span: swc_common::Span) -> (usize, usize) {
        let loc = self.source_map.lookup_char_pos(span.lo);
        (loc.line, loc.col_display)
    }

    fn expr_to_string(&self, expr: &Expr) -> String {
        match expr {
            Expr::Ident(ident) => ident.sym.to_string(),
            Expr::Member(member) => {
                if let (Expr::Ident(obj), MemberProp::Ident(prop)) = (&*member.obj, &member.prop) {
                    format!("{}.{}", obj.sym, prop.sym)
                } else {
                    "member_expr".to_string()
                }
            }
            Expr::Lit(Lit::Str(s)) => s.value.to_string(),
            Expr::Lit(Lit::Num(n)) => n.value.to_string(),
            _ => "...".to_string(),
        }
    }

    fn extract_template_literal(&self, tpl: &Tpl) -> String {
        let mut value = String::new();
        for (i, quasi) in tpl.quasis.iter().enumerate() {
            value.push_str(&quasi.raw);
            if i < tpl.exprs.len() {
                let expr = &tpl.exprs[i];
                let expr_str = self.expr_to_string(expr);
                value.push_str(&format!("${{{}}}", expr_str));
            }
        }
        value
    }

    fn extract_argument(&self, expr: &Expr) -> CallArgument {
        match expr {
            Expr::Lit(Lit::Str(str_lit)) => CallArgument {
                arg_type: ArgumentType::StringLiteral,
                value: Some(str_lit.value.to_string()),
                resolved_value: Some(str_lit.value.to_string()),
            },
            Expr::Ident(ident) => {
                let name = ident.sym.to_string();
                let resolved = self.argument_values.get(&name).cloned();
                CallArgument {
                    arg_type: ArgumentType::Identifier,
                    value: Some(name),
                    resolved_value: resolved,
                }
            }
            Expr::Fn(_) => CallArgument {
                arg_type: ArgumentType::FunctionExpression,
                value: None,
                resolved_value: None,
            },
            Expr::Arrow(_) => CallArgument {
                arg_type: ArgumentType::ArrowFunction,
                value: None,
                resolved_value: None,
            },
            Expr::Object(_) => CallArgument {
                arg_type: ArgumentType::ObjectLiteral,
                value: None,
                resolved_value: None,
            },
            Expr::Array(_) => CallArgument {
                arg_type: ArgumentType::ArrayLiteral,
                value: None,
                resolved_value: None,
            },
            Expr::Tpl(tpl) => CallArgument {
                arg_type: ArgumentType::TemplateLiteral,
                value: Some(self.extract_template_literal(tpl)),
                resolved_value: None,
            },
            _ => CallArgument {
                arg_type: ArgumentType::Other,
                value: None,
                resolved_value: None,
            },
        }
    }
}

impl Visit for CallSiteExtractor {
    fn visit_var_decl(&mut self, var_decl: &VarDecl) {
        for decl in &var_decl.decls {
            if let Pat::Ident(ident) = &decl.name {
                let var_name = ident.id.sym.to_string();

                if let Some(init) = &decl.init {
                    match &**init {
                        Expr::Lit(Lit::Str(str_lit)) => {
                            self.argument_values
                                .insert(var_name.clone(), str_lit.value.to_string());
                        }
                        Expr::Tpl(tpl) => {
                            self.argument_values
                                .insert(var_name.clone(), self.extract_template_literal(tpl));
                        }
                        _ => {}
                    }

                    let definition = match &**init {
                        Expr::Call(call) => {
                            if let Callee::Expr(callee) = &call.callee {
                                match &**callee {
                                    Expr::Ident(func_ident) => {
                                        format!("{}()", func_ident.sym)
                                    }
                                    Expr::Member(member) => {
                                        if let (Expr::Ident(obj), MemberProp::Ident(prop)) =
                                            (&*member.obj, &member.prop)
                                        {
                                            format!("{}.{}()", obj.sym, prop.sym)
                                        } else {
                                            "member_call()".to_string()
                                        }
                                    }
                                    _ => "call_expression()".to_string(),
                                }
                            } else {
                                "function_call()".to_string()
                            }
                        }
                        Expr::New(new_expr) => {
                            if let Expr::Ident(ident) = &*new_expr.callee {
                                format!("new {}()", ident.sym)
                            } else {
                                "new_expression()".to_string()
                            }
                        }
                        Expr::Ident(ident) => {
                            format!("= {}", ident.sym)
                        }
                        Expr::Lit(Lit::Str(str_lit)) => {
                            format!("= \"{}\"", str_lit.value)
                        }
                        Expr::Tpl(tpl) => {
                            format!("= `{}`", self.extract_template_literal(tpl))
                        }
                        _ => "variable_assignment".to_string(),
                    };

                    self.variable_definitions.insert(var_name, definition);
                }
            }
        }

        var_decl.visit_children_with(self);
    }

    fn visit_call_expr(&mut self, call: &CallExpr) {
        // Extract ALL member call expressions AND direct function calls without filtering
        if let Callee::Expr(callee_expr) = &call.callee {
            let (object_name, property_name) = match &**callee_expr {
                Expr::Member(member) => {
                    if let (Expr::Ident(obj_ident), MemberProp::Ident(prop_ident)) =
                        (&*member.obj, &member.prop)
                    {
                        (obj_ident.sym.to_string(), prop_ident.sym.to_string())
                    } else {
                        return;
                    }
                }
                Expr::Ident(ident) => ("global".to_string(), ident.sym.to_string()),
                _ => return,
            };

            let args = call
                .args
                .iter()
                .map(|arg| self.extract_argument(&arg.expr))
                .collect();

            let (line, column) = self.get_line_and_column(call.span);
            let location = format!("{}:{}:{}", self.current_file.display(), line, column);

            // For member calls, look up definition of object
            // For global calls, look up definition of function
            let definition_key = if object_name == "global" {
                &property_name
            } else {
                &object_name
            };
            let definition = self.variable_definitions.get(definition_key).cloned();

            self.call_sites.push(CallSite {
                callee_object: object_name,
                callee_property: property_name,
                args,
                definition,
                location,
            });
        }

        call.visit_children_with(self);
    }
}

/// Service for extracting call sites from multiple files
pub struct CallSiteExtractionService {
    call_sites: Vec<CallSite>,
}

impl CallSiteExtractionService {
    pub fn new() -> Self {
        Self {
            call_sites: Vec::new(),
        }
    }

    pub fn extract_from_visitors(&mut self, visitors: Vec<CallSiteExtractor>) {
        for visitor in visitors {
            self.call_sites.extend(visitor.call_sites);
        }
    }

    pub fn get_call_sites(&self) -> &[CallSite] {
        &self.call_sites
    }

    /// Prepare call sites for LLM classification with framework context
    pub fn prepare_for_classification(&self) -> serde_json::Value {
        serde_json::json!({
            "call_sites": self.call_sites,
            "total_count": self.call_sites.len()
        })
    }
}
