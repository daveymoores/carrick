//! Deterministic GraphQL contract extraction.
//!
//! Producers are SDL schemas: `.graphql`/`.gql` files and `gql`/`graphql`
//! tagged template literals containing type-system definitions. Consumers are
//! executable documents: tagged template literals and `.graphql` files
//! containing operations. Everything here is parse-based — no LLM. Sources
//! that fail to parse (e.g. documents with interpolations mid-token) are
//! skipped silently: per the brittleness guardrails, drift findings may only
//! come from deterministic evidence, so a miss is a coverage gap, never a
//! false positive.
//!
//! Out of scope by design: Relay compiled artifacts and persisted-query
//! manifests (no document in source), and code-first schemas
//! (Pothos/TypeGraphQL/Nexus) unless an emitted `schema.graphql` is
//! committed — the formatter suggests committing one when GraphQL libraries
//! are detected but no operations were extracted.

use crate::operation::{GraphqlOperationKind, OperationKey};
use crate::parser::parse_file;
use std::path::{Path, PathBuf};
use swc_common::errors::{ColorConfig, Handler};
use swc_common::{GLOBALS, Globals, SourceMap, Spanned, sync::Lrc};
use swc_ecma_ast::{Expr, TaggedTpl};
use swc_ecma_visit::{Visit, VisitWith};
use tracing::debug;
use walkdir::WalkDir;

/// A producer or consumer GraphQL operation with its source location.
#[derive(Debug, Clone)]
pub struct GraphqlOp {
    pub key: OperationKey,
    pub file_path: PathBuf,
    pub line: u32,
}

#[derive(Debug, Clone, Default)]
pub struct GraphqlExtraction {
    /// Schema root fields this service provides.
    pub producers: Vec<GraphqlOp>,
    /// Top-level fields of executable documents this service sends.
    pub consumers: Vec<GraphqlOp>,
}

impl GraphqlExtraction {
    pub fn is_empty(&self) -> bool {
        self.producers.is_empty() && self.consumers.is_empty()
    }

    fn merge(&mut self, other: GraphqlExtraction) {
        self.producers.extend(other.producers);
        self.consumers.extend(other.consumers);
    }
}

/// Directories never scanned for GraphQL sources.
const SKIP_DIRS: &[&str] = &[
    "node_modules",
    ".git",
    "dist",
    "build",
    "out",
    "coverage",
    "__generated__", // Relay artifacts — out of scope
];

/// Extract GraphQL operations from a repository: `.graphql`/`.gql` files
/// under `repo_root` plus tagged template literals in the given service
/// files (the same TS/JS set the rest of the pipeline analyzes).
pub fn scan_repo(repo_root: &Path, service_files: &[PathBuf]) -> GraphqlExtraction {
    let mut extraction = GraphqlExtraction::default();

    for entry in WalkDir::new(repo_root)
        .into_iter()
        .filter_entry(|e| {
            !e.file_name()
                .to_str()
                .map(|name| SKIP_DIRS.contains(&name))
                .unwrap_or(false)
        })
        .filter_map(Result::ok)
    {
        let path = entry.path();
        let is_graphql_file = path
            .extension()
            .and_then(|e| e.to_str())
            .is_some_and(|ext| ext == "graphql" || ext == "gql");
        if !is_graphql_file {
            continue;
        }
        let Ok(content) = std::fs::read_to_string(path) else {
            continue;
        };
        extraction.merge(extract_from_document_text(&content, path, 1));
    }

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
        producers = extraction.producers.len(),
        consumers = extraction.consumers.len(),
        "GraphQL extraction complete"
    );
    extraction
}

