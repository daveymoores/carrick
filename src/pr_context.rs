//! Which pull request a run is scanning, and from where it learns it
//! (carrick#2229).
//!
//! On GitHub Actions the number, head and base come from `GITHUB_*`. A runner
//! outside Actions passes `--pr`, `--pr-head` and `--pr-base` instead, and the
//! flags win. Either way the engine asks [`PrContext`] and not the
//! environment. The contract is `docs/reference/runner.md`.

use std::sync::OnceLock;

/// The pull request a run scans.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PrContext {
    pub number: u64,
    /// The PR branch's tip. `None` when an Actions run's event payload is
    /// unreadable.
    pub head: Option<String>,
    /// The commit the PR's tree is based on.
    pub base: Option<String>,
    /// The Actions run id. Absent on the flag path: the cloud identifies a
    /// runner run by its scan.
    pub run_id: Option<String>,
    /// Set from `--pr`, not from the environment. Only such a run is told
    /// never to upload and closes its scan with `pull_request`; an Actions PR
    /// run behaves as it did before the flags existed.
    pub declared: bool,
}

/// The three flags as written, before validation.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PrFlags {
    pub number: Option<String>,
    pub head: Option<String>,
    pub base: Option<String>,
}

/// What the environment says of the pull request, read by the engine.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct EnvPr {
    pub number: Option<u64>,
    pub head: Option<String>,
    pub base: Option<String>,
    pub run_id: Option<String>,
}

pub const PARTIAL_SET: &str = "--pr, --pr-head and --pr-base go together; pass all three.";
pub const UPLOAD_CONFLICT: &str =
    "--upload cannot be combined with --pr: a pull request scan never writes the index.";
/// Closes the scan a `--pr` run opened (the reason is validated by shape).
pub const CLOSE_REASON: &str = "pull_request";

fn declared_slot() -> &'static OnceLock<PrContext> {
    static DECLARED: OnceLock<PrContext> = OnceLock::new();
    &DECLARED
}

/// Record the pull request `--pr` named. One run is one PR; a second call is
/// ignored.
pub fn declare(context: PrContext) {
    let _ = declared_slot().set(context);
}

/// The pull request named by flags, or `None` on a run that passed none.
pub fn declared() -> Option<&'static PrContext> {
    declared_slot().get()
}

/// Whether a full commit id: 40 hex characters.
fn is_commit_id(value: &str) -> bool {
    value.len() == 40 && value.bytes().all(|b| b.is_ascii_hexdigit())
}

/// The flags, validated: all three or none; a positive integer; two full
/// commit ids. `Ok(None)` when none was passed.
pub fn parse_flags(flags: &PrFlags) -> Result<Option<(u64, String, String)>, String> {
    let (number, head, base) = match (&flags.number, &flags.head, &flags.base) {
        (None, None, None) => return Ok(None),
        (Some(number), Some(head), Some(base)) => (number, head, base),
        _ => return Err(PARTIAL_SET.to_string()),
    };
    let number = number
        .parse::<u64>()
        .ok()
        .filter(|n| *n > 0 && number.bytes().all(|b| b.is_ascii_digit()))
        .ok_or_else(|| format!("--pr takes a pull request number, not '{number}'."))?;
    for value in [head, base] {
        if !is_commit_id(value) {
            return Err(format!(
                "--pr-head and --pr-base take a full commit id (40 hex characters), not '{value}'."
            ));
        }
    }
    Ok(Some((number, head.to_lowercase(), base.to_lowercase())))
}

/// The base of the tree a `--pr` run was given, or the refusal.
///
/// - HEAD is the PR head: a head checkout (the PR has conflicts), based on
///   `--pr-base`.
/// - HEAD has two parents and the second is the PR head: a merge checkout,
///   based on its first parent.
/// - Anything else is a stale merge ref or another tree.
pub fn checkout_base(
    head_commit: Result<&str, &str>,
    parents: &[String],
    pr_head: &str,
    pr_base: &str,
) -> Result<String, String> {
    if let Ok(head) = head_commit
        && head.eq_ignore_ascii_case(pr_head)
    {
        return Ok(pr_base.to_string());
    }
    if let [base, second] = parents
        && second.eq_ignore_ascii_case(pr_head)
    {
        return Ok(base.clone());
    }
    Err(format!(
        "this checkout is neither the pull request's head ({pr_head}) nor a merge of it, so its \
         results would describe another tree."
    ))
}

