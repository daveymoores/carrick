//! Deterministic Socket.IO contract extraction.
//!
//! Socket.IO has a real operation key — event name plus message-flow
//! direction — and event names are string literals in idiomatic code, so
//! extraction is AST-based with no LLM. Listeners (`socket.on("x", ...)`)
//! are producers of the key for the direction they receive; emitters
//! (`socket.emit("x", ...)`) are consumers for the direction they send.
//! Which side of the wire a call site is on is derived from imports:
//! `socket.io-client` factories make client sockets, `new Server(...)` from
//! `socket.io` makes server roots, and the first parameter of a
//! `connection` handler is a per-connection server socket.
//!
//! Precision over recall, per the brittleness guardrails:
//! - only string-literal event names count; dynamic names are skipped,
//! - reserved lifecycle events (`connect`, `disconnect`, ...) never become
//!   contract events,
//! - files using custom namespaces (`io.of(...)`) are skipped entirely —
//!   default-namespace identity would be ambiguous there,
//! - CommonJS `require("socket.io")` bootstrapping is not traced (coverage
//!   gap, not a false positive),
//! - socket identity is tracked by binding name, not full scope analysis;
//!   bindings are only created from socket.io factories and connection
//!   handler parameters.

use crate::operation::{OperationKey, SocketDirection};
use crate::parser::parse_file;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use swc_common::errors::{ColorConfig, Handler};
use swc_common::{GLOBALS, Globals, SourceMap, Spanned, sync::Lrc};
use swc_ecma_ast::{
    Callee, Expr, ImportDecl, ImportSpecifier, Lit, ModuleExportName, NewExpr, Pat, TsEntityName,
    TsType, TsTypeAnn, VarDeclarator,
};
use swc_ecma_visit::{Visit, VisitWith};
use tracing::debug;

/// A socket listener or emitter with its source location.
///
/// `payload_type_symbol`/`payload_type_source` carry the message payload's TS
/// type so the op can be anchored and resolved through the existing
/// SymbolRequest/sidecar bundle path (#245 Phase 1). They are populated only
/// when the payload is an explicitly-typed named reference whose declaration is
/// `import`ed (precision over recall): inline object types, generics, unions,
/// and untyped payloads stay `None` so they degrade to an honest `Unknown`
/// rather than a phantom anchor.
#[derive(Debug, Clone)]
pub struct SocketOp {
    pub key: OperationKey,
    pub file_path: PathBuf,
    pub line: u32,
    /// Bare symbol name of the payload type (e.g. `Payment`), when explicitly
    /// annotated as a named reference. `None` for inline/generic/untyped payloads.
    pub payload_type_symbol: Option<String>,
    /// Module specifier the payload type is imported from (e.g.
    /// `./types/payment`), paired with `payload_type_symbol`. `None` when the
    /// symbol is not imported (same-file or untyped).
    pub payload_type_source: Option<String>,
}

#[derive(Debug, Clone, Default)]
pub struct SocketExtraction {
    /// Listeners: producers of the direction they receive.
    pub listeners: Vec<SocketOp>,
    /// Emitters: consumers of the direction they send.
    pub emitters: Vec<SocketOp>,
}

impl SocketExtraction {
    pub fn is_empty(&self) -> bool {
        self.listeners.is_empty() && self.emitters.is_empty()
    }

    fn merge(&mut self, other: SocketExtraction) {
        self.listeners.extend(other.listeners);
        self.emitters.extend(other.emitters);
    }
}

/// Socket.IO lifecycle/reserved events that are not application contract
/// events.
const RESERVED_EVENTS: &[&str] = &[
    "connection",
    "connect",
    "connect_error",
    "disconnect",
    "disconnecting",
    "error",
    "reconnect",
    "reconnect_attempt",
    "reconnect_error",
    "reconnect_failed",
    "ping",
    "pong",
    "newListener",
    "removeListener",
];

/// Extract Socket.IO operations from the service's TS/JS files.
pub fn scan_files(service_files: &[PathBuf]) -> SocketExtraction {
    let mut extraction = SocketExtraction::default();
    for file in service_files {
        let is_script = file
            .extension()
            .and_then(|e| e.to_str())
            .is_some_and(|ext| matches!(ext, "ts" | "tsx" | "js" | "jsx"));
        if !is_script {
            continue;
        }
        extraction.merge(extract_from_ts_file(file));
    }
    debug!(
        listeners = extraction.listeners.len(),
        emitters = extraction.emitters.len(),
        "Socket.IO extraction complete"
    );
    extraction
}

