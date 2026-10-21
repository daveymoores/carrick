//! What binding a service's position rows costs, step by step, and the
//! time budget that keeps it from stalling a scan (carrick#2564).

use std::time::{Duration, Instant};
use tracing::debug;

/// How long binding one service's rows may take
/// ([`super::RequestSummaryInputs::linked`]): past it, the service reads as
/// with no rows. A bound join of a large service takes seconds; this is far
/// past that, and far short of a run's own limit.
pub const ROWS_JOIN_BUDGET: Duration = Duration::from_secs(300);

/// One service's rows join, timed step by step: each step is written to
/// the run log as it ends, so a join that stalls shows the step it is in.
#[derive(Debug)]
pub(super) struct RowsJoin {
    service: String,
    budget: Duration,
    started: Instant,
    step_started: Instant,
}

impl RowsJoin {
    pub(super) fn start(service: &str, budget: Duration) -> Self {
        let now = Instant::now();
        Self {
            service: service.to_string(),
            budget,
            started: now,
            step_started: now,
        }
    }

    /// When the budget runs out.
    pub(super) fn deadline(&self) -> Instant {
        self.started + self.budget
    }

    /// `step` ended.
    pub(super) fn step(&mut self, step: &'static str) {
        debug!(
            "Position rows for service {}: {step} bound in {:.2}s",
            self.service,
            self.step_started.elapsed().as_secs_f64()
        );
        self.step_started = Instant::now();
    }

    /// The budget ran out in `step`: the service reads as with no rows.
    pub(super) fn run_out(&self, step: &'static str) {
        debug!(
            "Position rows for service {}: binding them ran past its {}s budget in {step} ({:.2}s in all); the service reads as with no rows",
            self.service,
            self.budget.as_secs(),
            self.started.elapsed().as_secs_f64()
        );
    }
}

#[cfg(test)]
mod tests {
    use super::super::fixture_walk::{Walked, walked_within};
    use super::super::row_calls::every_call;
    use super::super::{LibrarySemantics, summarize};
    use std::collections::BTreeMap;
    use std::fmt::Write as _;
    use std::time::Duration;

    /// Every summary row and every call's reach of `walked`, by
    /// repo-relative file, as text: what the service reads as.
    fn read_as(walked: &Walked) -> BTreeMap<String, String> {
        let index = summarize(&walked.inputs, &LibrarySemantics::default());
        let repo = walked.repo.to_string_lossy().to_string();
        let mut out = BTreeMap::new();
        for file in walked.inputs.files.keys() {
            let reaches: Vec<String> = every_call(&walked.inputs.files)
                .into_iter()
                .filter(|(at, _, _)| *at == file)
                .map(|(_, call, _)| {
                    format!(
                        "{}: {:?}",
                        call.site.line,
                        walked.inputs.reach(file, call.site.lo, call.site.hi)
                    )
                })
                .collect();
            let text = format!("{:?} {reaches:?}", index.rows(file)).replace(&repo, "");
            out.insert(file.to_string_lossy().replace(&repo, ""), text);
        }
        out
    }

    /// A join past its budget leaves the service exactly as a run without
    /// rows reads it, and one within it keeps the rows (carrick#2564).
    #[test]
    fn a_join_past_its_budget_reads_as_with_no_rows() {
        let walk = |with_rows, budget| {
            walked_within(
                "position-rows-calls",
                "services/calls",
                "calls",
                &[],
                with_rows,
                budget,
            )
        };
        let Some(forced) = walk(true, Duration::ZERO) else {
            return;
        };
        let off = walk(false, super::ROWS_JOIN_BUDGET).unwrap();
        let on = walk(true, super::ROWS_JOIN_BUDGET).unwrap();
        assert!(forced.inputs.positions.is_none());
        assert!(forced.inputs.links.is_none() && forced.inputs.row_calls.is_none());
        assert_eq!(read_as(&forced), read_as(&off));
        assert!(on.inputs.positions.is_some() && on.inputs.row_calls.is_some());
        assert_ne!(
            read_as(&on),
            read_as(&off),
            "the fixture reads otherwise with rows, so the comparison means something"
        );
    }

    /// A service with one large file that is not plain ASCII and makes a
    /// call through an import on every line, and a link graph with a long
    /// re-export chain and two barrels that re-export each other: the
    /// shapes whose rows join costs the most per lookup.
    fn heavy_service(lines: usize, chain: usize) -> Vec<(String, String)> {
        let mut big = String::from(
            "// Übersetzungen und Kommentare: 日本語のテキスト, emoji 😀, café.\n\
             import { send } from \"./chain/c0\";\n\
             import { BASE } from \"./barrel_a\";\n",
        );
        // Text past the first wide character is where the rows' offsets
        // and SWC's part ways; a few more keep them parting.
        for line in 0..lines {
            let note = if line % 500 == 0 {
                "日本 — é"
            } else {
                "plain"
            };
            writeln!(
                big,
                "export function f{line}() {{ return send(`${{BASE}}/items/{line}`, \"{note} {line}\"); }}"
            )
            .unwrap();
        }
        let mut files = vec![("src/big.ts".to_string(), big)];
        for link in 0..chain {
            files.push((
                format!("src/chain/c{link}.ts"),
                format!("export {{ send }} from \"./c{}\";\n", link + 1),
            ));
        }
        files.push((
            format!("src/chain/c{chain}.ts"),
            "export function send(url: string, body: string) {\n  return fetch(url, { method: \"POST\", body });\n}\n"
                .to_string(),
        ));
        files.push((
            "src/barrel_a.ts".to_string(),
            "export * from \"./barrel_b\";\nexport const BASE = \"/api\";\n".to_string(),
        ));
        files.push((
            "src/barrel_b.ts".to_string(),
            "export * from \"./barrel_a\";\nexport const OTHER = \"/other\";\n".to_string(),
        ));
        files
    }

    fn walk_heavy(lines: usize, with_rows: bool, budget: Duration) -> Option<Walked> {
        let files = heavy_service(lines, 40);
        let edit: Vec<(&str, &str)> = files
            .iter()
            .map(|(name, text)| (name.as_str(), text.as_str()))
            .collect();
        walked_within(
            "position-rows-names",
            "services/names",
            "names",
            &edit,
            with_rows,
            budget,
        )
    }

    /// The rows of a large file that is not plain ASCII bind well within a
    /// minute: a lookup converts its span from a per-file table, not by
    /// counting from the start of the file (carrick#2564). Counting from
    /// the start, this join runs for many minutes, so it runs out of the
    /// budget and the rows are dropped.
    #[test]
    fn a_large_non_ascii_file_binds_its_rows_within_a_budget() {
        const LINES: usize = 20_000;
        let budget = Duration::from_secs(60);
        let Some(on) = walk_heavy(LINES, true, budget) else {
            return;
        };
        let off = walk_heavy(LINES, false, budget).unwrap();
        crate::errln!(
            "rows join, {LINES} lines: discovery with rows {:?}, without {:?}",
            on.discovered_in,
            off.discovered_in
        );
        let row_calls = on
            .inputs
            .row_calls
            .as_ref()
            .expect("the rows are bound within the budget");
        assert!(
            row_calls.describe().contains("agree"),
            "the calls are read from the rows: {}",
            row_calls.describe()
        );
    }
}
