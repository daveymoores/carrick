//! Lightweight SWC Scanner - AST Gatekeeper for file-centric analysis.
//!
//! This module implements the first stage of the AST-Gated architecture:
//! scan files using SWC to find potential API call sites BEFORE sending
//! to the LLM. If no candidates are found, the file is skipped entirely
//! (Cost: $0).
//!
//! The scanner is intentionally broad - it's better to have false positives
//! (which the LLM will filter out) than false negatives (which would cause
//! missed API patterns).
//!
//! Additionally, this module provides utilities for finding type annotations
//! at specific line numbers, which is used to get accurate character positions
//! for type extraction after LLM analysis.

use std::path::Path;
use swc_common::{
    SourceMap, SourceMapper,
    errors::{ColorConfig, Handler},
    sync::Lrc,
};
use swc_ecma_ast::*;
use swc_ecma_visit::{Visit, VisitWith};

use crate::parser::parse_file;

/// Result of finding a type annotation at a specific line
#[derive(Debug, Clone)]
pub struct TypePositionInfo {
    /// Character position (0-based byte offset) where the type starts
    pub position: usize,
    /// The full type string found (e.g., "Response<User[]>")
    pub type_string: String,
    /// The file path
    pub file_path: String,
}

/// Find type annotation position on a specific line in a file.
///
/// This function parses the file with SWC and looks for type annotations
/// (like `Response<User[]>`) on the specified line number. This is used
/// to get accurate character positions after the LLM provides line numbers.
///
/// # Arguments
/// * `file_path` - Path to the TypeScript/JavaScript file
/// * `line_number` - 1-based line number to search
/// * `type_hint` - Optional type string hint from LLM (e.g., "Response<User[]>")
///
/// # Returns
/// `Some(TypePositionInfo)` if a type annotation is found, `None` otherwise
#[allow(dead_code)]
pub fn find_type_position_at_line(
    file_path: &Path,
    line_number: usize,
    type_hint: Option<&str>,
) -> Option<TypePositionInfo> {
    let source_map: Lrc<SourceMap> = Lrc::new(SourceMap::default());
    let handler =
        Handler::with_tty_emitter(ColorConfig::Never, true, false, Some(source_map.clone()));

    let module = parse_file(file_path, &source_map, &handler)?;

    let mut finder = TypePositionFinder {
        source_map: source_map.clone(),
        target_line: line_number,
        type_hint: type_hint.map(|s| s.to_string()),
        found: None,
    };

    module.visit_with(&mut finder);

    finder.found.map(|(pos, type_str)| TypePositionInfo {
        position: pos,
        type_string: type_str,
        file_path: file_path.to_string_lossy().to_string(),
    })
}

/// Find type position from file content (without reading from disk)
pub fn find_type_position_at_line_from_content(
    file_path: &str,
    content: &str,
    line_number: usize,
    type_hint: Option<&str>,
) -> Option<TypePositionInfo> {
    let source_map: Lrc<SourceMap> = Lrc::new(SourceMap::default());
    let _handler =
        Handler::with_tty_emitter(ColorConfig::Never, true, false, Some(source_map.clone()));

    // Create a source file from the content
    let source_file = source_map.new_source_file(
        swc_common::FileName::Custom(file_path.to_string()).into(),
        content.to_string(),
    );

    let lexer = swc_ecma_parser::lexer::Lexer::new(
        swc_ecma_parser::Syntax::Typescript(swc_ecma_parser::TsSyntax {
            tsx: file_path.ends_with(".tsx"),
            decorators: true,
            ..Default::default()
        }),
        swc_ecma_ast::EsVersion::Es2022,
        swc_ecma_parser::StringInput::from(&*source_file),
        None,
    );

    let mut parser = swc_ecma_parser::Parser::new_from(lexer);
    let module = match parser.parse_module() {
        Ok(m) => m,
        Err(_) => return None,
    };

    let mut finder = TypePositionFinder {
        source_map: source_map.clone(),
        target_line: line_number,
        type_hint: type_hint.map(|s| s.to_string()),
        found: None,
    };

    module.visit_with(&mut finder);

    finder.found.map(|(pos, type_str)| TypePositionInfo {
        position: pos,
        type_string: type_str,
        file_path: file_path.to_string(),
    })
}

