//! The definition each call reaches, as the compiler's per-position rows say
//! (carrick#2413 slice D).
//!
//! Without rows, a call reaches what the call graph's resolution names
//! ([`CallSiteTargets`]): a member call is joined to a definition by its
//! name, and a receiver's type is read as written. With rows, each call has
//! a `call` row naming the declaration its callee resolves to, as the
//! compiler resolves it: the receiver's declared or inferred type, an
//! override, an overload, a re-export chain. That declaration joins the
//! definition the extractor keyed where it opens ([`FileIr::definition_lines`]):
//! the same file and line, and a key whose last part is the row's symbol.
//!
//! Two kinds of call stay on the call graph: a JSX element, which the rows
//! give no kind (carrick#2430), and a bare call of a function the same file
//! declares, whose binding SWC already names exactly.
//!
//! A call whose row says nothing is [`Reach::Unknown`], never a fallback to
//! the name join: the summary it sits in is not complete.

use super::*;
use crate::position_rows::{FLAG_ANY, FLAG_ERROR_TYPE, Kind};
use crate::visitor::FunctionDefinition;

/// The declarations of a file no definition the extractor keys opens at,
/// read once per file.
#[derive(Debug, Default)]
pub(crate) struct DeclarationLines {
    /// Each function or method declared with no body, by its line, with its
    /// name: an overload's signature, which the declaration with a body
    /// after it implements. The compiler names an overloaded function by its
    /// first signature; the extractor keys it where its body is.
    bodyless: HashMap<u32, String>,
    /// Each method or property an interface or type literal declares, by
    /// its line, with its name: no body implements it here (carrick#2431's
    /// `interface_member` stop).
    pub(crate) signatures: HashMap<u32, String>,
}

impl DeclarationLines {
    pub(crate) fn of(module: &Module, source_map: &Lrc<SourceMap>) -> Self {
        struct Lines<'a> {
            source_map: &'a Lrc<SourceMap>,
            out: DeclarationLines,
        }
        impl Lines<'_> {
            fn line(&self, span: Span) -> u32 {
                self.source_map.lookup_char_pos(span.lo).line as u32
            }
        }
        impl Visit for Lines<'_> {
            fn visit_fn_decl(&mut self, declaration: &FnDecl) {
                if declaration.function.body.is_none() {
                    let line = self.line(declaration.function.span);
                    self.out
                        .bodyless
                        .insert(line, declaration.ident.sym.to_string());
                }
                declaration.visit_children_with(self);
            }
            fn visit_class_method(&mut self, method: &ClassMethod) {
                if method.function.body.is_none()
                    && !method.is_abstract
                    && let PropName::Ident(name) = &method.key
                {
                    let line = self.line(method.span);
                    self.out.bodyless.insert(line, name.sym.to_string());
                }
                method.visit_children_with(self);
            }
            fn visit_ts_method_signature(&mut self, signature: &TsMethodSignature) {
                if let Expr::Ident(name) = &*signature.key {
                    let line = self.line(signature.span);
                    self.out.signatures.insert(line, name.sym.to_string());
                }
            }
            fn visit_ts_property_signature(&mut self, signature: &TsPropertySignature) {
                if let Expr::Ident(name) = &*signature.key {
                    let line = self.line(signature.span);
                    self.out.signatures.insert(line, name.sym.to_string());
                }
            }
        }
        let mut lines = Lines {
            source_map,
            out: DeclarationLines::default(),
        };
        module.visit_with(&mut lines);
        lines.out
    }
}

/// Each line a definition opens on, as the extractor places it, with the
/// definitions there. The compiler starts a declaration on the same line,
/// its decorators included.
pub(crate) fn definition_lines<'d>(
    definitions: impl Iterator<Item = (&'d str, &'d FunctionDefinition)>,
) -> HashMap<u32, Vec<String>> {
    let mut at: HashMap<u32, Vec<String>> = HashMap::new();
    for (key, definition) in definitions {
        at.entry(definition.line_number)
            .or_default()
            .push(key.to_string());
    }
    for keys in at.values_mut() {
        keys.sort();
        keys.dedup();
    }
    at
}

/// Whether the definition key `key` names the symbol `symbol`: the key
/// itself, or its last part (`Users.list`, `Users.static.list`).
fn names(key: &str, symbol: &str) -> bool {
    key == symbol || key.rsplit('.').next() == Some(symbol)
}

/// What a call reaches.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Reach {
    /// A definition of a file of the service: the file as walked and its
    /// key.
    Function(PathBuf, String),
    /// A declaration the repository makes outside the service's files: a
    /// function of this repository, which this service does not read.
    OutsideService(PathBuf, String),
    /// No function of the repository: the default library, a package, or a
    /// declaration that is no definition (a class, a parameter, an
    /// interface's member).
    NoFunction,
    /// The rows do not say.
    Unknown(HoleReason),
}

