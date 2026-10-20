//! The environment variables and URL origins one service's source reads, for
//! `carrick derive --candidates` (carrick#2232).
//!
//! A config written before a repository's first index needs something to
//! label: which env vars and which hosts the service's code actually names.
//! These are the candidates, read with the readers the scan already has
//! ([`crate::env_alias::env_read_name`], the env-schema extractor) over the
//! files a scan of the service reads ([`crate::file_finder::find_service_files`]).
//! Nothing here is classified; the caller labels each candidate.
//!
//! The output is a stored key's input, so it is deterministic: repo-relative
//! `/` paths, every collection ordered, and nothing that depends on the order
//! the filesystem lists files in.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Component, Path};

use serde::Serialize;
use swc_common::{BytePos, SourceMap, errors::Handler, sync::Lrc};
use swc_ecma_ast::{BinaryOp, Expr, JSXAttrValue, Lit, Tpl};
use swc_ecma_visit::{Visit, VisitWith};

use crate::config::Config;
use crate::env_alias::{env_read_name, fallback_literal, is_loopback_origin};

/// Sites listed per candidate; `site_count` counts all of them.
pub const MAX_SITES: usize = 3;
/// Entries listed per list per service; `truncated` counts the rest.
pub const MAX_ENTRIES: usize = 300;
/// Characters of a site's source line kept.
pub const MAX_TEXT_CHARS: usize = 200;

/// The schemes whose literals name an origin a request is sent to.
const URL_SCHEMES: [&str; 4] = ["http", "https", "ws", "wss"];

#[derive(Debug, Serialize, PartialEq, Eq)]
pub struct Candidates {
    /// How many files the walk listed for the service.
    pub files_read: usize,
    pub env: Vec<EnvCandidate>,
    pub origins: Vec<OriginCandidate>,
    pub truncated: Truncated,
    /// What the service's declared roots reach (carrick#2397).
    pub reach: crate::derive_reach::Reach,
    /// The units reach cannot place that nothing outside them loads, each
    /// with the pattern that would leave it out; at most
    /// [`crate::derive_reach::MAX_UNITS`].
    pub directories: Vec<crate::derive_reach::Unit>,
}

#[derive(Debug, Serialize, PartialEq, Eq)]
pub struct EnvCandidate {
    pub name: String,
    /// The one literal default the source states for the variable (a `??` or
    /// `||` fallback at a read, or a schema `.default("…")`), verbatim. `None`
    /// when it states none, or more than one different literal: two defaults
    /// are two statements and neither is picked.
    pub default: Option<String>,
    /// An env schema in the service's files declares the variable.
    pub schema_declared: bool,
    pub site_count: usize,
    pub sites: Vec<Site>,
}

#[derive(Debug, Serialize, PartialEq, Eq)]
pub struct OriginCandidate {
    /// `scheme://host[:port]`, scheme and host lowercased.
    pub origin: String,
    pub site_count: usize,
    pub sites: Vec<Site>,
}

#[derive(Debug, Serialize, PartialEq, Eq, Clone)]
pub struct Site {
    /// Repo-relative, `/`-separated.
    pub file: String,
    /// 1-based.
    pub line: usize,
    /// The source line, trimmed and cut to [`MAX_TEXT_CHARS`].
    pub text: String,
}

#[derive(Debug, Serialize, PartialEq, Eq, Default)]
pub struct Truncated {
    pub env: usize,
    pub origins: usize,
    /// Offered units past a cap: not listed, so read.
    pub directories: usize,
}

/// Every site of one candidate, keyed so the order is file, then line.
type Sites = BTreeMap<(String, usize), String>;

#[derive(Default)]
struct EnvFacts {
    sites: Sites,
    defaults: BTreeSet<String>,
    schema_declared: bool,
}

