//! Failure causes on the reference interpreter (time-and-outcomes.md, "Failure
//! context").
//!
//! A cause is diagnostic metadata beside a failure, a defect, or a cancellation:
//! never part of a value, so equality, ordering, matching and failure rows ignore
//! it. Causes are immutable and shared; adding a frame or a link builds a new one.
//! The native runtime mirrors this module in the `cause_*` functions of runtime.c,
//! and the language tests run each cause program on both backends.

use super::RuntimeValue;
use crate::executable::CoreProgram;
use crate::ids::CoreNodeId;
use std::sync::Arc;

/// Context frames kept per cause.
pub(super) const FRAME_LIMIT: usize = 32;
/// Suppressed causes kept per cause.
pub(super) const SUPPRESSED_LIMIT: usize = 16;
/// `caused_by` links kept below a cause.
pub(super) const LINK_LIMIT: u32 = 64;

#[derive(Clone, Debug)]
pub(super) enum CauseKind {
    ExpectedFailure(RuntimeValue),
    Defect(String),
    Cancellation(String),
}

impl CauseKind {
    /// The code `std/cause.kind` maps to its `Kind` variant.
    pub(super) const fn code(&self) -> i64 {
        match self {
            Self::ExpectedFailure(_) => 0,
            Self::Defect(_) => 1,
            Self::Cancellation(_) => 2,
        }
    }
}

/// Why a cause sits in another's suppressed list; the code `std/cause.reason`
/// maps to its `Suppression` variant.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[allow(dead_code)] // Produced by Task.all (CHI-205) and the file-close rule.
pub(super) enum Suppression {
    Cleanup = 0,
    Sibling = 1,
    Displaced = 2,
}

impl Suppression {
    const fn label(self) -> &'static str {
        match self {
            Self::Cleanup => "cleanup",
            Self::Sibling => "sibling",
            Self::Displaced => "displaced",
        }
    }
}

#[derive(Clone, Debug)]
pub(super) struct Frame {
    pub(super) text: String,
    pub(super) node: Option<CoreNodeId>,
}

#[derive(Clone, Debug)]
pub(super) struct Cause {
    pub(super) kind: CauseKind,
    /// Where the outcome started, when known.
    pub(super) origin: Option<CoreNodeId>,
    /// Oldest first, at most `FRAME_LIMIT`.
    pub(super) frames: Vec<Frame>,
    pub(super) dropped_frames: u32,
    pub(super) caused_by: Option<Arc<Cause>>,
    /// The `caused_by` links below this cause, so bounding a chain is O(1) to check.
    pub(super) links: u32,
    /// Links dropped between this cause and its `caused_by`.
    pub(super) dropped_links: u32,
    /// At most `SUPPRESSED_LIMIT`, in the order they were attached.
    pub(super) suppressed: Vec<Arc<Cause>>,
    pub(super) dropped_suppressed: u32,
    /// Set on a cause inside another's suppressed list.
    pub(super) reason: Option<Suppression>,
}

impl Cause {
    pub(super) const fn new(kind: CauseKind, origin: Option<CoreNodeId>) -> Self {
        Self {
            kind,
            origin,
            frames: Vec::new(),
            dropped_frames: 0,
            caused_by: None,
            links: 0,
            dropped_links: 0,
            suppressed: Vec::new(),
            dropped_suppressed: 0,
            reason: None,
        }
    }

    /// A copy of this cause with `frame` appended. At the limit it keeps the
    /// oldest half and the newest entries and counts the one it drops.
    pub(super) fn with_frame(&self, frame: Frame) -> Self {
        let mut cause = self.clone();
        cause.dropped_frames += u32::from(push_bounded(&mut cause.frames, frame, FRAME_LIMIT));
        cause
    }

    /// This cause with `caused_by` linked below it. A chain longer than
    /// `LINK_LIMIT` keeps its newest and oldest links and drops one in the middle.
    pub(super) fn linked_to(mut self, caused_by: Arc<Self>) -> Self {
        self.links = caused_by.links + 1;
        self.caused_by = Some(caused_by);
        if self.links > LINK_LIMIT {
            drop_middle_link(&mut self);
        }
        self
    }

