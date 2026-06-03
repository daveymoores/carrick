//! Generic file-based routing: derive HTTP route paths from filesystem layout.
//!
//! Some frameworks (Next.js, Remix, SvelteKit, Nuxt, ...) declare API routes by
//! *file location* rather than by a path string in code. The route `/users/:id`
//! for `app/users/[id]/route.ts` appears nowhere in that file's bytes — it lives
//! in the directory structure. The LLM pipeline cannot recover information that
//! is absent from the source it reads, so this module supplies that one
//! structural fact deterministically.
//!
//! The module is framework-agnostic: it executes a [`RoutingConvention`] (plain
//! data), never a hardcoded framework branch. Built-in conventions
//! ([`builtin_conventions`]) exist only as a *bootstrap* so common stacks work
//! out of the box; a convention supplied by framework detection (carrick-cloud)
//! or by `carrick.json` overrides them. This keeps framework knowledge out of
//! the scanner core while still shipping value today.

use serde::{Deserialize, Serialize};
use std::path::Path;

/// How the HTTP method of a file-based endpoint is determined.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum MethodSource {
    /// The HTTP method is the name of an exported handler function, e.g.
    /// Next.js app-router `export async function GET(...) {}`.
    ExportName,
    /// A single default-exported handler serves every method and branches on the
    /// request at runtime (e.g. pages-router `req.method`). The concrete method
    /// is not derivable from structure and is left to the LLM / downstream.
    DefaultExport,
}

/// Whether route path segments come from the directory chain (with a fixed
/// terminal filename) or from the filename itself.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", tag = "kind")]
pub enum SegmentSource {
    /// App-router style: the endpoint is marked by a fixed terminal filename
    /// (e.g. `route.ts`); the path is built from the enclosing directory chain.
    DirectoryChain { terminal_files: Vec<String> },
    /// Pages-router style: the filename (minus extension) is the final path
    /// segment. `index` collapses to its directory.
    FileName { extensions: Vec<String> },
}

/// A declarative description of a file-based routing scheme. Executed by
/// [`derive_route`]; never branched on by framework name.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RoutingConvention {
    /// Human label (e.g. "nextjs-app"). Diagnostic only — not matched against.
    pub name: String,
    /// Directory prefixes (repo-relative, `/`-separated) under which route files
    /// live, e.g. `["app", "src/app"]`. The longest matching root wins.
    pub root_globs: Vec<String>,
    /// Where path segments come from.
    pub segment_source: SegmentSource,
    /// Prefix prepended to every derived path, e.g. `""` or `"/api"`.
    #[serde(default)]
    pub path_prefix: String,
    /// Opening delimiter for a dynamic segment, e.g. `"["`.
    pub dynamic_open: String,
    /// Closing delimiter for a dynamic segment, e.g. `"]"`.
    pub dynamic_close: String,
    /// Marker that turns a dynamic segment into a catch-all, e.g. `"..."`.
    pub catch_all_marker: String,
    /// Opening delimiter for a non-path "group" segment, e.g. `"("`.
    pub group_open: String,
    /// Closing delimiter for a non-path "group" segment, e.g. `")"`.
    pub group_close: String,
    /// How the HTTP method is determined for endpoints under this convention.
    pub method_source: MethodSource,
}

/// A route successfully derived from a file's location.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DerivedRoute {
    /// The normalized route path (leading slash, `:param` for dynamic segments,
    /// `*` for catch-alls).
    pub path: String,
    /// How to determine the HTTP method(s) for this endpoint.
    pub method_source: MethodSource,
    /// The convention name that matched (diagnostic).
    pub convention: String,
}