fn extract_from_ts_file(file_path: &Path) -> SocketExtraction {
    let cm: Lrc<SourceMap> = Default::default();
    let handler = Handler::with_tty_emitter(ColorConfig::Never, false, false, Some(cm.clone()));

    let globals = Globals::new();
    GLOBALS.set(&globals, || {
        let Some(module) = parse_file(file_path, &cm, &handler) else {
            return SocketExtraction::default();
        };

        // Pass A: collect socket-rooted binding names. Run to fixpoint (a
        // connection-handler socket needs the server root known first);
        // two iterations cover every realistic nesting.
        let mut roots = SocketRoots::default();
        loop {
            let before = roots.size();
            let mut collector = RootCollector { roots: &mut roots };
            module.visit_with(&mut collector);
            if roots.size() == before {
                break;
            }
        }

        if roots.size() == 0 {
            return SocketExtraction::default();
        }

        // Pass B: collect ops on socket-rooted identifiers.
        let mut ops = OpCollector {
            cm: cm.clone(),
            file_path,
            roots: &roots,
            uses_namespaces: false,
            extraction: SocketExtraction::default(),
        };
        module.visit_with(&mut ops);

        if ops.uses_namespaces {
            debug!(
                "Skipping Socket.IO extraction for {} (custom namespaces in use)",
                file_path.display()
            );
            return SocketExtraction::default();
        }
        ops.extraction
    })
}

#[derive(Default)]
struct SocketRoots {
    /// Local names of `socket.io-client` factories (`io`, `connect`, ...).
    client_factories: HashSet<String>,
    /// Local names of the `socket.io` `Server` class.
    server_classes: HashSet<String>,
    /// Bindings holding client sockets (`const s = io(url)`).
    client_sockets: HashSet<String>,
    /// Bindings holding server roots (`const io = new Server(...)`) or
    /// per-connection sockets (`io.on("connection", (socket) => ...)`).
    server_sockets: HashSet<String>,
    /// Imported type symbols → their module specifier. Drives payload-anchor
    /// resolution (#245): an emitted/received payload typed as an imported
    /// named reference gets a `(symbol, source)` pair the SymbolRequest path
    /// can bundle. Same-file types are absent here and resolve with `None`
    /// source.
    type_imports: HashMap<String, String>,
    /// Binding name → payload type symbol, from `const x: T = …` declarators
    /// and typed function parameters. Lets `socket.emit("e", payment)` recover
    /// `Payment` from the `payment` binding's annotation. File-level and flat
    /// (binding shadowing is ignored — a precision tradeoff consistent with the
    /// module's other guardrails); only simple named references are recorded,
    /// so generics/unions/inline object types never produce an anchor.
    binding_types: HashMap<String, String>,
}

impl SocketRoots {
    fn size(&self) -> usize {
        self.client_factories.len()
            + self.server_classes.len()
            + self.client_sockets.len()
            + self.server_sockets.len()
            + self.type_imports.len()
            + self.binding_types.len()
    }

    fn direction_for(&self, root: &str, is_listener: bool) -> Option<SocketDirection> {
        if self.client_sockets.contains(root) {
            // A client listens to server→client messages and emits
            // client→server messages.
            Some(if is_listener {
                SocketDirection::ServerToClient
            } else {
                SocketDirection::ClientToServer
            })
        } else if self.server_sockets.contains(root) {
            Some(if is_listener {
                SocketDirection::ClientToServer
            } else {
                SocketDirection::ServerToClient
            })
        } else {
            None
        }
    }
}

struct RootCollector<'a> {
    roots: &'a mut SocketRoots,
}

