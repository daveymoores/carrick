//! What a function compares a request value against, and where that value
//! comes from (carrick#2386): the receive side of the request summaries.
//!
//! A service that dispatches by hand states each route it serves as a
//! comparison of something it was handed: `pathname === "/items"`, `case
//! "/health":`, `pathname.startsWith("/items/")`, `ROUTES[pathname]`. Those
//! are the language's own comparison forms, so they are read with no library
//! or framework named. What this pass adds over [`crate::compared_route_literals`]
//! is the chain: the value compared is read as a [`Subject`] (a root and the
//! accesses down from it), and a root that is a parameter is followed out of
//! the function, the way the send side composes a wrapper's request over the
//! call graph:
//!
//! - into each in-repo caller, with that caller's argument in its place;
//! - into the call a function is handed to (a callback, a function passed by
//!   name, or one written inside an argument), whose callee then names where
//!   the parameter comes from: a parameter of the enclosing function, a
//!   package export, or a global;
//! - and, with no caller and no call it is handed to, to the function itself
//!   when the module offers it (an entry point) or to a hole.
//!
//! Every chain ends at one of a short list of kinds ([`ChainEnd`]), or at a
//! hole with a reason from a closed list ([`HoleReason`]). Nothing here names
//! what a package or a global is; the end says which one it is, and what it
//! means is the model's to say.
//!
//! The reading happens in the same walk as the send side's
//! ([`super::CallWalker`]) and is kept apart from it: every field it fills is
//! one the send side never reads. Nothing outside the tests and the offline
//! census reads the chains yet.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use serde::Serialize;
use swc_common::{Span, Spanned};
use swc_ecma_ast::*;
use swc_ecma_visit::{Visit, VisitWith};

use super::{
    CallWalker, FileIr, FnIr, ImportBinding, Piece, Reader, Reassigned, RequestSummaryInputs,
    Scope, Site, Value, keyed_names, member_prop, prop_name, reads_the_environment,
};
use crate::binding_scope::{BindingKey, Declarations, ident_key};
use crate::graphql_document_sites::unwrap_expression;
use crate::swc_scanner::is_producer_route_path;
use crate::visitor::{FunctionDefinition, FunctionNodeType};
use crate::wrapper_request_shape::literal_string;

/// How many caller paths one guard is followed down before the rest are
/// counted as one [`HoleReason::FanOutCap`] chain.
const MAX_CHAINS_PER_GUARD: usize = 16;

/// How deep into an argument's objects and arrays a function written there
/// is placed ([`WalkState::place_functions_in`]).
const MAX_PLACEMENT_DEPTH: usize = 6;

/// The String methods that take no argument and hand back the text changed:
/// a comparison of what they return compares the text they were called on.
const STRING_METHODS: &[&str] = &[
    "toLowerCase",
    "toUpperCase",
    "toLocaleLowerCase",
    "toLocaleUpperCase",
    "trim",
    "trimStart",
    "trimEnd",
    "normalize",
    "toString",
    "valueOf",
];

/// Why a chain stops before it reaches an end. A closed list: every chain
/// that does not end at a kind of [`ChainEnd`] says which of these stopped
/// it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum HoleReason {
    /// The binding is assigned again, or declared more than once.
    Reassigned,
    /// An expression this pass does not read: a computed member with a
    /// non-literal key, a call result other than a String method, `this.x`,
    /// a spread, `await`.
    UnreadExpression,
    /// A value imported from a module of this service.
    InRepoValueImport,
    /// The function is handed to a function this repository defines.
    CallbackIntoOwnFunction,
    /// The function is handed to a call whose callee is none of a
    /// parameter, an import or a global.
    CalleeUnread,
    /// A named function nothing calls, hands on or exports.
    NoCallerNotExported,
    /// A function written in a body that is held in a local, returned or
    /// assigned rather than handed to a call.
    DetachedUnplaced,
    /// A caller written in a file this service does not read.
    OutsideService,
    /// The caller passes nothing this pass reads in the position.
    CallerArgMissing,
    /// More caller paths than [`MAX_CHAINS_PER_GUARD`].
    FanOutCap,
    /// The chain reaches a function it already passed through.
    Recursion,
}

/// Where a compared value comes from, before any access is applied to it.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(super) enum Root {
    /// The function's own parameter, by position and name.
    Param(usize, String),
    /// A parameter of an enclosing function, `depth` functions out.
    Outer(u8, usize, String),
    /// An environment read, as written (`process.env.BASE`).
    Env(String),
    /// A name the file neither declares nor imports.
    Global(String),
    /// An import binding: its local name, the specifier as written and the
    /// export it names (`*` for a namespace import).
    Import {
        local: String,
        specifier: String,
        export: String,
    },
    /// Text the source writes.
    Literal(String),
    Hole(HoleReason),
}

/// One access from a [`Root`] down to the value compared.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(super) enum Step {
    /// `.url`, `.pathname`, a destructured key, or a computed literal key.
    Key(String),
    /// `new URL(x)` or `new URL(x, base)`, read as the URL `x` states.
    Url,
    /// A String method called with no argument (`.toLowerCase()`).
    Call(String),
    /// `new <Global>(x, …)` for any other global constructor, read through
    /// its first argument.
    New(String),
}

impl Step {
    fn render(&self) -> String {
        match self {
            Self::Key(key) => key.clone(),
            Self::Url => "URL".to_string(),
            Self::Call(method) => format!("{method}()"),
            Self::New(name) => format!("new {name}"),
        }
    }
}

/// A compared value: where it comes from, and the accesses down from there.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(super) struct Subject {
    root: Root,
    steps: Vec<Step>,
}

impl Subject {
    fn of(root: Root) -> Self {
        Self {
            root,
            steps: Vec::new(),
        }
    }

    fn hole(reason: HoleReason) -> Self {
        Self::of(Root::Hole(reason))
    }

    /// One more access. A hole stays the hole it is.
    fn then(mut self, step: Step) -> Self {
        if !matches!(self.root, Root::Hole(_)) {
            self.steps.push(step);
        }
        self
    }

    /// The subject as a function written inside this one reads it: each
    /// parameter is an enclosing one, one function further out.
    pub(super) fn enclosed(&self) -> Self {
        let root = match &self.root {
            Root::Param(index, name) => Root::Outer(1, *index, name.clone()),
            Root::Outer(depth, index, name) => {
                Root::Outer(depth.saturating_add(1), *index, name.clone())
            }
            other => other.clone(),
        };
        Self {
            root,
            steps: self.steps.clone(),
        }
    }

    /// `self` with a callee's subject's accesses after it: what the callee
    /// compares, read from what this call hands it.
    fn joined(&self, steps: &[Step]) -> Self {
        let mut joined = self.clone();
        for step in steps {
            joined = joined.then(step.clone());
        }
        joined
    }

    fn rendered_steps(&self) -> Vec<String> {
        self.steps.iter().map(Step::render).collect()
    }
}

/// Where a guard's literal stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum GuardPosition {
    /// An operand of `===`, `==`, `!==` or `!=`.
    Equality,
    /// The test of a `switch` `case`, compared with the discriminant.
    Case,
    /// The first argument of `.startsWith(`.
    StartsWith,
    /// A key of an object-literal table the value indexes (`ROUTES[path]`).
    TableKey,
}

/// What a guard guards.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum GuardBranch {
    /// The `if` consequent, the `case` statements, the ternary consequent or
    /// the right side of `&&`, as a span in the discovery source map.
    Span(u32, u32),
    /// The table's value at the key.
    Value { text: String, line: u32 },
    /// The guard is returned: in a predicate, its branch is its caller's.
    Returned,
    /// Nothing the guard decides is read.
    None,
}

/// One route-shaped literal a function compares a value against.
#[derive(Debug, Clone)]
pub(super) struct GuardIr {
    literal: String,
    position: GuardPosition,
    /// Written as `!==` or `!=`, or under `!`.
    negated: bool,
    /// The comparison: the binary expression, the `case`, the `startsWith`
    /// call or the indexing member.
    site: Site,
    subject: Subject,
    subject_text: String,
    /// The other operands of the `&&` test it stands in, as text and line,
    /// unfiltered: a method test rides along without this pass naming it.
    conjoined: Vec<(String, u32)>,
    branch: GuardBranch,
}

/// A call an `if`, a ternary or a `&&` tests: the branch its result guards.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct Tested {
    branch: (u32, u32),
    /// The test reads the call's result under an odd number of `!`.
    negated: bool,
}

/// Where a function written in a call's argument sits: the call, the
/// argument, and the keys from the argument down to the function (empty for
/// the argument itself).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct WrittenIn {
    site: Site,
    callee: String,
    arg: usize,
    path: Vec<String>,
}

