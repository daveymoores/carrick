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
//! Note: Type extraction is now handled by the TypeSidecar (src/sidecar).
//! The legacy TypePositionFinder and related code has been removed as part
//! of the compiler sidecar architecture migration.

use serde::Serialize;
use std::path::Path;
use swc_common::{
    SourceMap, SourceMapper, Spanned,
    errors::{ColorConfig, Handler},
    sync::Lrc,
};
use swc_ecma_ast::*;
use swc_ecma_parser::TsSyntax;
use swc_ecma_visit::{Visit, VisitWith};

use crate::parser::parse_file;

/// A candidate API call site detected by the SWC scanner.
/// This is passed as a "hint" to the LLM to ensure 100% recall.
#[derive(Debug, Clone, Serialize)]
pub struct CandidateTarget {
    /// Stable identifier for this call site within the file
    pub candidate_id: String,
    /// Start byte offset of the call expression
    pub span_start: u32,
    /// End byte offset of the call expression
    pub span_end: u32,
    /// 1-based line number where the call was detected
    pub line_number: usize,
    /// The callee object (e.g., "app", "router", "fetch")
    pub callee_object: String,
    /// The callee property/method (e.g., "get", "post", "use")
    pub callee_property: Option<String>,
    /// Name of the enclosing function (if any)
    pub enclosing_function: Option<String>,
    /// First-argument snippet (e.g., URL/path literal/template)
    pub path_snippet: Option<String>,
    /// A snippet of the code at this location
    pub code_snippet: String,
}

