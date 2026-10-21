// FROZEN (old path, 2026-10-21): deletions and safety fixes only. Fixes for
// wrong or missing rows go in request summaries (`crate::request_summary`).
// Delete when summaries give the same or better rows on this pass's fixtures
// and the four-repo replay, with no true row lost, including the verb this
// pass corrects on a golden repo (carrick#1561).

//! The verb at a call through a declaration is the declaration's, not the
//! model's (carrick#1384).
//!
//! A call site written on an imported binding states no method: the verb is in
//! the declaration it reaches. The model answers such a site with one anyway,
//! and nothing checked it — [`crate::wrapper_request_shape`] propagates a
//! module's verb only onto rows with no span of their own, and folds it over
//! every wrapper module a FILE imports rather than the one this call reaches.
//! So a guard that delegates to a GET fetcher was indexed as a POST.
//!
//! This pass reads the verb where it is written. For a model row at a call site
//! that states no request of its own, the declaration the call reaches is
//! resolved through the module graph
//! ([`crate::wrapper_call_join::declaration_reached`]), and the requests that
//! module issues are folded: when every one of them states the same literal
//! verb, that verb is the site's.
//!
//! Every part of that is read off the AST, and no library, framework or hook is
//! named. The guards are the same "say nothing" ones the fold uses:
//!
//! - **The site must state no verb.** A call the scanner reads as a request
//!   itself — an HTTP-verb callee, a request-options bag — states its own
//!   method, and this pass never touches it.
//! - **Only a model row is corrected.** A deterministic row read its method off
//!   the source it was resolved from.
//! - **Only the requests of the function the call reaches are read**
//!   (carrick#2384): that function and the functions of its own module it
//!   calls, not the module's other functions. A verb-spelled call counts as a
//!   request only when it can name a route
//!   ([`crate::wrapper_request_shape::call_request_verb`]).
//! - **The declaration's requests must agree** on one literal verb
//!   ([`fold_module`]). A module that parameterizes its method, or issues
//!   requests that disagree, states nothing, and the row keeps what it had —
//!   counted, so a scan can say how often it could not answer.
//! - **One hop for a module that issues no request of its own.** A guard
//!   sitting between the site and the fetcher delegates to a module it imports,
//!   so the modules IT imports are read as well, and they must all agree. A
//!   verb two modules further out is not in what this pass reads, and the row
//!   keeps the model's.
//!
//! The correction moves the row's operation, so it runs before the passes that
//! decide which rows are the same request
//! ([`crate::consumer_row_fold`], [`crate::wrapper_call_join`]).

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use tracing::debug;

use crate::agents::file_analyzer_agent::{DataCallResult, FileAnalysisResult, ResolutionSource};
use crate::import_bindings::BindingResolver;
use crate::type_manifest::normalize_manifest_method;
use crate::workspace_resolver::WorkspaceIndex;
use crate::wrapper_call_join::{
    CallSite, FileCalls, declaration_reached, read_call_sites, site_of,
};
use crate::wrapper_request_shape::{RequestShapeSignal, WrapperRequestShape, fold_module};

/// What the pass decided, so a scan can state it rather than change the index
/// silently.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct WrapperMethodCorrections {
    /// Rows whose verb the declaration they reach states, and disagreed with.
    pub corrected: usize,
    /// Rows left alone because the declaration issues requests whose verb it
    /// could not read, or whose verbs disagree. The row keeps the model's, and
    /// this is how often that happened.
    pub declaration_unreadable: usize,
}

