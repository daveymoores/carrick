//! Env-var alias resolution.
//!
//! A very common real pattern aliases an environment variable through a local
//! const before interpolating it into a request URL:
//!
//! ```ts
//! const ORDERS_BASE = process.env.ORDERS_SERVICE_URL ?? "http://localhost:3001";
//! await fetch(`${ORDERS_BASE}/orders/${orderId}`);
//! ```
//!
//! The file analyzer extracts the call target verbatim as
//! `${ORDERS_BASE}/orders/${orderId}`, so every downstream consumer that keys
//! on the env-var *name* (`Config::is_internal_call`, the cross-repo matcher)
//! sees the local const `ORDERS_BASE` rather than the real env var
//! `ORDERS_SERVICE_URL`. Internal/external classification and cross-repo
//! matching then silently fail.
//!
//! This module builds a per-file map of `local const -> process.env name` for
//! the direct-alias case and rewrites a target URL's leading `${ALIAS}` to
//! `${process.env.NAME}`. That funnels the call back through the existing
//! direct-`process.env` handling in [`crate::url_normalizer`] and
//! [`crate::analyzer`], rather than duplicating env-var parsing.
//!
//! Scope is deliberately tight: only bindings initialized *directly* from
//! `process.env` (optionally with a `??`/`||` default) are tracked. Anything
//! beyond the direct alias — reassignment, string concatenation building the
//! base URL, cross-file/imported bases, deep data flow — is intentionally not
//! resolved. See the TODO in [`EnvAliasExtractor`].

use std::collections::HashMap;
use swc_ecma_ast::*;
use swc_ecma_visit::{Visit, VisitWith};

/// Maps a local binding name (e.g. `ORDERS_BASE`) to the `process.env` variable
/// it was initialized from (e.g. `ORDERS_SERVICE_URL`).
pub type EnvAliasMap = HashMap<String, String>;

/// Visitor that collects `const/let/var X = process.env.NAME [?? default]`
/// bindings into an [`EnvAliasMap`].
///
/// TODO(#218 follow-up): only direct `process.env` aliases are tracked. We do
/// not follow reassignments, concatenated/templated bases
/// (`const b = process.env.X + "/v1"`), or aliases imported from another
/// module. Those need real data-flow / cross-file analysis and are out of
/// scope for the deterministic direct-alias fix.
#[derive(Default)]
pub struct EnvAliasExtractor {
    pub aliases: EnvAliasMap,
}

impl EnvAliasExtractor {
    /// Build the alias map for a parsed module.
    pub fn build(module: &Module) -> EnvAliasMap {
        let mut extractor = EnvAliasExtractor::default();
        module.visit_with(&mut extractor);
        extractor.aliases
    }
}

impl Visit for EnvAliasExtractor {
    fn visit_var_decl(&mut self, var_decl: &VarDecl) {
        for decl in &var_decl.decls {
            // Only simple identifier bindings: `const X = ...`. Destructuring
            // (`const { X } = process.env`) is a different, rarer pattern.
            let Pat::Ident(binding) = &decl.name else {
                continue;
            };
            let Some(init) = &decl.init else {
                continue;
            };

            if let Some(env_name) = process_env_name(init) {
                // SWC resolver gives each binding a unique SyntaxContext, but the
                // call target the LLM emits is just the bare symbol text. Key on
                // the symbol so `${ORDERS_BASE}` resolves. A name shadowed in a
                // nested scope would collide here, but that is vanishingly rare
                // for a base-URL const and far better than not resolving at all.
                self.aliases.insert(binding.id.sym.to_string(), env_name);
            }
        }

        var_decl.visit_children_with(self);
    }
}

