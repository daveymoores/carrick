//! Which service's analysis is running, for the prompt-lambda header builder
//! and for everything else a service records about itself.
//!
//! Every prompt-lambda call a service makes is billed to the repo as a whole.
//! Nothing on the request said which tree spent the money, so attributing a
//! monorepo's spend meant counting `framework-detect` calls to find the
//! service boundaries and segmenting every other request by the gaps between
//! them. [`name`] is what `X-Carrick-Service` carries so that is a log filter
//! instead (carrick#1221, carrick-cloud#978). The same name keys the losses a
//! service records ([`crate::scan_health`]), the requests it makes
//! ([`crate::agent_service`]) and the line its tickers speak for
//! ([`crate::progress`]).
//!
//! The name belongs to the service's analysis future, not to the process: it
//! is a task-local, so two services analysed side by side each read their own
//! (carrick#1066). The four prompt-lambda clients are built far from the loop,
//! with no handle on it, which is why it is ambient rather than passed.
//!
//! A task-local follows what its future awaits and nothing it spawns, and not a
//! thread. The tasks and threads a service starts are therefore started
//! through [`spawn`] or [`Carried`], which re-establish the scope where they
//! run; a spawn that skipped them would bill its calls to nobody.
//!
//! The contract is **present while that service's analysis runs and absent
//! otherwise**: the cross-repo phase and the upload belong to no service, and
//! a service that fails mid-loop must not leak its name into what follows.
//! That is what [`scope`] gives: the name ends when the future it wraps ends,
//! including on the `?` that carries an error out of the middle of it.
//!
//! One kind of call is made for a service the loop has not reached: its
//! detection and guidance, asked ahead of its analysis (carrick#1895). Those
//! run on a task of their own inside [`asked_for`], which names the service
//! for every call that task makes, whatever the loop is analysing meanwhile.

use std::future::Future;

tokio::task_local! {
    /// The service whose analysis the running task belongs to. `None` inside
    /// the scope is an unnamed service, the single-service case.
    static SERVICE: Option<String>;
    /// The service the running task is asking for, when that is not the one
    /// the loop is analysing. See [`asked_for`].
    static ASKED_FOR: Option<String>;
}

/// The name of the service a call is made for: the one the running task was
/// started for ([`asked_for`]), otherwise the one being analysed. `None` when
/// no analysis is in scope (the cross-repo phase, the upload) or when the
/// service is unnamed.
pub fn name() -> Option<String> {
    match ASKED_FOR.try_with(Clone::clone) {
        Ok(asked_for) => asked_for,
        Err(_) => SERVICE.try_with(Clone::clone).unwrap_or(None),
    }
}

/// Whether the running task is asking for a service ahead of its analysis
/// ([`asked_for`]), and for which. The outer `None` is "no": the call belongs
/// to whatever the loop is analysing.
pub fn asked_ahead() -> Option<Option<String>> {
    ASKED_FOR.try_with(Clone::clone).ok()
}

/// Whom the running task works for: a service asked for ahead of its analysis,
/// else the service being analysed. The outer `None` is "no service at all",
/// which an unnamed service (`Some(None)`) is not.
pub fn working_for() -> Option<Option<String>> {
    asked_ahead().or_else(|| SERVICE.try_with(Clone::clone).ok())
}

/// Run `work` as `service`'s: every call it makes names `service`, and none
/// names the service the loop is analysing while it runs. `None` is an
/// unnamed service, which names nobody, as in [`scope`].
///
/// The name follows the task, not the process, so it covers what `work`
/// awaits and nothing it spawns.
pub async fn asked_for<Work: Future>(service: Option<String>, work: Work) -> Work::Output {
    ASKED_FOR.scope(service, work).await
}

/// Run `work` as the analysis of `service`, until it ends. `None` — a single
/// unnamed service — scopes an analysis that reports no name, which is the
/// same fact on the cloud side as no header at all.
pub async fn scope<Work: Future>(service: Option<String>, work: Work) -> Work::Output {
    SERVICE.scope(service, work).await
}