    #[allow(dead_code)] // Produced by Task.all (CHI-205) and the file-close rule.
    pub(super) fn with_suppressed(&self, mut suppressed: Self, reason: Suppression) -> Self {
        suppressed.reason = Some(reason);
        let mut cause = self.clone();
        cause.dropped_suppressed += u32::from(push_bounded(
            &mut cause.suppressed,
            Arc::new(suppressed),
            SUPPRESSED_LIMIT,
        ));
        cause
    }

    /// The `Debug.render` text of an expected failure's value, a defect's
    /// message, or a cancellation's reason.
    pub(super) fn summary(&self, core: &CoreProgram) -> String {
        match &self.kind {
            CauseKind::ExpectedFailure(value) => super::debug::render(core, value, false),
            CauseKind::Defect(message) | CauseKind::Cancellation(message) => message.clone(),
        }
    }
}

/// Pushes `item`, first removing the entry just past the kept oldest half when
/// `items` is full. Returns whether it dropped one.
fn push_bounded<T>(items: &mut Vec<T>, item: T, limit: usize) -> bool {
    let dropped = items.len() == limit;
    if dropped {
        items.remove(limit / 2);
    }
    items.push(item);
    dropped
}

/// Removes the link at depth `LINK_LIMIT / 2` below `cause`, copying the newer
/// half of the chain above it, so the newest half and the oldest half remain.
fn drop_middle_link(cause: &mut Cause) {
    let keep_newest = (LINK_LIMIT / 2) as usize;
    let mut newer = Vec::with_capacity(keep_newest);
    let mut below = cause
        .caused_by
        .take()
        .expect("a linked cause has caused_by");
    for _ in 1..keep_newest {
        let next = below
            .caused_by
            .clone()
            .expect("the chain is longer than the limit");
        newer.push(below);
        below = next;
    }
    // `below` is the link to drop; the chain resumes at its `caused_by`.
    let dropped = below.dropped_links + 1;
    let mut resumed = below.caused_by.clone();
    let mut first = true;
    for link in newer.into_iter().rev() {
        let mut copy = Cause::clone(&link);
        if first {
            copy.dropped_links += dropped;
            first = false;
        }
        copy.links = resumed.as_ref().map_or(0, |next| next.links + 1);
        copy.caused_by = resumed;
        resumed = Some(Arc::new(copy));
    }
    if first {
        cause.dropped_links += dropped;
    }
    cause.links = resumed.as_ref().map_or(0, |next| next.links + 1);
    cause.caused_by = resumed;
}

/// `Cause.to_text`: the cause tree, one line per cause, frame, and suppressed
/// cause, each detail indented two spaces below what it describes.
pub(super) fn render(core: &CoreProgram, cause: &Cause) -> String {
    let mut output = String::new();
    render_into(core, cause, 0, &mut output);
    output
}

fn render_into(core: &CoreProgram, cause: &Cause, depth: usize, output: &mut String) {
    let kind = match cause.kind {
        CauseKind::ExpectedFailure(_) => "expected failure",
        CauseKind::Defect(_) => "defect",
        CauseKind::Cancellation(_) => "cancellation",
    };
    output.push_str(kind);
    output.push(' ');
    output.push_str(&cause.summary(core));
    push_location(core, cause.origin, output);
    let indent = "  ".repeat(depth + 1);
    for (index, frame) in cause.frames.iter().enumerate() {
        if index == FRAME_LIMIT / 2 && cause.dropped_frames > 0 {
            push_dropped(&indent, cause.dropped_frames, "context frame", output);
        }
        output.push('\n');
        output.push_str(&indent);
        output.push_str("context: ");
        output.push_str(&frame.text);
        push_location(core, frame.node, output);
    }
    for (index, suppressed) in cause.suppressed.iter().enumerate() {
        if index == SUPPRESSED_LIMIT / 2 && cause.dropped_suppressed > 0 {
            push_dropped(
                &indent,
                cause.dropped_suppressed,
                "suppressed cause",
                output,
            );
        }
        output.push('\n');
        output.push_str(&indent);
        output.push_str("suppressed (");
        output.push_str(suppressed.reason.map_or("unknown", Suppression::label));
        output.push_str("): ");
        render_into(core, suppressed, depth + 1, output);
    }
    if let Some(caused_by) = &cause.caused_by {
        if cause.dropped_links > 0 {
            push_dropped(&indent, cause.dropped_links, "cause", output);
        }
        output.push('\n');
        output.push_str(&indent);
        output.push_str("caused by: ");
        render_into(core, caused_by, depth + 1, output);
    }
}