/// Extract operations from raw GraphQL text. Tries SDL first (producers),
/// then executable-document parsing (consumers). `base_line` is the 1-based
/// line of the text's first line in its host file, so tagged-template
/// contents report host-file line numbers.
pub fn extract_from_document_text(
    text: &str,
    file_path: &Path,
    base_line: u32,
) -> GraphqlExtraction {
    let mut extraction = GraphqlExtraction::default();
    let to_line = |pos_line: usize| base_line.saturating_add(pos_line.saturating_sub(1) as u32);

    if let Ok(schema) = graphql_parser::parse_schema::<String>(text) {
        use graphql_parser::schema::{Definition, TypeDefinition, TypeExtension};

        // Root operation type names default to Query/Mutation/Subscription
        // but can be remapped by an explicit `schema { ... }` definition.
        let mut roots: Vec<(String, GraphqlOperationKind)> = vec![
            ("Query".to_string(), GraphqlOperationKind::Query),
            ("Mutation".to_string(), GraphqlOperationKind::Mutation),
            (
                "Subscription".to_string(),
                GraphqlOperationKind::Subscription,
            ),
        ];
        let mut has_type_system_definitions = false;

        for definition in &schema.definitions {
            if let Definition::SchemaDefinition(schema_def) = definition {
                has_type_system_definitions = true;
                roots.clear();
                if let Some(name) = &schema_def.query {
                    roots.push((name.clone(), GraphqlOperationKind::Query));
                }
                if let Some(name) = &schema_def.mutation {
                    roots.push((name.clone(), GraphqlOperationKind::Mutation));
                }
                if let Some(name) = &schema_def.subscription {
                    roots.push((name.clone(), GraphqlOperationKind::Subscription));
                }
            }
        }

        for definition in &schema.definitions {
            let (name, fields) = match definition {
                Definition::TypeDefinition(TypeDefinition::Object(obj)) => {
                    has_type_system_definitions = true;
                    (&obj.name, &obj.fields)
                }
                Definition::TypeExtension(TypeExtension::Object(ext)) => {
                    has_type_system_definitions = true;
                    (&ext.name, &ext.fields)
                }
                Definition::TypeDefinition(_)
                | Definition::TypeExtension(_)
                | Definition::DirectiveDefinition(_) => {
                    has_type_system_definitions = true;
                    continue;
                }
                Definition::SchemaDefinition(_) => continue,
            };
            let Some((_, kind)) = roots.iter().find(|(root, _)| root == name) else {
                continue;
            };
            for field in fields {
                extraction.producers.push(GraphqlOp {
                    key: OperationKey::graphql(*kind, field.name.clone()),
                    file_path: file_path.to_path_buf(),
                    line: to_line(field.position.line),
                });
            }
        }

        // SDL parsed and contained type-system definitions: this text is a
        // schema, not an executable document — done, even if no root fields
        // were found (e.g. a file defining only `type User`).
        if has_type_system_definitions {
            return extraction;
        }
    }

    if let Ok(document) = graphql_parser::parse_query::<String>(text) {
        use graphql_parser::query::{Definition, OperationDefinition, Selection};

        for definition in &document.definitions {
            let Definition::Operation(operation) = definition else {
                continue; // standalone fragments carry no operation identity
            };
            let (kind, selection_set) = match operation {
                // `{ user }` shorthand is an anonymous query
                OperationDefinition::SelectionSet(set) => (GraphqlOperationKind::Query, set),
                OperationDefinition::Query(q) => (GraphqlOperationKind::Query, &q.selection_set),
                OperationDefinition::Mutation(m) => {
                    (GraphqlOperationKind::Mutation, &m.selection_set)
                }
                OperationDefinition::Subscription(s) => {
                    (GraphqlOperationKind::Subscription, &s.selection_set)
                }
            };
            for selection in &selection_set.items {
                // Top-level fragment spreads can't be resolved without the
                // fragment source (often interpolated) — skip, never guess.
                let Selection::Field(field) = selection else {
                    continue;
                };
                if field.name.starts_with("__") {
                    continue; // introspection
                }
                extraction.consumers.push(GraphqlOp {
                    // alias-aware: match on the real field name, not the alias
                    key: OperationKey::graphql(kind, field.name.clone()),
                    file_path: file_path.to_path_buf(),
                    line: to_line(field.position.line),
                });
            }
        }
    }

    extraction
}

/// Extract operations from `gql`/`graphql` tagged template literals in a
/// TypeScript/JavaScript file.
fn extract_from_ts_file(file_path: &Path) -> GraphqlExtraction {
    let cm: Lrc<SourceMap> = Default::default();
    let handler = Handler::with_tty_emitter(ColorConfig::Never, false, false, Some(cm.clone()));

    let globals = Globals::new();
    GLOBALS.set(&globals, || {
        let Some(module) = parse_file(file_path, &cm, &handler) else {
            return GraphqlExtraction::default();
        };

        let mut visitor = TaggedTplVisitor {
            cm: cm.clone(),
            file_path,
            extraction: GraphqlExtraction::default(),
        };
        module.visit_with(&mut visitor);
        visitor.extraction
    })
}

struct TaggedTplVisitor<'a> {
    cm: Lrc<SourceMap>,
    file_path: &'a Path,
    extraction: GraphqlExtraction,
}