/// The run's pull request: the flags when given, else the environment's.
pub fn resolve(declared: Option<&PrContext>, env: EnvPr) -> Option<PrContext> {
    if let Some(declared) = declared {
        return Some(declared.clone());
    }
    Some(PrContext {
        number: env.number?,
        head: env.head,
        base: env.base,
        run_id: env.run_id,
        declared: false,
    })
}

/// The pull request the flags name, once the checkout at `repo_path` is shown
/// to be its tree. `Ok(None)` without flags.
pub fn from_flags(flags: &PrFlags, repo_path: &str) -> Result<Option<PrContext>, String> {
    let Some((number, head, base)) = parse_flags(flags)? else {
        return Ok(None);
    };
    let head_commit = crate::git_state::head_commit(repo_path);
    let parents = crate::git_state::head_parents(repo_path).unwrap_or_default();
    let base = checkout_base(
        head_commit.as_deref().map_err(String::as_str),
        &parents,
        &head,
        &base,
    )?;
    Ok(Some(PrContext {
        number,
        head: Some(head),
        base: Some(base),
        run_id: None,
        declared: true,
    }))
}

/// [`from_flags`], recorded for the run. Called in `main`, before anything is
/// analysed.
pub fn declare_from_flags(flags: &PrFlags, repo_path: &str) -> Result<(), String> {
    if let Some(context) = from_flags(flags, repo_path)? {
        declare(context);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const H1: &str = "1111111111111111111111111111111111111111";
    const B0: &str = "0000000000000000000000000000000000000000";
    const M: &str = "mmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmm";

    fn flags(n: Option<&str>, h: Option<&str>, b: Option<&str>) -> PrFlags {
        PrFlags {
            number: n.map(str::to_string),
            head: h.map(str::to_string),
            base: b.map(str::to_string),
        }
    }

    #[test]
    fn flags_are_all_or_none_and_validated() {
        assert_eq!(parse_flags(&flags(None, None, None)), Ok(None));
        assert_eq!(
            parse_flags(&flags(Some("7"), Some(H1), Some(B0))),
            Ok(Some((7, H1.to_string(), B0.to_string())))
        );
        assert_eq!(
            parse_flags(&flags(Some("7"), None, None)),
            Err(PARTIAL_SET.to_string())
        );
        assert_eq!(
            parse_flags(&flags(None, Some(H1), Some(B0))),
            Err(PARTIAL_SET.to_string())
        );
        for bad in ["0", "-3", "+3", "x", "", "1.5"] {
            assert_eq!(
                parse_flags(&flags(Some(bad), Some(H1), Some(B0))),
                Err(format!("--pr takes a pull request number, not '{bad}'.")),
                "{bad}"
            );
        }
        for bad in [
            "abc",
            "",
            &H1[..39],
            &format!("{H1}0"),
            &H1.replace('1', "g"),
        ] {
            let message = format!(
                "--pr-head and --pr-base take a full commit id (40 hex characters), not '{bad}'."
            );
            assert_eq!(
                parse_flags(&flags(Some("7"), Some(bad), Some(B0))),
                Err(message.clone()),
                "{bad}"
            );
            assert_eq!(
                parse_flags(&flags(Some("7"), Some(H1), Some(bad))),
                Err(message),
                "{bad}"
            );
        }
    }

    #[test]
    fn the_checkout_is_the_prs_head_or_a_merge_of_it() {
        let merge = vec![B0.to_string(), H1.to_string()];
        let other_base = "2222222222222222222222222222222222222222";
        // Merge checkout: the base is the first parent, whatever --pr-base says.
        assert_eq!(
            checkout_base(Ok(M), &merge, H1, other_base),
            Ok(B0.to_string())
        );
        // Head checkout: the base is --pr-base.
        assert_eq!(
            checkout_base(Ok(H1), &[B0.to_string()], H1, other_base),
            Ok(other_base.to_string())
        );
        // A merge of some other commit, or an unrelated tree.
        let refused = checkout_base(
            Ok(M),
            &merge,
            "3333333333333333333333333333333333333333",
            B0,
        )
        .unwrap_err();
        assert!(
            refused.starts_with("this checkout is neither the pull request's head (3333"),
            "{refused}"
        );
        assert!(checkout_base(Err("not a repository"), &[], H1, B0).is_err());
    }

    #[test]
    fn the_flags_win_over_the_environment() {
        let env = EnvPr {
            number: Some(12),
            head: Some("envhead".to_string()),
            base: Some("envbase".to_string()),
            run_id: Some("99".to_string()),
        };
        let declared = PrContext {
            number: 7,
            head: Some(H1.to_string()),
            base: Some(B0.to_string()),
            run_id: None,
            declared: true,
        };
        // Flags and an Actions environment: the flags, with no run id.
        assert_eq!(
            resolve(Some(&declared), env.clone()),
            Some(declared.clone())
        );
        // Flags and an empty environment (a runner).
        assert_eq!(
            resolve(Some(&declared), EnvPr::default()),
            Some(declared.clone())
        );
        // No flags: today's answer, run id included.
        assert_eq!(
            resolve(None, env),
            Some(PrContext {
                number: 12,
                head: Some("envhead".to_string()),
                base: Some("envbase".to_string()),
                run_id: Some("99".to_string()),
                declared: false,
            })
        );
        // No flags, not a PR: not a PR run.
        assert_eq!(resolve(None, EnvPr::default()), None);
    }

    /// The checkout check against real commits: B0, a branch head H1, and a
    /// merge M of the two.
    #[test]
    fn a_real_checkout_is_judged_by_its_head_and_parents() {
        let repo = tempfile::tempdir().expect("a repo");
        let dir = repo.path();
        let git = |args: &[&str]| {
            let out = std::process::Command::new("git")
                .args(args)
                .current_dir(dir)
                .env("GIT_AUTHOR_NAME", "t")
                .env("GIT_AUTHOR_EMAIL", "t@example.com")
                .env("GIT_COMMITTER_NAME", "t")
                .env("GIT_COMMITTER_EMAIL", "t@example.com")
                .output()
                .expect("git runs");
            assert!(out.status.success(), "{args:?}: {out:?}");
            String::from_utf8(out.stdout).unwrap().trim().to_string()
        };
        git(&["init", "-q", "-b", "main"]);
        git(&["commit", "-q", "--allow-empty", "-m", "b0"]);
        let b0 = git(&["rev-parse", "HEAD"]);
        git(&["checkout", "-q", "-b", "feature"]);
        std::fs::write(dir.join("a.txt"), "a").unwrap();
        git(&["add", "."]);
        git(&["commit", "-q", "-m", "h1"]);
        let h1 = git(&["rev-parse", "HEAD"]);
        git(&["checkout", "-q", "main"]);
        git(&["merge", "-q", "--no-ff", "-m", "m", "feature"]);
        let m = git(&["rev-parse", "HEAD"]);
        assert_ne!(m, h1);
        let path = dir.to_str().unwrap();
        let flags = |head: &str, base: &str| PrFlags {
            number: Some("7".to_string()),
            head: Some(head.to_string()),
            base: Some(base.to_string()),
        };
        let other = "2222222222222222222222222222222222222222";

        // HEAD = M, --pr-head H1: a merge checkout, based on its first parent.
        let merge = from_flags(&flags(&h1, other), path).unwrap().unwrap();
        assert_eq!(
            (merge.number, merge.base.as_deref()),
            (7, Some(b0.as_str()))
        );
        assert_eq!(merge.head.as_deref(), Some(h1.as_str()));
        assert_eq!(merge.run_id, None);
        assert!(merge.declared);

        // HEAD = M, --pr-head naming another commit: refused.
        let refused = from_flags(&flags(other, &b0), path).unwrap_err();
        assert!(
            refused.contains("neither the pull request's head"),
            "{refused}"
        );

        // HEAD = H1: a head checkout, based on --pr-base.
        git(&["checkout", "-q", "feature"]);
        let head = from_flags(&flags(&h1, &b0), path).unwrap().unwrap();
        assert_eq!(head.base.as_deref(), Some(b0.as_str()));
        assert_eq!(head.head.as_deref(), Some(h1.as_str()));
    }
}