impl Visit for RootCollector<'_> {
    fn visit_import_decl(&mut self, node: &ImportDecl) {
        let source = node.src.value.as_ref();
        // Record every named import's local name → module specifier so a
        // socket payload typed as an imported symbol (`import type { Payment }
        // from "./types"`) can be anchored. Default/namespace imports are
        // skipped: payload type references are named, and a default import's
        // local name is not the exported declaration the bundler resolves by.
        for specifier in &node.specifiers {
            if let ImportSpecifier::Named(named) = specifier {
                self.roots
                    .type_imports
                    .insert(named.local.sym.to_string(), source.to_string());
            }
        }
        if source != "socket.io" && source != "socket.io-client" {
            return;
        }
        for specifier in &node.specifiers {
            match specifier {
                ImportSpecifier::Default(default) if source == "socket.io-client" => {
                    self.roots
                        .client_factories
                        .insert(default.local.sym.to_string());
                }
                ImportSpecifier::Named(named) => {
                    let imported = named
                        .imported
                        .as_ref()
                        .map(|name| match name {
                            ModuleExportName::Ident(ident) => ident.sym.to_string(),
                            ModuleExportName::Str(s) => s.value.to_string(),
                        })
                        .unwrap_or_else(|| named.local.sym.to_string());
                    match (source, imported.as_str()) {
                        ("socket.io-client", "io" | "connect" | "default") => {
                            self.roots
                                .client_factories
                                .insert(named.local.sym.to_string());
                        }
                        ("socket.io", "Server") => {
                            self.roots
                                .server_classes
                                .insert(named.local.sym.to_string());
                        }
                        _ => {}
                    }
                }
                _ => {}
            }
        }
    }

    fn visit_var_declarator(&mut self, node: &VarDeclarator) {
        if let (Pat::Ident(binding), Some(init)) = (&node.name, node.init.as_deref()) {
            match init {
                // const socket = io(url) — client socket
                Expr::Call(call) => {
                    if let Callee::Expr(callee) = &call.callee
                        && let Expr::Ident(factory) = &**callee
                        && self.roots.client_factories.contains(factory.sym.as_ref())
                    {
                        self.roots.client_sockets.insert(binding.id.sym.to_string());
                    }
                }
                // const io = new Server(httpServer) — server root
                Expr::New(NewExpr { callee, .. }) => {
                    if let Expr::Ident(class) = &**callee
                        && self.roots.server_classes.contains(class.sym.as_ref())
                    {
                        self.roots.server_sockets.insert(binding.id.sym.to_string());
                    }
                }
                _ => {}
            }
        }
        node.visit_children_with(self);
    }

    fn visit_call_expr(&mut self, node: &swc_ecma_ast::CallExpr) {
        // io.on("connection", (socket) => ...) — the handler's first param
        // is a per-connection server socket.
        if let Callee::Expr(callee) = &node.callee
            && let Expr::Member(member) = &**callee
            && member
                .prop
                .as_ident()
                .is_some_and(|prop| prop.sym.as_ref() == "on")
            && let Expr::Ident(receiver) = &*member.obj
            && self.roots.server_sockets.contains(receiver.sym.as_ref())
            && let Some(first) = node.args.first()
            && matches!(&*first.expr, Expr::Lit(Lit::Str(event)) if matches!(event.value.as_ref(), "connection" | "connect"))
            && let Some(handler) = node.args.get(1)
        {
            let param = match &*handler.expr {
                Expr::Arrow(arrow) => arrow.params.first().and_then(|p| p.as_ident()),
                Expr::Fn(func) => func.function.params.first().and_then(|p| p.pat.as_ident()),
                _ => None,
            };
            if let Some(param) = param {
                self.roots.server_sockets.insert(param.id.sym.to_string());
            }
        }
        node.visit_children_with(self);
    }

    fn visit_pat(&mut self, node: &Pat) {
        // Record `const payment: Payment` / `(payment: Payment) => …` style
        // typed bindings so an emitted payload identifier can recover its
        // type symbol. Only simple named references count (see
        // `named_type_symbol`); anything else leaves the binding unanchored.
        if let Pat::Ident(ident) = node
            && let Some(type_ann) = ident.type_ann.as_ref()
            && let Some(symbol) = named_type_symbol(type_ann)
        {
            self.roots
                .binding_types
                .insert(ident.id.sym.to_string(), symbol);
        }
        node.visit_children_with(self);
    }
}