/// AST visitor that finds type annotations at a specific line
struct TypePositionFinder {
    source_map: Lrc<SourceMap>,
    target_line: usize,
    type_hint: Option<String>,
    found: Option<(usize, String)>,
}

impl TypePositionFinder {
    fn check_type_ann(&mut self, type_ann: &TsTypeAnn) {
        let loc = self.source_map.lookup_char_pos(type_ann.span.lo);
        let line = loc.line; // 1-based

        if line == self.target_line {
            let type_str = self.get_type_string(&type_ann.type_ann);

            // If we have a hint, check if it matches
            if let Some(ref hint) = self.type_hint {
                // Check if this type matches the hint (allowing for some variation)
                if type_str.contains(hint)
                    || hint.contains(&type_str)
                    || self.types_match(&type_str, hint)
                {
                    let pos = type_ann.span.lo.0 as usize;
                    self.found = Some((pos, type_str));
                }
            } else {
                // No hint, just use the first type annotation on this line
                let pos = type_ann.span.lo.0 as usize;
                self.found = Some((pos, type_str));
            }
        }
    }

    fn types_match(&self, found: &str, hint: &str) -> bool {
        // Extract the main type name from both
        let found_main = found.split('<').next().unwrap_or(found).trim();
        let hint_main = hint.split('<').next().unwrap_or(hint).trim();
        found_main == hint_main
    }

    fn get_type_string(&self, ts_type: &TsType) -> String {
        match ts_type {
            TsType::TsTypeRef(type_ref) => {
                let name = match &type_ref.type_name {
                    TsEntityName::Ident(ident) => ident.sym.to_string(),
                    TsEntityName::TsQualifiedName(qn) => {
                        format!("{}.{}", Self::get_entity_name(&qn.left), qn.right.sym)
                    }
                };

                if let Some(type_params) = &type_ref.type_params {
                    let params: Vec<String> = type_params
                        .params
                        .iter()
                        .map(|p| self.get_type_string(p))
                        .collect();
                    format!("{}<{}>", name, params.join(", "))
                } else {
                    name
                }
            }
            TsType::TsArrayType(arr) => {
                format!("{}[]", self.get_type_string(&arr.elem_type))
            }
            TsType::TsTypeLit(lit) => {
                let members: Vec<String> = lit
                    .members
                    .iter()
                    .filter_map(|m| self.get_type_element_string(m))
                    .collect();
                format!("{{ {} }}", members.join("; "))
            }
            TsType::TsKeywordType(kw) => match kw.kind {
                TsKeywordTypeKind::TsStringKeyword => "string".to_string(),
                TsKeywordTypeKind::TsNumberKeyword => "number".to_string(),
                TsKeywordTypeKind::TsBooleanKeyword => "boolean".to_string(),
                TsKeywordTypeKind::TsVoidKeyword => "void".to_string(),
                TsKeywordTypeKind::TsAnyKeyword => "any".to_string(),
                TsKeywordTypeKind::TsNullKeyword => "null".to_string(),
                TsKeywordTypeKind::TsUndefinedKeyword => "undefined".to_string(),
                _ => "unknown".to_string(),
            },
            TsType::TsUnionOrIntersectionType(union_or_inter) => match union_or_inter {
                TsUnionOrIntersectionType::TsUnionType(union) => {
                    let types: Vec<String> = union
                        .types
                        .iter()
                        .map(|t| self.get_type_string(t))
                        .collect();
                    types.join(" | ")
                }
                TsUnionOrIntersectionType::TsIntersectionType(inter) => {
                    let types: Vec<String> = inter
                        .types
                        .iter()
                        .map(|t| self.get_type_string(t))
                        .collect();
                    types.join(" & ")
                }
            },
            _ => "unknown".to_string(),
        }
    }

    fn get_entity_name(name: &TsEntityName) -> String {
        match name {
            TsEntityName::Ident(ident) => ident.sym.to_string(),
            TsEntityName::TsQualifiedName(qn) => {
                format!("{}.{}", Self::get_entity_name(&qn.left), qn.right.sym)
            }
        }
    }

