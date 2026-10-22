//! The instance a decided site's labelled call is made on, per deciding
//! caller (carrick#2474 R2a). Record-only, with the receiver trace on.
//!
//! The receiver is read as the body that writes the labelled call reads it
//! ([`super::super::InstanceIr`]): a parameter, a key of one, or an
//! enclosing function's, is filled at each call above it exactly as the
//! name slot is ([`super::filled`], [`super::to_definition`]): first down
//! the decided site's own chain, then up every caller of the function that
//! still holds it ([`super::Calls::callers_of`]). There is no agreement
//! rule: each caller that hands an instance in gives its own end, so a
//! helper handed two apps serves both. A definition walked once is not
//! walked again for the same site, which ends a cycle; no hop limit.
//!
//! Each end is one of:
//!
//! - **a root**: the result of a maker the store labels (`make()`,
//!   `new Server()`), at the maker call's `file:line`, read where the
//!   syntax holds it ([`super::super::HeldInstance`]): a binding the
//!   library-site reader holds an instance in, a maker or own factory call
//!   (followed into what the factory returns), or an import of this
//!   service's module, whose declaration the compiler names;
//! - **a scope** a labelled member opens (carrick#2474 R2b): the parameter
//!   of a function a member labelled with `runs` and `yields` runs with a
//!   scoped instance (`app.register(plugin, { prefix })`), under the prefix
//!   its `name` slot states, whose own instance is the member's receiver,
//!   walked the same way ([`InstanceAt::Scope`]);
//! - **a hole**, with its reason ([`InstanceHole`]).
//!
//! An instance handed to a member labelled with `mounts` is served under
//! that member's prefix: those edges are read once per service and joined
//! into each site's composed path ([`super::composed`]).

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use serde::Serialize;

use super::super::{
    CallIr, ClientRef, HeldInstance, MadeBy, MakerForm, Piece, RequestSummaryInputs, Value,
};
use super::{Calls, Placed, filled, holds_parameter, relative, to_definition};
use crate::position_rows::{FLAG_ANY, FLAG_DECLARATION_UNNAMED, Kind};

/// A maker the store labels: the package (as the store keys it), the
/// export, the member (`None`: the export itself) and whether it is
/// constructed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MakerLabel {
    pub package: String,
    pub export: String,
    pub member: Option<String>,
    pub new: bool,
}

impl MakerLabel {
    /// Every maker the settled claims label.
    pub fn from_exports(exports: &[crate::library_claims::ExportClaims]) -> Vec<MakerLabel> {
        use crate::services::type_sidecar::{LibraryClaim, MakeForm};
        let mut makers = Vec::new();
        for export in exports {
            for claim in &export.claims {
                if let LibraryClaim::Make { form, member, .. } = claim {
                    let maker = MakerLabel {
                        package: export.package.clone(),
                        export: export.export.clone(),
                        member: member.clone(),
                        new: *form == MakeForm::New,
                    };
                    if !makers.contains(&maker) {
                        makers.push(maker);
                    }
                }
            }
        }
        makers
    }
}

/// What a scope member binds (carrick#2474 R0's fields).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ScopeBinds {
    /// It runs the function at `arg` (`key`) and hands it the scoped
    /// instance at its parameter `param`.
    Runs {
        arg: usize,
        key: Option<String>,
        param: usize,
    },
    /// The instance at `arg` (`key`) is served under the prefix.
    Mounts { arg: usize, key: Option<String> },
    /// It returns the scoped instance.
    Returns,
}

/// A scope member the store labels (carrick#2474 R2b): the package (as the
/// store keys it), the export, the member, the receivers it applies to, the
/// slot its prefix sits in, and what it binds.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScopeLabel {
    pub package: String,
    pub export: String,
    pub member: String,
    on: Option<crate::services::type_sidecar::ClaimOn>,
    of: Option<String>,
    /// The receiver ids the verifier verified the claim on; `None` where
    /// nothing was verified (a census reading the store's answer as is).
    verified_on: Option<Vec<String>>,
    pub name: (usize, Option<String>),
    pub binds: ScopeBinds,
}