impl RoutingConvention {
    /// Next.js App Router: `app/**/route.{ts,js,tsx}` with method-per-export.
    pub fn nextjs_app() -> Self {
        Self {
            name: "nextjs-app".to_string(),
            root_globs: vec!["app".to_string(), "src/app".to_string()],
            segment_source: SegmentSource::DirectoryChain {
                terminal_files: vec![
                    "route.ts".to_string(),
                    "route.js".to_string(),
                    "route.tsx".to_string(),
                    "route.mts".to_string(),
                ],
            },
            path_prefix: String::new(),
            dynamic_open: "[".to_string(),
            dynamic_close: "]".to_string(),
            catch_all_marker: "...".to_string(),
            group_open: "(".to_string(),
            group_close: ")".to_string(),
            method_source: MethodSource::ExportName,
        }
    }

    /// Next.js Pages Router API: `pages/api/**` (or `src/pages/api/**`) where the
    /// filename is the last segment and a single default export serves the route.
    pub fn nextjs_pages() -> Self {
        Self {
            name: "nextjs-pages".to_string(),
            root_globs: vec!["pages/api".to_string(), "src/pages/api".to_string()],
            segment_source: SegmentSource::FileName {
                extensions: vec![
                    "ts".to_string(),
                    "js".to_string(),
                    "tsx".to_string(),
                    "jsx".to_string(),
                ],
            },
            path_prefix: "/api".to_string(),
            dynamic_open: "[".to_string(),
            dynamic_close: "]".to_string(),
            catch_all_marker: "...".to_string(),
            group_open: "(".to_string(),
            group_close: ")".to_string(),
            method_source: MethodSource::DefaultExport,
        }
    }

    /// Astro endpoints: `src/pages/**` where the filename is the last path
    /// segment and methods are named exports (`export function GET() {}`,
    /// `export const POST = ...`). Unlike Next.js pages-router, Astro has no
    /// forced `/api` prefix — the route is literally the file's path under
    /// `src/pages` — and methods come from export names, not a single default
    /// export. Only `.ts`/`.js` files are endpoints; `.astro` files are HTML
    /// pages and are deliberately excluded. (Astro's `ALL` fallback export is
    /// not an HTTP method per [`is_http_method`](crate::type_manifest), so a
    /// route defined solely via `ALL` is not synthesized.)
    pub fn astro() -> Self {
        Self {
            name: "astro".to_string(),
            root_globs: vec!["src/pages".to_string()],
            segment_source: SegmentSource::FileName {
                // Astro routes only `.ts`/`.js` endpoint files under src/pages
                // (`.astro` files are HTML pages, handled elsewhere). `.mts`/
                // `.mjs` are not Astro route extensions, and the SWC handler
                // extractor doesn't parse TS syntax in `.mts` anyway.
                extensions: vec!["ts".to_string(), "js".to_string()],
            },
            path_prefix: String::new(),
            dynamic_open: "[".to_string(),
            dynamic_close: "]".to_string(),
            catch_all_marker: "...".to_string(),
            // Astro has no route-group syntax; leave the delimiters empty so the
            // group check in `transform_segment` never fires.
            group_open: String::new(),
            group_close: String::new(),
            method_source: MethodSource::ExportName,
        }
    }

