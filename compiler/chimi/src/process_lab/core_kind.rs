//! Core scenarios (CO01-CO04): checks over the Core the compiler lowers, and over
//! Core built by hand to show the verifier does not trust the frontend.
//!
//! Each check lowers a project as `chimi build` does (constants evaluated, Core
//! verified in debug builds) and then inspects the result. Hand-built programs start
//! from verified Core and replace one node's operation
//! (`CoreProgram::replace_operation`).

use crate::compiler::{ProjectSource, resolve_project_sources};
use crate::executable::{
    CoreOperation, CoreProgram, CoreSymbolKind, CoreViolation, lower, seal_entry, verify_core,
};
use crate::ids::{CoreNodeId, SymbolId};
use crate::project::{LoadedProject, StandardTree};
use crate::resolution::ProjectSymbolKind;
use crate::source::SourceSnapshot;
use crate::standard::{StandardFunction, StandardSymbol};
use crate::typing::process::process_name_label;
use crate::typing::{TypedProject, check_project};
use std::collections::BTreeSet;
use std::path::PathBuf;
use std::sync::Arc;

struct Lowered {
    core: CoreProgram,
    typed: Arc<TypedProject>,
}

fn lower_sources(sources: Vec<ProjectSource>) -> Result<Lowered, String> {
    let fail = |diagnostics: Vec<crate::diagnostic::Diagnostic>| {
        let codes = diagnostics
            .iter()
            .map(|diagnostic| format!("{} {}", diagnostic.code(), diagnostic.message()))
            .collect::<Vec<_>>();
        format!("the project does not compile: {}", codes.join("; "))
    };
    let root = seal_entry(check_project(resolve_project_sources(sources))).map_err(fail)?;
    let typed = Arc::clone(&root.typed);
    let core = lower(root).map_err(fail)?;
    Ok(Lowered { core, typed })
}

/// Lowers another scenario's fixture project.
fn lower_fixture(directory: &str) -> Result<Lowered, String> {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/process-lab")
        .join(directory);
    let project = LoadedProject::load(&root, "app/main", &StandardTree::embedded())
        .map_err(|error| format!("load {}: {error}", root.display()))?;
    lower_sources(project.sources())
}

fn lower_modules(modules: &[(&str, &str)]) -> Result<Lowered, String> {
    let sources = modules
        .iter()
        .map(|(path, text)| {
            let file = format!("{path}.chimi");
            ProjectSource::new(*path, SourceSnapshot::from_text(file, *text))
                .map_err(|error| format!("module {path}: {error:?}"))
        })
        .collect::<Result<Vec<_>, _>>()?;
    lower_sources(sources)
}

impl Lowered {
    fn calls_of(&self, function: StandardFunction) -> Vec<(CoreNodeId, Vec<CoreNodeId>)> {
        self.core
            .nodes
            .iter()
            .filter_map(|node| match &node.operation {
                CoreOperation::Call { callee, arguments }
                    if self.callee_symbol(*callee).is_some_and(|symbol| {
                        self.core.symbol(symbol).standard
                            == Some(StandardSymbol::Function(function))
                    }) =>
                {
                    Some((node.id, arguments.clone()))
                }
                _ => None,
            })
            .collect()
    }

    fn callee_symbol(&self, callee: CoreNodeId) -> Option<SymbolId> {
        match self.core.nodes[callee.raw() as usize].operation {
            CoreOperation::Reference(symbol) => Some(symbol),
            _ => None,
        }
    }

    fn symbol_named(&self, name: &str, kind: CoreSymbolKind) -> Result<SymbolId, String> {
        self.core
            .symbols
            .iter()
            .find(|symbol| symbol.name == name && symbol.kind == kind)
            .map(|symbol| symbol.id)
            .ok_or_else(|| format!("no {kind:?} symbol `{name}` in Core"))
    }

    fn verify(&self) -> Result<(), CoreViolation> {
        verify_core(&self.core, &self.typed)
    }
}