impl Reach {
    /// The definition, as [`CallSiteTargets::target_at`] names one: any
    /// function of the repository.
    pub(crate) fn target(&self) -> Option<(PathBuf, String)> {
        match self {
            Reach::Function(file, key) | Reach::OutsideService(file, key) => {
                Some((file.clone(), key.clone()))
            }
            Reach::NoFunction | Reach::Unknown(_) => None,
        }
    }
}

/// The calls that stay on the call graph with rows, and what the rows
/// changed, for the run log.
#[derive(Debug, Default)]
pub(crate) struct RowCalls {
    /// (file, span start, span end), discovery numbering.
    kept: HashSet<(PathBuf, u32, u32)>,
    counts: BTreeMap<String, usize>,
}

impl RowCalls {
    /// One line for the run log: how each call was answered, against what
    /// the call graph names.
    pub(crate) fn describe(&self) -> String {
        format!("call targets {:?}", self.counts)
    }
}

/// Every call a file's IR records, in every body: (file, the call, whether
/// a construction).
pub(crate) fn every_call<'a>(
    files: &'a HashMap<PathBuf, FileIr>,
) -> Vec<(&'a Path, &'a CallIr, bool)> {
    fn walk<'a>(file: &'a Path, ir: &'a FnIr, out: &mut Vec<(&'a Path, &'a CallIr, bool)>) {
        out.extend(ir.calls.iter().map(|call| (file, call, false)));
        out.extend(ir.constructions.iter().map(|call| (file, call, true)));
        for inner in ir.nested.iter().chain(&ir.detached) {
            walk(file, inner, out);
        }
    }
    let mut out = Vec::new();
    let mut paths: Vec<&PathBuf> = files.keys().collect();
    paths.sort();
    for path in paths {
        let file = &files[path];
        let mut keys: Vec<&String> = file.functions.keys().collect();
        keys.sort();
        for key in keys {
            walk(path, &file.functions[key], &mut out);
        }
        walk(path, &file.module_level, &mut out);
    }
    out
}

fn is_identifier(text: &str) -> bool {
    !text.is_empty()
        && text
            .chars()
            .all(|c| c.is_alphanumeric() || c == '_' || c == '$')
}

impl RequestSummaryInputs {
    /// The calls that stay on the call graph, and the run log's counts:
    /// read once, when the rows are bound ([`Self::linked`]).
    pub(crate) fn row_calls(&self) -> RowCalls {
        let mut out = RowCalls::default();
        for (file, call, _) in every_call(&self.files) {
            let graph = self.sites.target_at(file, call.site.lo, call.site.hi);
            let kept = call.jsx
                || (is_identifier(&call.callee_text)
                    && graph.is_some_and(|(target, _)| target == file));
            if kept {
                out.kept
                    .insert((file.to_path_buf(), call.site.lo, call.site.hi));
                *out.counts.entry("kept".to_string()).or_default() += 1;
                continue;
            }
            let reach = self.row_reach(file, call.site.lo, call.site.hi);
            let key = match (&reach, graph) {
                (Reach::Unknown(reason), Some(_)) => format!("graph_only/{}", hole_key(*reason)),
                (Reach::Unknown(reason), None) => format!("neither/{}", hole_key(*reason)),
                (reach, graph) => match (reach.target(), graph) {
                    (Some(rows), Some(graph)) if rows == *graph => "agree".to_string(),
                    (Some(_), Some(_)) => "differ".to_string(),
                    (Some(_), None) => "rows_only".to_string(),
                    (None, Some(_)) => "graph_only/no_function".to_string(),
                    (None, None) => "no_function".to_string(),
                },
            };
            *out.counts.entry(key).or_default() += 1;
        }
        out
    }

    /// What the call spanning exactly `lo..hi` (discovery numbering) in
    /// `file` reaches: from the rows when the run walks them, from the call
    /// graph otherwise.
    pub(crate) fn reach(&self, file: &Path, lo: u32, hi: u32) -> Reach {
        let graph = || match self.sites.target_at(file, lo, hi) {
            Some((target, key)) if self.files.contains_key(target) => {
                Reach::Function(target.clone(), key.clone())
            }
            Some((target, key)) => Reach::OutsideService(target.clone(), key.clone()),
            None => Reach::NoFunction,
        };
        match &self.row_calls {
            None => graph(),
            Some(row_calls) if row_calls.kept.contains(&(file.to_path_buf(), lo, hi)) => graph(),
            Some(_) => self.row_reach(file, lo, hi),
        }
    }

    /// [`Self::reach`]'s definition, as [`CallSiteTargets::target_at`]
    /// names one.
    pub(crate) fn call_target(&self, file: &Path, lo: u32, hi: u32) -> Option<(PathBuf, String)> {
        self.reach(file, lo, hi).target()
    }