/// Correct the method of every model row at a call through a declaration whose
/// requests state one.
///
/// `workspace` resolves the specifiers the repo declares (aliases, workspace
/// packages) as well as relative ones; `None` follows relative specifiers only.
pub fn correct_wrapper_call_methods(
    file_results: &mut HashMap<String, FileAnalysisResult>,
    workspace: Option<&WorkspaceIndex>,
) -> WrapperMethodCorrections {
    let mut corrections = WrapperMethodCorrections::default();

    // Only a file with a model row has a site to read; nothing else is parsed.
    let mut keys: Vec<String> = file_results
        .iter()
        .filter(|(_, result)| result.data_calls.iter().any(is_correctable_row))
        .map(|(key, _)| key.clone())
        .collect();
    keys.sort();
    if keys.is_empty() {
        return corrections;
    }

    let mut resolver = match workspace {
        Some(workspace) => BindingResolver::with_workspace(workspace.clone()),
        None => BindingResolver::new(),
    };
    // One declaration is read once however many sites reach it.
    let mut declarations: HashMap<DeclarationKey, DeclarationRequests> = HashMap::new();

    for key in keys {
        let file = Path::new(&key);
        let Ok(canonical) = file.canonicalize() else {
            continue;
        };
        let Some(calls) = read_call_sites(file) else {
            continue;
        };
        let Some(result) = file_results.get(&key) else {
            continue;
        };

        let mut stated: Vec<(usize, String)> = Vec::new();
        for (index, call) in result.data_calls.iter().enumerate() {
            if !is_correctable_row(call) {
                continue;
            }
            let Some(site) = site_of(&calls.sites, call) else {
                continue;
            };
            // The site's own source states the request: its method is what the
            // scanner read there, not something to look up elsewhere.
            if site.request != RequestShapeSignal::NotARequest {
                continue;
            }
            let Some(reached) = declaration_reached(
                &mut resolver,
                workspace,
                file,
                &canonical,
                &calls.imports,
                &site.root,
            ) else {
                continue;
            };
            // The function the call runs, not the module it sits in: a sibling
            // function's request is not this call's (carrick#2384).
            let member = site.member.clone().filter(|_| !site.direct);
            let identity = (reached.file.clone(), reached.local.clone(), member);
            let requests = match declarations.get(&identity) {
                Some(requests) => requests.clone(),
                None => {
                    let requests = declaration_requests(
                        &mut resolver,
                        workspace,
                        &reached.file,
                        reached.local.as_deref(),
                        identity.2.as_deref(),
                        true,
                    );
                    declarations.insert(identity, requests.clone());
                    requests
                }
            };
            match requests {
                DeclarationRequests::None => {}
                DeclarationRequests::Unreadable => {
                    corrections.declaration_unreadable += 1;
                    debug!(
                        "  - {key}: line {} reaches {} in {}, whose requests state no single \
                         verb; keeping the extracted method",
                        call.line_number,
                        reached.published,
                        reached.file.display()
                    );
                }
                DeclarationRequests::Stated(shape) => {
                    let extracted = call
                        .method
                        .as_deref()
                        .map(normalize_manifest_method)
                        .unwrap_or_default();
                    if extracted != shape.method {
                        debug!(
                            "  - {key}: line {} states no verb and reaches {} in {}, which \
                             requests {}; correcting {extracted}",
                            call.line_number,
                            reached.published,
                            reached.file.display(),
                            shape.method
                        );
                        stated.push((index, shape.method));
                    }
                }
            }
        }

        if stated.is_empty() {
            continue;
        }
        let Some(result) = file_results.get_mut(&key) else {
            continue;
        };
        for (index, method) in stated {
            result.data_calls[index].method = Some(method);
            corrections.corrected += 1;
        }
    }

    corrections
}

/// What the requests of one declaring module state about their verb.
#[derive(Debug, Clone, PartialEq, Eq)]
enum DeclarationRequests {
    /// The module issues no request this pass can reach, one hop included.
    None,
    /// It issues requests, and they state no single literal verb.
    Unreadable,
    /// Every request it issues states the same literal verb.
    Stated(WrapperRequestShape),
}

/// A declaration a call reaches: the module, the name it binds there, and the
/// member of it the call runs.
type DeclarationKey = (PathBuf, Option<String>, Option<String>);

/// Read what the requests of one declaration state, following one hop for a
/// declaration that issues none of its own.
///
/// The declaration is the function the call runs (`local`, and `member` for a
/// call through an object's or class's member), with the functions of the same
/// module it calls. A request elsewhere in the module is another function's:
/// `clear()` calling `cache.delete(…)` says nothing about `send()` (carrick#2384).
/// A declaration the module does not state by name (a default export, a
/// binding the resolver could not follow) is read as the whole module.
fn declaration_requests(
    resolver: &mut BindingResolver,
    workspace: Option<&WorkspaceIndex>,
    module: &Path,
    local: Option<&str>,
    member: Option<&str>,
    hop: bool,
) -> DeclarationRequests {
    let Some(calls) = read_call_sites(module) else {
        return DeclarationRequests::None;
    };
    let scoped = scoped_sites(&calls, local, member);
    let signals: Vec<&RequestShapeSignal> = scoped.iter().map(|site| &site.request).collect();
    if signals
        .iter()
        .any(|signal| **signal != RequestShapeSignal::NotARequest)
    {
        // The declaration performs HTTP itself, so its own requests are what
        // it issues. What it imports is not read then: its own requests
        // already answer, and a client module that also imports another client
        // says nothing about which one a member uses.
        return match fold_module(signals) {
            Some(shape) => DeclarationRequests::Stated(shape),
            None => DeclarationRequests::Unreadable,
        };
    }
    if !hop {
        return DeclarationRequests::None;
    }

    // A declaration that issues no request delegates to one it imports: the
    // guard between the site and the fetcher. Exactly one hop: every
    // declaration it calls that issues a request of its own has to agree, and
    // a verb one module further out is not read at all.
    let mut folded: Option<WrapperRequestShape> = None;
    let mut seen: Vec<DeclarationKey> = Vec::new();
    for site in scoped {
        let Some(reached) = declaration_reached(
            resolver,
            workspace,
            module,
            module,
            &calls.imports,
            &site.root,
        ) else {
            continue;
        };
        let next_member = site.member.clone().filter(|_| !site.direct);
        let identity = (reached.file.clone(), reached.local.clone(), next_member);
        if seen.contains(&identity) {
            continue;
        }
        seen.push(identity.clone());
        let delegated = declaration_requests(
            resolver,
            workspace,
            &identity.0,
            identity.1.as_deref(),
            identity.2.as_deref(),
            false,
        );
        match delegated {
            DeclarationRequests::None => continue,
            DeclarationRequests::Unreadable => return DeclarationRequests::Unreadable,
            DeclarationRequests::Stated(shape) => match &mut folded {
                None => folded = Some(shape),
                Some(accumulated) => {
                    if accumulated.method != shape.method {
                        return DeclarationRequests::Unreadable;
                    }
                    if accumulated.has_body != shape.has_body {
                        accumulated.has_body = None;
                    }
                }
            },
        }
    }
    match folded {
        Some(shape) => DeclarationRequests::Stated(shape),
        None => DeclarationRequests::None,
    }
}