/// CO01: each `process name` lowers to a reference carrying its declaration's symbol.
/// SP09 declares `counter` in two modules; its Core must reference two symbols, each
/// the declaration of its module, with distinct labels.
pub(super) fn co01() -> Result<(), String> {
    let lowered = lower_fixture("SP09-same-process-name-in-two-modules")?;
    let referenced = lowered
        .core
        .nodes
        .iter()
        .filter_map(|node| match node.operation {
            CoreOperation::Reference(symbol)
                if lowered.core.symbol(symbol).kind == CoreSymbolKind::ProcessName =>
            {
                Some(symbol)
            }
            _ => None,
        })
        .collect::<BTreeSet<_>>();
    let declared = lowered
        .typed
        .resolved
        .symbols
        .iter()
        .filter(|symbol| symbol.kind == ProjectSymbolKind::ProcessName)
        .map(|symbol| symbol.id)
        .collect::<BTreeSet<_>>();
    if referenced != declared {
        return Err(format!(
            "Core references process names {referenced:?}, but the declarations are {declared:?}"
        ));
    }
    let labels = referenced
        .iter()
        .map(|symbol| process_name_label(&lowered.typed, *symbol))
        .collect::<BTreeSet<_>>();
    let expected = ["app/first.counter", "app/second.counter"]
        .map(str::to_owned)
        .into_iter()
        .collect::<BTreeSet<_>>();
    if labels != expected {
        return Err(format!(
            "process name labels are {labels:?}, not {expected:?}"
        ));
    }
    Ok(())
}

/// CO02: the verifier accepts SP01's Core, where `Process.register` takes the
/// `counter` reference, and rejects it once that argument reads a local instead.
pub(super) fn co02() -> Result<(), String> {
    let mut lowered = lower_fixture("SP01-static-contract-example")?;
    lowered
        .verify()
        .map_err(|violation| format!("verified Core of SP01 rejected: {violation}"))?;
    let calls = lowered.calls_of(StandardFunction::ProcessRegister);
    let [(call, ref arguments)] = calls[..] else {
        return Err("SP01's Core does not have exactly one Process.register call".into());
    };
    let local = lowered.symbol_named("root", CoreSymbolKind::Local)?;
    lowered
        .core
        .replace_operation(arguments[1], CoreOperation::Reference(local));
    match lowered.verify() {
        Err(CoreViolation::RegisterTakesComputedName { call: reported }) if reported == call => {
            Ok(())
        }
        other => Err(format!(
            "a register of a local address: expected a CO02 violation at n{}, got {other:?}",
            call.raw()
        )),
    }
}

const CO03_MAIN: &str = "import std/process as Process
import app/process as Local

enum Message {
  Ping
}

pub process name peer: Message

pub fn main() -> Int {
  Process.send(peer, Message.Ping)
  Local.send(1)
}
";

const CO03_PROCESS: &str = "pub fn send(value: Int) -> Int {
  value + 1
}
";

/// CO03: `Process.send` is an intrinsic call with standard symbol identity; a user
/// function `send` in a user module `process` is an ordinary call of its own symbol.
pub(super) fn co03() -> Result<(), String> {
    let lowered = lower_modules(&[("app/main", CO03_MAIN), ("app/process", CO03_PROCESS)])?;
    let [(standard, _)] = lowered.calls_of(StandardFunction::ProcessSend)[..] else {
        return Err("expected exactly one Process.send call in Core".into());
    };
    let mut user_calls = 0;
    for node in &lowered.core.nodes {
        let CoreOperation::Call { callee, .. } = node.operation else {
            continue;
        };
        let Some(symbol) = lowered.callee_symbol(callee) else {
            continue;
        };
        let symbol = lowered.core.symbol(symbol);
        let process_operation = matches!(
            symbol.standard,
            Some(StandardSymbol::Function(function)) if is_process_operation(function)
        );
        if process_operation != (symbol.kind == CoreSymbolKind::Intrinsic) {
            return Err(format!(
                "call n{} of `{}`: kind {:?} with standard identity {:?}",
                node.id.raw(),
                symbol.name,
                symbol.kind,
                symbol.standard
            ));
        }
        if symbol.name == "send" && node.id != standard {
            if symbol.kind != CoreSymbolKind::Function || symbol.standard.is_some() {
                return Err(format!(
                    "the user `send` lowers to a {:?} call with standard identity {:?}",
                    symbol.kind, symbol.standard
                ));
            }
            user_calls += 1;
        }
    }
    if user_calls != 1 {
        return Err(format!(
            "expected one ordinary call of app/process.send, found {user_calls}"
        ));
    }
    Ok(())
}