/// If `expr` reads a single `process.env` variable (optionally with a `??`/`||`
/// default), return that variable's name.
///
/// Handles:
/// - `process.env.NAME`
/// - `process.env["NAME"]`
/// - `process.env.NAME ?? <default>` / `process.env.NAME || <default>`
fn process_env_name(expr: &Expr) -> Option<String> {
    match expr {
        Expr::Member(member) => process_env_member_name(member),
        // `process.env.NAME ?? "default"` / `... || "default"`: the env read is
        // the left operand. The default literal is discarded — the env-var name
        // is all the classifier needs.
        Expr::Bin(bin) if matches!(bin.op, BinaryOp::NullishCoalescing | BinaryOp::LogicalOr) => {
            process_env_name(&bin.left)
        }
        // Unwrap transparent wrappers so `(process.env.X)`, `process.env.X!`,
        // and `process.env.X as string` still resolve.
        Expr::Paren(paren) => process_env_name(&paren.expr),
        Expr::TsNonNull(non_null) => process_env_name(&non_null.expr),
        Expr::TsAs(ts_as) => process_env_name(&ts_as.expr),
        _ => None,
    }
}

/// If `member` is `process.env.NAME` or `process.env["NAME"]`, return `NAME`.
fn process_env_member_name(member: &MemberExpr) -> Option<String> {
    // The object must be exactly `process.env`.
    let Expr::Member(obj) = &*member.obj else {
        return None;
    };
    if !is_ident(&obj.obj, "process") || !is_ident_prop(&obj.prop, "env") {
        return None;
    }

    match &member.prop {
        MemberProp::Ident(ident) => Some(ident.sym.to_string()),
        MemberProp::Computed(computed) => match &*computed.expr {
            Expr::Lit(Lit::Str(s)) => Some(s.value.to_string()),
            _ => None,
        },
        MemberProp::PrivateName(_) => None,
    }
}

fn is_ident(expr: &Expr, name: &str) -> bool {
    matches!(expr, Expr::Ident(ident) if ident.sym.as_ref() == name)
}

fn is_ident_prop(prop: &MemberProp, name: &str) -> bool {
    matches!(prop, MemberProp::Ident(ident) if ident.sym.as_ref() == name)
}