    /// What the call row at `lo..hi` in `file` names.
    fn row_reach(&self, file: &Path, lo: u32, hi: u32) -> Reach {
        let Some(positions) = &self.positions else {
            return Reach::Unknown(HoleReason::RowsMissing);
        };
        let Some(ir) = self.files.get(file) else {
            return Reach::Unknown(HoleReason::RowsMissing);
        };
        let row = match positions.lookup(file, ir.swc_start(lo), ir.swc_start(hi), Kind::Call) {
            Ok(row) => row,
            Err(reason) => return Reach::Unknown(reason),
        };
        let unresolved = row.flags() & FLAG_ERROR_TYPE != 0;
        let Some(symbol) = row.symbol() else {
            return Reach::Unknown(if row.flags() & FLAG_ANY != 0 {
                HoleReason::UntypedReceiver
            } else {
                HoleReason::UnresolvedName
            });
        };
        let Some(decl_file) = row.decl_file() else {
            return Reach::NoFunction;
        };
        if decl_file.starts_with("lib:")
            || decl_file.starts_with("pkg:")
            || decl_file.split('/').any(|part| part == "node_modules")
        {
            return Reach::NoFunction;
        }
        let declaring = positions.path_of(decl_file);
        let Some(declared) = self.files.get(&declaring) else {
            return Reach::OutsideService(declaring, symbol.to_string());
        };
        let joined = join(declared, row.decl_line(), symbol);
        match joined {
            Join::One(key) => Reach::Function(declaring, key),
            Join::Several => Reach::Unknown(HoleReason::RowsMissing),
            // A call whose result did not resolve, at a declaration that is
            // no definition: an import of nothing, as the import line is.
            Join::None if unresolved => Reach::Unknown(if declaring == file {
                HoleReason::UnresolvedImport
            } else {
                HoleReason::UnresolvedName
            }),
            Join::None => Reach::NoFunction,
        }
    }
}

enum Join {
    One(String),
    Several,
    None,
}

/// The definition of `declared` the compiler's declaration of `symbol` at
/// `line` is: the one opening there whose key names the symbol, or, for an
/// overload's signature, the first definition of that name after it, which
/// is the overloads' body.
fn join(declared: &FileIr, line: Option<u32>, symbol: &str) -> Join {
    let Some(line) = line else {
        return Join::None;
    };
    let at: Vec<&String> = declared
        .definition_lines
        .get(&line)
        .into_iter()
        .flatten()
        .filter(|key| names(key, symbol))
        .collect();
    match at.as_slice() {
        [one] => return Join::One((*one).clone()),
        [_, _, ..] => return Join::Several,
        [] => {}
    }
    if declared
        .declaration_lines
        .bodyless
        .get(&line)
        .map(String::as_str)
        != Some(symbol)
    {
        return Join::None;
    }
    let implementation = declared
        .definition_lines
        .iter()
        .filter(|(opens, _)| **opens > line)
        .flat_map(|(opens, keys)| keys.iter().map(move |key| (*opens, key)))
        .filter(|(_, key)| names(key, symbol))
        .min();
    match implementation {
        Some((_, key)) => Join::One(key.clone()),
        None => Join::None,
    }
}