fn is_process_operation(function: StandardFunction) -> bool {
    use StandardFunction::*;
    matches!(
        function,
        ProcessSpec
            | ProcessRegister
            | ProcessReceive
            | ProcessSend
            | SupervisorProcess
            | SupervisorSupervisor
            | SupervisorPolicy
            | SupervisorRestarts
            | SupervisorStart
            | SupervisorStartProcess
            | SupervisorStartSupervisor
            | SupervisorAddress
            | SupervisorRemoveProcess
            | SupervisorRemoveSupervisor
            | SupervisorStop
            | SupervisorWait
    )
}

const CO04_MAIN: &str = "import std/duration as Duration
import std/process as Process
import std/process.{Inbox}

enum Message {
  Add(amount: Int)
  Finish
}

const limit = 3

fn same(inbox: Inbox<Message>) -> Inbox<Message> {
  inbox
}

fn next(inbox: Inbox<Message>) -> Message {
  Process.receive(same(inbox))
}

fn body(inbox: Inbox<Message>) -> Void {
  loop total = 0 {
    case next(inbox) {
      Message.Add(amount) -> total + amount
      Message.Finish -> break
    }
  }
}

pub fn main() -> Int {
  let spec = Process.spec(body, limit, Duration.seconds(1))
  0
}
";

/// CO04: the verifier accepts Core where inboxes pass through helpers, and rejects
/// the same Core once an inbox is read from a module constant or made from a literal.
pub(super) fn co04() -> Result<(), String> {
    let lowered = lower_modules(&[("app/main", CO04_MAIN)])?;
    lowered
        .verify()
        .map_err(|violation| format!("verified Core rejected: {violation}"))?;
    let calls = lowered.calls_of(StandardFunction::ProcessReceive);
    let [(_, ref arguments)] = calls[..] else {
        return Err("expected exactly one Process.receive call in Core".into());
    };
    let inbox = arguments[0];
    let constant = lowered.symbol_named("limit", CoreSymbolKind::Constant)?;
    for forged in [
        CoreOperation::Reference(constant),
        CoreOperation::Constant(crate::executable::CoreLiteral::Void),
    ] {
        let mut bad = Lowered {
            core: lowered.core.clone(),
            typed: Arc::clone(&lowered.typed),
        };
        let description = format!("{forged:?}");
        bad.core.replace_operation(inbox, forged);
        match bad.verify() {
            Err(CoreViolation::InboxOutsideBody { node }) if node == inbox => {}
            other => {
                return Err(format!(
                    "an inbox made by {description}: expected a CO04 violation at n{}, got {other:?}",
                    inbox.raw()
                ));
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    // The verifier's third rule, which no CO scenario isolates: rejects a verifier
    // that checks only the register and inbox rules and trusts T4260 for spec bodies.
    #[test]
    fn the_verifier_rejects_a_spec_body_capturing_an_exclusive_value() {
        let source = "import std/channel as Channel
import std/duration as Duration
import std/process as Process
import std/process.{Inbox}

enum Message {
  Ping
}

pub process name peer: Message

pub fn main() -> Int {
  let channel = Channel.bounded(1)
  let receiver = channel.receiver
  let address = peer
  let spec = Process.spec(fn(inbox: Inbox<Message>) -> Void {
    let copy = address
    Void
  }, 1, Duration.seconds(1))
  let kept = receiver
  0
}
";
        let mut lowered = lower_modules(&[("app/main", source)]).unwrap();
        lowered.verify().unwrap();
        let calls = lowered.calls_of(StandardFunction::ProcessSpec);
        let [(call, ref arguments)] = calls[..] else {
            panic!("one Process.spec call");
        };
        let body = arguments[0];
        let CoreOperation::Closure {
            parameters,
            body: closure_body,
            ..
        } = lowered.core.nodes[body.raw() as usize].operation.clone()
        else {
            panic!("the spec body is a closure");
        };
        let receiver = lowered
            .symbol_named("receiver", CoreSymbolKind::Local)
            .unwrap();
        lowered.core.replace_operation(
            body,
            CoreOperation::Closure {
                parameters,
                captures: vec![receiver],
                body: closure_body,
            },
        );
        assert_eq!(
            lowered.verify(),
            Err(CoreViolation::SpecBodyNotShareable { call, value: body })
        );
    }
}