impl Visit for TaggedTplVisitor<'_> {
    fn visit_tagged_tpl(&mut self, node: &TaggedTpl) {
        if let Expr::Ident(tag) = &*node.tag
            && matches!(tag.sym.as_ref(), "gql" | "graphql")
        {
            // Join the literal parts, dropping interpolations. Interpolated
            // fragments leave unresolved spreads behind, which still parse;
            // an interpolation mid-token breaks the parse and the template
            // is skipped silently.
            let text: String = node
                .tpl
                .quasis
                .iter()
                .map(|quasi| {
                    quasi
                        .cooked
                        .as_ref()
                        .map(|c| c.to_string())
                        .unwrap_or_else(|| quasi.raw.to_string())
                })
                .collect::<Vec<_>>()
                .join("\n");

            let base_line = self.cm.lookup_char_pos(node.span().lo).line as u32;
            self.extraction
                .merge(extract_from_document_text(&text, self.file_path, base_line));
        }
        node.visit_children_with(self);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn keys(ops: &[GraphqlOp]) -> Vec<String> {
        let mut keys: Vec<String> = ops.iter().map(|op| op.key.canonical()).collect();
        keys.sort();
        keys
    }

    #[test]
    fn sdl_root_fields_become_producers() {
        let sdl = r#"
            type User { id: ID!, name: String }
            type Query {
                user(id: ID!): User
                users: [User!]!
            }
            type Mutation {
                createUser(name: String!): User
            }
        "#;
        let result = extract_from_document_text(sdl, Path::new("schema.graphql"), 1);
        assert_eq!(
            keys(&result.producers),
            vec![
                "graphql|mutation|createUser",
                "graphql|query|user",
                "graphql|query|users",
            ]
        );
        assert!(result.consumers.is_empty());
    }

    #[test]
    fn schema_definition_remaps_root_types() {
        let sdl = r#"
            schema { query: RootQuery }
            type RootQuery { health: String }
            type Query { ignored: String }
        "#;
        let result = extract_from_document_text(sdl, Path::new("schema.graphql"), 1);
        assert_eq!(keys(&result.producers), vec!["graphql|query|health"]);
    }

    #[test]
    fn extend_type_query_adds_producers() {
        let sdl = r#"
            extend type Query { extra: String }
        "#;
        let result = extract_from_document_text(sdl, Path::new("schema.graphql"), 1);
        assert_eq!(keys(&result.producers), vec!["graphql|query|extra"]);
    }

    #[test]
    fn non_root_only_sdl_is_schema_not_document() {
        let sdl = "type User { id: ID! }";
        let result = extract_from_document_text(sdl, Path::new("types.graphql"), 1);
        assert!(result.producers.is_empty());
        assert!(result.consumers.is_empty());
    }

    #[test]
    fn operations_become_consumers_with_real_field_names() {
        let doc = r#"
            query GetUser($id: ID!) {
                currentUser: user(id: $id) { id name }
                __typename
            }
            mutation { createUser(name: "x") { id } }
        "#;
        let result = extract_from_document_text(doc, Path::new("queries.graphql"), 1);
        assert_eq!(
            keys(&result.consumers),
            vec!["graphql|mutation|createUser", "graphql|query|user"]
        );
        assert!(result.producers.is_empty());
    }

    #[test]
    fn anonymous_shorthand_is_a_query() {
        let result = extract_from_document_text("{ health }", Path::new("q.graphql"), 1);
        assert_eq!(keys(&result.consumers), vec!["graphql|query|health"]);
    }

    #[test]
    fn unparseable_text_is_skipped_silently() {
        let result = extract_from_document_text("query { broken", Path::new("q.graphql"), 1);
        assert!(result.is_empty());
    }

    #[test]
    fn tagged_templates_are_extracted_from_ts() {
        let dir = std::env::temp_dir().join(format!("carrick-gql-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("client.ts");
        std::fs::write(
            &file,
            r#"
import { gql } from "graphql-tag";
const FRAGMENT = gql`fragment UserFields on User { id name }`;
const GET_USER = gql`
  query GetUser($id: ID!) {
    user(id: $id) { ...UserFields }
  }
  ${FRAGMENT}
`;
const notGraphql = sql`SELECT 1`;
"#,
        )
        .unwrap();

        let result = extract_from_ts_file(&file);
        std::fs::remove_dir_all(&dir).ok();

        assert_eq!(keys(&result.consumers), vec!["graphql|query|user"]);
        assert!(result.producers.is_empty());
    }

    #[test]
    fn typedefs_template_yields_producers() {
        let dir = std::env::temp_dir().join(format!("carrick-gql-sdl-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("server.ts");
        std::fs::write(
            &file,
            r#"
import gql from "graphql-tag";
export const typeDefs = gql`
  type Query { orders: [Order!]! }
  type Order { id: ID! }
`;
"#,
        )
        .unwrap();

        let result = extract_from_ts_file(&file);
        std::fs::remove_dir_all(&dir).ok();

        assert_eq!(keys(&result.producers), vec!["graphql|query|orders"]);
    }
}