fn hole_key(reason: HoleReason) -> String {
    serde_json::to_value(reason)
        .ok()
        .and_then(|value| value.as_str().map(str::to_string))
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::super::fixture_walk::{Walked, walked};
    use super::*;

    fn calls(edit: &[(&str, &str)], with_rows: bool) -> Option<Walked> {
        walked(
            "position-rows-calls",
            "services/calls",
            "calls",
            edit,
            with_rows,
        )
    }

    /// The URLs each line of `callers.ts` states a summary row for.
    fn stated(walked: &Walked) -> BTreeMap<u32, Vec<String>> {
        let index = summarize(&walked.inputs, &LibrarySemantics::default());
        let path = walked.path("services/calls/src/callers.ts");
        let mut out: BTreeMap<u32, Vec<String>> = BTreeMap::new();
        for rows in index.rows(&path).into_iter().flat_map(BTreeMap::values) {
            for row in rows {
                out.entry(row.line).or_default().push(row.target.clone());
            }
        }
        for targets in out.values_mut() {
            targets.sort();
        }
        out
    }

    /// What each call written in `callers.ts` reaches, by line, as
    /// `file:key` or the outcome's name.
    fn reached(walked: &Walked) -> BTreeMap<u32, Vec<String>> {
        let path = walked.path("services/calls/src/callers.ts");
        let mut out: BTreeMap<u32, Vec<String>> = BTreeMap::new();
        for (file, call, _) in every_call(&walked.inputs.files) {
            if file != path {
                continue;
            }
            let text = match walked.inputs.reach(file, call.site.lo, call.site.hi) {
                Reach::Function(target, key) => {
                    format!("{}:{key}", target.file_name().unwrap().to_string_lossy())
                }
                Reach::OutsideService(target, key) => format!(
                    "outside {}:{key}",
                    target.file_name().unwrap().to_string_lossy()
                ),
                Reach::NoFunction => "no_function".to_string(),
                Reach::Unknown(reason) => format!("unknown {}", hole_key(reason)),
            };
            out.entry(call.site.line).or_default().push(text);
        }
        for texts in out.values_mut() {
            texts.sort();
        }
        out
    }

    fn line(n: u32, target: &str) -> (u32, Vec<String>) {
        (n, vec![target.to_string()])
    }

    /// carrick#2413 slice D: with rows, each call reaches the declaration
    /// the compiler resolves its callee to. The receiver's type picks one of
    /// two classes' `list`, an override is the subclass's, an overload's
    /// first signature joins the implementation, a decorated method joins
    /// at its decorator's line, a chain's outer call is its own, and an
    /// `export *` chain is followed. A call through an `any` receiver says
    /// nothing, so its body is not complete, and a function of a sibling
    /// package is outside the service, as without rows.
    #[test]
    fn with_rows_a_call_reaches_the_declaration_the_compiler_names() {
        let Some(on) = calls(&[], true) else {
            return;
        };
        let texts = |items: &[&str]| items.iter().map(|item| item.to_string()).collect();
        assert_eq!(
            reached(&on),
            BTreeMap::from([
                (9, texts(&["no_function"])),
                (13, texts(&["models.ts:Users.list"])),
                (17, texts(&["models.ts:Child.load"])),
                (21, texts(&["overload.ts:readPath"])),
                (25, texts(&["unknown untyped_receiver"])),
                (29, texts(&["helpers.ts:ping"])),
                (33, texts(&["reports.ts:Reports.fetchAll"])),
                (37, texts(&["outside index.ts:remote"])),
                (41, texts(&["callers.ts:users", "models.ts:Users.list"])),
            ])
        );
        assert_eq!(
            stated(&on),
            BTreeMap::from([
                line(13, "/users"),
                line(17, "/child"),
                line(21, "/a"),
                line(29, "/ping"),
                line(33, "/reports"),
                line(41, "/users"),
            ])
        );
    }

    /// The specifier each library site in `kit-user.ts` is read through.
    fn kit_sites(walked: &Walked) -> Vec<(u32, String)> {
        let path = walked.path("services/calls/src/kit-user.ts");
        let mut sites: Vec<(u32, String)> = super::super::library_sites(&walked.inputs)
            .sites
            .into_iter()
            .filter(|site| site.file == path)
            .map(|site| (site.line, site.specifier))
            .collect();
        sites.sort();
        sites
    }

    /// The end of the chain from the `/kit-health` comparison in
    /// `server.ts`.
    fn kit_health_end(walked: &Walked) -> serde_json::Value {
        let chains = super::super::guard_chains(&walked.inputs, &walked.repo);
        let chain = chains
            .iter()
            .find(|chain| chain.literal == "/kit-health")
            .expect("a chain for /kit-health");
        serde_json::to_value(&chain.end).unwrap()
    }

    /// carrick#2413 slice E: an import whose rows name a declaration of the
    /// repository is no package's client, whatever its specifier says: a
    /// tsconfig alias to in-repo code reads as a package without rows and as
    /// nothing with them. An external import's end names the package the
    /// compiler says its export is declared in.
    #[test]
    fn with_rows_a_package_is_the_one_the_declaration_is_in() {
        let Some(off) = calls(&[], false) else {
            return;
        };
        let on = calls(&[], true).unwrap();
        assert_eq!(
            kit_sites(&off),
            vec![(5, "http-kit".to_string()), (9, "fixture-kit".to_string())]
        );
        assert_eq!(kit_sites(&on), vec![(9, "fixture-kit".to_string())]);
        assert_eq!(
            kit_health_end(&off),
            serde_json::json!({
                "kind": "external_import", "specifier": "fixture-kit",
                "export": "serve", "steps": []
            })
        );
        assert_eq!(
            kit_health_end(&on),
            serde_json::json!({
                "kind": "external_import", "specifier": "fixture-kit",
                "export": "serve", "steps": [], "package": "fixture-kit"
            })
        );
    }

    /// Without rows, the call graph's answer stands, byte for byte: the
    /// lines it cannot follow state nothing.
    #[test]
    fn without_rows_the_call_graph_answers() {
        let Some(off) = calls(&[], false) else {
            return;
        };
        assert!(off.inputs.row_calls.is_none());
        assert_eq!(
            stated(&off),
            BTreeMap::from([
                line(17, "/child"),
                line(21, "/a"),
                line(29, "/ping"),
                line(33, "/reports"),
            ])
        );
    }
}