    fn get_type_element_string(&self, elem: &TsTypeElement) -> Option<String> {
        match elem {
            TsTypeElement::TsPropertySignature(prop) => {
                let key = match &*prop.key {
                    Expr::Ident(ident) => ident.sym.to_string(),
                    _ => return None,
                };
                let type_str = prop
                    .type_ann
                    .as_ref()
                    .map(|ta| self.get_type_string(&ta.type_ann))
                    .unwrap_or_else(|| "any".to_string());
                Some(format!("{}: {}", key, type_str))
            }
            _ => None,
        }
    }
}

impl Visit for TypePositionFinder {
    fn visit_ts_type_ann(&mut self, type_ann: &TsTypeAnn) {
        if self.found.is_none() {
            self.check_type_ann(type_ann);
        }
        // Continue visiting children
        type_ann.visit_children_with(self);
    }

    fn visit_param(&mut self, param: &Param) {
        // Check parameter type annotations (e.g., res: Response<User[]>)
        if self.found.is_none() {
            if let Pat::Ident(ident) = &param.pat {
                if let Some(type_ann) = &ident.type_ann {
                    self.check_type_ann(type_ann);
                }
            }
        }
        param.visit_children_with(self);
    }

    fn visit_arrow_expr(&mut self, arrow: &ArrowExpr) {
        // Check arrow function parameter types
        if self.found.is_none() {
            for param in &arrow.params {
                if let Pat::Ident(ident) = param {
                    if let Some(type_ann) = &ident.type_ann {
                        self.check_type_ann(type_ann);
                    }
                }
            }
        }
        arrow.visit_children_with(self);
    }
}

/// A candidate API call site detected by the SWC scanner.
/// This is passed as a "hint" to the LLM to ensure 100% recall.
#[derive(Debug, Clone)]
pub struct CandidateTarget {
    /// 1-based line number where the call was detected
    pub line_number: usize,
    /// The callee object (e.g., "app", "router", "fetch")
    pub callee_object: String,
    /// The callee property/method (e.g., "get", "post", "use")
    pub callee_property: Option<String>,
    /// A snippet of the code at this location
    pub code_snippet: String,
}

impl CandidateTarget {
    /// Format as a hint string for the LLM prompt
    pub fn format_hint(&self) -> String {
        match &self.callee_property {
            Some(prop) => format!(
                "- Line {}: {}.{}(...) - `{}`",
                self.line_number, self.callee_object, prop, self.code_snippet
            ),
            None => format!(
                "- Line {}: {}(...) - `{}`",
                self.line_number, self.callee_object, self.code_snippet
            ),
        }
    }
}

/// Result of scanning a file for API candidates
#[derive(Debug)]
pub struct ScanResult {
    /// List of candidate API call sites
    pub candidates: Vec<CandidateTarget>,
    /// Whether the file should be analyzed (has candidates)
    pub should_analyze: bool,
}

/// Lightweight SWC-based scanner for detecting potential API patterns.
///
/// This scanner looks for method call expressions that match common
/// API patterns across frameworks. It's intentionally broad to avoid
/// missing any potential API calls.
pub struct SwcScanner {
    source_map: Lrc<SourceMap>,
}

impl Default for SwcScanner {
    fn default() -> Self {
        Self::new()
    }
}

impl SwcScanner {
    pub fn new() -> Self {
        Self {
            source_map: Lrc::new(SourceMap::default()),
        }
    }

    /// Scan a file for potential API call sites.
    ///
    /// Returns a ScanResult with candidates and whether the file should be analyzed.
    /// If no candidates are found, `should_analyze` is false and the file can be skipped.
    #[allow(dead_code)]
    pub fn scan_file(&self, file_path: &Path) -> ScanResult {
        let handler = Handler::with_tty_emitter(
            ColorConfig::Never,
            true,
            false,
            Some(self.source_map.clone()),
        );

        let module = match parse_file(file_path, &self.source_map, &handler) {
            Some(m) => m,
            None => {
                return ScanResult {
                    candidates: Vec::new(),
                    should_analyze: false,
                };
            }
        };

        let mut visitor = CandidateVisitor::new(self.source_map.clone());
        module.visit_with(&mut visitor);

        let should_analyze = !visitor.candidates.is_empty();

        ScanResult {
            candidates: visitor.candidates,
            should_analyze,
        }
    }