/// The calls the declaration `local` (and its `member`) makes: those inside
/// its source, and inside any function of the same module they call, followed
/// to a fixed point. The whole module when it states no such declaration.
fn scoped_sites<'a>(
    calls: &'a FileCalls,
    local: Option<&str>,
    member: Option<&str>,
) -> Vec<&'a CallSite> {
    let Some(start) = local.and_then(|name| declaration_ranges(calls, name, member)) else {
        return calls.sites.iter().collect();
    };
    let mut ranges: Vec<(u32, u32)> = start;
    let mut followed: Vec<(&str, Option<&str>)> = Vec::new();
    loop {
        let inside = |site: &CallSite| {
            ranges
                .iter()
                .any(|(lo, hi)| *lo <= site.span && site.span < *hi)
        };
        let mut added: Vec<(u32, u32)> = Vec::new();
        for site in calls.sites.iter().filter(|site| inside(site)) {
            // A binding the module declares, called: its own source is part of
            // what this call runs. An imported name is another module's.
            if calls.imports.contains_key(&site.root) {
                continue;
            }
            let called = (
                site.root.as_str(),
                site.member.as_deref().filter(|_| !site.direct),
            );
            if followed.contains(&called) {
                continue;
            }
            followed.push(called);
            if let Some(more) = declaration_ranges(calls, called.0, called.1) {
                added.extend(more);
            }
        }
        if added.is_empty() {
            break;
        }
        ranges.extend(added);
    }
    calls
        .sites
        .iter()
        .filter(|site| {
            ranges
                .iter()
                .any(|(lo, hi)| *lo <= site.span && site.span < *hi)
        })
        .collect()
}

/// The source of the declaration `name`, narrowed to `member` when the module
/// states that member. `None` when the module declares no such name.
fn declaration_ranges(
    calls: &FileCalls,
    name: &str,
    member: Option<&str>,
) -> Option<Vec<(u32, u32)>> {
    let named = |wanted: Option<&str>| -> Vec<(u32, u32)> {
        calls
            .bindings
            .iter()
            .filter(|binding| binding.name == name && binding.member.as_deref() == wanted)
            .map(|binding| (binding.lo, binding.hi))
            .collect()
    };
    let narrowed = member.map(|member| named(Some(member))).unwrap_or_default();
    let ranges = if narrowed.is_empty() {
        named(None)
    } else {
        narrowed
    };
    (!ranges.is_empty()).then_some(ranges)
}

/// Whether this row is one the pass may correct: the model's own reading.
///
/// Deliberately NOT restricted to a row with a span. A call written on an
/// imported binding raises no HTTP candidate of its own — that is what makes
/// its verb unreadable at the site in the first place — so the rows this pass
/// exists for are routinely the ones with no span, placed by their line.
fn is_correctable_row(call: &DataCallResult) -> bool {
    call.resolution_source == Some(ResolutionSource::Model)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::swc_scanner::SWC_SPAN_BASE;

    /// The fetcher: every request it issues is a GET, read off an options bag
    /// that states no method.
    const FETCHER: &str = r#"export async function sendJson(path: string) {
  const response = await fetch(path, { headers: { accept: "application/json" } });
  return response.json();
}
"#;

    /// The guard between the site and the fetcher. It issues no request of its
    /// own: the verb is one module further out.
    const GUARD: &str = r#"import { sendJson } from "./fetcher";

export async function requireEntitlement(scope: string) {
  return sendJson(`/v1/shelves?scope=${scope}`);
}
"#;

    /// The site: a call on an imported binding, stating no verb at all.
    const SITE: &str = r#"import { requireEntitlement } from "../lib/guard";