/// The candidates of `service` in the repository at `repo`.
pub fn service_candidates(repo: &Path, service: &Config) -> Candidates {
    let repo = repo.canonicalize().unwrap_or_else(|_| repo.to_path_buf());
    let (mut files, _) = crate::file_finder::find_service_files(
        &repo.to_string_lossy(),
        service,
        &crate::packages::MANIFEST_SKIP_DIRS,
    );
    files.sort();
    files.dedup();

    let mut env: BTreeMap<String, EnvFacts> = BTreeMap::new();
    let mut origins: BTreeMap<String, Sites> = BTreeMap::new();
    // Parse diagnostics are the scan's to report; a derive prints a document.
    let handler = Handler::with_emitter_writer(Box::new(std::io::sink()), None);
    for file in &files {
        // The walk joins every file onto the root it was given, so this holds;
        // a file it does not is left out rather than named absolutely.
        let Some(relative) = repo_relative(&repo, file) else {
            continue;
        };
        let source_map: Lrc<SourceMap> = Default::default();
        let Some(module) = crate::parser::parse_file(file, &source_map, &handler) else {
            continue;
        };
        let site = |pos: BytePos| -> ((String, usize), String) {
            let loc = source_map.lookup_char_pos(pos);
            let text = loc
                .file
                .get_line(loc.line.saturating_sub(1))
                .map(|line| line.trim().chars().take(MAX_TEXT_CHARS).collect())
                .unwrap_or_default();
            ((relative.clone(), loc.line), text)
        };

        let mut reads = FileReads::default();
        module.visit_with(&mut reads);
        for (name, pos) in reads.env_reads {
            let (key, text) = site(pos);
            env.entry(name).or_default().sites.insert(key, text);
        }
        for (name, default) in reads.env_defaults {
            env.entry(name).or_default().defaults.insert(default);
        }
        for (origin, pos) in reads.origins {
            let (key, text) = site(pos);
            origins.entry(origin).or_default().insert(key, text);
        }

        let (declarations, schema_sites) = crate::env_alias::module_env_schema_sites(&module);
        for (name, pos) in schema_sites {
            let facts = env.entry(name.clone()).or_default();
            facts.schema_declared = true;
            let (key, text) = site(pos);
            facts.sites.insert(key, text);
            if let Some(default) = declarations
                .get(&name)
                .and_then(|declaration| declaration.default_literal.clone())
            {
                facts.defaults.insert(default);
            }
        }
    }

    let env: Vec<EnvCandidate> = env
        .into_iter()
        .map(|(name, facts)| EnvCandidate {
            default: (facts.defaults.len() == 1)
                .then(|| facts.defaults.into_iter().next())
                .flatten(),
            schema_declared: facts.schema_declared,
            site_count: facts.sites.len(),
            sites: listed_sites(facts.sites),
            name,
        })
        .collect();
    let origins: Vec<OriginCandidate> = origins
        .into_iter()
        .map(|(origin, sites)| OriginCandidate {
            origin,
            site_count: sites.len(),
            sites: listed_sites(sites),
        })
        .collect();
    let (env, env_cut) = capped(env, |entry| (entry.site_count, entry.name.as_str()));
    let (origins, origins_cut) = capped(origins, |entry| (entry.site_count, entry.origin.as_str()));
    let crate::derive_reach::ServiceReach { reach, mut units } =
        crate::derive_reach::service_reach(&repo, service, &files);
    let directories_cut = units.len().saturating_sub(crate::derive_reach::MAX_UNITS);
    units.truncate(crate::derive_reach::MAX_UNITS);
    Candidates {
        files_read: files.len(),
        env,
        origins,
        truncated: Truncated {
            env: env_cut,
            origins: origins_cut,
            directories: directories_cut,
        },
        reach,
        directories: units,
    }
}

fn listed_sites(sites: Sites) -> Vec<Site> {
    sites
        .into_iter()
        .take(MAX_SITES)
        .map(|((file, line), text)| Site { file, line, text })
        .collect()
}

/// The entries ordered by site count, most first, then by name; at most
/// [`MAX_ENTRIES`] of them, and how many were cut.
fn capped<T>(mut entries: Vec<T>, key: impl Fn(&T) -> (usize, &str)) -> (Vec<T>, usize) {
    entries.sort_by(|a, b| {
        let (a_count, a_name) = key(a);
        let (b_count, b_name) = key(b);
        b_count.cmp(&a_count).then_with(|| a_name.cmp(b_name))
    });
    let cut = entries.len().saturating_sub(MAX_ENTRIES);
    entries.truncate(MAX_ENTRIES);
    (entries, cut)
}

/// `file` relative to `repo`, `/`-separated, or `None` when it is not under it.
fn repo_relative(repo: &Path, file: &Path) -> Option<String> {
    let relative = file.strip_prefix(repo).ok()?;
    let mut parts = Vec::new();
    for component in relative.components() {
        match component {
            Component::Normal(part) => parts.push(part.to_string_lossy().into_owned()),
            Component::ParentDir => parts.push("..".to_string()),
            Component::CurDir => {}
            Component::RootDir | Component::Prefix(_) => return None,
        }
    }
    Some(parts.join("/"))
}

/// What one module reads: env reads at their position, the literal defaults
/// stated beside them, and the origins its URL literals name.
#[derive(Default)]
struct FileReads {
    env_reads: Vec<(String, BytePos)>,
    env_defaults: Vec<(String, String)>,
    origins: Vec<(String, BytePos)>,
}