    /// Scan file content directly (useful for testing or when content is already loaded).
    pub fn scan_content(&self, file_path: &Path, content: &str) -> ScanResult {
        use swc_common::{FileName, GLOBALS, Globals, Mark};
        use swc_ecma_parser::{Parser, StringInput, Syntax, lexer::Lexer};
        use swc_ecma_transforms_base::resolver;
        use swc_ecma_visit::VisitMutWith;

        // Determine syntax based on file extension
        let (syntax, is_typescript) = if let Some(ext) = file_path.extension() {
            match ext.to_string_lossy().as_ref() {
                "ts" | "tsx" => (Syntax::Typescript(Default::default()), true),
                _ => (Syntax::Es(Default::default()), false),
            }
        } else {
            (Syntax::Es(Default::default()), false)
        };

        let source_file = self.source_map.new_source_file(
            Lrc::new(FileName::Real(file_path.to_path_buf())),
            content.to_string(),
        );

        let lexer = Lexer::new(
            syntax,
            Default::default(),
            StringInput::from(&*source_file),
            None,
        );
        let mut parser = Parser::new_from(lexer);

        let mut module = match parser.parse_module() {
            Ok(m) => m,
            Err(_) => {
                return ScanResult {
                    candidates: Vec::new(),
                    should_analyze: false,
                };
            }
        };

        // Apply resolver for proper scope handling
        GLOBALS.set(&Globals::new(), || {
            let unresolved_mark = Mark::new();
            let top_level_mark = Mark::new();
            let mut pass = resolver(unresolved_mark, top_level_mark, is_typescript);
            module.visit_mut_with(&mut pass);
        });

        let mut visitor = CandidateVisitor::new(self.source_map.clone());
        module.visit_with(&mut visitor);

        let should_analyze = !visitor.candidates.is_empty();

        ScanResult {
            candidates: visitor.candidates,
            should_analyze,
        }
    }
}

/// Visitor that collects potential API call sites.
struct CandidateVisitor {
    candidates: Vec<CandidateTarget>,
    source_map: Lrc<SourceMap>,
}

impl CandidateVisitor {
    fn new(source_map: Lrc<SourceMap>) -> Self {
        Self {
            candidates: Vec::new(),
            source_map,
        }
    }

    /// Check if an identifier looks like an API-related object
    fn is_potential_api_object(&self, name: &str) -> bool {
        // Common API object patterns (framework-agnostic)
        let api_objects = [
            // Generic router/app patterns
            "app",
            "router",
            "server",
            "api",
            "route",
            "routes",
            // HTTP client patterns
            "fetch",
            "axios",
            "http",
            "https",
            "request",
            "client",
            "response",
            "res",
            "resp",
            // Common variations
            "apiRouter",
            "appRouter",
            "mainRouter",
            "authRouter",
            "userRouter",
            "v1Router",
            "v2Router",
        ];

        // Check exact matches
        if api_objects.contains(&name) {
            return true;
        }

        // Check if name ends with common API suffixes
        let lower = name.to_lowercase();
        lower.ends_with("router")
            || lower.ends_with("route")
            || lower.ends_with("routes")
            || lower.ends_with("app")
            || lower.ends_with("server")
            || lower.ends_with("api")
            || lower.ends_with("client")
            || lower.ends_with("handler")
            || lower.ends_with("controller")
    }

    /// Check if a method name looks like an API method
    fn is_potential_api_method(&self, name: &str) -> bool {
        let api_methods = [
            // HTTP methods
            "get",
            "post",
            "put",
            "delete",
            "patch",
            "head",
            "options",
            "all",
            // Mounting/middleware
            "use",
            "mount",
            "register",
            "plugin",
            "route",
            // Data fetching
            "fetch",
            "json",
            "text",
            "blob",
            "send",
            "request",
            // Common framework patterns
            "listen",
            "handle",
            "handler",
            "middleware",
            "define",
        ];

        api_methods.contains(&name.to_lowercase().as_str())
    }