export function PanelRoute() {
  return requireEntitlement("panel");
}
"#;

    fn write(root: &Path, rel: &str, content: &str) -> String {
        let path = root.join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, content).unwrap();
        path.to_string_lossy().into_owned()
    }

    /// A model row at the call that starts at `needle`, placed by its LINE and
    /// carrying no span — the shape a call through an imported binding has,
    /// because no HTTP candidate is raised at such a site.
    fn row(content: &str, needle: &str, method: &str, target: &str) -> DataCallResult {
        DataCallResult {
            candidate_id: "model".to_string(),
            call_expression_span_start: None,
            call_expression_span_end: None,
            ..spanned_row(content, needle, method, target)
        }
    }

    /// The same row as the candidate scanner joined it: with its span.
    fn spanned_row(content: &str, needle: &str, method: &str, target: &str) -> DataCallResult {
        let offset = content.find(needle).expect("needle is in the source") as u32;
        let span = offset + SWC_SPAN_BASE;
        let line = content[..offset as usize].lines().count() as i32;
        DataCallResult {
            candidate_id: format!("span:{span}"),
            line_number: line,
            target: target.to_string(),
            method: Some(method.to_string()),
            call_kind: None,
            pattern_matched: "requireEntitlement(".to_string(),
            call_expression_span_start: Some(span),
            call_expression_span_end: Some(span + needle.len() as u32),
            call_expression_text: None,
            call_expression_line: Some(line),
            payload_expression_text: None,
            payload_expression_line: None,
            primary_type_symbol: None,
            type_import_source: None,
            loopback_default_url: None,
            base: None,
            consumers_not_resolved: None,
            dispatch: None,
            resolution_source: Some(ResolutionSource::Model),
            reaches_request: None,
            body_literals: Default::default(),
            library_semantics: Vec::new(),
            at_caller: false,
            call_body: None,
        }
    }

    fn correct(
        files: Vec<(String, Vec<DataCallResult>)>,
    ) -> (
        HashMap<String, FileAnalysisResult>,
        WrapperMethodCorrections,
    ) {
        correct_with(files, None)
    }

    /// The same, with the module index a scan hands the pass. `None` follows
    /// relative specifiers only, which is what a repo's declared aliases are
    /// tested against.
    fn correct_with(
        files: Vec<(String, Vec<DataCallResult>)>,
        workspace: Option<&WorkspaceIndex>,
    ) -> (
        HashMap<String, FileAnalysisResult>,
        WrapperMethodCorrections,
    ) {
        let mut results: HashMap<String, FileAnalysisResult> = files
            .into_iter()
            .map(|(path, data_calls)| {
                (
                    path,
                    FileAnalysisResult {
                        data_calls,
                        ..Default::default()
                    },
                )
            })
            .collect();
        let corrections = correct_wrapper_call_methods(&mut results, workspace);
        (results, corrections)
    }

    fn method(results: &HashMap<String, FileAnalysisResult>, file: &str) -> Option<String> {
        results[file].data_calls[0].method.clone()
    }

    /// The acceptance shape again, with every import written through an alias
    /// the repo declares — which is how a repo that declares one writes them
    /// all, including the guard's own import of the fetcher, so the hop needs
    /// the index too. Without the index the specifiers reach nothing and the
    /// row keeps the verb the model gave it.
    #[test]
    fn a_guard_reached_through_a_declared_alias_still_states_its_fetcher_s_verb() {
        for (config, contents) in [
            (
                "tsconfig.json",
                "{\n  \"compilerOptions\": {\n    \"baseUrl\": \".\",\n    \"paths\": { \"@/*\": [\"src/*\"] }\n  }\n}\n",
            ),
            (
                "deno.jsonc",
                "{\n  // the import map a Deno repo declares\n  \"imports\": { \"@/\": \"./src/\" }\n}\n",
            ),
        ] {
            let tmp = tempfile::tempdir().unwrap();
            let root = tmp.path();
            write(root, config, contents);
            write(root, "src/lib/fetcher.ts", FETCHER);
            write(
                root,
                "src/lib/guard.ts",
                &GUARD.replace("./fetcher", "@/lib/fetcher"),
            );
            let source = SITE.replace("../lib/guard", "@/lib/guard");
            let site = write(root, "src/routes/panel.tsx", &source);

            let rows = || {
                vec![(
                    site.clone(),
                    vec![row(
                        &source,
                        "requireEntitlement(\"panel\")",
                        "POST",
                        "/v1/shelves",
                    )],
                )]
            };

            let workspace = WorkspaceIndex::build_with_aliases(root, None);
            let (corrected, corrections) = correct_with(rows(), Some(&workspace));
            assert_eq!(
                method(&corrected, &site).as_deref(),
                Some("GET"),
                "{config}: the aliases the repo declares reach the guard and its fetcher"
            );
            assert_eq!(corrections.corrected, 1, "{config}");

            let (untouched, corrections) = correct_with(rows(), None);
            assert_eq!(
                method(&untouched, &site).as_deref(),
                Some("POST"),
                "{config}: without the repo's own resolver nothing states a verb, and the \
                 row keeps what it had"
            );
            assert_eq!(corrections, WrapperMethodCorrections::default(), "{config}");
        }
    }

    /// carrick#1384's acceptance: the site calls an imported guard, the guard
    /// delegates to a GET fetcher, and the model answered the site with POST.
    #[test]
    fn a_guard_delegating_to_a_get_fetcher_states_get_at_its_call_site() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        write(root, "src/lib/fetcher.ts", FETCHER);
        write(root, "src/lib/guard.ts", GUARD);
        let site = write(root, "src/routes/panel.tsx", SITE);

        let (results, corrections) = correct(vec![(
            site.clone(),
            vec![row(
                SITE,
                "requireEntitlement(\"panel\")",
                "POST",
                "/v1/shelves",
            )],
        )]);

        assert_eq!(
            method(&results, &site).as_deref(),
            Some("GET"),
            "the site states no verb; the requests it reaches are GETs"
        );
        assert_eq!(
            corrections,
            WrapperMethodCorrections {
                corrected: 1,
                declaration_unreadable: 0
            }
        );
    }

    #[test]
    fn a_site_that_states_its_own_verb_is_left_alone() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        write(root, "src/lib/fetcher.ts", FETCHER);
        // The site calls the fetcher's module directly, with a verb of its own.
        let source = "import { client } from \"../lib/fetcher\";\n\nexport function save() {\n  return client.post(\"/v1/shelves\", {});\n}\n";
        let site = write(root, "src/routes/panel.tsx", source);

        let (results, corrections) = correct(vec![(
            site.clone(),
            vec![spanned_row(
                source,
                "client.post(\"/v1/shelves\", {})",
                "POST",
                "/v1/shelves",
            )],
        )]);

        assert_eq!(
            method(&results, &site).as_deref(),
            Some("POST"),
            "the verb is written at this line; nothing elsewhere overrules it"
        );
        assert_eq!(corrections, WrapperMethodCorrections::default());
    }

    #[test]
    fn a_declaration_whose_requests_disagree_keeps_the_extracted_verb() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        write(
            root,
            "src/lib/guard.ts",
            "export async function requireEntitlement(scope: string) {\n  if (scope === \"panel\") {\n    return fetch(\"/v1/shelves\", { method: \"POST\", body: \"{}\" });\n  }\n  return fetch(\"/v1/shelves\", { headers: {} });\n}\n",
        );
        let site = write(root, "src/routes/panel.tsx", SITE);

        let (results, corrections) = correct(vec![(
            site.clone(),
            vec![row(
                SITE,
                "requireEntitlement(\"panel\")",
                "PUT",
                "/v1/shelves",
            )],
        )]);

        assert_eq!(
            method(&results, &site).as_deref(),
            Some("PUT"),
            "the declaration issues a POST and a GET: nothing to correct from"
        );
        assert_eq!(
            corrections,
            WrapperMethodCorrections {
                corrected: 0,
                declaration_unreadable: 1
            },
            "and the scan says how often it could not answer"
        );
    }

    /// The hop budget, stated. The guard delegates to a helper that delegates
    /// to the fetcher, so the verb is two modules out and this pass does not
    /// read it.
    #[test]
    fn a_verb_two_modules_out_is_not_read() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        write(root, "src/lib/fetcher.ts", FETCHER);
        write(
            root,
            "src/lib/helper.ts",
            "import { sendJson } from \"./fetcher\";\n\nexport const send = (path: string) => sendJson(path);\n",
        );
        write(
            root,
            "src/lib/guard.ts",
            "import { send } from \"./helper\";\n\nexport async function requireEntitlement(scope: string) {\n  return send(`/v1/shelves?scope=${scope}`);\n}\n",
        );
        let site = write(root, "src/routes/panel.tsx", SITE);

        let (results, corrections) = correct(vec![(
            site.clone(),
            vec![row(
                SITE,
                "requireEntitlement(\"panel\")",
                "POST",
                "/v1/shelves",
            )],
        )]);

        assert_eq!(
            method(&results, &site).as_deref(),
            Some("POST"),
            "no request this pass reads states a verb, so the row keeps what it had"
        );
        assert_eq!(corrections, WrapperMethodCorrections::default());
    }

    #[test]
    fn a_deterministic_row_keeps_the_method_its_pass_read() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        write(root, "src/lib/fetcher.ts", FETCHER);
        write(root, "src/lib/guard.ts", GUARD);
        let site = write(root, "src/routes/panel.tsx", SITE);

        let (results, corrections) = correct(vec![(
            site.clone(),
            vec![DataCallResult {
                // The imported-member pass read this row out of the declaring
                // module: its method came from the same source this pass reads.
                resolution_source: Some(ResolutionSource::ImportedMember),
                ..row(SITE, "requireEntitlement(\"panel\")", "POST", "/v1/shelves")
            }],
        )]);

        assert_eq!(
            method(&results, &site).as_deref(),
            Some("POST"),
            "a deterministic row's method is not this pass's to change"
        );
        assert_eq!(corrections, WrapperMethodCorrections::default());
    }

    /// A row with no span is placed by its line, and a line carrying more than
    /// one call does not say which of them the row is. The imported binding is
    /// FIRST on the line here, so a pass that took the first call would
    /// correct this row.
    #[test]
    fn a_line_carrying_more_than_one_call_states_nothing() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        write(root, "src/lib/fetcher.ts", FETCHER);
        write(root, "src/lib/guard.ts", GUARD);
        let source = "import { requireEntitlement } from \"../lib/guard\";\n\nexport function PanelRoute() {\n  return requireEntitlement(\"panel\") && save(\"panel\");\n}\n";
        let site = write(root, "src/routes/panel.tsx", source);

        let (results, corrections) = correct(vec![(
            site.clone(),
            vec![row(
                source,
                "requireEntitlement(\"panel\")",
                "POST",
                "/v1/shelves",
            )],
        )]);

        assert_eq!(
            method(&results, &site).as_deref(),
            Some("POST"),
            "which call on the line this row is, is not something the line says"
        );
        assert_eq!(corrections, WrapperMethodCorrections::default());
    }

    /// carrick#1603's client: it passes its verb to a stream helper one
    /// property deep, in the request options the helper forwards.
    const STREAM_SITE: &str = r#"import { openStream } from "./stream";

