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
//! - **a hole**, with its reason ([`InstanceHole`]).

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use serde::Serialize;

use super::super::{
    CallIr, ClientRef, HeldInstance, MadeBy, MakerForm, Piece, RequestSummaryInputs,
};
use super::{Calls, Placed, filled, holds_parameter, relative, to_definition};
use crate::position_rows::{FLAG_ANY, Kind};

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
    /// to it as an argument, or written as one): R2b's.
    RunsByLibrary,
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
    /// The hole reason, or `None` for a root.
    pub fn hole(&self) -> Option<InstanceHole> {
        match &self.end {
            InstanceAt::Hole { reason, .. } => Some(*reason),
            InstanceAt::Root { .. } => None,
        }
    }
}

/// One site's instance walk.
pub(super) struct InstanceWalk<'w, 'a> {
    pub(super) calls: &'w Calls<'a>,
    pub(super) inputs: &'w RequestSummaryInputs,
    pub(super) root: &'w Path,
    pub(super) makers: &'w [MakerLabel],
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
    ) -> Self {
        InstanceWalk {
            calls,
            inputs,
            root,
            makers,
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
    pub(super) fn of(mut self, down: &[Placed<'a>]) -> Vec<InstanceEnd> {
        let last = down[down.len() - 1];
        let instance = &last.call.instance;
        let Some(value) = &instance.receiver_value else {
            if let Some(held) = &instance.receiver_held {
                let at = self.at(&last);
                self.held(held, last.file, at);
            }
            return self.ends;
        };
        let mut pieces = value.pieces();
        if !holds_parameter(&pieces) {
            let at = self.at(&last);
            match &instance.receiver_held {
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
                self.callback(&below);
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
            self.callback(&placed);
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
        if callers.is_empty() {
            let reason = if self.handed_to_library(placed.file, key) {
                InstanceHole::RunsByLibrary
            } else {
                InstanceHole::EntryPoint
            };
            let at = self.at(&placed);
            self.hole(at, reason, None);
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
    fn callback(&mut self, placed: &Placed<'_>) {
        let reason = match placed.host {
            Some(host) if self.library_call(placed.file, host) => InstanceHole::RunsByLibrary,
            _ => InstanceHole::CallbackParameter,
        };
        let at = self.at(placed);
        self.hole(at, reason, None);
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
                let labelled = self.makers.iter().any(|maker| {
                    maker.package == package
                        && maker.export == client.export
                        && maker.member == instance.member
                        && maker.new == (instance.form == MakerForm::New)
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
                        },
                    );
                } else {
                    self.hole(at, InstanceHole::Unread, Some(client.package.clone()));
                }
            }
            MadeBy::Call(_) => {
                let target = self
                    .calls
                    .at_span(file, instance.site.span_start, instance.site.span_end)
                    .and_then(|placed| self.calls.target(&placed))
                    .cloned();
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
            .and_then(|(_, name)| declared.module_clients.get(name))
            .cloned();
        match client {
            Some(client) => self.client(&client, &declaring, at),
            None => self.hole(at, InstanceHole::Unread, Some(symbol.to_string())),
        }
    }

    /// Whether `call`, written in `file`, is a library call: its callee is
    /// declared outside the repository, as the compiler names it.
    fn library_call(&self, file: &Path, call: &CallIr) -> bool {
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
            .is_ok_and(|row| row.decl_file().is_some() && !super::super::discovery::in_repo(row))
    }

    /// Whether a library call in `file` is handed the function `key` by
    /// name (`server.register(plugin)`).
    fn handed_to_library(&self, file: &Path, key: &str) -> bool {
        self.calls.in_file(file).any(|placed| {
            placed
                .call
                .handed
                .iter()
                .any(|(_, keys, name)| keys.is_empty() && name == key)
                && self.library_call(file, placed.call)
        })
    }
}

/// The text of a value that is one opaque piece, as written.
fn opaque_text(pieces: &[Piece]) -> Option<String> {
    match pieces {
        [Piece::Opaque { text, .. }] => Some(text.clone()),
        _ => None,
    }
}
