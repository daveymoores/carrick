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
use std::collections::HashSet;
use std::path::Path;
use swc_common::{
    SourceMap, SourceMapper, Spanned,
    errors::{ColorConfig, Handler},
    sync::Lrc,
};
use swc_ecma_ast::*;
use swc_ecma_parser::{EsSyntax, TsSyntax};
use swc_ecma_visit::{Visit, VisitWith};

use crate::operation::Protocol;
use crate::parser::parse_file;

/// A candidate API call site detected by the SWC scanner.
/// This is passed as a "hint" to the LLM to ensure 100% recall.
#[derive(Debug, Clone, Serialize)]
pub struct CandidateTarget {
    /// Protocol family this call site belongs to. Routes the candidate to
    /// that protocol's analyze-file prompt (or skips it when no prompt is
    /// registered). Not serialized: the JSON candidate context the HTTP
    /// prompt receives stays exactly as before.
    #[serde(skip)]
    pub protocol: Protocol,
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
    /// True when the file could not be parsed at all. Callers must surface
    /// this: a parse failure excludes the whole file from the index, which is
    /// very different from a healthy file with no API candidates.
    pub parse_failed: bool,
}

/// A value exported from a module. Used by file-based routing to recover the
/// HTTP method of an app-router handler (`export async function GET(...)`),
/// which is structural information the call-site scanner does not capture.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExportedHandler {
    /// The exported binding name (`GET`, `POST`, …), or `"default"` for a
    /// default export.
    pub name: String,
    /// 1-based line number of the export.
    pub line_number: usize,
    /// Start byte offset of the exported declaration.
    pub span_start: u32,
    /// End byte offset of the exported declaration.
    pub span_end: u32,
}