export class BuildClient {
  constructor(private apiUrl: string) {}

  complete(id: string, body: { note: string }) {
    return openStream({
      url: `${this.apiUrl}/api/v1/builds/${id}/complete`,
      request: { method: "PATCH", body: JSON.stringify(body) },
    });
  }
}
"#;

    /// The stream helper, with `init` as the second argument of its `fetch`.
    fn stream_helper(init: &str) -> String {
        format!(
            "export type StreamOptions = {{ url: string; request?: RequestInit; key?: string }};\n\n\
             export function openStream(options: StreamOptions): Promise<Response> {{\n  \
             return fetch(options.url, {init});\n}}\n"
        )
    }

    /// What the site's verb is once the pass has read the helper written with
    /// `init`, when the model answered the site with PATCH.
    fn site_verb_through(helper: &str) -> (Option<String>, WrapperMethodCorrections) {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        write(root, "src/stream.ts", helper);
        let site = write(root, "src/build-client.ts", STREAM_SITE);
        let (results, corrections) = correct(vec![(
            site.clone(),
            vec![row(
                STREAM_SITE,
                "openStream({",
                "PATCH",
                "${apiUrl}/api/v1/builds/${id}/complete",
            )],
        )]);
        (method(&results, &site), corrections)
    }

    /// The row keeps the model's verb, and the scan counts one site whose
    /// declaration states no verb it could read.
    fn kept(verb: (Option<String>, WrapperMethodCorrections), shape: &str) {
        assert_eq!(
            verb,
            (
                Some("PATCH".to_string()),
                WrapperMethodCorrections {
                    corrected: 0,
                    declaration_unreadable: 1
                }
            ),
            "{shape}: the helper's request states no verb the source proves, so the row keeps \
             the model's and is counted"
        );
    }

    /// The row takes the helper's verb.
    fn corrected_to_get(verb: (Option<String>, WrapperMethodCorrections), shape: &str) {
        assert_eq!(
            verb,
            (
                Some("GET".to_string()),
                WrapperMethodCorrections {
                    corrected: 1,
                    declaration_unreadable: 0
                }
            ),
            "{shape}: the helper's request states GET whatever its caller passes"
        );
    }

    /// carrick#1603: the helper spreads the caller's options and adds a header.
    /// A spread of a `RequestInit` can carry `method`, so the helper states no
    /// verb, and the one literal key beside the spread does not make it a GET.
    #[test]
    fn a_spread_of_the_caller_s_options_states_no_verb() {
        kept(
            site_verb_through(&stream_helper(
                "{ ...options.request, headers: { Accept: \"text/event-stream\" } }",
            )),
            "spread, then headers",
        );
    }

    /// A spread written AFTER the helper's own `method` overwrites it.
    #[test]
    fn a_spread_after_the_method_overwrites_it() {
        kept(
            site_verb_through(&stream_helper("{ method: \"GET\", ...options.request }")),
            "method, then spread",
        );
    }

    /// A `method` written after every spread is the one the request sends,
    /// whatever the spread held.
    #[test]
    fn a_method_after_every_spread_is_the_verb_sent() {
        corrected_to_get(
            site_verb_through(&stream_helper("{ ...options.request, method: \"GET\" }")),
            "spread, then method",
        );
    }

    /// The helper forwards nothing of its caller's: its bag names no method
    /// and holds no spread, so every request it sends is a GET.
    #[test]
    fn a_helper_that_forwards_nothing_still_states_get() {
        corrected_to_get(
            site_verb_through(&stream_helper(
                "{ headers: { Accept: \"text/event-stream\" } }",
            )),
            "headers only",
        );
    }

    /// The method comes in through a parameter of the helper: written as a
    /// shorthand, and as a spread of the parameter.
    #[test]
    fn a_method_passed_through_a_parameter_states_no_verb() {
        for (helper, shape) in [
            (
                "export function openStream(options: { url: string }, method: string) {\n  \
                 return fetch(options.url, { method, headers: {} });\n}\n",
                "shorthand method parameter",
            ),
            (
                "export function openStream(options: { url: string }, init?: RequestInit) {\n  \
                 return fetch(options.url, { ...init, headers: {} });\n}\n",
                "spread of an init parameter",
            ),
        ] {
            kept(site_verb_through(helper), shape);
        }
    }

    /// The method is read one property deep in the caller's options.
    #[test]
    fn a_method_read_one_property_deep_states_no_verb() {
        kept(
            site_verb_through(&stream_helper(
                "{ method: options.request?.method ?? \"GET\", headers: {} }",
            )),
            "method read off options.request",
        );
    }

    /// The method is set under a condition: a conditional expression, and a
    /// conditional spread.
    #[test]
    fn a_method_set_conditionally_states_no_verb() {
        for (init, shape) in [
            (
                "{ method: options.request ? \"POST\" : \"GET\", headers: {} }",
                "conditional method",
            ),
            (
                "{ headers: {}, ...(options.request?.body ? { method: \"POST\" } : {}) }",
                "conditional spread",
            ),
        ] {
            kept(site_verb_through(&stream_helper(init)), shape);
        }
    }

    /// A computed key may be `method`, the same as a spread.
    #[test]
    fn a_computed_key_after_the_method_states_no_verb() {
        kept(
            site_verb_through(&stream_helper(
                "{ headers: {}, [options.key ?? \"cache\"]: \"POST\" }",
            )),
            "computed key",
        );
    }

    /// A call spelled with its verb states that verb, and a spread in its
    /// options does not take it away: the request reader that replaces this
    /// pass reads it the same way (`request_summary`).
    #[test]
    fn a_verb_spelled_request_keeps_its_verb_beside_a_spread() {
        let helper = "import { api } from \"./api\";\n\n\
                      export function openStream(options: { url: string; request?: object }) {\n  \
                      return api.get(options.url, { ...options.request, headers: {} });\n}\n";
        corrected_to_get(site_verb_through(helper), "verb-spelled call with a spread");
    }

    /// carrick#1624's site: a call through a member of an imported object,
    /// stating no verb of its own.
    const ORDER_SITE: &str = "import { orders } from \"../lib/orders\";\n\n\
                              export function checkout() {\n  \
                              return orders.place({ sku: \"a1\" });\n}\n";

    /// What the site's verb is once the pass has read `declaration` as the
    /// module `orders` comes from, when the model answered the site `model`.
    fn order_verb_through(
        declaration: &str,
        model: &str,
    ) -> (Option<String>, WrapperMethodCorrections) {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        write(root, "src/lib/orders.ts", declaration);
        let site = write(root, "src/routes/checkout.ts", ORDER_SITE);
        let (results, corrections) = correct(vec![(
            site.clone(),
            vec![row(ORDER_SITE, "orders.place(", model, "/v1/orders")],
        )]);
        (method(&results, &site), corrections)
    }

    /// The row keeps the model's POST, counted as a site whose declaration
    /// states no verb.
    fn kept_post(verb: (Option<String>, WrapperMethodCorrections), shape: &str) {
        assert_eq!(
            verb,
            (
                Some("POST".to_string()),
                WrapperMethodCorrections {
                    corrected: 0,
                    declaration_unreadable: 1
                }
            ),
            "{shape}: nothing in the module proves the verb its requests send, so the row \
             keeps the model's and is counted"
        );
    }

    /// carrick#1624: the module's real request is a client member the reader
    /// cannot see, and the one call it reads as a request is a decode helper
    /// handed `{ data }`. A bag carrying a payload and no method states no GET.
    #[test]
    fn a_decode_helper_taking_data_states_no_verb() {
        kept_post(
            order_verb_through(
                "import { decodeEnvelope } from \"./envelope\";\n\n\
                 export const orders = {\n  \
                 place(body: { sku: string }) {\n    \
                 const order = client.createOrder(body);\n    \
                 return decodeEnvelope({ data: order, format: \"json\" }, client);\n  },\n};\n",
                "POST",
            ),
            "decode helper with data",
        );
    }

    #[test]
    fn a_logger_call_with_a_body_key_states_no_verb() {
        kept_post(
            order_verb_through(
                "import { logger } from \"./logger\";\n\n\
                 export const orders = {\n  \
                 async place(body: { sku: string }) {\n    \
                 logger.info({ body, event: \"order.place\" });\n    \
                 return client.createOrder(body);\n  },\n};\n",
                "POST",
            ),
            "logger call with body",
        );
    }

    #[test]
    fn a_form_helper_taking_data_states_no_verb() {
        kept_post(
            order_verb_through(
                "import { useForm } from \"./form\";\n\n\
                 export const orders = {\n  \
                 place(body: { sku: string }) {\n    \
                 const form = useForm({ data: body, validateOn: \"submit\" });\n    \
                 return client.createOrder(form.values);\n  },\n};\n",
                "POST",
            ),
            "form helper with data",
        );
    }

    /// Failing closed: a module that issues a GET and also hands `{ data }` to
    /// a call that may be a request with a verb of its own does not prove
    /// that every request it sends is a GET.
    #[test]
    fn a_get_request_beside_a_payload_call_states_no_verb() {
        kept_post(
            order_verb_through(
                "import { decodeEnvelope } from \"./envelope\";\n\n\
                 export const orders = {\n  \
                 async place(body: { sku: string }) {\n    \
                 const response = await fetch(`/v1/orders?sku=${body.sku}`, { headers: {} });\n    \
                 return decodeEnvelope({ data: await response.json() }, client);\n  },\n};\n",
                "POST",
            ),
            "GET beside a payload call",
        );
    }

    /// A module whose only request is a GET still states it.
    #[test]
    fn a_genuine_get_wrapper_still_states_get() {
        assert_eq!(
            order_verb_through(
                "export const orders = {\n  \
                 async place(body: { sku: string }) {\n    \
                 const response = await fetch(`/v1/orders?sku=${body.sku}`, {\n      \
                 headers: { accept: \"application/json\" },\n    });\n    \
                 return response.json();\n  },\n};\n",
                "POST",
            ),
            (
                Some("GET".to_string()),
                WrapperMethodCorrections {
                    corrected: 1,
                    declaration_unreadable: 0
                }
            )
        );
    }

    /// A payload with a method written beside it is a request stating its
    /// verb: the payload rule does not reach it.
    #[test]
    fn a_payload_with_an_explicit_method_states_that_method() {
        assert_eq!(
            order_verb_through(
                "export const orders = {\n  \
                 place(body: { sku: string }) {\n    \
                 return client.request({ method: \"POST\", url: \"/v1/orders\", data: body });\n  },\n};\n",
                "GET",
            ),
            (
                Some("POST".to_string()),
                WrapperMethodCorrections {
                    corrected: 1,
                    declaration_unreadable: 0
                }
            )
        );
    }

    /// A call spelled with its verb states it, payload or not.
    #[test]
    fn a_verb_spelled_payload_states_its_verb() {
        assert_eq!(
            order_verb_through(
                "export const orders = {\n  \
                 place(body: { sku: string }) {\n    \
                 return client.post(\"/v1/orders\", { data: body });\n  },\n};\n",
                "GET",
            ),
            (
                Some("POST".to_string()),
                WrapperMethodCorrections {
                    corrected: 1,
                    declaration_unreadable: 0
                }
            )
        );
    }

    /// carrick#2384's site: a call through a function of an imported module,
    /// stating no verb.
    const SEND_SITE: &str = "import { send } from \"../lib/items\";\n\n\
                             export function load() {\n  \
                             return send(\"a1\");\n}\n";

    /// What the site's verb is once the pass has read `declaration` as the
    /// module `send` comes from, when the model answered the site `GET`.
    fn send_verb_through(declaration: &str) -> (Option<String>, WrapperMethodCorrections) {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        write(root, "src/lib/items.ts", declaration);
        let site = write(root, "src/routes/load.ts", SEND_SITE);
        let (results, corrections) = correct(vec![(
            site.clone(),
            vec![row(SEND_SITE, "send(\"a1\")", "GET", "/v1/items/a1")],
        )]);
        (method(&results, &site), corrections)
    }

    /// carrick#2384: `send` goes through a client the reader cannot see, and a
    /// sibling function of the same module clears a store. The sibling's
    /// `.delete` is not a request `send` makes, so the site keeps its GET.
    #[test]
    fn a_sibling_function_s_store_delete_is_not_the_site_s_request() {
        assert_eq!(
            send_verb_through(
                "export function send(name: string) {\n  \
                 return client.dispatch(`/v1/items/${name}`);\n}\n\n\
                 export function clear() {\n  cache.delete(\"k\");\n}\n",
            ),
            (Some("GET".to_string()), WrapperMethodCorrections::default())
        );
    }

    /// The same module read for the function the call reaches: its own verb
    /// call is the request, wherever the module's other functions sit.
    #[test]
    fn the_reached_function_s_own_verb_call_still_states_its_verb() {
        assert_eq!(
            send_verb_through(
                "export function clear() {\n  cache.get(\"/v1/other\");\n}\n\n\
                 export function send(name: string) {\n  \
                 return client.delete(`/v1/items/${name}`);\n}\n",
            ),
            (
                Some("DELETE".to_string()),
                WrapperMethodCorrections {
                    corrected: 1,
                    declaration_unreadable: 0
                }
            )
        );
    }

    /// A function the reached one calls in its own module is part of what the
    /// call reaches.
    #[test]
    fn a_same_module_function_the_reached_one_calls_is_read() {
        assert_eq!(
            send_verb_through(
                "function post(path: string) {\n  \
                 return client.delete(path);\n}\n\n\
                 export function send(name: string) {\n  return post(`/v1/items/${name}`);\n}\n",
            ),
            (
                Some("DELETE".to_string()),
                WrapperMethodCorrections {
                    corrected: 1,
                    declaration_unreadable: 0
                }
            )
        );
    }

    /// A wrapper forwards its caller's route in an argument the source does not
    /// spell out; that is still a request with its verb.
    #[test]
    fn a_verb_call_forwarding_a_route_binding_states_its_verb() {
        assert_eq!(
            send_verb_through(
                "export function send(path: string) {\n  return client.delete(path);\n}\n",
            ),
            (
                Some("DELETE".to_string()),
                WrapperMethodCorrections {
                    corrected: 1,
                    declaration_unreadable: 0
                }
            )
        );
    }

    /// The same through a member of an exported object: a sibling member's
    /// `{ data }` helper or store call is not the reached member's request.
    #[test]
    fn a_sibling_member_s_calls_are_not_the_reached_member_s() {
        assert_eq!(
            order_verb_through(
                "import { decodeEnvelope } from \"./envelope\";\n\n\
                 export const orders = {\n  \
                 place(body: { sku: string }) {\n    \
                 return client.createOrder(body);\n  },\n  \
                 async decode(output: unknown) {\n    \
                 return decodeEnvelope({ data: output, format: \"json\" }, client);\n  },\n  \
                 clear() {\n    cache.delete(\"k\");\n  },\n};\n",
                "POST",
            ),
            (
                Some("POST".to_string()),
                WrapperMethodCorrections::default()
            )
        );
    }

    /// A verb-named member call with no route-shaped argument is not read as
    /// a request: inside the reached function it states no verb, and the scan
    /// counts the site.
    #[test]
    fn a_verb_call_with_no_route_argument_states_no_verb() {
        assert_eq!(
            send_verb_through(
                "export function send(name: string) {\n  \
                 cache.delete(\"k\");\n  return client.dispatch(name);\n}\n",
            ),
            (
                Some("GET".to_string()),
                WrapperMethodCorrections {
                    corrected: 0,
                    declaration_unreadable: 1
                }
            )
        );
    }

    #[test]
    fn a_call_on_a_binding_this_file_declares_is_left_alone() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        write(root, "src/lib/fetcher.ts", FETCHER);
        write(root, "src/lib/guard.ts", GUARD);
        // Imports the guard, and the row is at a call on a binding of its own.
        let source = "import { requireEntitlement } from \"../lib/guard\";\n\nconst save = makeSaver();\n\nexport function PanelRoute() {\n  requireEntitlement(\"panel\");\n  return save(\"panel\");\n}\n";
        let site = write(root, "src/routes/panel.tsx", source);

        let (results, corrections) = correct(vec![(
            site.clone(),
            vec![row(source, "save(\"panel\")", "POST", "/v1/shelves")],
        )]);

        assert_eq!(
            method(&results, &site).as_deref(),
            Some("POST"),
            "the call is written on a binding this file declares: the import \
             beside it states nothing about it"
        );
        assert_eq!(corrections, WrapperMethodCorrections::default());
    }
}