impl ScopeLabel {
    /// Every scope claim of `exports` whose member is one path hop off the
    /// receiver. With `verified`, only the receivers each was verified on
    /// (the trace's own request, [`super::super::discovery::declaration_ask`]).
    pub fn from_exports(
        exports: &[crate::library_claims::ExportClaims],
        verified: Option<&HashSet<super::super::discovery::AskedScope>>,
    ) -> Vec<ScopeLabel> {
        use crate::services::type_sidecar::LibraryClaim;
        let mut labels = Vec::new();
        for export in exports {
            for claim in &export.claims {
                let LibraryClaim::Scope {
                    member,
                    name,
                    path,
                    on,
                    of,
                    runs,
                    yields,
                    mounts,
                    ..
                } = claim
                else {
                    continue;
                };
                if !path.is_empty() {
                    continue;
                }
                let binds = match (runs, yields, mounts) {
                    (Some(runs), Some(yields), None) => ScopeBinds::Runs {
                        arg: runs.arg as usize,
                        key: runs.key.clone(),
                        param: yields.param as usize,
                    },
                    (None, None, Some(mounts)) => ScopeBinds::Mounts {
                        arg: mounts.arg as usize,
                        key: mounts.key.clone(),
                    },
                    (None, None, None) => ScopeBinds::Returns,
                    _ => continue,
                };
                let verified_on = verified.map(|verified| {
                    let mut on: Vec<String> = verified
                        .iter()
                        .filter(|asked| {
                            asked.package == export.package
                                && asked.export == export.export
                                && asked.member == *member
                        })
                        .map(|asked| asked.receiver.clone())
                        .collect();
                    on.sort();
                    on
                });
                if verified_on.as_ref().is_some_and(Vec::is_empty) {
                    continue;
                }
                let label = ScopeLabel {
                    package: export.package.clone(),
                    export: export.export.clone(),
                    member: member.clone(),
                    on: *on,
                    of: of.clone(),
                    verified_on,
                    name: (name.arg as usize, name.key.clone()),
                    binds,
                };
                if !labels.contains(&label) {
                    labels.push(label);
                }
            }
        }
        labels
    }

    /// Whether the claim applies to an instance of `package`'s `export`
    /// read through `receiver`, and was verified there.
    fn applies(&self, package: &str, export: &str, receiver: &str) -> bool {
        self.package == package
            && self.export == export
            && crate::library_claims::applies(self.on, self.of.as_deref(), receiver)
            && self
                .verified_on
                .as_ref()
                .is_none_or(|on| on.iter().any(|at| at == receiver))
    }

    /// Whether the claim applies to the instance `end` ends at.
    pub(super) fn applies_to(&self, end: &InstanceEnd) -> bool {
        match &end.end {
            InstanceAt::Root {
                package,
                export,
                receiver,
                ..
            }
            | InstanceAt::Scope {
                package,
                export,
                receiver,
                ..
            } => self.applies(package, export, receiver),
            InstanceAt::Hole { .. } => false,
        }
    }
}

/// The prefix a scope or mount call states in its label's `name` slot
/// (carrick#2474 R2b).
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize)]
#[serde(tag = "prefix", rename_all = "snake_case")]
pub enum Prefix {
    /// The call writes nothing there: it binds no prefix.
    Unstated,
    /// Every value the call states. All are recorded (#2474 Q2).
    Stated { values: Vec<String> },
    /// A value this pass does not read as text, as written where one
    /// expression is.
    Unread {
        #[serde(skip_serializing_if = "Option::is_none")]
        text: Option<String>,
    },
}

/// Why an instance walk ends at no maker.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum InstanceHole {
    /// The function that holds the receiver is called by nothing in the
    /// service, and handed to no library call.
    EntryPoint,
    /// The receiver is a parameter of a function written as an argument
    /// of a call this service makes to its own code, or one written
    /// anywhere else: what fills it is whatever runs it.
    CallbackParameter,
    /// The value handed in is typed `any`.
    UntypedReceiver,
    /// The value handed in is declared outside the service's files: a
    /// package, or a file the scan does not read.
    OutsideService,
    /// A library call runs the function that holds the receiver (handed
    /// to it as an argument, or written as one), and no scope label that
    /// applies to its receiver runs it there.
    RunsByLibrary,
    /// The function that holds the receiver is written under a key of an
    /// object literal handed to a call (`{ extra: (app) => … }`): what calls
    /// it is the function that reads that key, which waits on
    /// carrick#2432's injected-callee step.
    CallbackOption,
    /// A value the walk does not read: a field, a call's value that is no
    /// maker's and no own factory's, a maker the store does not label, a
    /// conditional.
    Unread,
}