impl WrittenIn {
    pub(super) fn argument(site: Site, callee: &str, arg: usize) -> Self {
        Self {
            site,
            callee: callee.to_string(),
            arg,
            path: Vec::new(),
        }
    }
}

/// One entry of an object-literal table a module declares.
#[derive(Debug, Clone)]
struct TableEntry {
    key: String,
    value_text: String,
    value_line: u32,
}

/// What a guard reads a name through at module scope.
#[derive(Debug, Default)]
pub(super) struct ModuleGuards {
    /// Each import binding: its specifier as written and the export it
    /// names (`*` for a namespace import).
    imports: HashMap<String, (String, String)>,
    /// Each module-scope `const` object literal, by the name declared once
    /// in the file, with its entries.
    tables: HashMap<String, Vec<TableEntry>>,
    /// Every declaration in the file, imports included: a name not here is
    /// a global.
    declared: Declarations,
    /// The extractor's definition keys for the file: a name a call hands on
    /// that is one of them is a function the module declares.
    definition_keys: HashSet<String>,
}

impl ModuleGuards {
    pub(super) fn of(
        module: &Module,
        imports: &HashMap<String, ImportBinding>,
        definition_keys: &HashSet<String>,
        reader: &Reader<'_>,
    ) -> Self {
        let declared = Declarations::of(module);
        let mut tables = HashMap::new();
        for item in &module.body {
            let Some(var) = super::module_var_decl(item) else {
                continue;
            };
            if var.kind != VarDeclKind::Const {
                continue;
            }
            for declarator in &var.decls {
                let (Pat::Ident(ident), Some(init)) = (&declarator.name, &declarator.init) else {
                    continue;
                };
                let name = ident.id.sym.to_string();
                let Expr::Object(object) = unwrap_expression(init) else {
                    continue;
                };
                if declared.name_count(&name) != 1 {
                    continue;
                }
                let entries = object
                    .props
                    .iter()
                    .filter_map(|prop| match prop {
                        PropOrSpread::Prop(prop) => match &**prop {
                            Prop::KeyValue(kv) => Some(TableEntry {
                                key: prop_name(&kv.key)?,
                                value_text: reader.text(kv.value.span()),
                                value_line: reader.line(kv.value.span()),
                            }),
                            _ => None,
                        },
                        PropOrSpread::Spread(_) => None,
                    })
                    .collect();
                tables.insert(name, entries);
            }
        }
        Self {
            imports: imports
                .iter()
                .map(|(local, binding)| {
                    let export = if binding.namespace {
                        "*".to_string()
                    } else {
                        binding.export.clone()
                    };
                    (local.clone(), (binding.specifier.clone(), export))
                })
                .collect(),
            tables,
            declared,
            definition_keys: definition_keys.clone(),
        }
    }
}

/// Whether `expr` is a test: what a predicate returns.
pub(super) fn is_test(expr: &Expr) -> bool {
    match unwrap_expression(expr) {
        Expr::Bin(bin) => matches!(
            bin.op,
            BinaryOp::EqEq
                | BinaryOp::NotEq
                | BinaryOp::EqEqEq
                | BinaryOp::NotEqEq
                | BinaryOp::LogicalAnd
                | BinaryOp::LogicalOr
        ),
        Expr::Unary(unary) => unary.op == UnaryOp::Bang,
        Expr::Call(_) | Expr::Lit(Lit::Bool(_)) => true,
        _ => false,
    }
}

/// The test an `if`, a ternary or a `&&` is reading.
#[derive(Debug)]
pub(super) struct TestScope {
    branch: (u32, u32),
    /// Each operand of the test's top-level `&&` chain: span, text, line.
    conjuncts: Vec<(u32, u32, String, u32)>,
    /// Each call whose result the test reads directly (through `!`, `&&`,
    /// `||` and parentheses only), with whether it is negated.
    calls: HashMap<(u32, u32), bool>,
}

impl TestScope {
    fn of(test: &Expr, branch: Span, reader: &Reader<'_>) -> Self {
        let mut conjuncts = Vec::new();
        conjuncts_of(test, reader, &mut conjuncts);
        let mut calls = HashMap::new();
        tested_calls(test, false, &mut calls);
        Self {
            branch: (branch.lo.0, branch.hi.0),
            conjuncts,
            calls,
        }
    }
}

fn conjuncts_of(expr: &Expr, reader: &Reader<'_>, out: &mut Vec<(u32, u32, String, u32)>) {
    match unwrap_expression(expr) {
        Expr::Bin(bin) if bin.op == BinaryOp::LogicalAnd => {
            conjuncts_of(&bin.left, reader, out);
            conjuncts_of(&bin.right, reader, out);
        }
        other => {
            let span = other.span();
            out.push((span.lo.0, span.hi.0, reader.text(span), reader.line(span)));
        }
    }
}

fn tested_calls(expr: &Expr, negated: bool, out: &mut HashMap<(u32, u32), bool>) {
    match unwrap_expression(expr) {
        Expr::Bin(bin) if matches!(bin.op, BinaryOp::LogicalAnd | BinaryOp::LogicalOr) => {
            tested_calls(&bin.left, negated, out);
            tested_calls(&bin.right, negated, out);
        }
        Expr::Unary(unary) if unary.op == UnaryOp::Bang => tested_calls(&unary.arg, !negated, out),
        Expr::Call(call) => {
            out.insert((call.span.lo.0, call.span.hi.0), negated);
        }
        _ => {}
    }
}

/// The guard state of one [`CallWalker`] walk.
#[derive(Debug, Default)]
pub(super) struct WalkState {
    test: Option<TestScope>,
    /// Under an odd number of `!`.
    pub(super) negated: bool,
    /// Inside a `return`, or an arrow's expression body.
    pub(super) in_return: bool,
    /// Each function written inside a call's argument, by where its node
    /// starts.
    placed: HashMap<u32, WrittenIn>,
    /// The bindings declared in the blocks the walk has read: a body's own
    /// statements are bound into its scope instead ([`Reader::bind_subjects`]).
    pub(super) bound: Bound,
}

impl WalkState {
    pub(super) fn returned() -> Self {
        Self {
            in_return: true,
            ..Self::default()
        }
    }

    pub(super) fn testing(&self) -> bool {
        self.test.is_some()
    }

    /// Start reading `test`, whose branch is `branch`; hands back the test
    /// read before, for [`Self::leave_test`].
    pub(super) fn enter_test(
        &mut self,
        test: &Expr,
        branch: Span,
        reader: &Reader<'_>,
    ) -> Option<TestScope> {
        self.test.replace(TestScope::of(test, branch, reader))
    }

    pub(super) fn leave_test(&mut self, previous: Option<TestScope>) {
        self.test = previous;
    }

    /// The branch a call written at `span` guards, when the test reads its
    /// result directly.
    pub(super) fn tested(&self, span: Span) -> Option<Tested> {
        let test = self.test.as_ref()?;
        let negated = test.calls.get(&(span.lo.0, span.hi.0))?;
        Some(Tested {
            branch: test.branch,
            negated: *negated,
        })
    }

    /// Record where each function written inside `expr`, an argument, sits.
    pub(super) fn place_functions_in(&mut self, expr: &Expr, written_in: &WrittenIn) {
        place(expr, written_in, &mut Vec::new(), &mut self.placed, 0);
    }

    /// Where the function whose node is `span` sits in an argument, if it
    /// sits in one.
    pub(super) fn placed(&mut self, span: Span) -> Option<WrittenIn> {
        self.placed.remove(&span.lo.0)
    }
}

fn place(
    expr: &Expr,
    written_in: &WrittenIn,
    path: &mut Vec<String>,
    out: &mut HashMap<u32, WrittenIn>,
    depth: usize,
) {
    if depth > MAX_PLACEMENT_DEPTH {
        return;
    }
    let at = |start: u32, path: &[String], out: &mut HashMap<u32, WrittenIn>| {
        out.insert(
            start,
            WrittenIn {
                path: path.to_vec(),
                ..written_in.clone()
            },
        );
    };
    let value = |expr: &Expr, path: &mut Vec<String>, out: &mut HashMap<u32, WrittenIn>| {
        match unwrap_expression(expr) {
            Expr::Arrow(arrow) => at(arrow.span.lo.0, path, out),
            Expr::Fn(function) => at(function.function.span.lo.0, path, out),
            other @ (Expr::Object(_) | Expr::Array(_)) => {
                place(other, written_in, path, out, depth + 1)
            }
            _ => {}
        }
    };
    match unwrap_expression(expr) {
        Expr::Object(object) => {
            for prop in &object.props {
                let PropOrSpread::Prop(prop) = prop else {
                    continue;
                };
                match &**prop {
                    Prop::KeyValue(kv) => {
                        if let Some(key) = prop_name(&kv.key) {
                            path.push(key);
                            value(&kv.value, path, out);
                            path.pop();
                        }
                    }
                    Prop::Method(method) => {
                        if let Some(key) = prop_name(&method.key) {
                            path.push(key);
                            at(method.function.span.lo.0, path, out);
                            path.pop();
                        }
                    }
                    _ => {}
                }
            }
        }
        Expr::Array(array) => {
            for (index, element) in array.elems.iter().enumerate() {
                if let Some(element) = element
                    && element.spread.is_none()
                {
                    path.push(index.to_string());
                    value(&element.expr, path, out);
                    path.pop();
                }
            }
        }
        _ => {}
    }
}

