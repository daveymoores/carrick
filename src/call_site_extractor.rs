use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;
use swc_ecma_ast::*;
use swc_ecma_visit::{Visit, VisitWith};
use swc_common::{SourceMap, sync::Lrc};

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
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ArgumentType {
    StringLiteral,
    Identifier,
    FunctionExpression,
    ArrowFunction,
    ObjectLiteral,
    ArrayLiteral,
    Other,
}

/// Framework-agnostic visitor that extracts ALL member call expressions
pub struct CallSiteExtractor {
    pub call_sites: Vec<CallSite>,
    pub variable_definitions: HashMap<String, String>,
    current_file: PathBuf,
    source_map: Lrc<SourceMap>,
}

impl CallSiteExtractor {
    pub fn new(file_path: PathBuf, source_map: Lrc<SourceMap>) -> Self {
        Self {
            call_sites: Vec::new(),
            variable_definitions: HashMap::new(),
            current_file: file_path,
            source_map,
        }
    }

    fn get_line_and_column(&self, span: swc_common::Span) -> (u32, u32) {
        let loc = self.source_map.lookup_char_pos(span.lo);
        (loc.line as u32, loc.col_display as u32)
    }

    fn extract_argument(&self, expr: &Expr) -> CallArgument {
        match expr {
            Expr::Lit(Lit::Str(str_lit)) => CallArgument {
                arg_type: ArgumentType::StringLiteral,
                value: Some(str_lit.value.to_string()),
            },
            Expr::Ident(ident) => CallArgument {
                arg_type: ArgumentType::Identifier,
                value: Some(ident.sym.to_string()),
            },
            Expr::Fn(_) => CallArgument {
                arg_type: ArgumentType::FunctionExpression,
                value: None,
            },
            Expr::Arrow(_) => CallArgument {
                arg_type: ArgumentType::ArrowFunction,
                value: None,
            },
            Expr::Object(_) => CallArgument {
                arg_type: ArgumentType::ObjectLiteral,
                value: None,
            },
            Expr::Array(_) => CallArgument {
                arg_type: ArgumentType::ArrayLiteral,
                value: None,
            },
            _ => CallArgument {
                arg_type: ArgumentType::Other,
                value: None,
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
                    let definition = match &**init {
                        Expr::Call(call) => {
                            if let Callee::Expr(callee) = &call.callee {
                                match &**callee {
                                    Expr::Ident(func_ident) => {
                                        format!("{}()", func_ident.sym)
                                    }
                                    Expr::Member(member) => {
                                        if let (Expr::Ident(obj), MemberProp::Ident(prop)) = 
                                            (&*member.obj, &member.prop) {
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
                        _ => "variable_assignment".to_string(),
                    };
                    
                    self.variable_definitions.insert(var_name, definition);
                }
            }
        }
        
        var_decl.visit_children_with(self);
    }

    fn visit_call_expr(&mut self, call: &CallExpr) {
        // Extract ALL member call expressions without filtering
        if let Callee::Expr(callee_expr) = &call.callee {
            if let Expr::Member(member) = &**callee_expr {
                if let (Expr::Ident(obj_ident), MemberProp::Ident(prop_ident)) = 
                    (&*member.obj, &member.prop) {
                    
                    let object_name = obj_ident.sym.to_string();
                    let property_name = prop_ident.sym.to_string();
                    
                    let args = call.args.iter()
                        .map(|arg| self.extract_argument(&arg.expr))
                        .collect();
                    
                    let (line, column) = self.get_line_and_column(call.span);
                    let location = format!("{}:{}:{}", 
                        self.current_file.display(), line, column);
                    
                    let definition = self.variable_definitions.get(&object_name).cloned();
                    
                    self.call_sites.push(CallSite {
                        callee_object: object_name,
                        callee_property: property_name,
                        args,
                        definition,
                        location,
                    });
                }
            }
        }
        
        call.visit_children_with(self);
    }
}

/// Service for extracting call sites from multiple files
pub struct CallSiteExtractionService {
    pub call_sites: Vec<CallSite>,
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
            "call_sites": self.call_sites
        })
    }
}

impl Default for CallSiteExtractionService {
    fn default() -> Self {
        Self::new()
    }
}