/// Bare symbol name of a simple named type annotation (`Payment` from
/// `: Payment`), or `None` for anything that is not a single unqualified type
/// reference. Precision over recall: generics (`Foo<T>`), unions, intersections,
/// inline object types, qualified names (`ns.Type`), and primitives are all
/// rejected so the socket anchor only fires when there is one resolvable symbol.
fn named_type_symbol(type_ann: &TsTypeAnn) -> Option<String> {
    match &*type_ann.type_ann {
        TsType::TsTypeRef(type_ref) if type_ref.type_params.is_none() => {
            match &type_ref.type_name {
                TsEntityName::Ident(ident) => {
                    let name = ident.sym.to_string();
                    // Reject TS built-in/primitive references that happen to parse
                    // as a type ref so they never become a bundle target.
                    if is_builtin_type(&name) {
                        None
                    } else {
                        Some(name)
                    }
                }
                TsEntityName::TsQualifiedName(_) => None,
            }
        }
        _ => None,
    }
}

/// Lowercase/well-known TS types that must never be treated as a resolvable
/// payload anchor.
fn is_builtin_type(name: &str) -> bool {
    matches!(
        name,
        "any"
            | "unknown"
            | "never"
            | "void"
            | "object"
            | "string"
            | "number"
            | "boolean"
            | "bigint"
            | "symbol"
            | "undefined"
            | "null"
            | "Array"
            | "Promise"
            | "Record"
            | "Map"
            | "Set"
            | "Date"
    )
}

struct OpCollector<'a> {
    cm: Lrc<SourceMap>,
    file_path: &'a Path,
    roots: &'a SocketRoots,
    uses_namespaces: bool,
    extraction: SocketExtraction,
}

/// Walk a callee chain (`io.to("room").emit`, `socket.broadcast.emit`) back
/// to its root identifier.
fn chain_root(expr: &Expr) -> Option<&swc_ecma_ast::Ident> {
    match expr {
        Expr::Ident(ident) => Some(ident),
        Expr::Member(member) => chain_root(&member.obj),
        Expr::Call(call) => match &call.callee {
            Callee::Expr(callee) => chain_root(callee),
            _ => None,
        },
        Expr::Paren(paren) => chain_root(&paren.expr),
        Expr::Await(awaited) => chain_root(&awaited.arg),
        _ => None,
    }
}

impl Visit for OpCollector<'_> {
    fn visit_call_expr(&mut self, node: &swc_ecma_ast::CallExpr) {
        if let Callee::Expr(callee) = &node.callee
            && let Expr::Member(member) = &**callee
            && let Some(prop) = member.prop.as_ident()
            && let Some(root) = chain_root(&member.obj)
        {
            let root_name = root.sym.as_ref();
            let is_socket_root = self.roots.client_sockets.contains(root_name)
                || self.roots.server_sockets.contains(root_name);

            if is_socket_root && prop.sym.as_ref() == "of" {
                self.uses_namespaces = true;
            }

            let is_listener = matches!(prop.sym.as_ref(), "on" | "once");
            let is_emitter = prop.sym.as_ref() == "emit";
            if is_socket_root
                && (is_listener || is_emitter)
                && let Some(first) = node.args.first()
                && let Expr::Lit(Lit::Str(event)) = &*first.expr
                && !RESERVED_EVENTS.contains(&event.value.as_ref())
            {
                let direction = self.roots.direction_for(root_name, is_listener);
                if let Some(direction) = direction {
                    let payload_symbol = if is_listener {
                        // Listener: the handler's first parameter is the
                        // received payload; read its type annotation directly.
                        self.listener_payload_symbol(node)
                    } else {
                        // Emitter: the second argument is the sent payload;
                        // recover its symbol from the binding's annotation.
                        self.emitter_payload_symbol(node)
                    };
                    let (payload_type_symbol, payload_type_source) = match payload_symbol {
                        Some(symbol) => {
                            let source = self.roots.type_imports.get(&symbol).cloned();
                            (Some(symbol), source)
                        }
                        None => (None, None),
                    };
                    let op = SocketOp {
                        key: OperationKey::socket(event.value.to_string(), direction),
                        file_path: self.file_path.to_path_buf(),
                        line: self.cm.lookup_char_pos(node.span().lo).line as u32,
                        payload_type_symbol,
                        payload_type_source,
                    };
                    if is_listener {
                        self.extraction.listeners.push(op);
                    } else {
                        self.extraction.emitters.push(op);
                    }
                }
            }
        }
        node.visit_children_with(self);
    }
}