/// Where one instance walk ends.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize)]
#[serde(tag = "end", rename_all = "snake_case")]
pub enum InstanceAt {
    /// A labelled maker's result, at the maker call.
    Root {
        file: String,
        line: u32,
        package: String,
        export: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        member: Option<String>,
        /// The receiver id its instances are read through
        /// (`instance:()`, `instance:new`).
        receiver: String,
    },
    /// A scope a labelled member opens (carrick#2474 R2b), at the member's
    /// call: the scoped instance is of the same export as the member's
    /// receiver, read through the same receiver id, and served under
    /// `prefix` below each of `parents`, the member's receiver's ends.
    Scope {
        file: String,
        line: u32,
        member: String,
        #[serde(flatten)]
        prefix: Prefix,
        package: String,
        export: String,
        receiver: String,
        parents: Vec<InstanceEnd>,
    },
    Hole {
        reason: InstanceHole,
        /// What the walk stopped at, as written, where one expression is.
        #[serde(skip_serializing_if = "Option::is_none")]
        text: Option<String>,
    },
}

/// One instance end of a decided site: the call that hands the instance
/// in (`file:line`, the deciding caller), or where the walk stopped, and
/// what it ends at.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize)]
pub struct InstanceEnd {
    pub at: String,
    #[serde(flatten)]
    pub end: InstanceAt,
}

impl InstanceEnd {
    /// The hole reason, or `None` for a root or a scope.
    pub fn hole(&self) -> Option<InstanceHole> {
        match &self.end {
            InstanceAt::Hole { reason, .. } => Some(*reason),
            InstanceAt::Root { .. } | InstanceAt::Scope { .. } => None,
        }
    }
}

/// One site's instance walk.
pub(super) struct InstanceWalk<'w, 'a> {
    pub(super) calls: &'w Calls<'a>,
    pub(super) inputs: &'w RequestSummaryInputs,
    pub(super) root: &'w Path,
    pub(super) makers: &'w [MakerLabel],
    pub(super) scopes: &'w [ScopeLabel],
    /// The scope calls whose receiver is being walked, outermost first: a
    /// scope call reached again inside its own walk ends it (a plugin that
    /// registers itself).
    hosts: Vec<(PathBuf, u32, u32)>,
    ends: Vec<InstanceEnd>,
    /// The definitions walked, with the receiver as their own body reads
    /// it: a definition reached again adds nothing (a cycle, or a second
    /// path to it).
    walked: HashSet<(PathBuf, String, Vec<Piece>)>,
    /// The own factories being followed, so a factory that returns its own
    /// call ends. Two calls of one factory each give their end.
    factories: HashSet<(PathBuf, String)>,
}

impl<'w, 'a> InstanceWalk<'w, 'a> {
    pub(super) fn new(
        calls: &'w Calls<'a>,
        inputs: &'w RequestSummaryInputs,
        root: &'w Path,
        makers: &'w [MakerLabel],
        scopes: &'w [ScopeLabel],
    ) -> Self {
        InstanceWalk {
            calls,
            inputs,
            root,
            makers,
            scopes,
            hosts: Vec::new(),
            ends: Vec::new(),
            walked: HashSet::new(),
            factories: HashSet::new(),
        }
    }

    fn at(&self, placed: &Placed<'_>) -> String {
        format!(
            "{}:{}",
            relative(self.root, placed.file),
            placed.call.site.line
        )
    }

    fn push(&mut self, at: String, end: InstanceAt) {
        let end = InstanceEnd { at, end };
        if !self.ends.contains(&end) {
            self.ends.push(end);
        }
    }

    fn hole(&mut self, at: String, reason: InstanceHole, text: Option<String>) {
        self.push(at, InstanceAt::Hole { reason, text });
    }