impl CandidateTarget {
    /// Format as a hint string for the LLM prompt
    pub fn format_hint(&self) -> String {
        let callee = match &self.callee_property {
            Some(prop) => format!("{}.{}", self.callee_object, prop),
            None => self.callee_object.clone(),
        };
        let func = self
            .enclosing_function
            .as_deref()
            .unwrap_or("unknown_function");
        let path = self.path_snippet.as_deref().unwrap_or("<path unavailable>");

        format!(
            "- Candidate {}: Line {} (span {}-{}) {} [fn: {}] [path: {}] - `{}`",
            self.candidate_id,
            self.line_number,
            self.span_start,
            self.span_end,
            callee,
            func,
            path,
            self.code_snippet
        )
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
    ///
    /// Creates a fresh SourceMap for each call to ensure per-file byte offsets.
    /// Previously, reusing `self.source_map` caused cumulative offset accumulation
    /// when scanning multiple files, breaking span-based type inference in the sidecar.
    pub fn scan_content(&self, file_path: &Path, content: &str) -> ScanResult {
        use swc_common::{FileName, GLOBALS, Globals, Mark};
        use swc_ecma_parser::{Parser, StringInput, Syntax, lexer::Lexer};
        use swc_ecma_transforms_base::resolver;
        use swc_ecma_visit::VisitMutWith;

        // Determine syntax based on file extension. Decorators must be enabled
        // so NestJS-style `@Controller('users')` / `@Get(':id')` parse into
        // `Decorator` nodes that the visitor can traverse.
        let (syntax, is_typescript) = if let Some(ext) = file_path.extension() {
            match ext.to_string_lossy().as_ref() {
                "ts" => (
                    Syntax::Typescript(TsSyntax {
                        decorators: true,
                        ..Default::default()
                    }),
                    true,
                ),
                "tsx" => (
                    Syntax::Typescript(TsSyntax {
                        tsx: true,
                        decorators: true,
                        ..Default::default()
                    }),
                    true,
                ),
                _ => (Syntax::Es(Default::default()), false),
            }
        } else {
            (Syntax::Es(Default::default()), false)
        };

        // Create a fresh SourceMap for each file to ensure per-file byte offsets.
        // SWC's SourceMap maintains cumulative offsets across new_source_file() calls,
        // so reusing a single map across files would shift all spans by the total size
        // of previously scanned files.
        let file_source_map: Lrc<SourceMap> = Default::default();
        let source_file = file_source_map.new_source_file(
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

        let mut visitor = CandidateVisitor::new(file_source_map);
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
    function_stack: Vec<String>,
}

impl CandidateVisitor {
    fn new(source_map: Lrc<SourceMap>) -> Self {
        Self {
            candidates: Vec::new(),
            source_map,
            function_stack: Vec::new(),
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
        if let Callee::Expr(expr) = callee
            && let Expr::Ident(ident) = &**expr
        {
            return ident.sym.as_ref() == "fetch";
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

    fn span_range(&self, span: swc_common::Span) -> (u32, u32) {
        (span.lo.0, span.hi.0)
    }

    fn candidate_id(&self, span_start: u32, span_end: u32) -> String {
        format!("span:{}-{}", span_start, span_end)
    }

    fn current_function(&self) -> Option<String> {
        self.function_stack.last().cloned()
    }

    fn extract_first_arg_snippet(&self, call: &CallExpr) -> Option<String> {
        let arg = call.args.first()?;
        self.source_map
            .span_to_snippet(arg.expr.span())
            .ok()
            .map(|s| s.lines().next().unwrap_or("").to_string())
            .map(|s| s.chars().take(120).collect())
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
    fn visit_fn_decl(&mut self, node: &FnDecl) {
        let name = Some(node.ident.sym.to_string());
        self.function_stack.push(name.clone().unwrap());
        node.visit_children_with(self);
        self.function_stack.pop();
    }

    fn visit_fn_expr(&mut self, node: &FnExpr) {
        if let Some(ident) = &node.ident {
            self.function_stack.push(ident.sym.to_string());
        }
        node.visit_children_with(self);
        if node.ident.is_some() {
            self.function_stack.pop();
        }
    }

    fn visit_arrow_expr(&mut self, node: &ArrowExpr) {
        self.function_stack.push("<arrow>".to_string());
        node.visit_children_with(self);
        self.function_stack.pop();
    }

    fn visit_class_method(&mut self, node: &ClassMethod) {
        if let Some(name) = match &node.key {
            PropName::Ident(id) => Some(id.sym.to_string()),
            PropName::Str(s) => Some(s.value.to_string()),
            _ => None,
        } {
            self.function_stack.push(name);
            node.visit_children_with(self);
            self.function_stack.pop();
        } else {
            node.visit_children_with(self);
        }
    }

    fn visit_method_prop(&mut self, node: &MethodProp) {
        if let Some(name) = match &node.key {
            PropName::Ident(id) => Some(id.sym.to_string()),
            PropName::Str(s) => Some(s.value.to_string()),
            _ => None,
        } {
            self.function_stack.push(name);
            node.visit_children_with(self);
            self.function_stack.pop();
        } else {
            node.visit_children_with(self);
        }
    }

    fn visit_decorator(&mut self, node: &Decorator) {
        // Emit a candidate for any decorator call expression. This is the
        // framework-agnostic path for class-method routing (NestJS) — the
        // scanner stays free of framework names; the LLM classifies the
        // decorator by its identifier via the Import Table.
        if let Expr::Call(call) = &*node.expr
            && let Callee::Expr(callee_expr) = &call.callee
        {
            let callee_name = Self::extract_callee_object(callee_expr);
            if let Some(name) = callee_name {
                let line_number = self.get_line_number(call.span);
                let (span_start, span_end) = self.span_range(call.span);
                let candidate_id = self.candidate_id(span_start, span_end);
                let code_snippet = self.get_code_snippet(call.span);
                let path_snippet = self.extract_first_arg_snippet(call);

                self.candidates.push(CandidateTarget {
                    candidate_id,
                    span_start,
                    span_end,
                    line_number,
                    callee_object: name,
                    callee_property: None,
                    enclosing_function: self.current_function(),
                    path_snippet,
                    code_snippet,
                });
            }
        }
        node.visit_children_with(self);
    }

    fn visit_call_expr(&mut self, call: &CallExpr) {
        // Check for global fetch
        if self.is_global_fetch(&call.callee) {
            let line_number = self.get_line_number(call.span);
            let (span_start, span_end) = self.span_range(call.span);
            let candidate_id = self.candidate_id(span_start, span_end);
            let code_snippet = self.get_code_snippet(call.span);
            let path_snippet = self.extract_first_arg_snippet(call);

            self.candidates.push(CandidateTarget {
                candidate_id,
                span_start,
                span_end,
                line_number,
                callee_object: "fetch".to_string(),
                callee_property: None,
                enclosing_function: self.current_function(),
                path_snippet,
                code_snippet,
            });
        }

        // Check for method calls (obj.method())
        if let Callee::Expr(callee_expr) = &call.callee
            && let Expr::Member(member) = &**callee_expr
        {
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
                        self.is_potential_api_object(name) || self.is_potential_api_method(&method)
                    }
                    None => self.is_potential_api_method(&method),
                };

                if is_api_call {
                    let line_number = self.get_line_number(call.span);
                    let (span_start, span_end) = self.span_range(call.span);
                    let candidate_id = self.candidate_id(span_start, span_end);
                    let code_snippet = self.get_code_snippet(call.span);
                    let path_snippet = self.extract_first_arg_snippet(call);

                    self.candidates.push(CandidateTarget {
                        candidate_id,
                        span_start,
                        span_end,
                        line_number,
                        callee_object: obj_name.unwrap_or_else(|| "<chain>".to_string()),
                        callee_property: Some(method),
                        enclosing_function: self.current_function(),
                        path_snippet,
                        code_snippet,
                    });
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
    fn test_candidate_spans_and_ids() {
        let content = "fetch('/api/users');";
        let result = scan_test_content(content);
        assert!(result.should_analyze);
        assert!(!result.candidates.is_empty());

        let candidate = &result.candidates[0];
        assert!(candidate.span_start < candidate.span_end);
        assert_eq!(
            candidate.candidate_id,
            format!("span:{}-{}", candidate.span_start, candidate.span_end)
        );
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
            candidate_id: "span:100-140".to_string(),
            span_start: 100,
            span_end: 140,
            line_number: 15,
            callee_object: "app".to_string(),
            callee_property: Some("get".to_string()),
            enclosing_function: Some("handler".to_string()),
            path_snippet: Some("'/users'".to_string()),
            code_snippet: "app.get('/users', handler)".to_string(),
        };

        let hint = candidate.format_hint();
        assert!(hint.contains("Line 15"));
        assert!(hint.contains("span:100-140"));
        assert!(hint.contains("app.get"));
        assert!(hint.contains("handler"));
        assert!(hint.contains("[path: '/users']"));
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
    fn test_scan_content_per_file_offsets_no_accumulation() {
        // Regression test: verify that scanning multiple files with the same SwcScanner
        // produces per-file byte offsets (not cumulative offsets).
        let scanner = SwcScanner::new();

        let file_a_content = "fetch('/api/a');";
        let file_b_content = "fetch('/api/b');";

        let result_a = scanner.scan_content(&PathBuf::from("a.ts"), file_a_content);
        let result_b = scanner.scan_content(&PathBuf::from("b.ts"), file_b_content);

        assert!(
            !result_a.candidates.is_empty(),
            "file a should have candidates"
        );
        assert!(
            !result_b.candidates.is_empty(),
            "file b should have candidates"
        );

        let span_a = (
            result_a.candidates[0].span_start,
            result_a.candidates[0].span_end,
        );
        let span_b = (
            result_b.candidates[0].span_start,
            result_b.candidates[0].span_end,
        );

        // Both files have the same content structure, so spans should be identical
        // (both start at offset 0-based within their own file).
        assert_eq!(
            span_a, span_b,
            "Spans should be identical for identically-structured files (per-file offsets). \
             Got a={:?}, b={:?}. If b is offset, SourceMap accumulation bug is present.",
            span_a, span_b
        );

        // Spans should be within the file size
        assert!(
            (span_b.1 as usize) <= file_b_content.len() + 1,
            "span_end {} should not exceed file size {}",
            span_b.1,
            file_b_content.len()
        );
    }

    #[test]
    fn test_detects_decorator_calls_for_nestjs_style() {
        // Regression for the gap verified in carrick-cloud/docs/internal/framework-coverage.md §2.3:
        // prior to Move 2, decorator calls produced zero candidates because the
        // visitor only fired on member calls. After widening the scanner, a
        // @Controller('users') class with @Get/@Post/@Get(':id') methods must
        // produce non-zero candidates — the LLM decides which are routing
        // decorators via the Import Table.
        let content = r#"
import { Controller, Get, Post } from '@nestjs/common';

@Controller('users')
export class UsersController {
  @Get()
  findAll() { return []; }

  @Get(':id')
  findOne() { return null; }

  @Post()
  create() { return { id: 1 }; }
}
"#;

        let result = scan_test_content(content);
        assert!(result.should_analyze, "NestJS controller should analyze");

        // At least four decorator candidates (one Controller + three method decorators).
        let decorator_candidates: Vec<_> = result
            .candidates
            .iter()
            .filter(|c| {
                matches!(
                    c.callee_object.as_str(),
                    "Controller" | "Get" | "Post" | "Put" | "Patch" | "Delete"
                )
            })
            .collect();
        assert!(
            decorator_candidates.len() >= 4,
            "expected >=4 decorator candidates, got {}: {:?}",
            decorator_candidates.len(),
            decorator_candidates
                .iter()
                .map(|c| &c.callee_object)
                .collect::<Vec<_>>()
        );
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