/// [`scope`] around a closure, for work that is not a future.
pub fn sync_scope<R>(service: Option<String>, work: impl FnOnce() -> R) -> R {
    SERVICE.sync_scope(service, work)
}

/// Whether a service's analysis is running, named or unnamed. `false` in the
/// cross-repo phase and the upload, which belong to no service.
pub fn in_scope() -> bool {
    SERVICE.try_with(|_| ()).is_ok()
}

/// The scope the running task is in, taken to be re-established where a task
/// or thread it starts runs. The log span it runs in goes with it, so a line
/// the started work logs names the service as its starter's do.
#[derive(Debug, Clone)]
pub struct Carried {
    service: Option<Option<String>>,
    asked_for: Option<Option<String>>,
    span: tracing::Span,
}

impl Carried {
    pub fn capture() -> Self {
        Self {
            service: SERVICE.try_with(Clone::clone).ok(),
            asked_for: asked_ahead(),
            span: tracing::Span::current(),
        }
    }

    /// Run `work` in the captured scope.
    pub async fn future<Work: Future>(self, work: Work) -> Work::Output {
        use tracing::Instrument;
        let work = work.instrument(self.span);
        match (self.service, self.asked_for) {
            (Some(service), Some(asked)) => {
                SERVICE.scope(service, ASKED_FOR.scope(asked, work)).await
            }
            (Some(service), None) => SERVICE.scope(service, work).await,
            (None, Some(asked)) => ASKED_FOR.scope(asked, work).await,
            (None, None) => work.await,
        }
    }

    /// Run `work` in the captured scope, on whatever thread calls this.
    pub fn run<R>(&self, work: impl FnOnce() -> R) -> R {
        let _entered = self.span.enter();
        let inner = || match &self.asked_for {
            Some(asked) => ASKED_FOR.sync_scope(asked.clone(), work),
            None => work(),
        };
        match &self.service {
            Some(service) => SERVICE.sync_scope(service.clone(), inner),
            None => inner(),
        }
    }
}