impl Visit for FileReads {
    fn visit_expr(&mut self, node: &Expr) {
        match node {
            // The read itself, recorded once where it is written: the `??`
            // around it and any wrapper above it are the same read.
            Expr::Member(_) | Expr::Call(_) => {
                if let Some(name) = env_read_name(node) {
                    self.env_reads.push((name, span_lo(node)));
                }
            }
            Expr::Bin(bin)
                if matches!(bin.op, BinaryOp::NullishCoalescing | BinaryOp::LogicalOr) =>
            {
                if let (Some(name), Some(default)) =
                    (env_read_name(&bin.left), fallback_literal(node))
                {
                    self.env_defaults.push((name, default));
                }
            }
            Expr::Lit(Lit::Str(literal)) => {
                if let Some(origin) = literal_origin(&literal.value.to_string_lossy(), true) {
                    self.origins.push((origin, literal.span.lo));
                }
            }
            _ => {}
        }
        node.visit_children_with(self);
    }

    fn visit_jsx_attr_value(&mut self, node: &JSXAttrValue) {
        if let JSXAttrValue::Str(literal) = node
            && let Some(origin) = literal_origin(&literal.value.to_string_lossy(), true)
        {
            self.origins.push((origin, literal.span.lo));
        }
        node.visit_children_with(self);
    }

    fn visit_tpl(&mut self, tpl: &Tpl) {
        if let Some(head) = tpl.quasis.first()
            && let Some(origin) = literal_origin(&head.raw, tpl.exprs.is_empty())
        {
            self.origins.push((origin, tpl.span.lo));
        }
        tpl.visit_children_with(self);
    }
}

fn span_lo(expr: &Expr) -> BytePos {
    use swc_common::Spanned;
    expr.span().lo
}

/// The origin a URL literal names: `scheme://host[:port]` with the scheme and
/// host lowercased and any userinfo dropped.
///
/// `whole` says the text is the literal's whole value. A template head is not:
/// an expression follows it, so the host is only known where the head itself
/// ends it with a `/`, `?` or `#`. `https://${host}/v1` and
/// `https://api.example:${port}/` name no origin. A loopback host names this
/// machine, never a service, and is left out too.
fn literal_origin(text: &str, whole: bool) -> Option<String> {
    let (scheme, rest) = URL_SCHEMES.iter().find_map(|scheme| {
        let prefix_len = scheme.len() + 3;
        let prefix = text.get(..prefix_len)?;
        prefix
            .eq_ignore_ascii_case(&format!("{scheme}://"))
            .then(|| (*scheme, &text[prefix_len..]))
    })?;
    let authority = match rest.find(['/', '?', '#']) {
        Some(end) => &rest[..end],
        None if whole => rest,
        None => return None,
    };
    let host_port = authority.rsplit('@').next().unwrap_or(authority);
    let valid = !host_port.is_empty()
        && !host_port.starts_with(':')
        && host_port
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_' | ':' | '[' | ']'));
    if !valid {
        return None;
    }
    // The loopback rule reads `http(s)://` literals; the host is the same
    // fact under a socket scheme.
    if is_loopback_origin(&format!("http://{host_port}")) {
        return None;
    }
    Some(format!("{scheme}://{}", host_port.to_ascii_lowercase()))
}

#[cfg(test)]
mod tests {
    use super::literal_origin;

    #[test]
    fn a_literal_names_its_origin_and_an_interpolated_host_names_none() {
        let whole = |text| literal_origin(text, true);
        let head = |text| literal_origin(text, false);
        assert_eq!(
            whole("https://API.Vendor.example/v1/x?y=1").as_deref(),
            Some("https://api.vendor.example")
        );
        assert_eq!(
            whole("HTTP://user:pw@host.example:8080").as_deref(),
            Some("http://host.example:8080")
        );
        assert_eq!(
            whole("wss://events.example/socket").as_deref(),
            Some("wss://events.example")
        );
        assert_eq!(
            head("https://api.example/").as_deref(),
            Some("https://api.example")
        );
        assert_eq!(head("https://"), None);
        assert_eq!(head("https://api."), None);
        assert_eq!(head("https://api.example:"), None);
        assert_eq!(whole("http://localhost:4000/x"), None);
        assert_eq!(whole("ws://127.0.0.1:3000"), None);
        assert_eq!(whole("https://${host}/v1"), None);
        assert_eq!(whole("see https://docs.example"), None);
        assert_eq!(whole("ftp://files.example"), None);
        assert_eq!(whole("https://"), None);
    }
}