    /// Strip the longest matching root prefix from a `/`-normalized relative
    /// path. Returns the remainder, or `None` if no root matches.
    fn strip_root<'a>(&self, rel: &'a str) -> Option<&'a str> {
        self.root_globs
            .iter()
            .filter_map(|root| {
                let root = root.trim_matches('/');
                if root.is_empty() {
                    return Some(rel);
                }
                if let Some(rest) = rel.strip_prefix(root) {
                    // Require a clean segment boundary so "apple/" doesn't match
                    // root "app".
                    if rest.is_empty() {
                        Some("")
                    } else {
                        rest.strip_prefix('/')
                    }
                } else {
                    None
                }
            })
            // Longest matching root wins (e.g. "src/pages/api" over "pages/api").
            .max_by_key(|rest| rest.len().wrapping_neg())
    }

    /// Transform a single raw directory/file segment into its route form.
    /// Returns `None` for group segments (which contribute no path segment).
    fn transform_segment(&self, raw: &str) -> Option<String> {
        // Group segment, e.g. "(marketing)" → omitted.
        if !self.group_open.is_empty()
            && raw.starts_with(&self.group_open)
            && raw.ends_with(&self.group_close)
        {
            return None;
        }

        // Catch-all "[...slug]" / optional catch-all "[[...slug]]" → `**`, the
        // multi-segment wildcard the mount graph matcher recognizes as a suffix
        // catch-all (see `path_matches_with_wildcards` in src/mount_graph.rs).
        // The param name plays no part in matching, so it is dropped. Catch-alls
        // are always terminal in these conventions, so `**` lands at the end.
        let double_open = format!("{}{}", self.dynamic_open, self.dynamic_open);
        let double_close = format!("{}{}", self.dynamic_close, self.dynamic_close);
        if raw.starts_with(&double_open) && raw.ends_with(&double_close) {
            return Some("**".to_string());
        }

        // Dynamic segment "[id]" or catch-all "[...slug]".
        if raw.starts_with(&self.dynamic_open) && raw.ends_with(&self.dynamic_close) {
            let inner = &raw[self.dynamic_open.len()..raw.len() - self.dynamic_close.len()];
            if inner.starts_with(&self.catch_all_marker) {
                return Some("**".to_string());
            }
            return Some(format!(":{}", sanitize_param(inner)));
        }

        // Literal segment.
        Some(raw.to_string())
    }

    /// Build the list of raw segments for a relative path under this convention,
    /// or `None` if the file is not a route file for this convention.
    fn raw_segments(&self, rel_after_root: &str) -> Option<Vec<String>> {
        let components: Vec<&str> = rel_after_root
            .split('/')
            .filter(|c| !c.is_empty())
            .collect();
        let (file, dirs) = components.split_last()?;

        match &self.segment_source {
            SegmentSource::DirectoryChain { terminal_files } => {
                // The file must be one of the terminal markers (e.g. route.ts).
                if !terminal_files.iter().any(|t| t == file) {
                    return None;
                }
                Some(dirs.iter().map(|s| s.to_string()).collect())
            }
            SegmentSource::FileName { extensions } => {
                // Skip framework-private files like _app / _document / _middleware.
                if file.starts_with('_') {
                    return None;
                }
                let (stem, ext) = file.rsplit_once('.')?;
                if !extensions.iter().any(|e| e == ext) {
                    return None;
                }
                let mut segs: Vec<String> = dirs.iter().map(|s| s.to_string()).collect();
                // `index` collapses to its directory; otherwise the stem is the
                // final segment.
                if stem != "index" {
                    segs.push(stem.to_string());
                }
                Some(segs)
            }
        }
    }
}

/// Replace characters illegal in a route param name (e.g. catch-all dots).
fn sanitize_param(name: &str) -> String {
    name.trim().replace('.', "")
}

/// Normalize OS path separators to `/` and strip any leading `./` or `/`.
fn normalize_rel(rel: &Path) -> String {
    let s = rel.to_string_lossy().replace('\\', "/");
    s.trim_start_matches("./")
        .trim_start_matches('/')
        .to_string()
}

/// Derive a route from a repo-relative file path using the first convention that
/// claims it. Returns `None` when no convention recognizes the file.
pub fn derive_route(rel_path: &Path, conventions: &[RoutingConvention]) -> Option<DerivedRoute> {
    let rel = normalize_rel(rel_path);
    for convention in conventions {
        let Some(after_root) = convention.strip_root(&rel) else {
            continue;
        };
        let Some(raw_segments) = convention.raw_segments(after_root) else {
            continue;
        };

        let mut path = String::new();
        for seg in &raw_segments {
            if let Some(transformed) = convention.transform_segment(seg) {
                path.push('/');
                path.push_str(&transformed);
            }
        }

        let prefix = convention.path_prefix.trim_end_matches('/');
        let mut full = format!("{}{}", prefix, path);
        if full.is_empty() {
            full = "/".to_string();
        }
        return Some(DerivedRoute {
            path: full,
            method_source: convention.method_source.clone(),
            convention: convention.name.clone(),
        });
    }
    None
}