/// [`tokio::spawn`] for a task that works for the service that starts it: the
/// task runs in the scope its starter is in.
pub fn spawn<Work>(work: Work) -> tokio::task::JoinHandle<Work::Output>
where
    Work: Future + Send + 'static,
    Work::Output: Send + 'static,
{
    tokio::spawn(Carried::capture().future(work))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Nothing is in scope before the loop starts or after it ends: the
    /// cross-repo phase and the upload belong to no service.
    #[test]
    fn nothing_is_in_scope_by_default() {
        assert_eq!(name(), None);
        assert!(!in_scope());
        assert_eq!(working_for(), None);
    }

    /// The loop's shape, two services deep: each one's name is current for
    /// its own analysis and for nothing either side of it.
    #[test]
    fn each_service_is_current_only_for_its_own_analysis() {
        assert!(!in_scope());
        sync_scope(Some("api".to_string()), || {
            assert_eq!(name().as_deref(), Some("api"));
            assert!(in_scope());
        });
        assert_eq!(name(), None, "the gap between two services names neither");
        assert!(!in_scope());
        sync_scope(Some("web".to_string()), || {
            assert_eq!(name().as_deref(), Some("web"));
        });
        assert_eq!(name(), None, "the cross-repo phase names no service");
        assert!(!in_scope(), "the cross-repo phase is no service's analysis");
    }

    /// Two services' futures joined on one runtime, awaits interleaving: each
    /// reads its own name after every yield (carrick#1066).
    #[tokio::test]
    async fn two_services_side_by_side_each_read_their_own_name() {
        async fn read_across_yields(name_in_scope: &'static str) -> Vec<Option<String>> {
            let mut seen = Vec::new();
            for _ in 0..3 {
                tokio::task::yield_now().await;
                assert_eq!(name().as_deref(), Some(name_in_scope));
                seen.push(name());
            }
            seen
        }
        let (api, web) = tokio::join!(
            scope(Some("api".to_string()), read_across_yields("api")),
            scope(Some("web".to_string()), read_across_yields("web")),
        );
        assert!(api.iter().all(|n| n.as_deref() == Some("api")));
        assert!(web.iter().all(|n| n.as_deref() == Some("web")));
        assert_eq!(name(), None, "neither name outlives its future");
    }

    /// A task a service starts through [`spawn`] works for that service; one
    /// started with `tokio::spawn` is outside every scope, which is why the
    /// engine never uses it for a service's own work.
    #[tokio::test]
    async fn a_task_a_service_spawns_works_for_that_service() {
        let (api, web) = tokio::join!(
            scope(Some("api".to_string()), async {
                let task = spawn(async {
                    tokio::task::yield_now().await;
                    (name(), in_scope())
                });
                let bare = tokio::spawn(async { (name(), in_scope()) });
                (task.await.unwrap(), bare.await.unwrap())
            }),
            scope(Some("web".to_string()), async {
                spawn(async {
                    tokio::task::yield_now().await;
                    name()
                })
                .await
                .unwrap()
            }),
        );
        assert_eq!(api.0, (Some("api".to_string()), true));
        assert_eq!(api.1, (None, false), "a bare spawn is outside the scope");
        assert_eq!(web.as_deref(), Some("web"));
    }

    /// A thread a service starts runs in the scope it is handed, and the
    /// handed scope ends with the closure.
    #[test]
    fn a_thread_runs_in_the_scope_it_was_carried_into() {
        let carried = sync_scope(Some("api".to_string()), Carried::capture);
        assert_eq!(name(), None);
        let seen = std::thread::scope(|threads| {
            threads
                .spawn(|| carried.run(|| (name(), in_scope())))
                .join()
                .unwrap()
        });
        assert_eq!(seen, (Some("api".to_string()), true));
        let bare = std::thread::scope(|threads| threads.spawn(name).join().unwrap());
        assert_eq!(bare, None);
    }

    /// A service's detection and guidance are asked while the loop analyses
    /// another service (carrick#1895). Each call names the service it is for,
    /// on its own task, and the loop's name is untouched beside it.
    #[tokio::test]
    async fn a_task_asking_ahead_names_its_own_service_and_not_the_loops() {
        scope(Some("api".to_string()), async {
            let ahead = tokio::spawn(asked_for(Some("web".to_string()), async {
                tokio::task::yield_now().await;
                (name(), asked_ahead(), working_for())
            }));
            let unnamed = tokio::spawn(asked_for(None, async {
                (name(), asked_ahead(), working_for())
            }));

            assert_eq!(
                ahead.await.unwrap(),
                (
                    Some("web".to_string()),
                    Some(Some("web".to_string())),
                    Some(Some("web".to_string()))
                )
            );
            assert_eq!(
                unnamed.await.unwrap(),
                (None, Some(None), Some(None)),
                "an unnamed service asked ahead names nobody, not the loop's service"
            );
            assert_eq!(name().as_deref(), Some("api"));
            assert_eq!(asked_ahead(), None, "the loop itself asks ahead for nobody");
        })
        .await;
    }

    /// An unnamed service — the single-service case — is in scope but has no
    /// name to report, so it sends no header rather than the `(root)` the log
    /// lines use.
    #[test]
    fn an_unnamed_service_reports_no_name() {
        sync_scope(None, || {
            assert_eq!(name(), None);
            assert!(in_scope(), "its analysis is running all the same");
            assert_eq!(working_for(), Some(None));
        });
    }

    /// A service whose analysis fails carries its error out through `?`,
    /// which ends the scope on the way. The service after it, and the
    /// cross-repo phase, must not be billed the failed one's name.
    #[test]
    fn an_error_mid_analysis_leaves_nothing_in_scope() {
        fn analyse_and_fail() -> Result<(), String> {
            sync_scope(Some("api".to_string()), || {
                assert_eq!(name().as_deref(), Some("api"));
                Err("manifest unreadable".to_string())?;
                unreachable!("the `?` above leaves the closure")
            })
        }

        assert!(analyse_and_fail().is_err());
        assert_eq!(name(), None);
    }
}
