extern crate swc_common;
extern crate swc_ecma_parser;

use std::{
    collections::{HashMap, HashSet},
    path::PathBuf,
};
use swc_common::SourceMapper;
use swc_ecma_ast::*;
use swc_ecma_visit::{Visit, VisitWith};

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct FunctionArgument {
    #[allow(dead_code)]
    pub name: String,
    #[serde(skip)]
    pub type_ann: Option<TsTypeAnn>, // swc_ecma_ast::TsTypeAnn
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct TypeReference {
    pub file_path: PathBuf,
    #[allow(dead_code)]
    #[serde(skip)]
    pub type_ann: Option<Box<TsType>>,
    pub start_position: usize,
    pub composite_type_string: String,
    pub alias: String,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum Json {
    Null,
    Boolean(bool),
    Number(f64),
    String(String),
    Array(Vec<Json>),
    Object(HashMap<String, Json>),
}

#[derive(Debug, Clone, Default)]
#[allow(dead_code)]
pub enum FunctionNodeType {
    ArrowFunction(Box<ArrowExpr>),
    FunctionDeclaration(Box<FnDecl>),
    FunctionExpression(Box<FnExpr>),
    // Used for deserialization when AST data is not available
    #[default]
    Placeholder,
}

impl serde::Serialize for FunctionNodeType {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        match self {
            FunctionNodeType::ArrowFunction(_) => serializer.serialize_str("ArrowFunction"),
            FunctionNodeType::FunctionDeclaration(_) => {
                serializer.serialize_str("FunctionDeclaration")
            }
            FunctionNodeType::FunctionExpression(_) => {
                serializer.serialize_str("FunctionExpression")
            }
            FunctionNodeType::Placeholder => serializer.serialize_str("Placeholder"),
        }
    }
}

impl<'de> serde::Deserialize<'de> for FunctionNodeType {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let s = String::deserialize(deserializer)?;
        match s.as_str() {
            "ArrowFunction" => Ok(FunctionNodeType::Placeholder),
            "FunctionDeclaration" => Ok(FunctionNodeType::Placeholder),
            "FunctionExpression" => Ok(FunctionNodeType::Placeholder),
            "Placeholder" => Ok(FunctionNodeType::Placeholder),
            _ => Ok(FunctionNodeType::Placeholder),
        }
    }
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct FunctionDefinition {
    #[allow(dead_code)]
    pub name: String,
    #[allow(dead_code)]
    pub file_path: PathBuf,
    pub node_type: FunctionNodeType,
    pub arguments: Vec<FunctionArgument>,
    /// Raw source text of function body (capped at 2000 chars)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub body_source: Option<String>,
    /// Whether the function is exported
    #[serde(default)]
    pub is_exported: bool,
    /// Start line number for navigation
    #[serde(default)]
    pub line_number: u32,
    /// LLM-generated description of what this function intends to do
    #[serde(skip_serializing_if = "Option::is_none")]
    pub intent: Option<String>,
    /// Local functions called by this function (name, file_path, line_number)
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub calls: Vec<FunctionCallRef>,
}

/// A reference to a called function, for navigating to its source via GitHub.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct FunctionCallRef {
    pub name: String,
    pub file_path: String,
    pub line_number: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum OwnerType {
    App(String),
    Router(String),
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Mount {
    pub parent: OwnerType, // App or Router doing the .use
    pub child: OwnerType,  // Router being mounted
    pub prefix: String,    // Path prefix for this mount
}

#[derive(Debug, Clone)]
pub struct Call {
    pub route: String,
    pub method: String,
    #[allow(dead_code)]
    pub response: Json,
    pub request: Option<Json>,
    pub response_type: Option<TypeReference>,
    pub request_type: Option<TypeReference>,
    pub call_file: PathBuf,
    pub call_id: Option<String>, // Unique identifier for this specific call instance
    pub call_number: Option<u32>, // Sequential number for this route+method combination
    pub common_type_name: Option<String>, // Common interface name for type comparison
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SymbolKind {
    Named,
    Default,
    Namespace,
}

#[derive(Debug, Clone)]
pub struct ImportedSymbol {
    #[allow(dead_code)]
    pub local_name: String,
    #[allow(dead_code)]
    pub imported_name: String,
    pub source: String,
    pub kind: SymbolKind,
}

/// Lightweight extractor focused only on import symbols.
/// Used by the multi-agent pipeline for import resolution.
#[derive(Debug)]
pub struct ImportSymbolExtractor {
    pub imported_symbols: HashMap<String, ImportedSymbol>,
}

impl ImportSymbolExtractor {
    pub fn new() -> Self {
        Self {
            imported_symbols: HashMap::new(),
        }
    }
}

impl Default for ImportSymbolExtractor {
    fn default() -> Self {
        Self::new()
    }
}

impl Visit for ImportSymbolExtractor {
    fn visit_import_decl(&mut self, import: &ImportDecl) {
        let source = import.src.value.to_string();

        for specifier in &import.specifiers {
            match specifier {
                ImportSpecifier::Named(named) => {
                    let local_name = named.local.sym.to_string();
                    let imported_name = match &named.imported {
                        Some(ModuleExportName::Ident(ident)) => ident.sym.to_string(),
                        Some(ModuleExportName::Str(str)) => str.value.to_string(),
                        None => local_name.clone(),
                    };
                    self.imported_symbols.insert(
                        local_name.to_string(),
                        ImportedSymbol {
                            local_name: local_name.clone(),
                            imported_name,
                            source: source.clone(),
                            kind: SymbolKind::Named,
                        },
                    );
                }
                ImportSpecifier::Default(default) => {
                    let local_name = default.local.sym.to_string();
                    self.imported_symbols.insert(
                        local_name.to_string(),
                        ImportedSymbol {
                            local_name: local_name.clone(),
                            imported_name: local_name,
                            source: source.clone(),
                            kind: SymbolKind::Default,
                        },
                    );
                }
                ImportSpecifier::Namespace(namespace) => {
                    let local_name = namespace.local.sym.to_string();
                    self.imported_symbols.insert(
                        local_name.to_string(),
                        ImportedSymbol {
                            local_name: local_name.clone(),
                            imported_name: local_name,
                            source: source.clone(),
                            kind: SymbolKind::Namespace,
                        },
                    );
                }
            }
        }
    }
}

/// Extracts locally-declared type symbols for validation.
#[derive(Debug, Default)]
pub struct TypeSymbolExtractor {
    pub type_symbols: HashSet<String>,
}

impl TypeSymbolExtractor {
    pub fn new() -> Self {
        Self {
            type_symbols: HashSet::new(),
        }
    }
}

impl Visit for TypeSymbolExtractor {
    fn visit_ts_type_alias_decl(&mut self, alias: &TsTypeAliasDecl) {
        self.type_symbols.insert(alias.id.sym.to_string());
        alias.visit_children_with(self);
    }

    fn visit_ts_interface_decl(&mut self, interface: &TsInterfaceDecl) {
        self.type_symbols.insert(interface.id.sym.to_string());
        interface.visit_children_with(self);
    }
}

/// Extractor for function definitions with type annotations.
/// Used to extract handler functions for type resolution in the multi-agent pipeline.
pub struct FunctionDefinitionExtractor {
    pub function_definitions: HashMap<String, FunctionDefinition>,
    current_file_path: PathBuf,
    source_map: swc_common::sync::Lrc<swc_common::SourceMap>,
    /// Names of functions that are exported (populated by visit_export_decl / visit_named_export)
    exported_names: HashSet<String>,
}

impl FunctionDefinitionExtractor {
    pub fn new(
        file_path: PathBuf,
        source_map: swc_common::sync::Lrc<swc_common::SourceMap>,
    ) -> Self {
        Self {
            function_definitions: HashMap::new(),
            current_file_path: file_path,
            source_map,
            exported_names: HashSet::new(),
        }
    }

    /// Extract source text from a span, capped at 2000 chars
    fn extract_source(&self, span: swc_common::Span) -> Option<String> {
        if span.is_dummy() {
            return None;
        }
        let src = self.source_map.span_to_snippet(span).ok()?;
        if src.len() > 2000 {
            Some(format!("{}...", &src[..2000]))
        } else {
            Some(src)
        }
    }

    /// Get the line number for a span
    fn line_number(&self, span: swc_common::Span) -> u32 {
        if span.is_dummy() {
            return 0;
        }
        self.source_map.lookup_char_pos(span.lo).line as u32
    }

    /// Extract function arguments with their type annotations
    fn extract_arguments(&self, params: &[Param]) -> Vec<FunctionArgument> {
        params
            .iter()
            .map(|param| {
                let name = match &param.pat {
                    Pat::Ident(ident) => ident.id.sym.to_string(),
                    Pat::Rest(rest) => match &*rest.arg {
                        Pat::Ident(ident) => format!("...{}", ident.id.sym),
                        _ => "...rest".to_string(),
                    },
                    _ => "param".to_string(),
                };

                // Get type annotation if present
                let type_ann = match &param.pat {
                    Pat::Ident(ident) => ident.type_ann.as_ref().map(|t| *t.clone()),
                    _ => None,
                };

                FunctionArgument { name, type_ann }
            })
            .collect()
    }

    /// Extract arguments from arrow function parameters
    fn extract_arrow_arguments(&self, params: &[Pat]) -> Vec<FunctionArgument> {
        params
            .iter()
            .map(|pat| {
                let (name, type_ann) = match pat {
                    Pat::Ident(ident) => (
                        ident.id.sym.to_string(),
                        ident.type_ann.as_ref().map(|t| *t.clone()),
                    ),
                    Pat::Rest(rest) => {
                        let rest_name = match &*rest.arg {
                            Pat::Ident(ident) => format!("...{}", ident.id.sym),
                            _ => "...rest".to_string(),
                        };
                        (rest_name, rest.type_ann.as_ref().map(|t| *t.clone()))
                    }
                    _ => ("param".to_string(), None),
                };

                FunctionArgument { name, type_ann }
            })
            .collect()
    }

    /// Mark functions as exported based on collected export names.
    /// Call this after `module.visit_with()` completes.
    pub fn finalize_exports(&mut self) {
        for (name, def) in self.function_definitions.iter_mut() {
            if self.exported_names.contains(name) {
                def.is_exported = true;
            }
        }
    }
}

impl Visit for FunctionDefinitionExtractor {
    /// Track `export function foo() {}` and `export default function() {}`
    fn visit_export_decl(&mut self, export: &ExportDecl) {
        match &export.decl {
            Decl::Fn(fn_decl) => {
                self.exported_names.insert(fn_decl.ident.sym.to_string());
            }
            Decl::Var(var_decl) => {
                for decl in &var_decl.decls {
                    if let Pat::Ident(ident) = &decl.name {
                        self.exported_names.insert(ident.id.sym.to_string());
                    }
                }
            }
            _ => {}
        }
        // Continue visiting so visit_fn_decl / visit_var_declarator fire
        export.visit_children_with(self);
    }

    /// Track `export { foo, bar }` named exports
    fn visit_named_export(&mut self, export: &NamedExport) {
        // Only track re-exports from local scope (no `from` source)
        if export.src.is_none() {
            for spec in &export.specifiers {
                if let ExportSpecifier::Named(named) = spec {
                    let name = match &named.orig {
                        ModuleExportName::Ident(ident) => ident.sym.to_string(),
                        ModuleExportName::Str(s) => s.value.to_string(),
                    };
                    self.exported_names.insert(name);
                }
            }
        }
    }

    fn visit_fn_decl(&mut self, fn_decl: &FnDecl) {
        let name = fn_decl.ident.sym.to_string();
        let arguments = self.extract_arguments(&fn_decl.function.params);
        let body_source = fn_decl
            .function
            .body
            .as_ref()
            .and_then(|b| self.extract_source(b.span));
        let line_number = self.line_number(fn_decl.function.span);

        self.function_definitions.insert(
            name.clone(),
            FunctionDefinition {
                name,
                file_path: self.current_file_path.clone(),
                node_type: FunctionNodeType::FunctionDeclaration(Box::new(fn_decl.clone())),
                arguments,
                body_source,
                is_exported: false, // Updated in a post-pass
                line_number,
                intent: None,
                calls: vec![],
            },
        );

        // Continue visiting child nodes
        fn_decl.visit_children_with(self);
    }

    fn visit_var_declarator(&mut self, var_decl: &VarDeclarator) {
        // Handle: const myHandler = (req, res) => { ... }
        // Or: const myHandler = function(req, res) { ... }
        if let Pat::Ident(ident) = &var_decl.name {
            let name = ident.id.sym.to_string();

            if let Some(init) = &var_decl.init {
                match &**init {
                    Expr::Arrow(arrow) => {
                        let arguments = self.extract_arrow_arguments(&arrow.params);
                        let body_source = self.extract_source(arrow.span);
                        let line_number = self.line_number(arrow.span);
                        self.function_definitions.insert(
                            name.clone(),
                            FunctionDefinition {
                                name,
                                file_path: self.current_file_path.clone(),
                                node_type: FunctionNodeType::ArrowFunction(Box::new(arrow.clone())),
                                arguments,
                                body_source,
                                is_exported: false,
                                line_number,
                                intent: None,
                                calls: vec![],
                            },
                        );
                    }
                    Expr::Fn(fn_expr) => {
                        let arguments = self.extract_arguments(&fn_expr.function.params);
                        let body_source = fn_expr
                            .function
                            .body
                            .as_ref()
                            .and_then(|b| self.extract_source(b.span));
                        let line_number = self.line_number(fn_expr.function.span);
                        self.function_definitions.insert(
                            name.clone(),
                            FunctionDefinition {
                                name,
                                file_path: self.current_file_path.clone(),
                                node_type: FunctionNodeType::FunctionExpression(Box::new(
                                    fn_expr.clone(),
                                )),
                                arguments,
                                body_source,
                                is_exported: false,
                                line_number,
                                intent: None,
                                calls: vec![],
                            },
                        );
                    }
                    _ => {}
                }
            }
        }

        // Continue visiting child nodes
        var_decl.visit_children_with(self);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser::parse_file;
    use swc_common::{
        SourceMap,
        errors::{ColorConfig, Handler},
        sync::Lrc,
    };

    fn parse_ts(source: &str) -> (Lrc<SourceMap>, Module) {
        let tmp_dir = tempfile::tempdir().expect("tempdir");
        let file_path = tmp_dir.path().join("input.ts");
        std::fs::write(&file_path, source).expect("write file");
        let cm: Lrc<SourceMap> = Default::default();
        let handler = Handler::with_tty_emitter(ColorConfig::Never, true, false, Some(cm.clone()));
        let module = parse_file(&file_path, &cm, &handler).expect("parsed module");
        (cm, module)
    }

    fn extract(source: &str) -> HashMap<String, FunctionDefinition> {
        let (cm, module) = parse_ts(source);
        let mut extractor = FunctionDefinitionExtractor::new(PathBuf::from("test.ts"), cm);
        module.visit_with(&mut extractor);
        extractor.finalize_exports();
        extractor.function_definitions
    }

    #[test]
    fn captures_body_source_for_function_declaration() {
        let defs = extract("function greet(name: string) { return `Hello ${name}`; }");
        let def = defs.get("greet").expect("should find greet");
        assert!(def.body_source.is_some(), "should have body_source");
        assert!(
            def.body_source.as_ref().unwrap().contains("Hello"),
            "body should contain function text"
        );
    }

    #[test]
    fn captures_body_source_for_arrow_function() {
        let defs = extract("const add = (a: number, b: number) => { return a + b; };");
        let def = defs.get("add").expect("should find add");
        assert!(def.body_source.is_some(), "should have body_source");
        assert!(
            def.body_source.as_ref().unwrap().contains("a + b"),
            "body should contain arrow text"
        );
    }

    #[test]
    fn detects_export_function_declaration() {
        let defs = extract("export function hello() { return 1; }");
        let def = defs.get("hello").expect("should find hello");
        assert!(def.is_exported, "export function should be marked exported");
    }

    #[test]
    fn detects_export_const_arrow() {
        let defs = extract("export const foo = () => { return 42; };");
        let def = defs.get("foo").expect("should find foo");
        assert!(
            def.is_exported,
            "export const arrow should be marked exported"
        );
    }

    #[test]
    fn detects_named_export() {
        let defs =
            extract("function bar() { return 1; }\nfunction baz() { return 2; }\nexport { bar };");
        let bar = defs.get("bar").expect("should find bar");
        let baz = defs.get("baz").expect("should find baz");
        assert!(
            bar.is_exported,
            "bar should be marked exported via named export"
        );
        assert!(!baz.is_exported, "baz should NOT be exported");
    }

    #[test]
    fn non_exported_function_is_not_marked() {
        let defs = extract("function internal() { return 'private'; }");
        let def = defs.get("internal").expect("should find internal");
        assert!(
            !def.is_exported,
            "non-exported function should not be marked"
        );
    }

    #[test]
    fn captures_line_number() {
        let defs = extract("function first() { return 1; }");
        let def = defs.get("first").expect("should find first");
        assert!(def.line_number > 0, "line_number should be positive");
    }

    #[test]
    fn caps_body_source_at_2000_chars() {
        // Generate a function with a body > 2000 chars
        let long_body = "x".repeat(2500);
        let source = format!(
            "function big() {{ const s = \"{}\"; return s; }}",
            long_body
        );
        let defs = extract(&source);
        let def = defs.get("big").expect("should find big");
        let body = def.body_source.as_ref().expect("should have body_source");
        assert!(
            body.len() <= 2003, // 2000 + "..."
            "body_source should be capped (got {} chars)",
            body.len()
        );
        assert!(body.ends_with("..."), "capped body should end with ...");
    }
}