/// Rewrite a leading `${ALIAS}` in a call target to `${process.env.NAME}` when
/// `ALIAS` is a known env-var alias, so the existing direct-`process.env`
/// handling resolves the real env-var name.
///
/// Only the *leading* interpolation is considered: that is the base-URL slot.
/// A mid-path `${id}` is a path parameter, never an env alias, so it is left
/// untouched. Returns `None` when nothing was rewritten.
pub fn resolve_target_env_alias(target: &str, aliases: &EnvAliasMap) -> Option<String> {
    if aliases.is_empty() {
        return None;
    }

    // The analyzer/normalizer trim wrapper backticks/quotes themselves, but the
    // alias sits at the very front, so peek past any leading wrapper char.
    let trimmed = target.trim_start_matches(['`', '"', '\'']);
    let rest = trimmed.strip_prefix("${")?;
    let end = rest.find('}')?;
    let alias = &rest[..end];

    let env_name = aliases.get(alias)?;

    // Splice `process.env.NAME` in for the bare alias, preserving everything the
    // wrapper-trim skipped and the rest of the path verbatim.
    let prefix_len = target.len() - trimmed.len();
    let after_brace = &rest[end + 1..];
    Some(format!(
        "{}${{process.env.{}}}{}",
        &target[..prefix_len],
        env_name,
        after_brace
    ))
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

    fn build_map(source: &str) -> EnvAliasMap {
        let tmp_dir = tempfile::tempdir().expect("tempdir");
        let file_path = tmp_dir.path().join("input.ts");
        std::fs::write(&file_path, source).expect("write file");

        let cm: Lrc<SourceMap> = Default::default();
        let handler = Handler::with_tty_emitter(ColorConfig::Never, true, false, Some(cm.clone()));
        let module = parse_file(&file_path, &cm, &handler).expect("parsed module");

        EnvAliasExtractor::build(&module)
    }

    #[test]
    fn extracts_direct_process_env_alias() {
        let map = build_map(r#"const ORDERS_BASE = process.env.ORDERS_SERVICE_URL;"#);
        assert_eq!(
            map.get("ORDERS_BASE").map(String::as_str),
            Some("ORDERS_SERVICE_URL")
        );
    }

    #[test]
    fn extracts_nullish_coalescing_default_form() {
        // The exact pattern from issue #218.
        let map = build_map(
            r#"const ORDERS_BASE = process.env.ORDERS_SERVICE_URL ?? "http://localhost:3001";"#,
        );
        assert_eq!(
            map.get("ORDERS_BASE").map(String::as_str),
            Some("ORDERS_SERVICE_URL")
        );
    }

    #[test]
    fn extracts_logical_or_default_form() {
        let map = build_map(r#"const BASE = process.env.SERVICE_URL || "http://localhost:3001";"#);
        assert_eq!(map.get("BASE").map(String::as_str), Some("SERVICE_URL"));
    }

    #[test]
    fn extracts_bracket_access_form() {
        let map = build_map(r#"const BASE = process.env["SERVICE_URL"];"#);
        assert_eq!(map.get("BASE").map(String::as_str), Some("SERVICE_URL"));
    }

    #[test]
    fn extracts_let_and_var_forms() {
        let map = build_map(
            r#"let A = process.env.A_URL;
var B = process.env.B_URL ?? "";"#,
        );
        assert_eq!(map.get("A").map(String::as_str), Some("A_URL"));
        assert_eq!(map.get("B").map(String::as_str), Some("B_URL"));
    }

    #[test]
    fn unwraps_paren_nonnull_and_as_casts() {
        let map = build_map(
            r#"const A = (process.env.A_URL);
const B = process.env.B_URL!;
const C = process.env.C_URL as string;"#,
        );
        assert_eq!(map.get("A").map(String::as_str), Some("A_URL"));
        assert_eq!(map.get("B").map(String::as_str), Some("B_URL"));
        assert_eq!(map.get("C").map(String::as_str), Some("C_URL"));
    }

    #[test]
    fn ignores_non_env_bindings() {
        // Not process.env, a concatenated base, and a destructure — all out of scope.
        let map = build_map(
            r#"const HOST = config.host;
const BASE = process.env.X_URL + "/v1";
const { Y_URL } = process.env;"#,
        );
        assert!(!map.contains_key("HOST"));
        // Concatenation is intentionally not resolved (TODO scope), so BASE must
        // NOT map to a partial env name.
        assert!(!map.contains_key("BASE"));
        assert!(!map.contains_key("Y_URL"));
    }

    #[test]
    fn resolves_leading_alias_in_target() {
        let mut aliases = EnvAliasMap::new();
        aliases.insert("ORDERS_BASE".to_string(), "ORDERS_SERVICE_URL".to_string());

        assert_eq!(
            resolve_target_env_alias("${ORDERS_BASE}/orders/${orderId}", &aliases).as_deref(),
            Some("${process.env.ORDERS_SERVICE_URL}/orders/${orderId}")
        );
    }

    #[test]
    fn resolves_leading_alias_past_wrapper_backtick() {
        let mut aliases = EnvAliasMap::new();
        aliases.insert("ORDERS_BASE".to_string(), "ORDERS_SERVICE_URL".to_string());

        assert_eq!(
            resolve_target_env_alias("`${ORDERS_BASE}/orders/${id}`", &aliases).as_deref(),
            Some("`${process.env.ORDERS_SERVICE_URL}/orders/${id}`")
        );
    }

    #[test]
    fn leaves_unknown_and_mid_path_interpolations_untouched() {
        let mut aliases = EnvAliasMap::new();
        aliases.insert("ORDERS_BASE".to_string(), "ORDERS_SERVICE_URL".to_string());

        // Unknown leading var: not an alias.
        assert!(resolve_target_env_alias("${API_URL}/users", &aliases).is_none());
        // A path parameter mid-URL must never be treated as a base-URL alias.
        assert!(resolve_target_env_alias("/orders/${ORDERS_BASE}", &aliases).is_none());
        // Empty alias map short-circuits.
        assert!(resolve_target_env_alias("${ORDERS_BASE}/x", &EnvAliasMap::new()).is_none());
    }
}