fn push_dropped(indent: &str, count: u32, what: &str, output: &mut String) {
    output.push('\n');
    output.push_str(indent);
    output.push_str(&format!(
        "({count} {what}{} dropped)",
        if count == 1 { "" } else { "s" }
    ));
}

fn push_location(core: &CoreProgram, node: Option<CoreNodeId>, output: &mut String) {
    if let Some(node) = node {
        let origin = core.nodes[node.raw() as usize].origin;
        output.push_str(" (");
        output.push_str(&core.origin_locations[origin.raw() as usize]);
        output.push(')');
    }
}

/// Whether a root outcome's report shows its cause: only when the cause says
/// more than the outcome record already does.
pub(super) fn has_detail(cause: &Cause) -> bool {
    !cause.frames.is_empty() || cause.caused_by.is_some() || !cause.suppressed.is_empty()
}

impl super::Evaluator<'_> {
    /// The native core of `std/cause`. `runtime.c` implements the same
    /// operations in `invoke_cause_operation`.
    pub(super) fn invoke_cause_operation(
        &mut self,
        function: crate::standard::StandardFunction,
        arguments: Vec<RuntimeValue>,
        span: u64,
        call_node: CoreNodeId,
        environment: super::Environment,
    ) -> super::Control {
        use super::Control;
        use crate::standard::{StandardConstructor, StandardFunction as F};
        if function == F::CauseContext {
            let [RuntimeValue::Callable(operation), RuntimeValue::Text(text)] =
                arguments.as_slice()
            else {
                return self.intrinsic_shape_defect(function, call_node);
            };
            let text = text.clone();
            let control = self.invoke(operation.clone(), Vec::new(), span, call_node, environment);
            return match control.cause() {
                Some(cause) => control.with_cause(cause.with_frame(Frame {
                    text,
                    node: Some(call_node),
                })),
                None => control,
            };
        }
        let option = |evaluator: &Self, cause: Option<&Arc<Cause>>| match cause {
            Some(cause) => evaluator.standard_value(
                StandardConstructor::OptionSome,
                vec![RuntimeValue::Cause(Arc::clone(cause))],
            ),
            None => evaluator.standard_value(StandardConstructor::OptionNone, Vec::new()),
        };
        if function == F::CauseOf {
            return match arguments.as_slice() {
                [RuntimeValue::Constructed { cause, .. }] => {
                    Control::Success(option(self, cause.as_ref()))
                }
                _ => self.intrinsic_shape_defect(function, call_node),
            };
        }
        let [RuntimeValue::Cause(cause)] = arguments.as_slice() else {
            return self.intrinsic_shape_defect(function, call_node);
        };
        Control::Success(match function {
            F::CauseKindCode => RuntimeValue::Integer(cause.kind.code()),
            F::CauseSummary => RuntimeValue::Text(cause.summary(self.core)),
            F::CauseFrames => RuntimeValue::List(
                cause
                    .frames
                    .iter()
                    .map(|frame| RuntimeValue::Text(frame.text.clone()))
                    .collect(),
            ),
            F::CauseCausedBy => option(self, cause.caused_by.as_ref()),
            F::CauseSuppressed => RuntimeValue::List(
                cause
                    .suppressed
                    .iter()
                    .map(|suppressed| RuntimeValue::Cause(Arc::clone(suppressed)))
                    .collect(),
            ),
            F::CauseReasonCode => {
                RuntimeValue::Integer(cause.reason.map_or(-1, |reason| reason as i64))
            }
            F::CauseToText => RuntimeValue::Text(render(self.core, cause)),
            _ => return self.intrinsic_shape_defect(function, call_node),
        })
    }
}
