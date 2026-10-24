//! A route registration the model said sends no payload is still asked of the
//! type layer, by its registration line (carrick#2719).
//!
//! On `router.route(path).get(middleware, importedHandler)` the handler's body
//! is in another file. The model reading the registration file never saw it,
//! so its "no payload" is a claim about code it did not read. Before this
//! change `collect_type_requests` took that claim as a gate and sent the
//! sidecar nothing for the route, so its response entry stayed `unknown`
//! however plainly the handler typed its reply.
//!
//! The registration line is the anchor: the type layer follows the handler
//! the registration names, wherever it is declared. A route whose handler
//! really sends no body still ends with no type, because a handler that
//! returns void answers nothing at a registration line.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use carrick::agent_service::AgentService;
use carrick::agents::file_analyzer_agent::{EmissionStyle, EndpointResult, FileAnalysisResult};
use carrick::agents::file_orchestrator::FileOrchestrator;
use carrick::config::Config;
use carrick::services::type_sidecar::{InferKind, InferRequestItem, TypeSidecar};
use carrick::url_normalizer::UrlNormalizer;
use tempfile::TempDir;

const ROUTES_TS: &str = r#"import { Router } from './framework';
import { requireAuth } from './middleware';
import { listItems } from './items.handlers';

export const router = new Router();

router.route('/items').get(requireAuth, listItems);
router.route('/ping').get(requireAuth, (_req, res) => {
  res.sendStatus(204);
});
"#;

/// 1-based line of the registration naming the imported handler.
const IMPORTED_LINE: i32 = 7;
/// 1-based line of the registration whose inline handler sends no body.
const INLINE_LINE: i32 = 8;

fn write_fixture_repo(repo: &Path) {
    fs::create_dir_all(repo.join("src")).expect("create src");
    fs::write(
        repo.join("package.json"),
        r#"{ "name": "route-chain-api", "version": "1.0.0" }"#,
    )
    .unwrap();
    fs::write(
        repo.join("tsconfig.json"),
        r#"{
  "compilerOptions": {
    "target": "ES2020",
    "module": "commonjs",
    "moduleResolution": "node",
    "strict": true,
    "esModuleInterop": true,
    "skipLibCheck": true
  },
  "include": ["src/**/*"]
}"#,
    )
    .unwrap();
    fs::write(
        repo.join("src/framework.ts"),
        r#"export interface Req {
  params: Record<string, string>;
}

export interface Res<TBody = unknown> {
  json(body: TBody): Res<TBody>;
  sendStatus(code: number): void;
}

export type Handler<TBody = unknown> = (req: Req, res: Res<TBody>) => unknown;

export interface RouteChain {
  get(...handlers: Handler<any>[]): RouteChain;
}

export class Router {
  route(path: string): RouteChain {
    throw new Error(path);
  }
}
"#,
    )
    .unwrap();
    fs::write(
        repo.join("src/middleware.ts"),
        r#"import { Req, Res } from './framework';

export function requireAuth(_req: Req, _res: Res, next?: () => void): void {
  next?.();
}
"#,
    )
    .unwrap();
    fs::write(
        repo.join("src/items.handlers.ts"),
        r#"import { Req } from './framework';

export interface ItemList {
  items: { id: string; label: string }[];
  total: number;
}

export async function listItems(_req: Req): Promise<ItemList> {
  const body: ItemList = { items: [{ id: 'a', label: 'first' }], total: 1 };
  return body;
}
"#,
    )
    .unwrap();
    fs::write(repo.join("src/items.routes.ts"), ROUTES_TS).unwrap();
}

/// The analyzer's rows for the two registrations, as the model states them
/// for a file whose handlers it cannot see into: no payload, no located
/// expression. The spans are the registration calls, as the deterministic
/// candidates carry them.
fn analyzer_result(routes_file: &str) -> HashMap<String, FileAnalysisResult> {
    let row = |line: i32, path: &str, handler: &str| {
        let needle = format!("router.route('{path}')");
        let start = ROUTES_TS
            .find(&needle)
            .expect("fixture registers the route") as u32;
        let end = start + ROUTES_TS[start as usize..].find(";\n").unwrap() as u32;
        EndpointResult {
            handler_declaration_line: None,
            registration_literal: None,
            view_module: false,
            candidate_id: format!("span:{start}-{end}"),
            line_number: line,
            owner_node: "router".to_string(),
            method: "GET".to_string(),
            path: path.to_string(),
            handler_name: handler.to_string(),
            pattern_matched: format!("{needle}.get("),
            call_expression_span_start: Some(start),
            call_expression_span_end: Some(end),
            payload_expression_text: None,
            payload_expression_line: None,
            response_expression_text: None,
            response_expression_line: None,
            emission_style: Some(EmissionStyle::NoPayload),
            primary_type_symbol: None,
            type_import_source: None,
            resolution_source: None,
            dispatch: None,
        }
    };
    let mut file_results = HashMap::new();
    file_results.insert(
        routes_file.to_string(),
        FileAnalysisResult {
            graphql_consumer_locates: vec![],
            mounts: vec![],
            endpoints: vec![
                row(IMPORTED_LINE, "/items", "listItems"),
                row(INLINE_LINE, "/ping", "anonymous"),
            ],
            data_calls: vec![],
            graphql_operations: vec![],
            pubsub_operations: vec![],
            dispatch_tables: Vec::new(),
        },
    );
    file_results
}