impl Reader<'_> {
    /// Where the value of `expr` comes from, as far as the source states it.
    /// `bound` holds the bindings the walk declared in the blocks it is in
    /// ([`CallWalker::bind_block`]).
    pub(super) fn subject(&self, expr: &Expr, scope: &Scope<'_>, bound: &Bound) -> Subject {
        match unwrap_expression(expr) {
            expr @ (Expr::Lit(Lit::Str(_)) | Expr::Tpl(_)) => match literal_string(expr) {
                Some(text) => Subject::of(Root::Literal(text)),
                None => Subject::hole(HoleReason::UnreadExpression),
            },
            Expr::Ident(ident) => self.ident_subject(ident, scope, bound),
            Expr::MetaProp(_) => Subject::of(Root::Global("import.meta".to_string())),
            Expr::Member(member) => self.member_subject(member, scope, bound),
            Expr::OptChain(chain) => match &*chain.base {
                OptChainBase::Member(member) => self.member_subject(member, scope, bound),
                OptChainBase::Call(_) => Subject::hole(HoleReason::UnreadExpression),
            },
            Expr::New(new) => {
                let Expr::Ident(callee) = &*new.callee else {
                    return Subject::hole(HoleReason::UnreadExpression);
                };
                let args = new.args.as_deref().unwrap_or_default();
                let Some(first) = args.first() else {
                    return Subject::hole(HoleReason::UnreadExpression);
                };
                if first.spread.is_some()
                    || !matches!(
                        self.ident_subject(callee, scope, bound).root,
                        Root::Global(_)
                    )
                {
                    return Subject::hole(HoleReason::UnreadExpression);
                }
                let name = callee.sym.to_string();
                if name != "URL" {
                    return self
                        .subject(&first.expr, scope, bound)
                        .then(Step::New(name));
                }
                // `new URL(".", base)`: a relative literal names a path only
                // the base decides, so no path is stated.
                if args.len() > 1
                    && let Some(text) = literal_string(unwrap_expression(&first.expr))
                    && !text.starts_with('/')
                    && !text.contains("://")
                {
                    return Subject::hole(HoleReason::UnreadExpression);
                }
                self.subject(&first.expr, scope, bound).then(Step::Url)
            }
            Expr::Call(call) => {
                let Callee::Expr(callee) = &call.callee else {
                    return Subject::hole(HoleReason::UnreadExpression);
                };
                if let Expr::Member(member) = unwrap_expression(callee)
                    && call.args.is_empty()
                    && let Some(method) = member_prop(member)
                    && STRING_METHODS.contains(&method.as_str())
                {
                    return self
                        .subject(&member.obj, scope, bound)
                        .then(Step::Call(method));
                }
                let text = self.text(call.span);
                if reads_the_environment(&text) {
                    return Subject::of(Root::Env(text));
                }
                Subject::hole(HoleReason::UnreadExpression)
            }
            Expr::Bin(bin)
                if matches!(bin.op, BinaryOp::NullishCoalescing | BinaryOp::LogicalOr) =>
            {
                self.subject(&bin.left, scope, bound)
            }
            _ => Subject::hole(HoleReason::UnreadExpression),
        }
    }

    fn member_subject(&self, member: &MemberExpr, scope: &Scope<'_>, bound: &Bound) -> Subject {
        let base = self.subject(&member.obj, scope, bound);
        if matches!(base.root, Root::Global(_)) {
            let text = self.text(member.span);
            if reads_the_environment(&text) {
                return Subject::of(Root::Env(text));
            }
        }
        match &member.prop {
            MemberProp::Ident(prop) => base.then(Step::Key(prop.sym.to_string())),
            MemberProp::Computed(computed) => match literal_string(&computed.expr) {
                Some(key) => base.then(Step::Key(key)),
                None => Subject::hole(HoleReason::UnreadExpression),
            },
            MemberProp::PrivateName(_) => Subject::hole(HoleReason::UnreadExpression),
        }
    }

    fn ident_subject(&self, ident: &Ident, scope: &Scope<'_>, bound: &Bound) -> Subject {
        let name = ident.sym.to_string();
        let key = ident_key(ident);
        if let Some(index) = scope.param_index(ident) {
            if scope.reassigned.contains(&name) || scope.written_through.contains(&name) {
                return Subject::hole(HoleReason::Reassigned);
            }
            return Subject::of(Root::Param(index, name));
        }
        if let Some((index, param_key)) = scope.keyed.get(&key) {
            return Subject::of(Root::Param(*index, String::new()))
                .then(Step::Key(param_key.clone()));
        }
        if let Some(subject) = bound.get(&key).or_else(|| scope.subjects.get(&key)) {
            return subject.clone();
        }
        // A binding of this function the walk read no declaration of, set
        // somewhere after it is declared.
        if scope.reassigned.contains(&name) {
            return Subject::hole(HoleReason::Reassigned);
        }
        if scope.locals.contains_key(&key) {
            return Subject::hole(HoleReason::UnreadExpression);
        }
        if let Some(Value::Str(pieces)) = scope.module.consts.get(&key)
            && let Some(text) = literal_text(pieces)
        {
            return Subject::of(Root::Literal(text));
        }
        let guards = &scope.module.guards;
        if let Some((specifier, export)) = guards.imports.get(&name)
            && !scope.module.declared_below.declares_name(&name)
        {
            return Subject::of(Root::Import {
                local: name,
                specifier: specifier.clone(),
                export: export.clone(),
            });
        }
        if guards.declared.declares_name(&name) {
            return Subject::hole(HoleReason::UnreadExpression);
        }
        Subject::of(Root::Global(name))
    }

    /// Each argument's subject, `None` for a function written in place and
    /// for every position from a spread on.
    pub(super) fn arg_subjects(
        &self,
        call: &CallExpr,
        scope: &Scope<'_>,
        bound: &Bound,
    ) -> Vec<Option<Subject>> {
        let mut spread = false;
        call.args
            .iter()
            .map(|arg| {
                spread |= arg.spread.is_some();
                match &*arg.expr {
                    _ if spread => None,
                    Expr::Arrow(_) | Expr::Fn(_) => None,
                    expr => Some(self.subject(expr, scope, bound)),
                }
            })
            .collect()
    }

    /// Each name of a function the module declares that an argument hands
    /// on, before any spread: the argument itself, or a value inside an
    /// object or array it writes (`{ skip: skipHealth }`), with the keys
    /// down to it.
    pub(super) fn handed(
        &self,
        call: &CallExpr,
        scope: &Scope<'_>,
        bound: &Bound,
    ) -> Vec<(usize, Vec<String>, String)> {
        let mut handed = Vec::new();
        for (index, arg) in call.args.iter().enumerate() {
            if arg.spread.is_some() {
                break;
            }
            let mut found = Vec::new();
            named_functions(&arg.expr, &mut Vec::new(), &mut found, 0);
            for (path, ident) in found {
                let name = ident.sym.to_string();
                let key = ident_key(ident);
                // A binding that holds a value (a subject) is not the
                // function of that name; one that holds a function is.
                if scope.module.guards.definition_keys.contains(&name)
                    && scope.param_index(ident).is_none()
                    && !scope.subjects.contains_key(&key)
                    && !bound.contains_key(&key)
                {
                    handed.push((index, path, name));
                }
            }
        }
        handed
    }

    /// Record where each binding `pat` declares from `init` comes from, by
    /// the rule `scope.locals` keeps: a binding the body declares once and
    /// never assigns again, nor writes a member of, holds what it was
    /// declared with. One declared with no value is set later, if at all.
    pub(super) fn bind_subjects(
        &self,
        pat: &Pat,
        init: Option<&Expr>,
        scope: &mut Scope<'_>,
        reassigned: &Reassigned,
        declared: &Declarations,
    ) {
        let settled = |key: &BindingKey| {
            !reassigned.names.contains(&key.0) && declared.binding_count(key) == 1
        };
        let bound = Bound::new();
        for (key, subject) in self.declared_subjects(pat, init, scope, &bound, settled) {
            scope.subjects.insert(key, subject);
        }
    }

    /// The subject of each binding `pat` declares from `init`, `settled`
    /// saying which bindings hold what they were declared with.
    fn declared_subjects(
        &self,
        pat: &Pat,
        init: Option<&Expr>,
        scope: &Scope<'_>,
        bound: &Bound,
        settled: impl Fn(&BindingKey) -> bool,
    ) -> Vec<(BindingKey, Subject)> {
        let settled = |key: &BindingKey| settled(key) && !scope.written_through.contains(&key.0);
        match (pat, init) {
            (Pat::Ident(ident), None) => {
                let key = ident_key(&ident.id);
                let reason = if scope.reassigned.contains(&key.0) {
                    HoleReason::Reassigned
                } else {
                    HoleReason::UnreadExpression
                };
                vec![(key, Subject::hole(reason))]
            }
            // A function is no compared value: the name hands it on
            // ([`Reader::handed`]).
            (Pat::Ident(_), Some(init))
                if matches!(unwrap_expression(init), Expr::Arrow(_) | Expr::Fn(_)) =>
            {
                Vec::new()
            }
            (Pat::Ident(ident), Some(init)) => {
                let key = ident_key(&ident.id);
                let subject = if settled(&key) {
                    self.subject(init, scope, bound)
                } else {
                    Subject::hole(HoleReason::Reassigned)
                };
                vec![(key, subject)]
            }
            (Pat::Object(object), Some(init)) => {
                let base = self.subject(init, scope, bound);
                keyed_names(object)
                    .into_iter()
                    .map(|(binding, key)| {
                        let subject = if settled(&binding) {
                            base.clone().then(Step::Key(key))
                        } else {
                            Subject::hole(HoleReason::Reassigned)
                        };
                        (binding, subject)
                    })
                    .collect()
            }
            _ => Vec::new(),
        }
    }
}