    /// Check if this is a global fetch call
    fn is_global_fetch(&self, callee: &Callee) -> bool {
        if let Callee::Expr(expr) = callee {
            if let Expr::Ident(ident) = &**expr {
                return ident.sym.as_ref() == "fetch";
            }
        }
        false
    }

    /// Extract a code snippet for the given span
    fn get_code_snippet(&self, span: swc_common::Span) -> String {
        self.source_map
            .span_to_snippet(span)
            .unwrap_or_else(|_| "<snippet unavailable>".to_string())
            .lines()
            .next()
            .unwrap_or("")
            .chars()
            .take(80)
            .collect::<String>()
    }

    /// Get line number from span
    fn get_line_number(&self, span: swc_common::Span) -> usize {
        self.source_map.lookup_char_pos(span.lo).line
    }

    /// Extract callee object name from expression
    fn extract_callee_object(expr: &Expr) -> Option<String> {
        match expr {
            Expr::Ident(ident) => Some(ident.sym.to_string()),
            Expr::Member(member) => Self::extract_callee_object(&member.obj),
            Expr::Call(call) => {
                // Handle chained calls like createApp().get()
                if let Callee::Expr(callee_expr) = &call.callee {
                    Self::extract_callee_object(callee_expr)
                } else {
                    None
                }
            }
            _ => None,
        }
    }
}