    /// The instance ends of the site whose calls, from the deciding call
    /// (first) to the labelled call (last), are `down`. Empty when the
    /// labelled call is made on no receiver the syntax names (a bare
    /// call).
    pub(super) fn of(self, down: &[Placed<'a>]) -> Vec<InstanceEnd> {
        let last = down[down.len() - 1];
        let instance = &last.call.instance;
        self.of_value(
            down,
            instance.receiver_value.as_ref(),
            instance.receiver_held.as_ref(),
        )
    }

    /// The instance ends of what `placed`'s call is handed at `arg`
    /// (`key`): what a mount member serves (carrick#2474 R2b).
    pub(super) fn of_arg(
        self,
        placed: Placed<'a>,
        arg: usize,
        key: Option<&str>,
    ) -> Vec<InstanceEnd> {
        let value = placed.call.args.get(arg);
        let held = placed.call.instance.args.get(arg);
        let (value, held) = match key {
            None => (value, held.and_then(|held| held.whole.as_ref())),
            Some(key) => (
                match value {
                    Some(Value::Obj(object)) => object.fields.get(key),
                    _ => None,
                },
                held.and_then(|held| held.keys.get(key)),
            ),
        };
        self.of_value(&[placed], value, held)
    }

    fn of_value(
        mut self,
        down: &[Placed<'a>],
        value: Option<&Value>,
        held: Option<&HeldInstance>,
    ) -> Vec<InstanceEnd> {
        let last = down[down.len() - 1];
        let Some(value) = value else {
            if let Some(held) = held {
                let at = self.at(&last);
                self.held(held, last.file, at);
            }
            return self.ends;
        };
        let mut pieces = value.pieces();
        if !holds_parameter(&pieces) {
            let at = self.at(&last);
            match held {
                Some(held) => self.held(held, last.file, at),
                None => self.hole(at, InstanceHole::Unread, opaque_text(&pieces)),
            }
            return self.ends;
        }
        // Down the site's own chain first: the deciding caller's value is
        // the one this site was decided by.
        for k in (0..down.len() - 1).rev() {
            let below = down[k + 1];
            let Some(own) = to_definition(&pieces, below.depth) else {
                self.callback(&below, &pieces);
                return self.ends;
            };
            let caller = down[k];
            pieces = filled(&own, caller.call);
            if !holds_parameter(&pieces) {
                self.handed(&caller, &own, &pieces);
                return self.ends;
            }
        }
        self.up(down[0], pieces);
        self.ends
    }

    /// Walk up every caller of the definition `placed` is written in,
    /// whose receiver, as `placed`'s body reads it, is `pieces`.
    fn up(&mut self, placed: Placed<'a>, pieces: Vec<Piece>) {
        let Some(own) = to_definition(&pieces, placed.depth) else {
            self.callback(&placed, &pieces);
            return;
        };
        let Some(key) = placed.key else {
            let at = self.at(&placed);
            self.hole(at, InstanceHole::Unread, None);
            return;
        };
        if !self
            .walked
            .insert((placed.file.to_path_buf(), key.to_string(), own.clone()))
        {
            return;
        }
        let callers = self.calls.callers_of(placed.file, key);
        // A library member may run the function as well as code calling
        // it: each scope a labelled member opens for it is an end too
        // (carrick#2474 R2b).
        let mut hosts = self.handed_to_library(placed.file, key);
        hosts.extend(self.handed_by_declaration(placed.file, key));
        let at = self.at(&placed);
        let param = match own.as_slice() {
            [Piece::Param(index, _)] => Some(*index),
            _ => None,
        };
        let mut scoped = false;
        for (host, arg) in &hosts {
            scoped |= self.scope(host, *arg, param, at.clone());
        }
        if callers.is_empty() {
            match (hosts.is_empty(), scoped) {
                (true, _) => self.hole(at, InstanceHole::EntryPoint, None),
                (false, false) => self.hole(at, InstanceHole::RunsByLibrary, None),
                (false, true) => {}
            }
            return;
        }
        for caller in callers {
            let value = filled(&own, caller.call);
            if holds_parameter(&value) {
                self.up(*caller, value);
            } else {
                self.handed(caller, &own, &value);
            }
        }
    }

    /// The receiver is a parameter of the function `placed` is written in,
    /// several functions in from its definition: a callback. A library
    /// call that runs it is R2b's; anything else is a callback parameter.
    ///
    /// A function written as a scope member's `runs` argument opens a scope
    /// (carrick#2474 R2b); one written under a key of an object literal is
    /// a callback option.
    fn callback(&mut self, placed: &Placed<'a>, pieces: &[Piece]) {
        let at = self.at(placed);
        let written = placed.written_in;
        if written.is_some_and(|written| !written.path.is_empty()) {
            self.hole(at, InstanceHole::CallbackOption, None);
            return;
        }
        let reason = match placed.host {
            Some(host) if self.library_call(placed.file, host) => {
                // The callback's own parameter, read in its own body.
                let param = match pieces {
                    [Piece::Param(index, _)] => Some(*index),
                    _ => None,
                };
                let host =
                    self.calls
                        .at_span(placed.file, host.site.span_start, host.site.span_end);
                if let (Some(host), Some(written)) = (host, written)
                    && self.scope(&host, written.arg, param, at.clone())
                {
                    return;
                }
                InstanceHole::RunsByLibrary
            }
            _ => InstanceHole::CallbackParameter,
        };
        self.hole(at, reason, None);
    }

    /// The scope a labelled member opens at `host` for the function it is
    /// handed at `arg`, whose parameter `param` holds the instance: a scope
    /// end under each of the host's receiver's ends a `runs` label applies
    /// to. False where no label applies.
    fn scope(&mut self, host: &Placed<'a>, arg: usize, param: Option<usize>, at: String) -> bool {
        let Some(param) = param else {
            return false;
        };
        let labels: Vec<&'w ScopeLabel> = self
            .scopes
            .iter()
            .filter(|label| {
                label.member == host.call.callee
                    && matches!(&label.binds, ScopeBinds::Runs { arg: runs, key: None, param: yields }
                        if *runs == arg && *yields == param)
            })
            .collect();
        self.open(host, labels, at)
    }

    /// The value of the call at `span` in `file`: the scope a member a
    /// `returns` label names opens (`app.basePath("/v2")`), or a hole.
    fn returned(&mut self, file: &Path, span: (u32, u32), at: String) {
        let host = self.inputs.files.get(file).and_then(|ir| {
            self.calls
                .at_span(file, ir.swc_start(span.0), ir.swc_start(span.1))
        });
        let Some(host) = host else {
            self.hole(at, InstanceHole::Unread, None);
            return;
        };
        self.returned_at(host, at);
    }

    /// The value of `host`'s call: the scope a member a `returns` label
    /// names opens there, or a hole.
    fn returned_at(&mut self, host: Placed<'a>, at: String) {
        let labels: Vec<&'w ScopeLabel> = self
            .scopes
            .iter()
            .filter(|label| label.member == host.call.callee && label.binds == ScopeBinds::Returns)
            .collect();
        if !self.library_call(host.file, host.call) || !self.open(&host, labels, at.clone()) {
            self.hole(
                at,
                InstanceHole::Unread,
                Some(host.call.callee_text.clone()),
            );
        }
    }

    /// A scope end under each of `host`'s receiver's ends one of `labels`
    /// applies to. False where none applies.
    fn open(&mut self, host: &Placed<'a>, labels: Vec<&'w ScopeLabel>, at: String) -> bool {
        if labels.is_empty() {
            return false;
        }
        let here = (
            host.file.to_path_buf(),
            host.call.site.span_start,
            host.call.site.span_end,
        );
        if self.hosts.contains(&here) {
            return false;
        }
        let parents = self.nested().with_host(here).of(&[*host]);
        // Where every end above is a hole (a plugin's own app, handed in by
        // whatever runs it), the member's declaration being the label's
        // package's says which scope opens; the path below is relative to
        // those holes.
        let unknown = !parents.is_empty() && parents.iter().all(|end| end.hole().is_some());
        let mut opened = false;
        for label in labels {
            let applies: Vec<InstanceEnd> = parents
                .iter()
                .filter(|end| label.applies_to(end))
                .cloned()
                .collect();
            let (identity, applies) = match applies.first().and_then(identity) {
                Some(identity) => (identity, applies),
                None if unknown && self.declared_in(host, &label.package) => (
                    (
                        label.package.clone(),
                        label.export.clone(),
                        label
                            .of
                            .clone()
                            .unwrap_or_else(|| "instance:()".to_string()),
                    ),
                    parents.clone(),
                ),
                None => continue,
            };
            let (package, export, receiver) = identity;
            self.push(
                at.clone(),
                InstanceAt::Scope {
                    file: relative(self.root, host.file),
                    line: host.call.site.line,
                    member: label.member.clone(),
                    prefix: prefix_of(host, &label.name),
                    package,
                    export,
                    receiver,
                    parents: applies,
                },
            );
            opened = true;
        }
        opened
    }

    /// A fresh walk for another value, inside this one's scope calls.
    pub(super) fn nested(&self) -> InstanceWalk<'w, 'a> {
        let mut walk =
            InstanceWalk::new(self.calls, self.inputs, self.root, self.makers, self.scopes);
        walk.hosts = self.hosts.clone();
        walk
    }

    pub(super) fn with_host(mut self, host: (PathBuf, u32, u32)) -> Self {
        self.hosts.push(host);
        self
    }

    /// The caller `caller` hands in `value`, its argument at the parameter
    /// (or key of one) `own` reads: the instance that argument holds.
    fn handed(&mut self, caller: &Placed<'_>, own: &[Piece], value: &[Piece]) {
        let at = self.at(caller);
        let args = &caller.call.instance.args;
        let held = match own {
            [Piece::Param(index, _)] => args.get(*index).and_then(|arg| arg.whole.as_ref()),
            [Piece::ParamKey(index, key, _)] => args.get(*index).and_then(|arg| arg.keys.get(key)),
            _ => None,
        };
        match held {
            Some(held) => self.held(held, caller.file, at),
            None => self.hole(at, InstanceHole::Unread, opaque_text(value)),
        }
    }

    /// What `held`, read in `file`, ends at.
    fn held(&mut self, held: &HeldInstance, file: &Path, at: String) {
        match held {
            HeldInstance::Client(client) => self.client(client, file, at),
            HeldInstance::Imported(lo, hi) => self.imported(file, (*lo, *hi), at),
            HeldInstance::Returned(lo, hi) => self.returned(file, (*lo, *hi), at),
        }
    }

    /// The maker of `client`, an instance read in `file`: a labelled maker
    /// is a root; an own factory call is followed into what the factory
    /// returns.
    fn client(&mut self, client: &ClientRef, file: &Path, at: String) {
        let Some(instance) = &client.instance else {
            self.hole(at, InstanceHole::Unread, None);
            return;
        };
        match &instance.made_by {
            MadeBy::Export => {
                let package = super::super::package_name(&client.package).to_string();
                let new = instance.form == MakerForm::New;
                let labelled = self.makers.iter().any(|maker| {
                    maker.package == package
                        && maker.export == client.export
                        && maker.member == instance.member
                        && maker.new == new
                });
                if labelled {
                    self.push(
                        at,
                        InstanceAt::Root {
                            file: relative(self.root, file),
                            line: instance.site.line,
                            package,
                            export: client.export.clone(),
                            member: instance.member.clone(),
                            receiver: super::super::library_sites::maker_receiver_id(
                                new,
                                instance.member.as_deref(),
                            ),
                        },
                    );
                } else {
                    self.hole(at, InstanceHole::Unread, Some(client.package.clone()));
                }
            }
            MadeBy::Call(_) => {
                let placed =
                    self.calls
                        .at_span(file, instance.site.span_start, instance.site.span_end);
                let target = placed
                    .and_then(|placed| self.calls.target(&placed))
                    .cloned();
                // A member call the call graph does not resolve into this
                // service: a scope a labelled member returns, if any
                // (carrick#2474 R2b).
                if let (None, Some(placed)) = (&target, placed) {
                    self.returned_at(placed, at);
                    return;
                }
                let returned = target.as_ref().and_then(|(target_file, key)| {
                    self.inputs
                        .files
                        .get(target_file)?
                        .functions
                        .get(key)?
                        .returned
                        .as_ref()
                });
                let (Some((target_file, key)), Some(returned)) = (target.clone(), returned) else {
                    self.hole(at, InstanceHole::Unread, None);
                    return;
                };
                let here = (target_file.clone(), key);
                if !self.factories.insert(here.clone()) {
                    return;
                }
                for maker in returned.makers.clone() {
                    self.client(&maker, &target_file, at.clone());
                }
                self.factories.remove(&here);
            }
        }
    }

    /// The name an import binds at `span` in `file`: the module-scope
    /// instance the declaration the compiler names holds.
    fn imported(&mut self, file: &Path, span: (u32, u32), at: String) {
        let Some(positions) = self.inputs.positions.as_ref() else {
            self.hole(at, InstanceHole::Unread, None);
            return;
        };
        let Some(ir) = self.inputs.files.get(file) else {
            self.hole(at, InstanceHole::Unread, None);
            return;
        };
        let Ok(row) = positions.lookup(
            file,
            ir.swc_start(span.0),
            ir.swc_start(span.1),
            Kind::Ident,
        ) else {
            self.hole(at, InstanceHole::Unread, None);
            return;
        };
        if row.flags() & FLAG_ANY != 0 || row.type_text() == Some("any") {
            self.hole(
                at,
                InstanceHole::UntypedReceiver,
                row.symbol().map(str::to_string),
            );
            return;
        }
        if !super::super::discovery::in_repo(row) {
            self.hole(
                at,
                InstanceHole::OutsideService,
                row.decl_file().map(str::to_string),
            );
            return;
        }
        let (Some(decl_file), Some(line), Some(symbol)) =
            (row.decl_file(), row.decl_line(), row.symbol())
        else {
            self.hole(at, InstanceHole::Unread, None);
            return;
        };
        let declaring = positions.path_of(decl_file);
        let Some(declared) = self.inputs.files.get(&declaring) else {
            self.hole(
                at,
                InstanceHole::OutsideService,
                Some(decl_file.to_string()),
            );
            return;
        };
        let client = declared
            .declared_at
            .get(symbol)
            .filter(|(starts, _)| *starts == line)
            .and_then(|(_, name)| {
                declared
                    .module_clients
                    .get(name)
                    .or_else(|| declared.shadowed_clients.get(name))
            })
            .cloned();
        match client {
            Some(client) => self.client(&client, &declaring, at),
            None => self.hole(at, InstanceHole::Unread, Some(symbol.to_string())),
        }
    }

    /// Whether `host`'s callee is declared in `package` (as the store keys
    /// it: a `@types/*` package stands for the one it types).
    fn declared_in(&self, host: &Placed<'_>, package: &str) -> bool {
        let (Some(positions), Some(ir)) = (
            self.inputs.positions.as_ref(),
            self.inputs.files.get(host.file),
        ) else {
            return false;
        };
        positions
            .lookup(
                host.file,
                ir.swc_start(host.call.site.lo),
                ir.swc_start(host.call.site.hi),
                Kind::Call,
            )
            .is_ok_and(|row| {
                !super::super::discovery::in_repo(row)
                    && row.decl_package().is_some_and(|declaring| {
                        crate::engine::type_compat_v2::types_package_runtime_name(declaring)
                            == package
                    })
            })
    }

    /// Whether `call`, written in `file`, is a library call: its callee is
    /// declared outside the repository, as the compiler names it.
    pub(super) fn library_call(&self, file: &Path, call: &CallIr) -> bool {
        let (Some(positions), Some(ir)) =
            (self.inputs.positions.as_ref(), self.inputs.files.get(file))
        else {
            return false;
        };
        positions
            .lookup(
                file,
                ir.swc_start(call.site.lo),
                ir.swc_start(call.site.hi),
                Kind::Call,
            )
            .is_ok_and(|row| {
                // A declaration the rows could not name a file for is
                // outside the repository too (carrick#2474 R2b).
                (row.decl_file().is_some() || row.flags() & FLAG_DECLARATION_UNNAMED != 0)
                    && !super::super::discovery::in_repo(row)
            })
    }

    /// The calls of a `runs` scope member in another file handed, as an
    /// argument, a name the compiler resolves to the function `key` of
    /// `file` (`app.register(items)`, `items` imported), each with the
    /// argument (carrick#2474 R2b), in file and source order.
    fn handed_by_declaration(&self, file: &Path, key: &str) -> Vec<(Placed<'a>, usize)> {
        let (Some(positions), Some(declared)) =
            (self.inputs.positions.as_ref(), self.inputs.files.get(file))
        else {
            return Vec::new();
        };
        let Some((line, _)) = declared.declared_at.get(key) else {
            return Vec::new();
        };
        let decl_file = relative(self.root, file);
        let members: HashSet<&str> = self
            .scopes
            .iter()
            .filter(|label| matches!(label.binds, ScopeBinds::Runs { .. }))
            .map(|label| label.member.as_str())
            .collect();
        let mut hosts: Vec<(Placed<'a>, usize)> = Vec::new();
        for placed in self.calls.by_span.values() {
            if placed.file == file || !members.contains(placed.call.callee.as_str()) {
                continue;
            }
            let Some(ir) = self.inputs.files.get(placed.file) else {
                continue;
            };
            for (arg, held) in placed.call.instance.args.iter().enumerate() {
                let Some((lo, hi)) = held.span else {
                    continue;
                };
                let names = positions
                    .lookup(placed.file, ir.swc_start(lo), ir.swc_start(hi), Kind::Ident)
                    .is_ok_and(|row| {
                        row.decl_file() == Some(decl_file.as_str())
                            && row.decl_line() == Some(*line)
                    });
                if names && self.library_call(placed.file, placed.call) {
                    hosts.push((*placed, arg));
                }
            }
        }
        hosts.sort_by(|a, b| {
            (a.0.file, a.0.call.site.span_start).cmp(&(b.0.file, b.0.call.site.span_start))
        });
        hosts
    }

    /// The library calls in `file` handed the function `key` by name
    /// (`server.register(plugin)`), each with the argument it is handed
    /// at, in source order.
    fn handed_to_library(&self, file: &Path, key: &str) -> Vec<(Placed<'a>, usize)> {
        let mut hosts: Vec<(Placed<'a>, usize)> = self
            .calls
            .in_file(file)
            .filter(|placed| self.library_call(file, placed.call))
            .filter_map(|placed| {
                let (arg, ..) = placed
                    .call
                    .handed
                    .iter()
                    .find(|(_, keys, name)| keys.is_empty() && name == key)?;
                Some((*placed, *arg))
            })
            .collect();
        hosts.sort_by_key(|(placed, _)| placed.call.site.span_start);
        hosts
    }
}

/// The package, export and receiver id of the instance `end` ends at.
pub(super) fn identity(end: &InstanceEnd) -> Option<(String, String, String)> {
    match &end.end {
        InstanceAt::Root {
            package,
            export,
            receiver,
            ..
        }
        | InstanceAt::Scope {
            package,
            export,
            receiver,
            ..
        } => Some((package.clone(), export.clone(), receiver.clone())),
        InstanceAt::Hole { .. } => None,
    }
}

/// The prefix `placed`'s call states in the slot `name` (an argument, or a
/// key of an object-literal one): text where every piece is literal.
pub(super) fn prefix_of(placed: &Placed<'_>, name: &(usize, Option<String>)) -> Prefix {
    let (arg, key) = name;
    let value = match (placed.call.args.get(*arg), key) {
        (None, _) => return Prefix::Unstated,
        (Some(value), None) => value,
        (Some(Value::Obj(object)), Some(key)) => match object.fields.get(key) {
            Some(value) => value,
            None if object.open => return Prefix::Unread { text: None },
            None => return Prefix::Unstated,
        },
        (Some(_), Some(_)) => return Prefix::Unread { text: None },
    };
    let pieces = value.pieces();
    let text: Option<String> = pieces
        .iter()
        .map(|piece| match piece {
            Piece::Lit(text) => Some(text.as_str()),
            _ => None,
        })
        .collect();
    match text {
        Some(text) => Prefix::Stated { values: vec![text] },
        None => Prefix::Unread {
            text: opaque_text(&pieces),
        },
    }
}

/// The text of a value that is one opaque piece, as written.
fn opaque_text(pieces: &[Piece]) -> Option<String> {
    match pieces {
        [Piece::Opaque { text, .. }] => Some(text.clone()),
        _ => None,
    }
}