/// The scanner's response requests for the fixture's routes, by line.
fn response_requests(repo: &Path) -> HashMap<u32, InferRequestItem> {
    let routes_file = repo
        .join("src/items.routes.ts")
        .to_string_lossy()
        .to_string();
    let file_results = analyzer_result(&routes_file);
    let orchestrator = FileOrchestrator::new(AgentService::new());
    let mount_graph = orchestrator.build_mount_graph(
        &file_results,
        &UrlNormalizer::default_permissive(),
        Path::new(""),
        Path::new(""),
    );
    let (_explicit, infer, _inline) = orchestrator.collect_type_requests(
        &file_results,
        &repo.to_string_lossy(),
        &mount_graph,
        &Config::default(),
        &carrick::workspace_resolver::WorkspaceIndex::build_with_aliases(repo, None),
    );
    infer
        .into_iter()
        .filter(|item| item.infer_kind == InferKind::ResponseBody)
        .map(|item| (item.line_number, item))
        .collect()
}

#[test]
fn test_no_payload_registration_is_asked_by_its_line() {
    let temp_dir = TempDir::new().expect("temp dir");
    let repo = temp_dir.path().join("api");
    write_fixture_repo(&repo);

    let requests = response_requests(&repo);
    for line in [IMPORTED_LINE, INLINE_LINE] {
        let item = requests.get(&(line as u32)).unwrap_or_else(|| {
            panic!("the route at line {line} must be asked for its response, got: {requests:?}")
        });
        assert!(
            item.span_start.is_none()
                && item.span_end.is_none()
                && item.expression_text.is_none()
                && item.expression_line.is_none(),
            "a no-payload row is asked by its registration line only: {item:?}"
        );
    }
}

fn is_node_available() -> bool {
    Command::new("node")
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

fn sidecar_path() -> Option<PathBuf> {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src/sidecar/dist/src/index.js");
    path.exists().then_some(path)
}

/// Through the real sidecar: the imported handler's reply is typed, and the
/// inline handler that sends no body answers nothing.
#[test]
fn test_no_payload_registration_line_types_imported_handler_only() {
    if !is_node_available() {
        eprintln!("Skipping test: Node.js not available");
        return;
    }
    let Some(sidecar_path) = sidecar_path() else {
        eprintln!("Skipping test: sidecar not built (run: cd src/sidecar && npm run build)");
        return;
    };
    let temp_dir = TempDir::new().expect("temp dir");
    let repo = temp_dir.path().join("api");
    write_fixture_repo(&repo);

    let requests = response_requests(&repo);
    let items: Vec<InferRequestItem> = [IMPORTED_LINE, INLINE_LINE]
        .iter()
        .map(|line| {
            let mut item = requests
                .get(&(*line as u32))
                .cloned()
                .unwrap_or_else(|| panic!("no request for line {line}: {requests:?}"));
            item.alias = Some(format!("Route_{line}_Response"));
            item
        })
        .collect();

    let sidecar = TypeSidecar::spawn(&sidecar_path).expect("spawn sidecar");
    sidecar.start_init(&repo, None);
    sidecar
        .wait_ready(Duration::from_secs(60))
        .expect("sidecar init");
    let result = sidecar
        .resolve_all_types(&[], &items, None, None)
        .expect("resolve_all_types");
    let type_of = |line: i32| {
        let alias = format!("Route_{line}_Response");
        result
            .inferred_types
            .iter()
            .find(|t| t.alias == alias)
            .map(|t| t.type_string.clone())
    };

    let imported = type_of(IMPORTED_LINE).unwrap_or_else(|| {
        panic!(
            "the imported handler's reply must be typed, got: {:?}",
            result.inferred_types
        )
    });
    assert!(
        imported.contains("total: number") && imported.contains("label: string"),
        "the imported handler's reply is its stated body, got: {imported}"
    );
    assert_eq!(
        type_of(INLINE_LINE),
        None,
        "an inline handler that sends no body must stay untyped"
    );
}