impl Visit for CandidateVisitor {
    fn visit_call_expr(&mut self, call: &CallExpr) {
        // Check for global fetch
        if self.is_global_fetch(&call.callee) {
            let line_number = self.get_line_number(call.span);
            let code_snippet = self.get_code_snippet(call.span);

            self.candidates.push(CandidateTarget {
                line_number,
                callee_object: "fetch".to_string(),
                callee_property: None,
                code_snippet,
            });
        }

        // Check for method calls (obj.method())
        if let Callee::Expr(callee_expr) = &call.callee {
            if let Expr::Member(member) = &**callee_expr {
                // Extract method name
                let method_name = match &member.prop {
                    MemberProp::Ident(ident) => Some(ident.sym.to_string()),
                    MemberProp::Computed(computed) => {
                        if let Expr::Lit(Lit::Str(s)) = &*computed.expr {
                            Some(s.value.to_string())
                        } else {
                            None
                        }
                    }
                    MemberProp::PrivateName(_) => None,
                };

                if let Some(method) = method_name {
                    // Extract object name
                    let obj_name = Self::extract_callee_object(&member.obj);

                    // Check if this looks like an API call
                    let is_api_call = match &obj_name {
                        Some(name) => {
                            self.is_potential_api_object(name)
                                || self.is_potential_api_method(&method)
                        }
                        None => self.is_potential_api_method(&method),
                    };

                    if is_api_call {
                        let line_number = self.get_line_number(call.span);
                        let code_snippet = self.get_code_snippet(call.span);

                        self.candidates.push(CandidateTarget {
                            line_number,
                            callee_object: obj_name.unwrap_or_else(|| "<chain>".to_string()),
                            callee_property: Some(method),
                            code_snippet,
                        });
                    }
                }
            }
        }

        // Continue visiting child nodes
        call.visit_children_with(self);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn scan_test_content(content: &str) -> ScanResult {
        let scanner = SwcScanner::new();
        let path = PathBuf::from("test.ts");
        scanner.scan_content(&path, content)
    }

    #[test]
    fn test_detects_express_style_endpoints() {
        let content = r#"
import express from 'express';
const app = express();

app.get('/users', getUsers);
app.post('/users', createUser);
router.delete('/users/:id', deleteUser);
"#;

        let result = scan_test_content(content);
        assert!(result.should_analyze);
        assert!(result.candidates.len() >= 3);

        // Should detect app.get, app.post, router.delete
        let methods: Vec<_> = result
            .candidates
            .iter()
            .filter_map(|c| c.callee_property.as_ref())
            .collect();
        assert!(methods.contains(&&"get".to_string()));
        assert!(methods.contains(&&"post".to_string()));
        assert!(methods.contains(&&"delete".to_string()));
    }

    #[test]
    fn test_detects_fetch_calls() {
        let content = r#"
async function getData() {
    const response = await fetch('/api/data');
    const data = await response.json();
    return data;
}
"#;

        let result = scan_test_content(content);
        assert!(result.should_analyze);

        // Should detect fetch and response.json
        let has_fetch = result.candidates.iter().any(|c| c.callee_object == "fetch");
        let has_json = result.candidates.iter().any(|c| {
            c.callee_property
                .as_ref()
                .map(|p| p == "json")
                .unwrap_or(false)
        });

        assert!(has_fetch, "Should detect global fetch call");
        assert!(has_json, "Should detect response.json() call");
    }

    #[test]
    fn test_detects_router_mounts() {
        let content = r#"
import userRouter from './routes/users';
import authRouter from './routes/auth';

app.use('/api/users', userRouter);
app.use('/api/auth', authRouter);
router.use('/v1', v1Router);
"#;

        let result = scan_test_content(content);
        assert!(result.should_analyze);

        // Should detect all .use() calls
        let use_calls: Vec<_> = result
            .candidates
            .iter()
            .filter(|c| {
                c.callee_property
                    .as_ref()
                    .map(|p| p == "use")
                    .unwrap_or(false)
            })
            .collect();

        assert!(use_calls.len() >= 3, "Should detect all router mounts");
    }

    #[test]
    fn test_skips_irrelevant_files() {
        let content = r#"
// A utility file with no API patterns
export function formatDate(date: Date): string {
    return date.toISOString();
}

export function calculateSum(numbers: number[]): number {
    return numbers.reduce((a, b) => a + b, 0);
}

const arr = [1, 2, 3];
arr.map(x => x * 2);
arr.filter(x => x > 1);
console.log('test');
"#;

        let result = scan_test_content(content);
        // This should have few or no candidates (map, filter, reduce, log are not API patterns)
        assert!(
            result.candidates.len() <= 1,
            "Utility files should have minimal candidates"
        );
    }

    #[test]
    fn test_detects_axios_calls() {
        let content = r#"
import axios from 'axios';

async function fetchUser(id: string) {
    const response = await axios.get(`/users/${id}`);
    return response.data;
}
"#;

        let result = scan_test_content(content);
        assert!(result.should_analyze);

        let has_axios = result.candidates.iter().any(|c| c.callee_object == "axios");
        assert!(has_axios, "Should detect axios calls");
    }

    #[test]
    fn test_candidate_format_hint() {
        let candidate = CandidateTarget {
            line_number: 15,
            callee_object: "app".to_string(),
            callee_property: Some("get".to_string()),
            code_snippet: "app.get('/users', handler)".to_string(),
        };

        let hint = candidate.format_hint();
        assert!(hint.contains("Line 15"));
        assert!(hint.contains("app.get"));
        assert!(hint.contains("app.get('/users', handler)"));
    }

    #[test]
    fn test_detects_chained_calls() {
        let content = r#"
createRouter()
    .get('/health', healthCheck)
    .post('/data', handleData);
"#;

        let result = scan_test_content(content);
        assert!(result.should_analyze);

        // Should detect the HTTP methods even in chained form
        let methods: Vec<_> = result
            .candidates
            .iter()
            .filter_map(|c| c.callee_property.as_ref())
            .collect();
        assert!(methods.contains(&&"get".to_string()));
        assert!(methods.contains(&&"post".to_string()));
    }

    #[test]
    fn test_detects_custom_router_names() {
        let content = r#"
const userRouter = createRouter();
const authRouter = createRouter();
const apiHandler = createHandler();

userRouter.get('/profile', getProfile);
authRouter.post('/login', login);
apiHandler.route('/data', handleData);
"#;

        let result = scan_test_content(content);
        assert!(result.should_analyze);

        // Should detect calls on userRouter, authRouter, apiHandler
        assert!(
            result.candidates.len() >= 3,
            "Should detect custom-named router calls"
        );
    }
}