/// A route declared as data in a registry array
/// (`{ method: 'GET', path: '/health', handler: healthCheckHandler }`). The
/// HTTP method, path, and handler owner are all structural facts — no call site
/// the candidate scanner can see — so they are emitted as a deterministic
/// endpoint instead of being routed through the LLM (#234). Only descriptors
/// whose method *and* path are string literals are reported; dynamic-handler
/// cases stay on the recall-boost candidate path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RouteDescriptorEndpoint {
    /// The HTTP method literal (`GET`, `POST`, …), verbatim from the object.
    pub method: String,
    /// The route path literal (`/gateway/health`), verbatim from the object.
    pub path: String,
    /// The handler identifier (`healthCheckHandler`) — the route's real owner.
    /// `None` when the handler is absent or not a bare identifier.
    pub handler: Option<String>,
    /// 1-based line number of the descriptor object literal.
    pub line_number: usize,
    /// Start byte offset of the descriptor object literal.
    pub span_start: u32,
    /// End byte offset of the descriptor object literal.
    pub span_end: u32,
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
    /// If no candidates are found, the file can be skipped.
    #[allow(dead_code)]
    pub fn scan_file(&self, file_path: &Path, data_fetchers: &[String]) -> ScanResult {
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
                    parse_failed: true,
                };
            }
        };

        let mut visitor = CandidateVisitor::new(
            self.source_map.clone(),
            network_import_locals(&module, data_fetchers),
        );
        module.visit_with(&mut visitor);

        ScanResult {
            candidates: visitor.candidates,
            parse_failed: false,
        }
    }

    /// Scan file content directly (useful for testing or when content is already loaded).
    ///
    /// Creates a fresh SourceMap for each call to ensure per-file byte offsets.
    /// Previously, reusing `self.source_map` caused cumulative offset accumulation
    /// when scanning multiple files, breaking span-based type inference in the sidecar.
    pub fn scan_content(
        &self,
        file_path: &Path,
        content: &str,
        data_fetchers: &[String],
    ) -> ScanResult {
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
                "jsx" => (
                    Syntax::Es(EsSyntax {
                        jsx: true,
                        ..Default::default()
                    }),
                    false,
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
                    parse_failed: true,
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

        let mut visitor = CandidateVisitor::new(
            file_source_map,
            network_import_locals(&module, data_fetchers),
        );
        module.visit_with(&mut visitor);

        ScanResult {
            candidates: visitor.candidates,
            parse_failed: false,
        }
    }

    /// Extract the top-level exported bindings of a module.
    ///
    /// This powers file-based routing: an app-router endpoint declares its HTTP
    /// method as the *name* of an exported handler (`export function GET`), which
    /// never appears as a call site, so the candidate scanner alone cannot see
    /// it. Returns one [`ExportedHandler`] per exported binding; `export default`
    /// is reported with the name `"default"`.
    pub fn exported_handlers(&self, file_path: &Path, content: &str) -> Vec<ExportedHandler> {
        use swc_common::{FileName, Spanned};
        use swc_ecma_parser::{Parser, StringInput, Syntax, lexer::Lexer};

        let syntax = match file_path.extension().and_then(|e| e.to_str()) {
            Some("ts") => Syntax::Typescript(TsSyntax {
                decorators: true,
                ..Default::default()
            }),
            Some("tsx") => Syntax::Typescript(TsSyntax {
                tsx: true,
                decorators: true,
                ..Default::default()
            }),
            Some("jsx") => Syntax::Es(EsSyntax {
                jsx: true,
                ..Default::default()
            }),
            _ => Syntax::Es(Default::default()),
        };

        let sm: Lrc<SourceMap> = Default::default();
        let source_file = sm.new_source_file(
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
        let module = match parser.parse_module() {
            Ok(m) => m,
            Err(_) => return Vec::new(),
        };

        let mut out = Vec::new();
        let mut push = |name: String, span: swc_common::Span| {
            out.push(ExportedHandler {
                name,
                line_number: sm.lookup_char_pos(span.lo).line,
                span_start: span.lo.0,
                span_end: span.hi.0,
            });
        };

        for item in &module.body {
            let ModuleItem::ModuleDecl(decl) = item else {
                continue;
            };
            match decl {
                // `export function GET() {}`, `export const POST = ...`, `export class X {}`
                ModuleDecl::ExportDecl(export) => match &export.decl {
                    Decl::Fn(f) => push(f.ident.sym.to_string(), export.span()),
                    Decl::Class(c) => push(c.ident.sym.to_string(), export.span()),
                    Decl::Var(var) => {
                        for d in &var.decls {
                            if let Pat::Ident(ident) = &d.name {
                                push(ident.id.sym.to_string(), export.span());
                            }
                        }
                    }
                    _ => {}
                },
                // `export { GET, POST as handler }`
                ModuleDecl::ExportNamed(named) => {
                    for spec in &named.specifiers {
                        if let ExportSpecifier::Named(n) = spec {
                            // Prefer the exported alias if present (`as handler`).
                            let name = match n.exported.as_ref().unwrap_or(&n.orig) {
                                ModuleExportName::Ident(id) => id.sym.to_string(),
                                ModuleExportName::Str(s) => s.value.to_string(),
                            };
                            push(name, n.span());
                        }
                    }
                }
                // `export default function () {}` / `export default expr`
                ModuleDecl::ExportDefaultDecl(d) => push("default".to_string(), d.span()),
                ModuleDecl::ExportDefaultExpr(e) => push("default".to_string(), e.span()),
                _ => {}
            }
        }

        out
    }

    /// Extract route-descriptor endpoints declared as data in a registry array
    /// (`{ method: 'GET', path: '/health', handler: healthCheckHandler }`).
    ///
    /// This powers deterministic route-descriptor extraction (#234): the method,
    /// path, and handler owner are all structural facts with no call site the
    /// candidate scanner can see, and the file-analyzer prompt only matches
    /// framework-call patterns — so the orchestrator builds the endpoint from
    /// these facts directly, bypassing the LLM. Only descriptors whose method
    /// *and* path are string literals are returned; the rest stay on the
    /// recall-boost candidate path.
    pub fn route_descriptor_endpoints(
        &self,
        file_path: &Path,
        content: &str,
    ) -> Vec<RouteDescriptorEndpoint> {
        use swc_common::FileName;
        use swc_ecma_parser::{Parser, StringInput, Syntax, lexer::Lexer};

        let syntax = match file_path.extension().and_then(|e| e.to_str()) {
            Some("ts") => Syntax::Typescript(TsSyntax {
                decorators: true,
                ..Default::default()
            }),
            Some("tsx") => Syntax::Typescript(TsSyntax {
                tsx: true,
                decorators: true,
                ..Default::default()
            }),
            Some("jsx") => Syntax::Es(EsSyntax {
                jsx: true,
                ..Default::default()
            }),
            _ => Syntax::Es(Default::default()),
        };

        let sm: Lrc<SourceMap> = Default::default();
        let source_file = sm.new_source_file(
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
        let module = match parser.parse_module() {
            Ok(m) => m,
            Err(_) => return Vec::new(),
        };

        let mut visitor = RouteDescriptorVisitor {
            source_map: sm,
            endpoints: Vec::new(),
        };
        module.visit_with(&mut visitor);
        visitor.endpoints
    }
}

/// Collects deterministic route descriptors (`{ method, path, handler }` with
/// literal method + path) for the no-LLM emission path (#234). The shape guard
/// is shared with the recall-boost candidate via
/// [`CandidateVisitor::route_descriptor`], but the deterministic gate is
/// strictly narrower (#241): a descriptor is emitted only when it is a *direct
/// element of an array literal* (a routes registry, not a standalone config
/// object) and its path is *route-shaped* (leading `/` or an http(s) URL, not a
/// bare token like `some-message`). Anything failing this gate is left for the
/// LLM extraction path; only genuine route registries are authoritative.
struct RouteDescriptorVisitor {
    source_map: Lrc<SourceMap>,
    endpoints: Vec<RouteDescriptorEndpoint>,
}

impl RouteDescriptorVisitor {
    /// A path is route-shaped when it is an absolute path (`/widgets`) or an
    /// http(s) URL. This rejects bare tokens (`some-message`), RPC method names,
    /// and other non-route strings that happen to sit under a `path` key.
    fn is_route_shaped_path(path: &str) -> bool {
        let trimmed = path.trim();
        trimmed.starts_with('/')
            || trimmed.starts_with("http://")
            || trimmed.starts_with("https://")
    }

    /// Emit a deterministic endpoint for `node` when it carries a literal
    /// method + a route-shaped literal path. Used only for object literals that
    /// are direct elements of an array literal (the registry context, #241).
    fn try_emit(&mut self, node: &ObjectLit) {
        let Some(descriptor) = CandidateVisitor::route_descriptor(node) else {
            return;
        };
        // The deterministic path requires literal method *and* path; a
        // descriptor missing either keeps only its recall-boost candidate.
        let (Some(method), Some(path)) = (descriptor.method, descriptor.path) else {
            return;
        };
        // #241: reject non-route paths (bare tokens, RPC method names) so a
        // config object that merely carries `method`/`path` keys is not
        // fabricated as an endpoint.
        if !Self::is_route_shaped_path(&path) {
            return;
        }
        let span = node.span;
        self.endpoints.push(RouteDescriptorEndpoint {
            method,
            path,
            handler: descriptor.handler,
            line_number: self.source_map.lookup_char_pos(span.lo).line,
            span_start: span.lo.0,
            span_end: span.hi.0,
        });
    }
}

impl Visit for RouteDescriptorVisitor {
    fn visit_array_lit(&mut self, node: &ArrayLit) {
        // #241: only object literals that are *direct elements* of an array
        // (a routes registry) qualify for deterministic emission. A standalone
        // config object — e.g. an axios `{ method, path, headers }` options bag
        // — never reaches `try_emit`, so it falls through to the LLM path.
        for element in node.elems.iter().flatten() {
            if let Expr::Object(obj) = &*element.expr {
                self.try_emit(obj);
            }
        }
        node.visit_children_with(self);
    }
}

/// The salient parts of a route-descriptor object literal
/// (`{ method, path, handler }`): the HTTP method literal (when it is a string
/// literal), the path literal snippet (when present) and the handler identifier
/// (when it is a bare identifier reference).
struct RouteDescriptor {
    method: Option<String>,
    path: Option<String>,
    handler: Option<String>,
}

/// Visitor that collects potential API call sites.
struct CandidateVisitor {
    candidates: Vec<CandidateTarget>,
    source_map: Lrc<SourceMap>,
    function_stack: Vec<String>,
    /// Local binding names imported from a known network/data-fetching package
    /// (e.g. `axios` from `import axios from 'axios'`). Calls rooted at one of
    /// these are emitted as candidates regardless of method name, so bespoke
    /// client wrappers (`client.users.list()`) are not missed.
    network_import_locals: HashSet<String>,
    /// Span ranges already emitted, so the broadened signals below don't push
    /// the same call site twice (candidate ids are span-based).
    seen_spans: HashSet<(u32, u32)>,
    /// Depth of enclosing `await` expressions. An awaited call with a string
    /// argument is a strong network-call signal even when the callee name is
    /// unknown.
    await_depth: usize,
}

impl CandidateVisitor {
    fn new(source_map: Lrc<SourceMap>, network_import_locals: HashSet<String>) -> Self {
        Self {
            candidates: Vec::new(),
            source_map,
            function_stack: Vec::new(),
            network_import_locals,
            seen_spans: HashSet::new(),
            await_depth: 0,
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

    /// Check if this is a call to a global network primitive (`fetch(...)`).
    /// Other primitives (`WebSocket`, `EventSource`, `XMLHttpRequest`) are
    /// constructed with `new` and handled in `visit_new_expr`.
    fn is_global_network_call(&self, callee: &Callee) -> bool {
        if let Callee::Expr(expr) = callee
            && let Expr::Ident(ident) = &**expr
        {
            return matches!(ident.sym.as_ref(), "fetch");
        }
        false
    }

    /// Is this a `navigator.sendBeacon(url, ...)` call? This is a web-platform
    /// data-transmitting primitive (a fire-and-forget HTTP POST), the same
    /// family as `fetch`/`XMLHttpRequest`. Matching the syntactic shape
    /// `navigator.sendBeacon(...)` keeps the scanner free of any third-party
    /// client allowlist. This is shape-based, not resolution-based: it keys off
    /// a receiver named `navigator`, which a local could shadow, so it does not
    /// prove the actual browser built-in is being called.
    fn is_navigator_send_beacon(callee: &Callee) -> bool {
        let Callee::Expr(expr) = callee else {
            return false;
        };
        let Expr::Member(member) = &**expr else {
            return false;
        };
        let MemberProp::Ident(prop) = &member.prop else {
            return false;
        };
        prop.sym.as_ref() == "sendBeacon"
            && matches!(&*member.obj, Expr::Ident(obj) if obj.sym.as_ref() == "navigator")
    }

    /// Root identifier of a callee expression, e.g. `client` in
    /// `client.users.list()` or `client(...)`.
    fn callee_root_ident(expr: &Expr) -> Option<String> {
        match expr {
            Expr::Ident(ident) => Some(ident.sym.to_string()),
            Expr::Member(member) => Self::callee_root_ident(&member.obj),
            Expr::Call(call) => match &call.callee {
                Callee::Expr(e) => Self::callee_root_ident(e),
                _ => None,
            },
            _ => None,
        }
    }

    /// Does the first argument look like a URL (has a network scheme)? This is a
    /// low-noise structural signal that catches bespoke clients without naming
    /// them, e.g. `httpClient('https://api.example.com/users')`.
    fn first_arg_has_url_scheme(call: &CallExpr) -> bool {
        let Some(arg) = call.args.first() else {
            return false;
        };
        let starts_with_scheme = |s: &str| {
            let s = s.trim_start();
            s.starts_with("http://")
                || s.starts_with("https://")
                || s.starts_with("ws://")
                || s.starts_with("wss://")
                || s.starts_with("//")
        };
        match &*arg.expr {
            Expr::Lit(Lit::Str(s)) => starts_with_scheme(s.value.as_ref()),
            Expr::Tpl(tpl) => tpl
                .quasis
                .first()
                .map(|q| starts_with_scheme(q.raw.as_ref()))
                .unwrap_or(false),
            _ => false,
        }
    }

    /// Is the first argument a string or template literal? Combined with an
    /// enclosing `await`, this flags awaited calls like `await load('/data')`.
    fn first_arg_is_stringish(call: &CallExpr) -> bool {
        matches!(
            call.args.first().map(|a| &*a.expr),
            Some(Expr::Lit(Lit::Str(_))) | Some(Expr::Tpl(_))
        )
    }

    /// Emit a candidate for `call`, deduplicating by span so the multiple
    /// broadened signals never double-count one call site.
    fn push_candidate(
        &mut self,
        call: &CallExpr,
        callee_object: String,
        callee_property: Option<String>,
    ) {
        let (span_start, span_end) = self.span_range(call.span);
        if !self.seen_spans.insert((span_start, span_end)) {
            return;
        }
        let line_number = self.get_line_number(call.span);
        let candidate_id = self.candidate_id(span_start, span_end);
        let code_snippet = self.get_code_snippet(call.span);
        let path_snippet = self.extract_first_arg_snippet(call);

        self.candidates.push(CandidateTarget {
            protocol: Protocol::Http,
            candidate_id,
            span_start,
            span_end,
            line_number,
            callee_object,
            callee_property,
            enclosing_function: self.current_function(),
            path_snippet,
            code_snippet,
        });
    }

    /// Emit a candidate from a raw span (for nodes that are not call
    /// expressions, e.g. `new WebSocket(...)` or a route-descriptor object
    /// literal). Deduplicates by span like [`push_candidate`].
    #[allow(clippy::too_many_arguments)]
    fn push_span_candidate(
        &mut self,
        span: swc_common::Span,
        protocol: Protocol,
        callee_object: String,
        callee_property: Option<String>,
        path_snippet: Option<String>,
    ) {
        let (span_start, span_end) = self.span_range(span);
        if !self.seen_spans.insert((span_start, span_end)) {
            return;
        }
        let line_number = self.get_line_number(span);
        let candidate_id = self.candidate_id(span_start, span_end);
        let code_snippet = self.get_code_snippet(span);
        self.candidates.push(CandidateTarget {
            protocol,
            candidate_id,
            span_start,
            span_end,
            line_number,
            callee_object,
            callee_property,
            enclosing_function: self.current_function(),
            path_snippet,
            code_snippet,
        });
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

    /// Inspect an object literal for the route-descriptor shape
    /// (`{ method, path, handler }`). Returns the path literal snippet and the
    /// handler identifier when the object carries *both* a `method` and a
    /// `path` property; otherwise `None`. Only string-keyed (ident or string)
    /// properties are considered, so spread/computed config objects don't
    /// accidentally match.
    fn route_descriptor(node: &ObjectLit) -> Option<RouteDescriptor> {
        let key_name = |key: &PropName| -> Option<String> {
            match key {
                PropName::Ident(id) => Some(id.sym.to_string()),
                PropName::Str(s) => Some(s.value.to_string()),
                _ => None,
            }
        };

        let mut has_method = false;
        let mut has_path = false;
        let mut method = None;
        let mut path = None;
        let mut handler = None;

        for prop in &node.props {
            let PropOrSpread::Prop(prop) = prop else {
                continue;
            };
            let Prop::KeyValue(kv) = &**prop else {
                continue;
            };
            let Some(name) = key_name(&kv.key) else {
                continue;
            };
            match name.as_str() {
                "method" => {
                    has_method = true;
                    // Keep the method literal so the route can be emitted
                    // deterministically (#234). A non-literal method (computed
                    // expr) still satisfies the shape guard but yields no
                    // deterministic emission — only the recall-boost candidate.
                    if let Expr::Lit(Lit::Str(s)) = &*kv.value {
                        method = Some(s.value.to_string());
                    }
                }
                "path" => {
                    has_path = true;
                    if let Expr::Lit(Lit::Str(s)) = &*kv.value {
                        path = Some(s.value.to_string());
                    }
                }
                "handler" => {
                    if let Expr::Ident(id) = &*kv.value {
                        handler = Some(id.sym.to_string());
                    }
                }
                _ => {}
            }
        }

        (has_method && has_path).then_some(RouteDescriptor {
            method,
            path,
            handler,
        })
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
            && let Some(name) = Self::extract_callee_object(callee_expr)
        {
            self.push_candidate(call, name, None);
        }
        node.visit_children_with(self);
    }

    fn visit_await_expr(&mut self, node: &AwaitExpr) {
        self.await_depth += 1;
        node.visit_children_with(self);
        self.await_depth -= 1;
    }

    fn visit_new_expr(&mut self, node: &NewExpr) {
        // Network primitives constructed with `new`: `new WebSocket(url)`,
        // `new EventSource(url)`, `new XMLHttpRequest()`. Emitting these as
        // candidates keeps files using them from being skipped by the gate.
        if let Expr::Ident(ident) = &*node.callee
            && matches!(
                ident.sym.as_ref(),
                "WebSocket" | "EventSource" | "XMLHttpRequest"
            )
        {
            let path_snippet = node
                .args
                .as_ref()
                .and_then(|args| args.first())
                .and_then(|a| self.source_map.span_to_snippet(a.expr.span()).ok())
                .map(|s| s.lines().next().unwrap_or("").chars().take(120).collect());
            // XMLHttpRequest is an HTTP client; WebSocket and EventSource
            // belong to the socket family (SSE rides the socket model) and
            // must not reach the HTTP prompt.
            let protocol = if ident.sym.as_ref() == "XMLHttpRequest" {
                Protocol::Http
            } else {
                Protocol::Websocket
            };
            self.push_span_candidate(
                node.span,
                protocol,
                ident.sym.to_string(),
                None,
                path_snippet,
            );
        }
        node.visit_children_with(self);
    }

    fn visit_object_lit(&mut self, node: &ObjectLit) {
        // Signal 6: route-descriptor object literals — a declarative routing
        // shape where the method, path, and handler are *data*, not a method
        // call (`{ method: 'GET', path: '/health', handler: healthCheckHandler }`,
        // typically collected in a `routeRegistry`-style array and registered
        // in a loop). None of the call-site signals fire on such a file, so the
        // gate would skip it and the endpoint would be missed entirely.
        //
        // The shape guard requires *both* a `method` and a `path` property to
        // avoid flagging ordinary config objects. The candidate is keyed on the
        // `handler` identifier when present so the hint points the LLM at the
        // real owner (the handler fn), not the HTTP method string — the
        // owner-fabrication trap.
        //
        // When the method and path are both string literals the route is now
        // emitted deterministically by the orchestrator (`route_descriptor_endpoints`,
        // #234), bypassing the LLM. This candidate stays as a recall booster for
        // the dynamic-handler cases the deterministic path can't own (e.g. a
        // computed method/path, or a handler that isn't a bare identifier): the
        // gate still keeps the file and the LLM classifies it.
        if let Some(descriptor) = Self::route_descriptor(node) {
            let path_snippet = descriptor.path.map(|p| format!("'{}'", p));
            self.push_span_candidate(
                node.span,
                Protocol::Http,
                descriptor
                    .handler
                    .unwrap_or_else(|| "<route-descriptor>".to_string()),
                None,
                path_snippet,
            );
        }
        node.visit_children_with(self);
    }

    fn visit_call_expr(&mut self, call: &CallExpr) {
        // Signal 1: global fetch primitive.
        if self.is_global_network_call(&call.callee) {
            self.push_candidate(call, "fetch".to_string(), None);
        }

        // Signal 1b: `navigator.sendBeacon(url, ...)` — a web-platform HTTP POST
        // primitive. Its first argument is the URL, so the existing
        // `push_candidate` (which records the first-arg path snippet and tags
        // Protocol::Http) routes it through the HTTP prompt, where the method is
        // inferred as POST. Recognized by structural shape, no client allowlist.
        if Self::is_navigator_send_beacon(&call.callee) {
            self.push_candidate(
                call,
                "navigator".to_string(),
                Some("sendBeacon".to_string()),
            );
        }

        // Signal 2: call rooted at an identifier imported from a known
        // network/data-fetching package (covers wrappers regardless of method
        // name), or direct invocation of such an import (`client(url)`).
        if let Callee::Expr(callee_expr) = &call.callee
            && let Some(root) = Self::callee_root_ident(callee_expr)
            && self.network_import_locals.contains(&root)
        {
            let property = match &**callee_expr {
                Expr::Member(member) => match &member.prop {
                    MemberProp::Ident(id) => Some(id.sym.to_string()),
                    _ => None,
                },
                _ => None,
            };
            self.push_candidate(call, root, property);
        }

        // Signal 3: first argument is a URL with a network scheme.
        if Self::first_arg_has_url_scheme(call) {
            let obj = match &call.callee {
                Callee::Expr(e) => {
                    Self::extract_callee_object(e).unwrap_or_else(|| "<url-call>".to_string())
                }
                _ => "<url-call>".to_string(),
            };
            self.push_candidate(call, obj, None);
        }

        // Signal 4: awaited call with a string/template argument.
        if self.await_depth > 0 && Self::first_arg_is_stringish(call) {
            let obj = match &call.callee {
                Callee::Expr(e) => {
                    Self::extract_callee_object(e).unwrap_or_else(|| "<awaited-call>".to_string())
                }
                _ => "<awaited-call>".to_string(),
            };
            self.push_candidate(call, obj, None);
        }

        // Signal 5 (existing): method calls matching the API name heuristics.
        if let Callee::Expr(callee_expr) = &call.callee
            && let Expr::Member(member) = &**callee_expr
        {
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
                let obj_name = Self::extract_callee_object(&member.obj);

                let is_api_call = match &obj_name {
                    Some(name) => {
                        self.is_potential_api_object(name) || self.is_potential_api_method(&method)
                    }
                    None => self.is_potential_api_method(&method),
                };

                if is_api_call {
                    self.push_candidate(
                        call,
                        obj_name.unwrap_or_else(|| "<chain>".to_string()),
                        Some(method),
                    );
                }
            }
        }

        // Continue visiting child nodes
        call.visit_children_with(self);
    }
}

/// Collect the local binding names introduced by imports from any of the
/// `data_fetchers` packages, covering default, named (incl. aliases), and
/// namespace imports. Matched exactly or as a scope/subpath prefix
/// (`pkg`, `@scope/pkg`, `pkg/sub`).
///
/// `data_fetchers` comes from framework detection — the LLM decides which of the
/// repo's dependencies are data-fetching libraries — so the scanner carries no
/// hardcoded package list. This is a recall booster for the gatekeeper, not an
/// authoritative classification: the LLM still decides what each call is.
fn network_import_locals(module: &Module, data_fetchers: &[String]) -> HashSet<String> {
    let is_data_fetcher = |src: &str| {
        data_fetchers
            .iter()
            .any(|pkg| src == pkg || src.starts_with(&format!("{}/", pkg)))
    };
    let mut locals = HashSet::new();
    for item in &module.body {
        let ModuleItem::ModuleDecl(ModuleDecl::Import(import)) = item else {
            continue;
        };
        if !is_data_fetcher(import.src.value.as_ref()) {
            continue;
        }
        for spec in &import.specifiers {
            match spec {
                ImportSpecifier::Default(d) => {
                    locals.insert(d.local.sym.to_string());
                }
                ImportSpecifier::Named(n) => {
                    locals.insert(n.local.sym.to_string());
                }
                ImportSpecifier::Namespace(ns) => {
                    locals.insert(ns.local.sym.to_string());
                }
            }
        }
    }
    locals
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn scan_test_content(content: &str) -> ScanResult {
        scan_test_content_with_fetchers(content, &[])
    }

    fn scan_test_content_with_fetchers(content: &str, data_fetchers: &[String]) -> ScanResult {
        let scanner = SwcScanner::new();
        let path = PathBuf::from("test.ts");
        scanner.scan_content(&path, content, data_fetchers)
    }

    fn handler_names(content: &str) -> Vec<String> {
        let scanner = SwcScanner::new();
        let mut names: Vec<String> = scanner
            .exported_handlers(&PathBuf::from("route.ts"), content)
            .into_iter()
            .map(|h| h.name)
            .collect();
        names.sort();
        names
    }

    #[test]
    fn scan_content_flags_parse_failures() {
        let result = scan_test_content("function broken( {{{");
        assert!(result.parse_failed);
        assert!(result.candidates.is_empty());

        let healthy = scan_test_content("const x = 1;");
        assert!(!healthy.parse_failed);
        assert!(healthy.candidates.is_empty());
    }

    #[test]
    fn exported_handlers_finds_app_router_methods() {
        let content = r#"
export async function GET(req: Request) { return Response.json({}); }
export function POST() {}
const helper = 1;
function notExported() {}
"#;
        assert_eq!(handler_names(content), vec!["GET", "POST"]);
    }

    #[test]
    fn exported_handlers_finds_const_and_named_and_default() {
        let content = r#"
export const PUT = async () => {};
function handlePatch() {}
export { handlePatch as PATCH };
export default function handler() {}
"#;
        assert_eq!(handler_names(content), vec!["PATCH", "PUT", "default"]);
    }

    #[test]
    fn exported_handlers_empty_when_no_exports() {
        let content = "const x = 1; function f() {}";
        assert!(handler_names(content).is_empty());
    }

    #[test]
    fn detects_imported_client_wrapper_calls() {
        // `sdk`/`doThing` match none of the name heuristics; only the
        // import-based signal catches this, and only because detection flagged
        // `got` as a data fetcher (no hardcoded package list in the scanner).
        let content = r#"
import sdk from 'got';
async function run() { return sdk.doThing(); }
"#;
        let fetchers = vec!["got".to_string()];
        assert!(
            !scan_test_content_with_fetchers(content, &fetchers)
                .candidates
                .is_empty()
        );
        // Without detection flagging the package, the wrapper call is invisible
        // to the import signal (the other signals don't apply here either).
        assert!(scan_test_content(content).candidates.is_empty());
    }

    #[test]
    fn detects_url_scheme_first_arg() {
        let content = r#"function run() { return notanapi('https://api.example.com/users'); }"#;
        assert!(!scan_test_content(content).candidates.is_empty());
    }

    #[test]
    fn detects_new_network_primitives() {
        let content =
            r#"function run() { const ws = new WebSocket('wss://example.com'); return ws; }"#;
        assert!(!scan_test_content(content).candidates.is_empty());
    }

    #[test]
    fn detects_navigator_send_beacon_relative_url() {
        // `navigator.sendBeacon('/collect', payload)` is a web-platform HTTP
        // POST primitive. None of the name heuristics match `navigator` or
        // `sendBeacon`, and a relative `/collect` has no URL scheme, so only the
        // dedicated shape signal keeps this file from being skipped by the gate.
        let content = r#"function track() { const ok = navigator.sendBeacon('/collect', payload); return ok; }"#;
        let result = scan_test_content(content);
        let beacon = result.candidates.iter().find(|c| {
            c.callee_object == "navigator" && c.callee_property.as_deref() == Some("sendBeacon")
        });
        assert!(
            beacon.is_some(),
            "expected a navigator.sendBeacon candidate, got {:?}",
            result
                .candidates
                .iter()
                .map(|c| (&c.callee_object, &c.callee_property))
                .collect::<Vec<_>>()
        );
        let beacon = beacon.unwrap();
        assert_eq!(beacon.callee_property.as_deref(), Some("sendBeacon"));
        assert_eq!(beacon.protocol, Protocol::Http);
        assert_eq!(beacon.path_snippet.as_deref(), Some("'/collect'"));
    }

    #[test]
    fn detects_navigator_send_beacon_absolute_url() {
        let content = r#"function track() {
    navigator.sendBeacon('https://metrics.example.com/collect', JSON.stringify(data));
}"#;
        let result = scan_test_content(content);
        assert!(
            result
                .candidates
                .iter()
                .any(|c| c.callee_object == "navigator"
                    && c.callee_property.as_deref() == Some("sendBeacon")),
            "expected a navigator.sendBeacon candidate for an absolute URL"
        );
    }

    #[test]
    fn ignores_unrelated_send_beacon_member() {
        // A `sendBeacon` method on some other object is NOT the web-platform
        // primitive; the shape guard requires the `navigator` receiver.
        let content = r#"function f() { return tracker.sendBeacon('/x'); }"#;
        let result = scan_test_content(content);
        assert!(
            !result
                .candidates
                .iter()
                .any(|c| c.callee_object == "navigator"
                    && c.callee_property.as_deref() == Some("sendBeacon")),
            "non-navigator.sendBeacon must not be tagged as the navigator primitive"
        );
    }

    #[test]
    fn detects_awaited_stringish_call() {
        let content = r#"async function run() { return await loadData('/data.json'); }"#;
        assert!(!scan_test_content(content).candidates.is_empty());
    }

    #[test]
    fn ignores_non_network_code() {
        let content = r#"
function run() {
    console.log('hello');
    const x = compute(1, 2);
    return x;
}
"#;
        assert!(scan_test_content(content).candidates.is_empty());
    }

    #[test]
    fn dedupes_candidate_spans_across_signals() {
        // `await axios.get('https://x.com/y')` matches the import-local,
        // url-scheme, awaited-stringish, and name heuristics simultaneously,
        // but the single call site must yield exactly one candidate.
        let content = r#"
import axios from 'axios';
async function run() { return await axios.get('https://x.com/y'); }
"#;
        let result = scan_test_content(content);
        assert_eq!(result.candidates.len(), 1);
    }

    #[test]
    fn exported_handlers_reports_line_numbers() {
        let content = "\n\nexport function GET() {}\n";
        let scanner = SwcScanner::new();
        let handlers = scanner.exported_handlers(&PathBuf::from("route.ts"), content);
        assert_eq!(handlers.len(), 1);
        assert_eq!(handlers[0].name, "GET");
        assert_eq!(handlers[0].line_number, 3);
    }

    #[test]
    fn detects_route_descriptor_object_literal() {
        // The gateway owner-fabrication trap (#227): a raw-handler block where
        // the route is declarative *data* in a registry array, not a method
        // call. No call-site signal fires, so without the object-literal signal
        // the whole file is skipped and `GET /gateway/health` is missed.
        let content = r#"
export const healthCheckHandler = async (_req: unknown, _res: unknown) => {
  return { ok: true, ts: Date.now() };
};

const routeRegistry = [
  { method: 'GET', path: '/gateway/health', handler: healthCheckHandler },
];

export { routeRegistry };
"#;
        let result = scan_test_content(content);
        let descriptor = result
            .candidates
            .iter()
            .find(|c| c.path_snippet.as_deref() == Some("'/gateway/health'"));
        assert!(
            descriptor.is_some(),
            "expected a route-descriptor candidate for the registry object, got {:?}",
            result
                .candidates
                .iter()
                .map(|c| (&c.callee_object, &c.path_snippet))
                .collect::<Vec<_>>()
        );
        let descriptor = descriptor.unwrap();
        assert_eq!(descriptor.protocol, Protocol::Http);
        // The candidate must be keyed on the real handler fn, never the HTTP
        // method string — the owner-fabrication bait.
        assert_eq!(descriptor.callee_object, "healthCheckHandler");
        assert_ne!(descriptor.callee_object, "GET");
    }

    #[test]
    fn route_descriptor_without_handler_still_flagged() {
        // `method` + `path` is enough for the gate to keep the file; a missing
        // or non-identifier handler falls back to a sentinel so the LLM still
        // sees and classifies the route.
        let content = r#"
const routes = [
  { method: 'POST', path: '/widgets' },
];
export { routes };
"#;
        let result = scan_test_content(content);
        let descriptor = result
            .candidates
            .iter()
            .find(|c| c.path_snippet.as_deref() == Some("'/widgets'"));
        assert!(
            descriptor.is_some(),
            "expected a route-descriptor candidate"
        );
        assert_eq!(descriptor.unwrap().callee_object, "<route-descriptor>");
    }

    #[test]
    fn plain_config_object_is_not_a_route_descriptor() {
        // An object with only one of the two required keys (or neither) is
        // ordinary config and must not be flagged, or the gate would light up
        // on every options bag in the codebase.
        let only_method = scan_test_content(r#"const a = { method: 'GET' };"#);
        let only_path = scan_test_content(r#"const b = { path: '/x' };"#);
        let neither = scan_test_content(r#"const c = { timeout: 5000, retries: 3 };"#);
        assert!(only_method.candidates.is_empty());
        assert!(only_path.candidates.is_empty());
        assert!(neither.candidates.is_empty());
    }

    #[test]
    fn route_descriptor_endpoints_extracts_method_path_handler() {
        // #234: the route declared as data carries the full method/path/handler
        // structurally, so it is emitted deterministically (no LLM). The owner is
        // the handler identifier `healthCheckHandler`, never the method literal
        // "GET" (the owner-fabrication trap).
        let content = r#"
export const healthCheckHandler = async (_req: unknown, _res: unknown) => {
  return { ok: true, ts: Date.now() };
};

const routeRegistry = [
  { method: 'GET', path: '/gateway/health', handler: healthCheckHandler },
];

export { routeRegistry };
"#;
        let scanner = SwcScanner::new();
        let endpoints =
            scanner.route_descriptor_endpoints(&PathBuf::from("health.handler.ts"), content);
        assert_eq!(endpoints.len(), 1, "expected one route descriptor endpoint");
        let ep = &endpoints[0];
        assert_eq!(ep.method, "GET");
        assert_eq!(ep.path, "/gateway/health");
        assert_eq!(ep.handler.as_deref(), Some("healthCheckHandler"));
        assert_ne!(ep.handler.as_deref(), Some("GET"));
        assert!(ep.span_end > ep.span_start);
    }

    #[test]
    fn route_descriptor_endpoints_skips_dynamic_method_or_path() {
        // A computed method/path can't be emitted deterministically — it stays on
        // the recall-boost candidate path, so no deterministic endpoint is built.
        let dynamic = r#"
const verb = 'GET';
const routes = [
  { method: verb, path: '/widgets', handler: listWidgets },
];
export { routes };
"#;
        let scanner = SwcScanner::new();
        let endpoints = scanner.route_descriptor_endpoints(&PathBuf::from("routes.ts"), dynamic);
        assert!(
            endpoints.is_empty(),
            "non-literal method must not yield a deterministic endpoint, got {endpoints:?}"
        );
    }

    #[test]
    fn route_descriptor_endpoints_allows_missing_handler() {
        // Literal method + path with no (or non-identifier) handler still emits a
        // deterministic endpoint; the owner is left unresolved for the caller.
        let content = r#"
const routes = [
  { method: 'POST', path: '/widgets' },
];
export { routes };
"#;
        let scanner = SwcScanner::new();
        let endpoints = scanner.route_descriptor_endpoints(&PathBuf::from("routes.ts"), content);
        assert_eq!(endpoints.len(), 1);
        assert_eq!(endpoints[0].method, "POST");
        assert_eq!(endpoints[0].path, "/widgets");
        assert_eq!(endpoints[0].handler, None);
    }

    #[test]
    fn standalone_two_key_config_object_is_not_a_deterministic_endpoint() {
        // #241 (the real gap): a *standalone* config object that happens to carry
        // string-literal `method` + `path` keys — an axios-style request spec — is
        // NOT a route registry. It must not be emitted as a deterministic endpoint
        // (which would also suppress the LLM that classifies the file correctly).
        // The one-key case was already covered; this is the two-key misfire.
        let axios_config = r#"
const response = await client({
  method: 'GET',
  path: '/data',
  headers: { 'x-api-key': key },
});
"#;
        let scanner = SwcScanner::new();
        let endpoints =
            scanner.route_descriptor_endpoints(&PathBuf::from("client.ts"), axios_config);
        assert!(
            endpoints.is_empty(),
            "standalone {{ method, path, headers }} config must not be a route descriptor, got {endpoints:?}"
        );

        // The recall-boost candidate still fires (the object has the shape), so
        // `http_candidates` stays non-empty and the file is NOT suppressed: it
        // falls through to the LLM extraction path, which is the whole point.
        let result = scan_test_content(axios_config);
        assert!(
            !result.candidates.is_empty(),
            "the LLM fall-through candidate must survive so the file is not skipped"
        );
    }

    #[test]
    fn registry_descriptor_with_non_route_path_is_not_a_deterministic_endpoint() {
        // #241: even inside an array, a `path` that is a bare token (`some-message`)
        // — an RPC channel name, message key, etc. — is not route-shaped, so it must
        // not be fabricated as a `GET some-message` endpoint. It falls through.
        let content = r#"
const handlers = [
  { method: 'GET', path: 'some-message', handler: onMessage },
];
export { handlers };
"#;
        let scanner = SwcScanner::new();
        let endpoints = scanner.route_descriptor_endpoints(&PathBuf::from("handlers.ts"), content);
        assert!(
            endpoints.is_empty(),
            "a non-route path (bare token) must not yield a deterministic endpoint, got {endpoints:?}"
        );
    }

    #[test]
    fn registry_descriptor_with_url_path_is_a_deterministic_endpoint() {
        // #241: an http(s) URL is route-shaped and qualifies inside a registry.
        let content = r#"
const routes = [
  { method: 'POST', path: 'https://api.example.com/webhook', handler: onHook },
];
export { routes };
"#;
        let scanner = SwcScanner::new();
        let endpoints = scanner.route_descriptor_endpoints(&PathBuf::from("routes.ts"), content);
        assert_eq!(endpoints.len(), 1);
        assert_eq!(endpoints[0].method, "POST");
        assert_eq!(endpoints[0].path, "https://api.example.com/webhook");
        assert_eq!(endpoints[0].handler.as_deref(), Some("onHook"));
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
        assert!(!result.candidates.is_empty());
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
        assert!(!result.candidates.is_empty());

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
        assert!(!result.candidates.is_empty());
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
        assert!(!result.candidates.is_empty());

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
        assert!(!result.candidates.is_empty());

        let has_axios = result.candidates.iter().any(|c| c.callee_object == "axios");
        assert!(has_axios, "Should detect axios calls");
    }

    #[test]
    fn test_candidate_format_hint() {
        let candidate = CandidateTarget {
            protocol: Protocol::Http,
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
        assert!(!result.candidates.is_empty());

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

        let result_a = scanner.scan_content(&PathBuf::from("a.ts"), file_a_content, &[]);
        let result_b = scanner.scan_content(&PathBuf::from("b.ts"), file_b_content, &[]);

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
        // Regression for the gap verified in the carrick-cloud repo's docs/internal/framework-coverage.md §2.3:
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
        assert!(
            !result.candidates.is_empty(),
            "NestJS controller should analyze"
        );

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
        assert!(!result.candidates.is_empty());

        // Should detect calls on userRouter, authRouter, apiHandler
        assert!(
            result.candidates.len() >= 3,
            "Should detect custom-named router calls"
        );
    }
}