/// Bindings declared in the blocks a walk is in, with their subjects.
pub(super) type Bound = HashMap<BindingKey, Subject>;

/// Each identifier `expr` hands on as a value, itself or inside the objects
/// and arrays it writes, with the keys down to it.
fn named_functions<'e>(
    expr: &'e Expr,
    path: &mut Vec<String>,
    out: &mut Vec<(Vec<String>, &'e Ident)>,
    depth: usize,
) {
    if depth > MAX_PLACEMENT_DEPTH {
        return;
    }
    match unwrap_expression(expr) {
        Expr::Ident(ident) => out.push((path.clone(), ident)),
        Expr::Object(object) => {
            for prop in &object.props {
                let PropOrSpread::Prop(prop) = prop else {
                    continue;
                };
                match &**prop {
                    Prop::Shorthand(ident) => {
                        path.push(ident.sym.to_string());
                        out.push((path.clone(), ident));
                        path.pop();
                    }
                    Prop::KeyValue(kv) => {
                        if let Some(key) = prop_name(&kv.key) {
                            path.push(key);
                            named_functions(&kv.value, path, out, depth + 1);
                            path.pop();
                        }
                    }
                    _ => {}
                }
            }
        }
        Expr::Array(array) => {
            for (index, element) in array.elems.iter().enumerate() {
                if let Some(element) = element
                    && element.spread.is_none()
                {
                    path.push(index.to_string());
                    named_functions(&element.expr, path, out, depth + 1);
                    path.pop();
                }
            }
        }
        _ => {}
    }
}

/// The names a function writes a member of, anywhere in it: `x.a = …`,
/// `x[k] = …`, `x.a.b += …`, `x.a++`, `delete x.a`.
#[derive(Default)]
pub(super) struct WrittenThrough {
    names: HashSet<String>,
}

impl WrittenThrough {
    pub(super) fn of<N: VisitWith<Self>>(node: &N) -> HashSet<String> {
        let mut written = Self::default();
        node.visit_with(&mut written);
        written.names
    }

    fn write(&mut self, member: &MemberExpr) {
        let mut object = &*member.obj;
        while let Expr::Member(inner) = unwrap_expression(object) {
            object = &inner.obj;
        }
        if let Expr::Ident(ident) = unwrap_expression(object) {
            self.names.insert(ident.sym.to_string());
        }
    }
}

impl Visit for WrittenThrough {
    fn visit_assign_expr(&mut self, assign: &AssignExpr) {
        if let AssignTarget::Simple(SimpleAssignTarget::Member(member)) = &assign.left {
            self.write(member);
        }
        assign.visit_children_with(self);
    }

    fn visit_update_expr(&mut self, update: &UpdateExpr) {
        if let Expr::Member(member) = unwrap_expression(&update.arg) {
            self.write(member);
        }
        update.visit_children_with(self);
    }

    fn visit_unary_expr(&mut self, unary: &UnaryExpr) {
        if unary.op == UnaryOp::Delete
            && let Expr::Member(member) = unwrap_expression(&unary.arg)
        {
            self.write(member);
        }
        unary.visit_children_with(self);
    }
}

/// The text of `pieces` when every piece is literal.
fn literal_text(pieces: &[Piece]) -> Option<String> {
    pieces
        .iter()
        .map(|piece| match piece {
            Piece::Lit(text) => Some(text.as_str()),
            _ => None,
        })
        .collect::<Option<Vec<_>>>()
        .map(|parts| parts.concat())
}

/// What a function written inside the one `scope` reads holds where each
/// value comes from: this body's own subjects (those of the blocks the walk
/// is in, `bound`, included), its parameters and the keys it reads of them,
/// each one function further out.
pub(super) fn captured_subjects(scope: &Scope<'_>, bound: &Bound) -> HashMap<BindingKey, Subject> {
    let mut subjects: HashMap<BindingKey, Subject> = scope
        .subjects
        .iter()
        .chain(bound)
        .map(|(key, subject)| (key.clone(), subject.enclosed()))
        .collect();
    for (index, key) in scope.params.iter().enumerate() {
        if let Some(key) = key {
            let subject =
                if scope.reassigned.contains(&key.0) || scope.written_through.contains(&key.0) {
                    Subject::hole(HoleReason::Reassigned)
                } else {
                    Subject::of(Root::Outer(1, index, key.0.clone()))
                };
            subjects.insert(key.clone(), subject);
        }
    }
    for (key, (index, param_key)) in &scope.keyed {
        subjects.insert(
            key.clone(),
            Subject::of(Root::Outer(1, *index, String::new())).then(Step::Key(param_key.clone())),
        );
    }
    subjects
}