impl OpCollector<'_> {
    /// Payload type symbol of a listener call's handler — the type annotation
    /// on the handler's first parameter (`socket.on("e", (p: Payment) => …)`).
    fn listener_payload_symbol(&self, node: &swc_ecma_ast::CallExpr) -> Option<String> {
        let handler = node.args.get(1)?;
        let first_param: Option<&Pat> = match &*handler.expr {
            Expr::Arrow(arrow) => arrow.params.first(),
            Expr::Fn(func) => func.function.params.first().map(|p| &p.pat),
            _ => None,
        };
        match first_param? {
            Pat::Ident(ident) => ident.type_ann.as_deref().and_then(named_type_symbol),
            _ => None,
        }
    }

    /// Payload type symbol of an emitter call — the second argument's binding
    /// type (`socket.emit("e", payment)` where `payment: Payment`). Only a bare
    /// identifier argument resolves; inline literals/expressions stay
    /// unanchored.
    fn emitter_payload_symbol(&self, node: &swc_ecma_ast::CallExpr) -> Option<String> {
        let payload = node.args.get(1)?;
        match &*payload.expr {
            Expr::Ident(ident) => self.roots.binding_types.get(ident.sym.as_ref()).cloned(),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn extract(source: &str) -> SocketExtraction {
        let dir = std::env::temp_dir().join(format!(
            "carrick-socket-test-{}-{:016x}",
            std::process::id(),
            {
                // unique-enough per test input to avoid tempdir collisions
                let mut hash: u64 = 0xcbf29ce484222325;
                for byte in source.as_bytes() {
                    hash ^= u64::from(*byte);
                    hash = hash.wrapping_mul(0x100000001b3);
                }
                hash
            }
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("file.ts");
        std::fs::write(&file, source).unwrap();
        let result = extract_from_ts_file(&file);
        std::fs::remove_dir_all(&dir).ok();
        result
    }

    fn keys(ops: &[SocketOp]) -> Vec<String> {
        let mut keys: Vec<String> = ops.iter().map(|op| op.key.canonical()).collect();
        keys.sort();
        keys
    }

    #[test]
    fn server_listeners_and_emitters() {
        let result = extract(
            r#"
import { Server } from "socket.io";
const io = new Server(httpServer);
io.on("connection", (socket) => {
  socket.on("chat:message", (msg) => { io.emit("chat:broadcast", msg); });
  socket.emit("welcome", { ok: true });
  socket.broadcast.emit("user:joined", socket.id);
  io.to("room").emit("room:update", {});
  socket.on("disconnect", () => {});
});
"#,
        );
        assert_eq!(
            keys(&result.listeners),
            vec!["socket|CLIENT->SERVER|chat:message"],
            "server listener is a producer of client->server"
        );
        assert_eq!(
            keys(&result.emitters),
            vec![
                "socket|SERVER->CLIENT|chat:broadcast",
                "socket|SERVER->CLIENT|room:update",
                "socket|SERVER->CLIENT|user:joined",
                "socket|SERVER->CLIENT|welcome",
            ],
            "server emits (incl. broadcast/to chains) are consumers of server->client"
        );
    }

    #[test]
    fn client_listeners_and_emitters() {
        let result = extract(
            r#"
import { io } from "socket.io-client";
const socket = io("https://chat.internal");
socket.on("chat:broadcast", (msg) => console.log(msg));
socket.emit("chat:message", "hello");
socket.on("connect", () => {});
"#,
        );
        assert_eq!(
            keys(&result.listeners),
            vec!["socket|SERVER->CLIENT|chat:broadcast"]
        );
        assert_eq!(
            keys(&result.emitters),
            vec!["socket|CLIENT->SERVER|chat:message"]
        );
    }

    #[test]
    fn unrelated_on_calls_are_ignored() {
        let result = extract(
            r#"
import { Server } from "socket.io";
const io = new Server(httpServer);
process.on("exit", () => {});
emitter.on("data", () => {});
emitter.emit("data", 1);
"#,
        );
        assert!(result.is_empty(), "non-socket .on/.emit must not match");
    }

    #[test]
    fn dynamic_event_names_are_skipped() {
        let result = extract(
            r#"
import { io } from "socket.io-client";
const socket = io(url);
socket.emit(EVENTS.USER_CREATED, payload);
socket.on(`chat:${kind}`, handler);
"#,
        );
        assert!(result.is_empty(), "only literal event names count");
    }

    #[test]
    fn namespace_files_are_skipped_entirely() {
        let result = extract(
            r#"
import { Server } from "socket.io";
const io = new Server(httpServer);
const chat = io.of("/chat");
io.on("connection", (socket) => {
  socket.on("chat:message", handler);
});
"#,
        );
        assert!(
            result.is_empty(),
            "custom namespaces make default-namespace identity ambiguous"
        );
    }

    #[test]
    fn files_without_socket_io_imports_are_ignored() {
        let result = extract(
            r#"
const socket = connectSomething();
socket.on("chat:message", handler);
socket.emit("chat:message", "hi");
"#,
        );
        assert!(result.is_empty());
    }

    fn find(ops: &[SocketOp], canonical: &str) -> SocketOp {
        ops.iter()
            .find(|op| op.key.canonical() == canonical)
            .unwrap_or_else(|| panic!("missing op {canonical} in {ops:?}"))
            .clone()
    }

    #[test]
    fn typed_emitter_payload_captures_symbol_and_source() {
        // `socket.emit("payment:settled", payment)` where `payment: Payment`
        // and `Payment` is imported — the corpus's resolvable case.
        let result = extract(
            r#"
import { io } from "socket.io-client";
import type { Payment } from "./types/payment";
const socket = io("https://payments.internal");
const settle = (payment: Payment) => {
  socket.emit("payment:settled", payment);
};
"#,
        );
        let op = find(&result.emitters, "socket|CLIENT->SERVER|payment:settled");
        assert_eq!(op.payload_type_symbol.as_deref(), Some("Payment"));
        assert_eq!(op.payload_type_source.as_deref(), Some("./types/payment"));
    }

    #[test]
    fn typed_listener_payload_captures_handler_param_type() {
        // server `io.on("connection", socket => socket.on("event", (p: Payment) => …))`
        let result = extract(
            r#"
import { Server } from "socket.io";
import type { Payment } from "./types/payment";
const io = new Server(httpServer);
io.on("connection", (socket) => {
  socket.on("payment:received", (payment: Payment) => { void payment; });
});
"#,
        );
        let op = find(&result.listeners, "socket|CLIENT->SERVER|payment:received");
        assert_eq!(op.payload_type_symbol.as_deref(), Some("Payment"));
        assert_eq!(op.payload_type_source.as_deref(), Some("./types/payment"));
    }

    #[test]
    fn same_file_typed_payload_has_symbol_but_no_source() {
        // Payload type declared in the same file — symbol resolves, but there is
        // no import source (the SymbolRequest path resolves it against the
        // emitting file).
        let result = extract(
            r#"
import { io } from "socket.io-client";
interface Payment { id: string }
const socket = io("https://payments.internal");
const settle = (payment: Payment) => {
  socket.emit("payment:settled", payment);
};
"#,
        );
        let op = find(&result.emitters, "socket|CLIENT->SERVER|payment:settled");
        assert_eq!(op.payload_type_symbol.as_deref(), Some("Payment"));
        assert_eq!(op.payload_type_source, None);
    }

    #[test]
    fn untyped_and_inline_payloads_have_no_symbol() {
        let result = extract(
            r#"
import { io } from "socket.io-client";
import type { Payment } from "./types/payment";
const socket = io("https://chat.internal");
socket.emit("chat:message", "hello");
socket.emit("chat:object", { ok: true });
socket.on("chat:broadcast", (msg) => console.log(msg));
const settle = (payment: Payment[]) => { socket.emit("chat:array", payment); };
"#,
        );
        for canonical in [
            "socket|CLIENT->SERVER|chat:message",
            "socket|CLIENT->SERVER|chat:object",
            "socket|CLIENT->SERVER|chat:array",
        ] {
            let op = find(&result.emitters, canonical);
            assert_eq!(
                op.payload_type_symbol, None,
                "{canonical} should be unanchored"
            );
            assert_eq!(op.payload_type_source, None);
        }
        let listener = find(&result.listeners, "socket|SERVER->CLIENT|chat:broadcast");
        assert_eq!(listener.payload_type_symbol, None);
    }
}