/// Bootstrap conventions for detected frameworks. This is the *only* place a
/// framework name appears in the scanner; a convention supplied by detection or
/// `carrick.json` should be preferred over these (see module docs).
pub fn builtin_conventions(frameworks: &[String]) -> Vec<RoutingConvention> {
    let mentions = |needle: &str| frameworks.iter().any(|f| f.to_lowercase().contains(needle));
    let mut out = Vec::new();
    if mentions("next") {
        out.push(RoutingConvention::nextjs_app());
        out.push(RoutingConvention::nextjs_pages());
    }
    if mentions("astro") {
        out.push(RoutingConvention::astro());
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn next() -> Vec<RoutingConvention> {
        vec![
            RoutingConvention::nextjs_app(),
            RoutingConvention::nextjs_pages(),
        ]
    }

    fn route(p: &str) -> Option<DerivedRoute> {
        derive_route(&PathBuf::from(p), &next())
    }

    // --- App Router ---

    #[test]
    fn app_router_static() {
        let r = route("app/users/route.ts").unwrap();
        assert_eq!(r.path, "/users");
        assert_eq!(r.method_source, MethodSource::ExportName);
        assert_eq!(r.convention, "nextjs-app");
    }

    #[test]
    fn app_router_root() {
        assert_eq!(route("app/route.ts").unwrap().path, "/");
    }

    #[test]
    fn app_router_dynamic() {
        assert_eq!(route("app/users/[id]/route.ts").unwrap().path, "/users/:id");
    }

    #[test]
    fn app_router_nested_dynamic() {
        assert_eq!(
            route("app/teams/[teamId]/members/[userId]/route.ts")
                .unwrap()
                .path,
            "/teams/:teamId/members/:userId"
        );
    }

    #[test]
    fn app_router_catch_all() {
        assert_eq!(
            route("app/files/[...slug]/route.ts").unwrap().path,
            "/files/**"
        );
    }

    #[test]
    fn app_router_optional_catch_all() {
        assert_eq!(
            route("app/shop/[[...slug]]/route.ts").unwrap().path,
            "/shop/**"
        );
    }

    #[test]
    fn app_router_strips_route_groups() {
        assert_eq!(
            route("app/(marketing)/about/route.ts").unwrap().path,
            "/about"
        );
    }

    #[test]
    fn app_router_src_prefix() {
        assert_eq!(route("src/app/health/route.ts").unwrap().path, "/health");
    }

    #[test]
    fn app_router_ignores_non_route_files() {
        assert!(route("app/users/page.tsx").is_none());
        assert!(route("app/users/layout.tsx").is_none());
        assert!(route("app/users/component.ts").is_none());
    }

    // --- Pages Router ---

    #[test]
    fn pages_api_static() {
        let r = route("pages/api/users.ts").unwrap();
        assert_eq!(r.path, "/api/users");
        assert_eq!(r.method_source, MethodSource::DefaultExport);
        assert_eq!(r.convention, "nextjs-pages");
    }

    #[test]
    fn pages_api_index_collapses() {
        assert_eq!(
            route("pages/api/users/index.ts").unwrap().path,
            "/api/users"
        );
        assert_eq!(route("pages/api/index.ts").unwrap().path, "/api");
    }

    #[test]
    fn pages_api_dynamic_filename() {
        assert_eq!(
            route("pages/api/users/[id].ts").unwrap().path,
            "/api/users/:id"
        );
    }

    #[test]
    fn pages_api_catch_all_filename() {
        assert_eq!(
            route("pages/api/proxy/[...path].ts").unwrap().path,
            "/api/proxy/**"
        );
    }

    #[test]
    fn pages_api_src_prefix() {
        assert_eq!(route("src/pages/api/ping.ts").unwrap().path, "/api/ping");
    }

    #[test]
    fn pages_api_skips_private_files() {
        assert!(route("pages/api/_middleware.ts").is_none());
    }

    // --- Astro ---

    fn astro_route(p: &str) -> Option<DerivedRoute> {
        derive_route(&PathBuf::from(p), &[RoutingConvention::astro()])
    }

    #[test]
    fn astro_static_endpoint() {
        // No forced /api prefix: "api" here is just a literal directory segment.
        let r = astro_route("src/pages/api/users.ts").unwrap();
        assert_eq!(r.path, "/api/users");
        // Methods come from named exports, not a single default handler.
        assert_eq!(r.method_source, MethodSource::ExportName);
        assert_eq!(r.convention, "astro");
    }

    #[test]
    fn astro_top_level_endpoint() {
        assert_eq!(astro_route("src/pages/health.ts").unwrap().path, "/health");
    }

    #[test]
    fn astro_index_collapses() {
        assert_eq!(astro_route("src/pages/index.ts").unwrap().path, "/");
        assert_eq!(astro_route("src/pages/api/index.ts").unwrap().path, "/api");
    }

    #[test]
    fn astro_dynamic_filename() {
        assert_eq!(
            astro_route("src/pages/posts/[id].ts").unwrap().path,
            "/posts/:id"
        );
    }

    #[test]
    fn astro_rest_param() {
        assert_eq!(
            astro_route("src/pages/files/[...path].ts").unwrap().path,
            "/files/**"
        );
    }

    #[test]
    fn astro_javascript_endpoint() {
        assert_eq!(astro_route("src/pages/ping.js").unwrap().path, "/ping");
    }

    #[test]
    fn astro_ignores_page_components_and_private_files() {
        // `.astro` files are HTML pages, not API endpoints.
        assert!(astro_route("src/pages/about.astro").is_none());
        // `_`-prefixed files are excluded from Astro routing.
        assert!(astro_route("src/pages/_helpers.ts").is_none());
        // Pages outside `src/pages` are not endpoints.
        assert!(astro_route("src/lib/db.ts").is_none());
    }

    #[test]
    fn astro_gated_on_framework_detection() {
        assert!(builtin_conventions(&["express".to_string()]).is_empty());
        let astro = builtin_conventions(&["Astro".to_string()]);
        assert_eq!(astro.len(), 1);
        assert_eq!(astro[0].name, "astro");
    }

    // --- Negative / boundary ---

    #[test]
    fn non_route_paths_return_none() {
        assert!(route("lib/db.ts").is_none());
        assert!(route("components/Button.tsx").is_none());
        // "app" prefix must respect segment boundaries.
        assert!(route("application/route.ts").is_none());
        // pages routes that aren't under /api are not API endpoints here.
        assert!(route("pages/about.tsx").is_none());
    }

    #[test]
    fn longest_root_wins() {
        // Both "pages/api" and "src/pages/api" exist; the src-prefixed file must
        // resolve via the longer root, not leave "src" in the path.
        assert_eq!(route("src/pages/api/x.ts").unwrap().path, "/api/x");
    }

    #[test]
    fn builtin_conventions_gated_on_framework() {
        assert!(builtin_conventions(&["express".to_string()]).is_empty());
        let next = builtin_conventions(&["Next.js".to_string()]);
        assert_eq!(next.len(), 2);
    }

    #[test]
    fn convention_roundtrips_through_serde() {
        // The B-contract: a cloud/config-supplied convention must deserialize.
        let c = RoutingConvention::nextjs_app();
        let json = serde_json::to_string(&c).unwrap();
        let back: RoutingConvention = serde_json::from_str(&json).unwrap();
        assert_eq!(c, back);
    }
}