impl CallWalker<'_, '_, '_> {
    /// A declaration inside a block of the body (`if (…) { const url = new
    /// URL(req.url); … }`), bound by the body's rule: settled when the
    /// function never assigns the name again nor writes a member of it.
    pub(super) fn bind_block(&mut self, declarator: &VarDeclarator) {
        let reassigned = &self.scope.reassigned;
        let subjects = self.reader.declared_subjects(
            &declarator.name,
            declarator.init.as_deref(),
            self.scope,
            &self.guard.bound,
            |key| !reassigned.contains(&key.0),
        );
        self.guard.bound.extend(subjects);
    }

    /// The route-shaped literal `expr` states: a string literal, a template
    /// with no interpolation, or a name the scope reads as one.
    fn route_literal(&self, expr: &Expr) -> Option<String> {
        let text = match literal_string(unwrap_expression(expr)) {
            Some(text) => text,
            None => match unwrap_expression(expr) {
                Expr::Ident(_) => match self.reader.eval(expr, self.scope) {
                    Value::Str(pieces) => literal_text(&pieces)?,
                    _ => return None,
                },
                _ => return None,
            },
        };
        is_producer_route_path(&text).then_some(text)
    }

    fn record_guard(
        &mut self,
        literal: String,
        position: GuardPosition,
        subject: &Expr,
        span: Span,
        negated: bool,
        branch: Option<GuardBranch>,
    ) {
        let branch = branch.unwrap_or(match &self.guard.test {
            Some(test) => GuardBranch::Span(test.branch.0, test.branch.1),
            None if self.guard.in_return => GuardBranch::Returned,
            None => GuardBranch::None,
        });
        let conjoined = match &self.guard.test {
            Some(test) if test.conjuncts.len() > 1 => test
                .conjuncts
                .iter()
                .filter(|(lo, hi, _, _)| !(*lo <= span.lo.0 && span.hi.0 <= *hi))
                .map(|(_, _, text, line)| (text.clone(), *line))
                .collect(),
            _ => Vec::new(),
        };
        let guard = GuardIr {
            literal,
            position,
            negated: self.guard.negated ^ negated,
            site: self.reader.site(span),
            subject: self.reader.subject(subject, self.scope, &self.guard.bound),
            subject_text: self.reader.text(subject.span()),
            conjoined,
            branch,
        };
        self.ir.guards.push(guard);
    }

    /// `pathname === "/items"`: either operand a route literal, the other the
    /// value compared.
    pub(super) fn read_comparison(&mut self, bin: &BinExpr) {
        let negated = match bin.op {
            BinaryOp::EqEq | BinaryOp::EqEqEq => false,
            BinaryOp::NotEq | BinaryOp::NotEqEq => true,
            _ => return,
        };
        let (literal, subject) = match (
            self.route_literal(&bin.left),
            self.route_literal(&bin.right),
        ) {
            (Some(literal), None) => (literal, &*bin.right),
            (None, Some(literal)) => (literal, &*bin.left),
            _ => return,
        };
        self.record_guard(
            literal,
            GuardPosition::Equality,
            subject,
            bin.span,
            negated,
            None,
        );
    }

    /// `pathname.startsWith("/items/")`.
    pub(super) fn read_starts_with(&mut self, call: &CallExpr) {
        let Callee::Expr(callee) = &call.callee else {
            return;
        };
        let Expr::Member(member) = unwrap_expression(callee) else {
            return;
        };
        if member_prop(member).as_deref() != Some("startsWith") {
            return;
        }
        let Some(first) = call.args.first().filter(|arg| arg.spread.is_none()) else {
            return;
        };
        if let Some(literal) = self.route_literal(&first.expr) {
            self.record_guard(
                literal,
                GuardPosition::StartsWith,
                &member.obj,
                call.span,
                false,
                None,
            );
        }
    }

    /// `case "/health":`, compared with the discriminant. An empty case
    /// guards the statements of the next case that has any.
    pub(super) fn read_switch(&mut self, switch: &SwitchStmt) {
        for (index, case) in switch.cases.iter().enumerate() {
            let Some(test) = &case.test else {
                continue;
            };
            let Some(literal) = self.route_literal(test) else {
                continue;
            };
            let branch = switch.cases[index..]
                .iter()
                .find(|later| !later.cons.is_empty())
                .map_or(GuardBranch::None, |later| {
                    let first = later.cons.first().map(Spanned::span).unwrap_or_default();
                    let last = later.cons.last().map(Spanned::span).unwrap_or_default();
                    GuardBranch::Span(first.lo.0, last.hi.0)
                });
            self.record_guard(
                literal,
                GuardPosition::Case,
                &switch.discriminant,
                case.span,
                false,
                Some(branch),
            );
        }
    }

    /// `ROUTES[pathname]`, where `ROUTES` is a module-scope object literal:
    /// one guard per route-shaped key.
    pub(super) fn read_table_key(&mut self, member: &MemberExpr) {
        let MemberProp::Computed(computed) = &member.prop else {
            return;
        };
        let Expr::Ident(table) = unwrap_expression(&member.obj) else {
            return;
        };
        let key = ident_key(table);
        if self.scope.param_index(table).is_some()
            || self.scope.locals.contains_key(&key)
            || self.scope.subjects.contains_key(&key)
            || self.guard.bound.contains_key(&key)
            || self
                .scope
                .module
                .declared_below
                .declares_name(table.sym.as_ref())
        {
            return;
        }
        let Some(entries) = self.scope.module.guards.tables.get(table.sym.as_ref()) else {
            return;
        };
        let routes: Vec<TableEntry> = entries
            .iter()
            .filter(|entry| is_producer_route_path(&entry.key))
            .cloned()
            .collect();
        for entry in routes {
            self.record_guard(
                entry.key,
                GuardPosition::TableKey,
                &computed.expr,
                member.span,
                false,
                Some(GuardBranch::Value {
                    text: entry.value_text,
                    line: entry.value_line,
                }),
            );
        }
    }
}

impl FileIr {
    /// Keep each definition the extractor keyed in this file, with where its
    /// function node starts and whether the module exports it: what names a
    /// detached function (an exported object's member) and what makes a
    /// function an entry point (carrick#2386).
    pub fn place_definitions(&mut self, definitions: &HashMap<String, FunctionDefinition>) {
        self.definitions = definitions
            .iter()
            .map(|(key, definition)| {
                let start = match &definition.node_type {
                    FunctionNodeType::ArrowFunction(arrow) => Some(arrow.span.lo.0),
                    FunctionNodeType::FunctionDeclaration(declaration) => {
                        Some(declaration.function.span.lo.0)
                    }
                    FunctionNodeType::FunctionExpression(expression) => {
                        Some(expression.function.span.lo.0)
                    }
                    FunctionNodeType::FromPayload(_) | FunctionNodeType::Placeholder => None,
                };
                (key.clone(), (start, definition.is_exported))
            })
            .collect();
    }
}

// ---------------------------------------------------------------------------
// The chains.

/// Where a guard's comparison is written.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize)]
pub struct ChainSite {
    pub file: String,
    pub line: u32,
}

/// Another operand of the `&&` test a guard stands in.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Conjoined {
    pub text: String,
    pub line: u32,
}

/// One step a chain takes out of the function it was in.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Hop {
    /// Into a caller: the call, and the argument position read.
    Call {
        file: String,
        line: u32,
        callee: String,
        arg: usize,
    },
    /// Into the call the function is handed to: the callee as written, the
    /// argument it is passed in and the keys down to it, the parameter the
    /// chain was following, and the accesses from that parameter.
    HandedTo {
        file: String,
        line: u32,
        callee: String,
        arg: usize,
        path: Vec<String>,
        param: usize,
        steps: Vec<String>,
    },
    /// Out to an enclosing function, whose parameter it is.
    Enclosed { file: String, line: u32 },
}

/// Where a chain ends.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ChainEnd {
    /// A function the module offers that nothing in the repository calls.
    EntryPoint {
        file: String,
        line: u32,
        function: String,
        param: usize,
        steps: Vec<String>,
    },
    Literal {
        text: String,
        steps: Vec<String>,
    },
    Env {
        name: String,
        steps: Vec<String>,
    },
    Global {
        name: String,
        steps: Vec<String>,
    },
    /// An import the call graph does not follow into the repository, named
    /// by its specifier whether or not a manifest declares it.
    ExternalImport {
        specifier: String,
        export: String,
        steps: Vec<String>,
    },
    Hole {
        reason: HoleReason,
        file: String,
        line: u32,
    },
}

/// What the guarded branch does.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Branch {
    /// A call written in the branch, with the definition the call graph
    /// resolves it to (`file:key`), where it resolves one.
    Call {
        callee: String,
        file: String,
        line: u32,
        target: Option<String>,
    },
    /// A table's value at the key.
    Value {
        text: String,
        file: String,
        line: u32,
    },
}

/// One route literal's chain to one end: the object a later step hands the
/// model that classifies it, whichever model that is.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct GuardChain {
    pub literal: String,
    pub position: GuardPosition,
    pub negated: bool,
    pub site: ChainSite,
    pub subject: String,
    pub conjoined: Vec<Conjoined>,
    pub hops: Vec<Hop>,
    pub end: ChainEnd,
    pub branch: Vec<Branch>,
}

/// One function the chains move through.
struct Node<'a> {
    file: &'a Path,
    /// The extractor's key, when it keys the function.
    key: Option<String>,
    exported: bool,
    ir: &'a FnIr,
    /// The function this one is written in.
    parent: Option<usize>,
}

/// A chain being followed.
#[derive(Clone)]
struct Trace {
    subject: Subject,
    hops: Vec<Hop>,
    /// `None` while a predicate's branch waits on its caller.
    branch: Option<Vec<Branch>>,
    negated: bool,
    /// The functions passed through.
    passed: Vec<usize>,
}

