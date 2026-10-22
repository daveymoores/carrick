//! The files the fact layer would send to the file reader (carrick#2613,
//! ticket T1): record-only. The engine builds this from the library store's
//! first answer, before the reader dispatches, and the orchestrator logs how
//! it compares with the files the hand-written signals select. Nothing here
//! changes which file is read.
//!
//! A file is a fact file when any of these holds (decisions D1-D2 on
//! carrick#2440's plan):
//! - discovery found a site in it, or the site's labelled call is in it;
//! - a decided-sites lead is in it: a discovered call whose name no call of
//!   this code decides. These are the leads known before any model question
//!   is asked; the possible calls the questions add later are not here, and
//!   each of those has a site that already put its file in the set;
//! - a call in it goes through a package the store's first answer leaves
//!   without exports.
//!
//! Not here: a package the scan never asked about (an alias, a workspace
//! package outside the service). Telling those from an installed package is
//! discovery's `outside_service` stop (ticket T2), not this record's.

use std::collections::BTreeMap;
use std::path::Path;

use super::LibrarySites;
use super::decided_sites::Decided;
use super::discovery::Discovery;
use crate::library_store::FirstAnswer;

/// What discovery recorded for one call.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Followed {
    /// The walk reached a labelled export.
    Site,
    /// The walk entered code and stopped.
    Stopped,
}

/// Why a file is a fact file. A file may carry several.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Why {
    pub site: bool,
    pub lead: bool,
    pub unanswered_package: bool,
}

/// The fact layer's files for one service, by repo-relative path with `/`
/// separators.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FactFiles {
    pub files: BTreeMap<String, Why>,
    /// What discovery recorded at each call, by file and SWC span.
    pub calls: BTreeMap<(String, u32, u32), Followed>,
    /// Packages the first answer left without exports.
    pub unanswered_packages: usize,
    /// The ask got no answer at all.
    pub ask_failed: bool,
}

impl FactFiles {
    /// The fact files of a service: `discovery` read through the first
    /// answer's labels, `decided` its leads, `library` its calls through
    /// packages, `first` what the store left unanswered. Paths relative to
    /// `root`.
    pub fn of(
        discovery: &Discovery,
        decided: &Decided,
        library: Option<&LibrarySites>,
        first: &FirstAnswer,
        root: &Path,
    ) -> Self {
        let mut facts = FactFiles {
            unanswered_packages: first.unanswered.len(),
            ask_failed: first.ask_failed,
            ..FactFiles::default()
        };
        for site in &discovery.sites {
            let followed = if site.end.is_some() {
                Followed::Site
            } else if site.stop.is_some() {
                Followed::Stopped
            } else {
                continue;
            };
            let key = (site.file.clone(), site.span_start, site.span_end);
            // A call reached by one walk and stopped by another is a site.
            let kept = facts.calls.entry(key).or_insert(followed);
            if followed == Followed::Site {
                *kept = Followed::Site;
            }
            if let Some(end) = &site.end {
                facts.files.entry(site.file.clone()).or_default().site = true;
                facts.files.entry(end.file.clone()).or_default().site = true;
            }
        }
        for lead in &decided.leads {
            facts.files.entry(lead.file.clone()).or_default().lead = true;
            facts.files.entry(lead.end.file.clone()).or_default().lead = true;
        }
        if let Some(library) = library {
            for site in &library.sites {
                if !first.unanswered.contains(site.package()) {
                    continue;
                }
                let relative = site.file.strip_prefix(root).unwrap_or(&site.file);
                let file = relative.to_string_lossy().replace('\\', "/");
                facts.files.entry(file).or_default().unanswered_package = true;
            }
        }
        facts
    }

    /// How many fact files hold a site, a lead and an unanswered-package
    /// call, in that order.
    pub fn counts(&self) -> (usize, usize, usize) {
        let count = |why: fn(&Why) -> bool| self.files.values().filter(|why_| why(why_)).count();
        (
            count(|why| why.site),
            count(|why| why.lead),
            count(|why| why.unanswered_package),
        )
    }

    /// What discovery recorded at the call at `span` in `file`, or `None`
    /// when it recorded nothing there.
    pub fn followed(&self, file: &str, span: (u32, u32)) -> Option<Followed> {
        self.calls.get(&(file.to_string(), span.0, span.1)).copied()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::request_summary::discovery::{DiscoverySite, SiteEnd};
    use crate::services::type_sidecar::LibraryRole;

    fn site(
        file: &str,
        span: (u32, u32),
        end: Option<SiteEnd>,
        stop: Option<&str>,
    ) -> DiscoverySite {
        DiscoverySite {
            file: file.to_string(),
            line: 1,
            span_start: span.0,
            span_end: span.1,
            callee: "send".to_string(),
            end,
            chain: Vec::new(),
            stop: stop.map(str::to_string),
            receiver: None,
            reached: None,
        }
    }

    fn end(file: &str) -> SiteEnd {
        SiteEnd {
            package: "lib".to_string(),
            export: "default".to_string(),
            member: None,
            role: LibraryRole::HttpClient,
            op: None,
            file: file.to_string(),
            line: 1,
            span_start: 5,
            span_end: 9,
            new: false,
        }
    }

    /// A site puts its own file and its labelled call's file in the set; a
    /// stopped walk records the call as stopped and selects nothing; a call
    /// one walk reached and another stopped is a site.
    #[test]
    fn a_site_selects_both_files_and_a_stop_selects_none() {
        let discovery = Discovery {
            sites: vec![
                site("a.ts", (1, 4), Some(end("b.ts")), None),
                site("c.ts", (1, 4), None, Some("no_body")),
                site("d.ts", (1, 4), None, Some("no_body")),
                site("d.ts", (1, 4), Some(end("e.ts")), None),
            ],
            ..Discovery::default()
        };
        let facts = FactFiles::of(
            &discovery,
            &Decided::default(),
            None,
            &FirstAnswer::default(),
            Path::new("/repo"),
        );
        let files: Vec<&str> = facts.files.keys().map(String::as_str).collect();
        assert_eq!(files, ["a.ts", "b.ts", "d.ts", "e.ts"]);
        assert_eq!(facts.followed("c.ts", (1, 4)), Some(Followed::Stopped));
        assert_eq!(facts.followed("d.ts", (1, 4)), Some(Followed::Site));
        assert_eq!(facts.followed("a.ts", (9, 9)), None);
        assert_eq!(facts.counts(), (4, 0, 0));
    }
}