struct Chains<'a> {
    inputs: &'a RequestSummaryInputs,
    root: &'a Path,
    nodes: Vec<Node<'a>>,
    /// (file, span start, span end) of each call -> (node, call index).
    calls: HashMap<(&'a Path, u32, u32), (usize, usize)>,
    /// Each definition's in-repo callers: (file, span start, span end).
    callers: HashMap<(PathBuf, String), Vec<CallerSite>>,
    /// (file, function name) -> each call that hands it on.
    handed: HashMap<(&'a Path, String), Vec<HandedAt>>,
}

/// A call site that reaches a definition: its file, span start and end.
type CallerSite = (PathBuf, u32, u32);

/// A call that hands a function on: (node, call index, argument, keys down
/// to the function inside the argument).
type HandedAt = (usize, usize, usize, Vec<String>);

/// Every chain the guards in `inputs` state, sorted, with each file named
/// relative to `root`.
pub fn guard_chains(inputs: &RequestSummaryInputs, root: &Path) -> Vec<GuardChain> {
    let chains = Chains::new(inputs, root);
    let mut out = Vec::new();
    for (index, node) in chains.nodes.iter().enumerate() {
        for guard in &node.ir.guards {
            out.extend(chains.of_guard(index, guard));
        }
    }
    out.sort_by_cached_key(|chain| {
        (
            chain.site.clone(),
            chain.literal.clone(),
            chain.position,
            serde_json::to_string(chain).unwrap_or_default(),
        )
    });
    out
}

impl<'a> Chains<'a> {
    fn new(inputs: &'a RequestSummaryInputs, root: &'a Path) -> Self {
        let mut chains = Self {
            inputs,
            root,
            nodes: Vec::new(),
            calls: HashMap::new(),
            callers: HashMap::new(),
            handed: HashMap::new(),
        };
        let mut paths: Vec<&PathBuf> = inputs.files.keys().collect();
        paths.sort();
        for path in paths {
            let file = &inputs.files[path];
            let by_start: HashMap<u32, (&String, bool)> = file
                .definitions
                .iter()
                .filter_map(|(key, (start, exported))| Some(((*start)?, (key, *exported))))
                .collect();
            let mut keys: Vec<&String> = file.functions.keys().collect();
            keys.sort();
            for key in keys {
                let exported = file.definitions.get(key).is_some_and(|(_, e)| *e);
                chains.collect(
                    path,
                    Some(key.clone()),
                    exported,
                    &file.functions[key],
                    None,
                    &by_start,
                );
            }
            chains.collect(path, None, false, &file.module_level, None, &by_start);
        }
        for (index, node) in chains.nodes.iter().enumerate() {
            for (call_index, call) in node.ir.calls.iter().enumerate() {
                chains
                    .calls
                    .insert((node.file, call.site.lo, call.site.hi), (index, call_index));
                for (arg, path, name) in &call.handed {
                    chains
                        .handed
                        .entry((node.file, name.clone()))
                        .or_default()
                        .push((index, call_index, *arg, path.clone()));
                }
            }
        }
        for (file, lo, hi, target) in inputs.sites.every_site() {
            chains
                .callers
                .entry(target.clone())
                .or_default()
                .push((file.to_path_buf(), lo, hi));
        }
        for callers in chains.callers.values_mut() {
            callers.sort();
        }
        chains
    }

    fn collect(
        &mut self,
        file: &'a Path,
        key: Option<String>,
        exported: bool,
        ir: &'a FnIr,
        parent: Option<usize>,
        by_start: &HashMap<u32, (&String, bool)>,
    ) {
        let index = self.nodes.len();
        self.nodes.push(Node {
            file,
            key,
            exported,
            ir,
            parent,
        });
        for child in ir.nested.iter().chain(&ir.detached) {
            let (key, exported) = child
                .site
                .and_then(|site| by_start.get(&site.lo))
                .map_or((None, false), |(key, exported)| {
                    (Some((*key).clone()), *exported)
                });
            self.collect(file, key, exported, child, Some(index), by_start);
        }
    }

    fn rel(&self, path: &Path) -> String {
        path.strip_prefix(self.root)
            .unwrap_or(path)
            .to_string_lossy()
            .into_owned()
    }

    fn line_of(&self, node: usize) -> u32 {
        self.nodes[node].ir.site.map_or(0, |site| site.line)
    }

    fn hole(&self, reason: HoleReason, node: usize) -> ChainEnd {
        ChainEnd::Hole {
            reason,
            file: self.rel(self.nodes[node].file),
            line: self.line_of(node),
        }
    }

    /// The calls written in `node` between `lo` and `hi`, in source order.
    fn branch_calls(&self, node: usize, (lo, hi): (u32, u32)) -> Vec<Branch> {
        let node = &self.nodes[node];
        let mut calls: Vec<_> = node
            .ir
            .calls
            .iter()
            .filter(|call| lo <= call.site.lo && call.site.hi <= hi)
            .collect();
        calls.sort_by_key(|call| (call.site.lo, call.site.hi));
        calls
            .into_iter()
            .map(|call| Branch::Call {
                callee: call.callee.clone(),
                file: self.rel(node.file),
                line: call.site.line,
                target: self
                    .inputs
                    .sites
                    .target_at(node.file, call.site.lo, call.site.hi)
                    .map(|(file, key)| format!("{}:{key}", self.rel(file))),
            })
            .collect()
    }

    fn of_guard(&self, node: usize, guard: &GuardIr) -> Vec<GuardChain> {
        let returns = &self.nodes[node].ir.returns_tests;
        let is_predicate = !returns.is_empty() && returns.iter().all(|test| *test);
        let branch = match &guard.branch {
            GuardBranch::Span(lo, hi) => Some(self.branch_calls(node, (*lo, *hi))),
            GuardBranch::Value { text, line } => Some(vec![Branch::Value {
                text: text.clone(),
                file: self.rel(self.nodes[node].file),
                line: *line,
            }]),
            GuardBranch::Returned if is_predicate => None,
            GuardBranch::Returned | GuardBranch::None => Some(Vec::new()),
        };
        let trace = Trace {
            subject: guard.subject.clone(),
            hops: Vec::new(),
            branch,
            negated: guard.negated,
            passed: Vec::new(),
        };
        let mut ends = Ends::default();
        self.follow(node, trace, &mut ends);
        ends.found
            .into_iter()
            .map(|(trace, end)| GuardChain {
                literal: guard.literal.clone(),
                position: guard.position,
                negated: trace.negated,
                site: ChainSite {
                    file: self.rel(self.nodes[node].file),
                    line: guard.site.line,
                },
                subject: guard.subject_text.clone(),
                conjoined: guard
                    .conjoined
                    .iter()
                    .map(|(text, line)| Conjoined {
                        text: text.clone(),
                        line: *line,
                    })
                    .collect(),
                hops: trace.hops,
                end,
                branch: trace.branch.unwrap_or_default(),
            })
            .collect()
    }

    /// Follow `trace`, whose subject is read in `node`, to its ends.
    fn follow(&self, node: usize, mut trace: Trace, ends: &mut Ends) {
        if ends.full() {
            ends.cap(self.hole(HoleReason::FanOutCap, node), trace);
            return;
        }
        let steps = trace.subject.rendered_steps();
        match trace.subject.root.clone() {
            Root::Literal(text) => ends.push(trace, ChainEnd::Literal { text, steps }),
            Root::Env(name) => ends.push(trace, ChainEnd::Env { name, steps }),
            Root::Global(name) => ends.push(trace, ChainEnd::Global { name, steps }),
            Root::Import {
                local,
                specifier,
                export,
            } => {
                let end = if self
                    .inputs
                    .bindings
                    .get(self.nodes[node].file, &local)
                    .is_some()
                {
                    self.hole(HoleReason::InRepoValueImport, node)
                } else {
                    ChainEnd::ExternalImport {
                        specifier,
                        export,
                        steps,
                    }
                };
                ends.push(trace, end);
            }
            Root::Hole(reason) => {
                let end = self.hole(reason, node);
                ends.push(trace, end);
            }
            Root::Outer(depth, index, name) => {
                let mut at = node;
                for _ in 0..depth {
                    match self.nodes[at].parent {
                        Some(parent) => at = parent,
                        None => {
                            let end = self.hole(HoleReason::UnreadExpression, node);
                            ends.push(trace, end);
                            return;
                        }
                    }
                }
                trace.hops.push(Hop::Enclosed {
                    file: self.rel(self.nodes[at].file),
                    line: self.line_of(at),
                });
                trace.subject.root = Root::Param(index, name);
                self.open(at, index, trace, ends);
            }
            Root::Param(index, _) => self.open(node, index, trace, ends),
        }
    }

    /// Where `node`'s parameter `index` comes from: its callers, the calls
    /// it is handed to, or the function itself.
    fn open(&self, node: usize, index: usize, mut trace: Trace, ends: &mut Ends) {
        if trace.passed.contains(&node) {
            let end = self.hole(HoleReason::Recursion, node);
            ends.push(trace, end);
            return;
        }
        trace.passed.push(node);
        let current = &self.nodes[node];
        let callers = current
            .key
            .as_ref()
            .and_then(|key| self.callers.get(&(current.file.to_path_buf(), key.clone())))
            .filter(|callers| !callers.is_empty());
        if let Some(callers) = callers {
            for (file, lo, hi) in callers {
                if ends.full() {
                    ends.cap(self.hole(HoleReason::FanOutCap, node), trace.clone());
                    return;
                }
                if !self.inputs.files.contains_key(file) {
                    let end = ChainEnd::Hole {
                        reason: HoleReason::OutsideService,
                        file: self.rel(file),
                        line: 0,
                    };
                    ends.push(trace.clone(), end);
                    continue;
                }
                let Some(&(caller, call_index)) = self.calls.get(&(file.as_path(), *lo, *hi))
                else {
                    let end = self.hole(HoleReason::CallerArgMissing, node);
                    ends.push(trace.clone(), end);
                    continue;
                };
                let call = &self.nodes[caller].ir.calls[call_index];
                let mut next = trace.clone();
                next.hops.push(Hop::Call {
                    file: self.rel(file),
                    line: call.site.line,
                    callee: call.callee.clone(),
                    arg: index,
                });
                if next.branch.is_none()
                    && let Some(tested) = call.tested
                {
                    next.branch = Some(self.branch_calls(caller, tested.branch));
                    next.negated ^= tested.negated;
                }
                match call.arg_subjects.get(index).cloned().flatten() {
                    Some(arg) => {
                        next.subject = arg.joined(&trace.subject.steps);
                        self.follow(caller, next, ends);
                    }
                    None => {
                        let end = ChainEnd::Hole {
                            reason: HoleReason::CallerArgMissing,
                            file: self.rel(file),
                            line: call.site.line,
                        };
                        ends.push(next, end);
                    }
                }
            }
            return;
        }
        // Handed to a call: by name, or written in one of its arguments.
        let mut handed: Vec<HandedAt> = Vec::new();
        if let Some(key) = &current.key
            && let Some(calls) = self.handed.get(&(current.file, key.clone()))
        {
            handed.extend(calls.iter().cloned());
        }
        if let Some(written_in) = &current.ir.written_in
            && let Some(&(n, c)) =
                self.calls
                    .get(&(current.file, written_in.site.lo, written_in.site.hi))
        {
            handed.push((n, c, written_in.arg, written_in.path.clone()));
        }
        if !handed.is_empty() {
            for (caller, call_index, arg, path) in handed {
                self.handed_to(caller, call_index, arg, path, index, trace.clone(), ends);
            }
            return;
        }
        let end = match &current.key {
            Some(key) if current.exported => ChainEnd::EntryPoint {
                file: self.rel(current.file),
                line: self.line_of(node),
                function: key.clone(),
                param: index,
                steps: trace.subject.rendered_steps(),
            },
            Some(_) => self.hole(HoleReason::NoCallerNotExported, node),
            None => self.hole(HoleReason::DetachedUnplaced, node),
        };
        ends.push(trace, end);
    }

    #[allow(clippy::too_many_arguments)]
    fn handed_to(
        &self,
        node: usize,
        call_index: usize,
        arg: usize,
        path: Vec<String>,
        param: usize,
        mut trace: Trace,
        ends: &mut Ends,
    ) {
        let file = self.nodes[node].file;
        let call = &self.nodes[node].ir.calls[call_index];
        trace.hops.push(Hop::HandedTo {
            file: self.rel(file),
            line: call.site.line,
            callee: call.callee_text.clone(),
            arg,
            path,
            param,
            steps: trace.subject.rendered_steps(),
        });
        let at_call = |reason| ChainEnd::Hole {
            reason,
            file: self.rel(file),
            line: call.site.line,
        };
        if self
            .inputs
            .sites
            .target_at(file, call.site.lo, call.site.hi)
            .is_some()
        {
            ends.push(trace, at_call(HoleReason::CallbackIntoOwnFunction));
            return;
        }
        let Some(callee) = call.callee_subject.clone() else {
            ends.push(trace, at_call(HoleReason::CalleeUnread));
            return;
        };
        let steps = callee.rendered_steps();
        match &callee.root {
            Root::Param(..) | Root::Outer(..) => {
                trace.subject = callee;
                self.follow(node, trace, ends);
            }
            Root::Import {
                specifier, export, ..
            } => {
                let end = ChainEnd::ExternalImport {
                    specifier: specifier.clone(),
                    export: export.clone(),
                    steps,
                };
                ends.push(trace, end);
            }
            Root::Global(name) => {
                let end = ChainEnd::Global {
                    name: name.clone(),
                    steps,
                };
                ends.push(trace, end);
            }
            _ => ends.push(trace, at_call(HoleReason::CalleeUnread)),
        }
    }
}

/// The ends one guard reaches, capped at [`MAX_CHAINS_PER_GUARD`].
#[derive(Default)]
struct Ends {
    found: Vec<(Trace, ChainEnd)>,
    capped: bool,
}

impl Ends {
    fn full(&self) -> bool {
        self.found.len() >= MAX_CHAINS_PER_GUARD
    }

    fn push(&mut self, trace: Trace, end: ChainEnd) {
        if self.full() {
            self.cap(end, trace);
            return;
        }
        self.found.push((trace, end));
    }

    /// One chain for every path past the cap.
    fn cap(&mut self, end: ChainEnd, trace: Trace) {
        if self.capped {
            return;
        }
        self.capped = true;
        let end = match end {
            hole @ ChainEnd::Hole {
                reason: HoleReason::FanOutCap,
                ..
            } => hole,
            _ => ChainEnd::Hole {
                reason: HoleReason::FanOutCap,
                file: String::new(),
                line: 0,
            },
        };
        self.found.push((trace, end));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(path: &str) -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures")
            .join(path)
    }

    /// The chains discovery reads in the fixture at `dir`, against the key
    /// written for it by hand.
    fn assert_chains(dir: &str, key: &str) {
        let root = fixture(dir);
        let inputs = crate::engine::discover_request_inputs(&root);
        let chains = serde_json::to_value(guard_chains(&inputs, &root)).expect("chains serialise");
        let key_path = fixture(&format!("receive-guards/{key}.guards.json"));
        let expected: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&key_path).expect("the key is readable"))
                .expect("the key is JSON");
        assert_eq!(
            chains,
            expected,
            "chains for {dir}:\n{}",
            serde_json::to_string_pretty(&chains).unwrap_or_default()
        );
    }

    #[test]
    fn a_platform_worker_states_each_route_up_to_its_exported_handler() {
        assert_chains("hand-dispatch-worker", "hand-dispatch-worker");
    }

    #[test]
    fn every_export_form_of_a_worker_reaches_its_handler_or_its_in_repo_caller() {
        assert_chains("receive-guards/worker-variants", "worker-variants");
    }

    #[test]
    fn a_node_server_handler_ends_at_the_package_export_it_is_handed_to() {
        assert_chains("receive-guards/node-server", "node-server");
    }

    #[test]
    fn a_dev_server_middleware_ends_at_the_config_export_through_its_plugin_hook() {
        assert_chains("receive-guards/dev-middleware", "dev-middleware");
    }

    #[test]
    fn a_predicate_in_another_file_composes_through_its_callers() {
        assert_chains("receive-guards/predicate-elsewhere", "predicate-elsewhere");
    }

    /// The chains of one service written from `files` (path, source), each
    /// as (literal, hops, end) JSON.
    fn chains_in(files: &[(&str, &str)]) -> Vec<(String, serde_json::Value, serde_json::Value)> {
        let dir = tempfile::tempdir().expect("a temporary directory");
        std::fs::write(dir.path().join("package.json"), r#"{ "name": "probe" }"#)
            .expect("the manifest is writable");
        for (path, source) in files {
            let path = dir.path().join(path);
            std::fs::create_dir_all(path.parent().expect("a parent")).expect("the tree");
            std::fs::write(path, source).expect("the source is writable");
        }
        let inputs = crate::engine::discover_request_inputs(dir.path());
        guard_chains(&inputs, dir.path())
            .into_iter()
            .map(|chain| {
                (
                    chain.literal,
                    serde_json::to_value(chain.hops).expect("hops serialise"),
                    serde_json::to_value(chain.end).expect("ends serialise"),
                )
            })
            .collect()
    }

    fn end_of<'c>(
        chains: &'c [(String, serde_json::Value, serde_json::Value)],
        literal: &str,
    ) -> &'c serde_json::Value {
        let ends: Vec<_> = chains.iter().filter(|(l, _, _)| l == literal).collect();
        assert_eq!(ends.len(), 1, "one chain for {literal}: {chains:#?}");
        &ends[0].2
    }

    /// A binding set after it is declared, or one whose members the body
    /// writes, holds no longer what it was declared with: the chain stops at
    /// `reassigned` rather than reading the declaration's value.
    #[test]
    fn a_binding_set_later_or_written_through_ends_reassigned() {
        let chains = chains_in(&[(
            "src/server.ts",
            r#"import { createServer } from "node:http";

createServer((req, res) => {
  let url: URL;
  try {
    url = new URL(req.url ?? "/", "http://localhost");
  } catch {
    return;
  }
  if (url.pathname === "/late") {
    res.end("late");
  }
});

export function rewritten(path: string): boolean {
  const url = new URL("https://placeholder.invalid");
  url.pathname = path;
  return url.pathname.startsWith("/written");
}
"#,
        )]);
        for literal in ["/late", "/written"] {
            let end = end_of(&chains, literal);
            assert_eq!(end["kind"], "hole", "{literal}: {end}");
            assert_eq!(end["reason"], "reassigned", "{literal}: {end}");
        }
    }

    /// `new URL(x, base)` states the path `x` names only where `x` names one
    /// itself: a relative literal resolves against the base, so no path is
    /// stated; a literal path is.
    #[test]
    fn a_relative_url_literal_states_no_path() {
        let chains = chains_in(&[(
            "src/base.ts",
            r#"export function atRoot(href: string): boolean {
  const pathname = new URL(".", href).pathname;
  return pathname === "/";
}

export function fixed(href: string): boolean {
  const pathname = new URL("/fixed", href).pathname;
  return pathname === "/fixed";
}
"#,
        )]);
        let relative = end_of(&chains, "/");
        assert_eq!(relative["kind"], "hole", "{relative}");
        assert_eq!(relative["reason"], "unread_expression", "{relative}");
        assert_eq!(
            end_of(&chains, "/fixed"),
            &serde_json::json!({ "kind": "literal", "text": "/fixed", "steps": ["URL", "pathname"] })
        );
    }

    /// A function handed on by name inside an object argument is followed
    /// to the call, as one written there is, with the keys down to it.
    #[test]
    fn a_function_named_inside_an_object_argument_is_handed_to_the_call() {
        let chains = chains_in(&[(
            "src/log.ts",
            r#"import { logger } from "invented-logger";

function skipHealth(req: { path: string }): boolean {
  return req.path === "/health";
}

logger({ skip: skipHealth });
logger({ skipHealth });
"#,
        )]);
        let mut paths: Vec<serde_json::Value> = chains
            .iter()
            .map(|(literal, hops, end)| {
                assert_eq!(literal, "/health");
                assert_eq!(
                    end,
                    &serde_json::json!({
                        "kind": "external_import", "specifier": "invented-logger",
                        "export": "logger", "steps": []
                    })
                );
                hops[0]["path"].clone()
            })
            .collect();
        paths.sort_by_key(|path| path.to_string());
        assert_eq!(
            paths,
            vec![
                serde_json::json!(["skip"]),
                serde_json::json!(["skipHealth"])
            ]
        );
    }

    /// A binding declared inside a block of the body is read by the body's
    /// rule, so a guard on it reaches where the request comes from; one that
    /// holds a function is handed on by its name.
    #[test]
    fn a_binding_declared_in_a_block_is_read_like_one_in_the_body() {
        let chains = chains_in(&[(
            "src/server.ts",
            r#"import { createServer } from "node:http";

createServer((req, res) => {
  if (req.method === "GET") {
    const { pathname } = new URL(req.url ?? "/", "http://localhost");
    if (pathname === "/inner") {
      res.end("inner");
    }
  }
});

export function register(ready: boolean): void {
  if (ready) {
    const onRequest = (req: { url: string }) => req.url === "/hooked";
    createServer(onRequest);
  }
}
"#,
        )]);
        let (_, hops, end) = chains
            .iter()
            .find(|(literal, _, _)| literal == "/inner")
            .expect("a chain for /inner");
        assert_eq!(
            hops[0]["steps"],
            serde_json::json!(["url", "URL", "pathname"])
        );
        assert_eq!(
            end_of(&chains, "/hooked"),
            &serde_json::json!({
                "kind": "external_import", "specifier": "node:http",
                "export": "createServer", "steps": []
            })
        );
        assert_eq!(
            end,
            &serde_json::json!({
                "kind": "external_import", "specifier": "node:http",
                "export": "createServer", "steps": []
            })
        );
    }

    /// A measurement, not a check: every chain the guards state over each
    /// root, read as one service with the default config, with the route
    /// literals in comparison positions the pass read no guard for. Offline
    /// and model-free.
    ///
    /// `CARRICK_GUARD_CENSUS=<root>[:<root>…] CARRICK_GUARD_CENSUS_OUT=<file>
    /// cargo test --lib request_summary::guards::tests::census -- --ignored --nocapture`
    #[test]
    #[ignore = "a measurement over the roots named by CARRICK_GUARD_CENSUS"]
    fn census() {
        use std::collections::BTreeMap;
        use std::io::Write;

        let Ok(roots) = std::env::var("CARRICK_GUARD_CENSUS") else {
            return;
        };
        let mut out = std::env::var("CARRICK_GUARD_CENSUS_OUT")
            .ok()
            .map(|path| std::fs::File::create(path).expect("the census output is writable"));
        for root in roots.split(':').filter(|root| !root.is_empty()) {
            let root = Path::new(root);
            let inputs = crate::engine::discover_request_inputs(root);
            let chains = guard_chains(&inputs, root);

            // The denominator: every route literal in a comparison position.
            let mut denominator: Vec<(String, u32, String, &'static str)> = Vec::new();
            let mut files: Vec<&PathBuf> = inputs.files.keys().collect();
            files.sort();
            for file in files {
                let Ok(content) = std::fs::read_to_string(file) else {
                    continue;
                };
                let Some((source_map, module)) =
                    crate::swc_scanner::parse_standalone_module(file, &content)
                else {
                    continue;
                };
                for found in crate::compared_route_literals::compared_route_literals(&module) {
                    let line = source_map.lookup_char_pos(found.span.lo).line as u32;
                    let position = match found.position {
                        crate::compared_route_literals::ComparisonPosition::Equality => "equality",
                        crate::compared_route_literals::ComparisonPosition::Case => "case",
                        crate::compared_route_literals::ComparisonPosition::StartsWith => {
                            "starts_with"
                        }
                    };
                    let rel = file
                        .strip_prefix(root)
                        .unwrap_or(file)
                        .to_string_lossy()
                        .into_owned();
                    denominator.push((rel, line, found.value, position));
                }
            }
            let read: HashSet<(String, u32, String)> = chains
                .iter()
                .map(|chain| {
                    (
                        chain.site.file.clone(),
                        chain.site.line,
                        chain.literal.clone(),
                    )
                })
                .collect();
            let unread: Vec<_> = denominator
                .iter()
                .filter(|(file, line, value, _)| {
                    !read.contains(&(file.clone(), *line, value.clone()))
                })
                .collect();

            let mut by_end: BTreeMap<&'static str, usize> = BTreeMap::new();
            let mut holes: BTreeMap<String, usize> = BTreeMap::new();
            for chain in &chains {
                let kind = match &chain.end {
                    ChainEnd::EntryPoint { .. } => "entry_point",
                    ChainEnd::Literal { .. } => "literal",
                    ChainEnd::Env { .. } => "env",
                    ChainEnd::Global { .. } => "global",
                    ChainEnd::ExternalImport { .. } => "external_import",
                    ChainEnd::Hole { reason, .. } => {
                        let reason = serde_json::to_value(reason)
                            .ok()
                            .and_then(|v| v.as_str().map(str::to_string))
                            .unwrap_or_default();
                        *holes.entry(reason).or_default() += 1;
                        "hole"
                    }
                };
                *by_end.entry(kind).or_default() += 1;
            }
            let guards: HashSet<(String, u32, String)> = read.clone();
            let table_keys = chains
                .iter()
                .filter(|chain| chain.position == GuardPosition::TableKey)
                .map(|chain| {
                    (
                        chain.site.file.clone(),
                        chain.site.line,
                        chain.literal.clone(),
                    )
                })
                .collect::<HashSet<_>>()
                .len();
            println!(
                "census {}: {} denominator literal(s), {} guard(s) read ({} table key(s)), {} chain(s), {} unread",
                root.display(),
                denominator.len(),
                guards.len(),
                table_keys,
                chains.len(),
                unread.len()
            );
            println!("  ends: {by_end:?}");
            println!("  holes: {holes:?}");
            if let Some(out) = out.as_mut() {
                for chain in &chains {
                    let line = serde_json::json!({ "root": root, "chain": chain });
                    writeln!(out, "{line}").expect("the census output is writable");
                }
                for (file, line, value, position) in &unread {
                    let line = serde_json::json!({
                        "root": root,
                        "unread": { "file": file, "line": line, "literal": value, "position": position },
                    });
                    writeln!(out, "{line}").expect("the census output is writable");
                }
            }
        }
    }
}
