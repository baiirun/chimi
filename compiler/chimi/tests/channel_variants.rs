//! Conformance for the channel variants (task-and-channel-capabilities.md,
//! "Channel variants"): several receivers of one queue through
//! `Channel.add_receiver`, `std/broadcast`, and `std/watch`, each also as a
//! `Wait` source.
//!
//! Every native run uses the virtual clock, so a program that would wait on
//! the wall clock times the suite out instead of passing slowly.

#[path = "support/interpreter.rs"]
mod interpreter;

use chimi::source::SourceSnapshot;
use chimi::{Compiler, ProjectSource};
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

const IMPORTS: &str = concat!(
    "import std/broadcast as Broadcast\n",
    "import std/channel as Channel\n",
    "import std/channel.{ChannelError, Receive, Receiver, SendError}\n",
    "import std/clock as Clock\n",
    "import std/duration as Duration\n",
    "import std/process as Process\n",
    "import std/process.{Inbox}\n",
    "import std/wait as Wait\n",
    "import std/supervisor as Supervisor\n",
    "import std/task as Task\n",
    "import std/watch as Watch\n",
);

/// `value` maps a receive to its value, or 0 once the channel drains; `drain`
/// sums a receiver until it drains; `count` counts what a receiver gets.
const HELPERS: &str = concat!(
    "fn value(next: Receive<Int>) -> Int {\n",
    "  case next {\n",
    "    Receive.Received(value) -> value\n",
    "    Receive.Drained -> 0\n",
    "  }\n",
    "}\n",
    "fn drain(receiver: Receiver<Int>) -> Int {\n",
    "  loop total = 0 {\n",
    "    let next = Channel.receive(receiver)\n",
    "    case next {\n",
    "      Receive.Received(value) -> total + value\n",
    "      Receive.Drained -> break total\n",
    "    }\n",
    "  }\n",
    "}\n",
    "fn count(receiver: Receiver<Int>) -> Int {\n",
    "  loop seen = 0 {\n",
    "    let next = Channel.receive(receiver)\n",
    "    case next {\n",
    "      Receive.Received(_) -> seen + 1\n",
    "      Receive.Drained -> break seen\n",
    "    }\n",
    "  }\n",
    "}\n",
);

fn scratch_directory(name: &str) -> PathBuf {
    let path = std::env::temp_dir().join(format!(
        "chimi-variants-{name}-{}-{:?}",
        std::process::id(),
        std::thread::current().id()
    ));
    if path.exists() {
        std::fs::remove_dir_all(&path).unwrap();
    }
    std::fs::create_dir_all(&path).unwrap();
    path
}

fn build_native(scratch: &Path, name: &str, body: &str) -> PathBuf {
    let source = scratch.join(format!("{name}.chimi"));
    let executable = scratch.join(name);
    std::fs::write(&source, format!("{IMPORTS}{HELPERS}{body}")).unwrap();
    let build = Command::new(env!("CARGO_BIN_EXE_chimi"))
        .current_dir(scratch)
        .arg("build")
        .arg(&source)
        .arg("-o")
        .arg(&executable)
        .output()
        .expect("build native channel-variant program");
    assert!(
        build.status.success(),
        "{}",
        String::from_utf8_lossy(&build.stderr)
    );
    executable
}

/// Builds `body`, runs it once under the virtual clock, and returns its
/// outcome record.
fn run_virtual(name: &str, body: &str) -> String {
    let scratch = scratch_directory(name);
    let executable = build_native(&scratch, name, body);
    let outcome_path = scratch.join("outcome.jsonl");
    let child = Command::new(&executable)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .arg("--chimi-runner")
        .arg("--clock=virtual")
        .arg(format!("--outcome-file={}", outcome_path.display()))
        .arg(format!("--execution-id={name}"))
        .arg("--")
        .spawn()
        .expect("execute native channel-variant program");
    wait_bounded(child, &scratch);
    let outcome = std::fs::read_to_string(outcome_path).unwrap_or_default();
    std::fs::remove_dir_all(scratch).unwrap();
    outcome
}

/// Waits for `child`, killing it after 15 s. A program that livelocks must fail rather than
/// hang the suite.
fn wait_bounded(mut child: std::process::Child, scratch: &Path) -> Output {
    let started = std::time::Instant::now();
    while child.try_wait().expect("poll variant program").is_none() {
        if started.elapsed() > std::time::Duration::from_secs(15) {
            child
                .kill()
                .expect("kill a variant program that never ends");
            let _ = child.wait();
            std::fs::remove_dir_all(scratch).unwrap();
            panic!("the variant program ran for more than 15 s");
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    child
        .wait_with_output()
        .expect("collect variant program output")
}

fn success(value: &str) -> String {
    format!("{{\"schema\":\"chimi.runtime.v1\",\"outcome\":\"success\",\"value\":{value}}}\n")
}

fn type_codes(source: &str) -> Vec<String> {
    let module = ProjectSource::new(
        "main",
        SourceSnapshot::from_text("main.chimi", format!("{IMPORTS}{HELPERS}{source}")),
    )
    .expect("valid module path");
    let report = Compiler::type_project(vec![module]);
    report
        .diagnostics()
        .iter()
        .map(|diagnostic| diagnostic.code().to_owned())
        .collect()
}

fn assert_defect(outcome: &str, message: &str) {
    assert!(
        outcome.contains("\"outcome\":\"defect\"") && outcome.contains(message),
        "{outcome}"
    );
}

// ----- Several receivers --------------------------------------------------------

// Three receivers of one queue share the work: every value reaches exactly one
// of them. The values are powers of two, so the sum shows each arrived once.
// Rejects a copy per receiver, and a value lost between receivers.
#[test]
fn each_value_reaches_exactly_one_receiver() {
    let result = run_virtual(
        "pool",
        concat!(
            "pub fn main() -> Int {\n",
            "  let jobs = Channel.bounded(2)\n",
            "  let first = Channel.add_receiver(jobs.receiver)\n",
            "  let second = Channel.add_receiver(jobs.receiver)\n",
            "  let a = Task.start(fn() { drain(first) })\n",
            "  let b = Task.start(fn() { drain(second) })\n",
            "  Channel.send(jobs.sender, 1)\n",
            "  Channel.send(jobs.sender, 2)\n",
            "  Channel.send(jobs.sender, 4)\n",
            "  Channel.send(jobs.sender, 8)\n",
            "  Channel.send(jobs.sender, 16)\n",
            "  Channel.close(jobs.completion)\n",
            "  let mine = drain(jobs.receiver)\n",
            "  mine + Task.result(a) + Task.result(b)\n",
            "}\n",
        ),
    );
    assert_eq!(result, success("31"));
}

// Aborting one receiver ends only that receiver; the queue aborts when its
// last receiver ends. A receiver added through an ended one is ended at once.
// Rejects an abort that ends the whole queue, and one that never does.
#[test]
fn the_queue_aborts_only_when_its_last_receiver_ends() {
    let result = run_virtual(
        "abort-one",
        concat!(
            "pub fn main() -> Int {\n",
            "  let jobs = Channel.bounded(2)\n",
            "  let other = Channel.add_receiver(jobs.receiver)\n",
            "  Channel.abort(other)\n",
            "  let late = Channel.add_receiver(other)\n",
            "  let ended = case Channel.receive(late) {\n",
            "    Success(_) -> 0\n",
            "    Failure(ChannelError.Aborted) -> 1\n",
            "  }\n",
            "  Channel.send(jobs.sender, 5)\n",
            "  let got = value(Channel.receive(jobs.receiver))\n",
            "  Channel.abort(jobs.receiver)\n",
            "  let after = case Channel.send(jobs.sender, 7) {\n",
            "    Success(_) -> 0\n",
            "    Failure(SendError.Aborted(back)) -> back\n",
            "    Failure(SendError.Closed(_)) -> 9\n",
            "  }\n",
            "  ended * 100 + got * 10 + after\n",
            "}\n",
        ),
    );
    assert_eq!(result, success("157"));
}

// The receiver a channel was made with is one receiver among the others:
// aborting it leaves the queue to the receiver added from it. Rejects a
// queue that counts only its first receiver.
#[test]
fn an_added_receiver_outlives_the_first_one() {
    let result = run_virtual(
        "first-ends",
        concat!(
            "pub fn main() -> Int {\n",
            "  let jobs = Channel.bounded(1)\n",
            "  let other = Channel.add_receiver(jobs.receiver)\n",
            "  Channel.abort(jobs.receiver)\n",
            "  Channel.send(jobs.sender, 6)\n",
            "  value(Channel.receive(other))\n",
            "}\n",
        ),
    );
    assert_eq!(result, success("6"));
}

// A task that ends while holding an added receiver gives it up without
// aborting the queue the other receivers still read.
#[test]
fn a_receiver_owner_exiting_leaves_the_queue_open() {
    let result = run_virtual(
        "owner-exit",
        concat!(
            "fn hold(receiver: Receiver<Int>) -> Int { 0 }\n",
            "pub fn main() -> Int {\n",
            "  let jobs = Channel.bounded(1)\n",
            "  let other = Channel.add_receiver(jobs.receiver)\n",
            "  let gone = Task.start(fn() { hold(other) })\n",
            "  let ended = Task.result(gone)\n",
            "  Channel.send(jobs.sender, 3)\n",
            "  value(Channel.receive(jobs.receiver)) + ended\n",
            "}\n",
        ),
    );
    assert_eq!(result, success("3"));
}

// ----- Broadcast ----------------------------------------------------------------

// Every subscription gets every value published after it subscribed, and
// publish counts the subscriptions it reached. A publish with no subscribers
// reaches none; a subscription made after the close is drained at once.
#[test]
fn broadcast_reaches_every_subscription_once() {
    let result = run_virtual(
        "fanout",
        concat!(
            "pub fn main() -> Int {\n",
            "  let news = Broadcast.bounded(2)\n",
            "  let none = Broadcast.publish(news.publisher, 100)\n",
            "  let a = Broadcast.subscribe(news.publisher)\n",
            "  let b = Broadcast.subscribe(news.publisher)\n",
            "  let reached = Broadcast.publish(news.publisher, 1)\n",
            "  let c = Broadcast.subscribe(news.publisher)\n",
            "  let again = Broadcast.publish(news.publisher, 2)\n",
            "  Channel.close(news.completion)\n",
            "  let late = Broadcast.subscribe(news.publisher)\n",
            "  drain(a) * 10000 + drain(b) * 1000 + drain(c) * 100 + reached * 10 + again + none + drain(late)\n",
            "}\n",
        ),
    );
    assert_eq!(result, success("33223"));
}

// Each subscription owns its own copy of a heap value: one subscriber
// dropping its copy leaves the other intact, and a watch replacing an unread
// list releases the old one. Rejects shared or freed copies.
#[test]
fn every_subscription_owns_its_copy_of_a_heap_value() {
    let result = run_virtual(
        "copies",
        concat!(
            "import std/list as List\n",
            "struct Order {\n",
            "  id: Int,\n",
            "  lines: List<Int>,\n",
            "}\n",
            "fn total(receiver: Receiver<Order>) -> Int {\n",
            "  loop sum = 0 {\n",
            "    let next = Channel.receive(receiver)\n",
            "    case next {\n",
            "      Receive.Received(order) -> sum + order.id * 10 + List.length(order.lines)\n",
            "      Receive.Drained -> break sum\n",
            "    }\n",
            "  }\n",
            "}\n",
            "pub fn main() -> Int {\n",
            "  let news = Broadcast.bounded(2)\n",
            "  let a = Broadcast.subscribe(news.publisher)\n",
            "  let b = Broadcast.subscribe(news.publisher)\n",
            "  let first = Task.start(fn() { total(a) })\n",
            "  Broadcast.publish(news.publisher, Order { id: 1, lines: [1, 2, 3] })\n",
            "  Broadcast.publish(news.publisher, Order { id: 2, lines: [4] })\n",
            "  Channel.close(news.completion)\n",
            "  let state = Watch.starting_with([1])\n",
            "  let follower = Watch.subscribe(state.publisher)\n",
            "  Watch.publish(state.publisher, [1, 2])\n",
            "  Watch.publish(state.publisher, [1, 2, 3, 4, 5])\n",
            "  let latest = case Watch.receive(follower) {\n",
            "    Success(Receive.Received(lines)) -> List.length(lines)\n",
            "    Success(Receive.Drained) -> 0\n",
            "    Failure(_) -> -1\n",
            "  }\n",
            "  Task.result(first) * 1000 + total(b) * 10 + latest\n",
            "}\n",
        ),
    );
    assert_eq!(result, success("34345"));
}

// A publish waits until every subscription has room, and then reaches all of
// them at once: while the slow subscription is full, the fast one gets
// nothing. Rejects a publish that delivers to the subscriptions with room and
// drops or retries the rest.
#[test]
fn broadcast_waits_for_the_slowest_subscription() {
    let result = run_virtual(
        "backpressure",
        concat!(
            "fn push(publisher: Broadcast.Publisher<Int>, message: Int) -> Int {\n",
            "  Broadcast.publish(publisher, message)\n",
            "}\n",
            "pub fn main() -> Int needs { clock: Clock.Clock } {\n",
            "  let news = Broadcast.bounded(1)\n",
            "  let fast = Broadcast.subscribe(news.publisher)\n",
            "  let slow = Broadcast.subscribe(news.publisher)\n",
            "  Broadcast.publish(news.publisher, 1)\n",
            "  let publisher = news.publisher\n",
            "  let pusher = Task.start(fn() { push(publisher, 2) })\n",
            "  let f1 = value(Channel.receive(fast))\n",
            "  let waited = Wait.first([Wait.receive(fast, value), Wait.after(Duration.seconds(1), 99)])\n",
            "  let s1 = value(Channel.receive(slow))\n",
            "  let f2 = value(Channel.receive(fast))\n",
            "  let s2 = value(Channel.receive(slow))\n",
            "  waited * 1000000 + f1 * 100000 + s1 * 10000 + f2 * 1000 + s2 * 100 + Task.result(pusher) * 10\n",
            "}\n",
        ),
    );
    assert_eq!(result, success("99112220"));
}

// A completion abandoned by its owner aborts every subscription, and a later
// subscription is aborted too.
#[test]
fn an_abandoned_broadcast_aborts_its_subscriptions() {
    let result = run_virtual(
        "abandoned",
        concat!(
            "fn abandon(completion: Channel.Completion<Int>) -> Int { 0 }\n",
            "fn aborted(receiver: Receiver<Int>) -> Int {\n",
            "  case Channel.receive(receiver) {\n",
            "    Success(_) -> 0\n",
            "    Failure(ChannelError.Aborted) -> 1\n",
            "  }\n",
            "}\n",
            "pub fn main() -> Int {\n",
            "  let news = Broadcast.bounded(1)\n",
            "  let sub = Broadcast.subscribe(news.publisher)\n",
            "  let completion = news.completion\n",
            "  let gone = Task.start(fn() { abandon(completion) })\n",
            "  let ended = Task.result(gone)\n",
            "  let late = Broadcast.subscribe(news.publisher)\n",
            "  aborted(sub) * 10 + aborted(late) + ended\n",
            "}\n",
        ),
    );
    assert_eq!(result, success("11"));
}

// A capacity below one defects where the broadcast is made.
#[test]
fn a_broadcast_needs_a_positive_capacity() {
    let result = run_virtual(
        "capacity",
        "pub fn main() -> Int {\n  let news = Broadcast.bounded(0)\n  1\n}\n",
    );
    assert_defect(&result, "`Broadcast.bounded(0)` needs a positive capacity");
}

// A negative channel capacity is refused where the channel is made.
#[test]
fn a_channel_refuses_a_negative_capacity() {
    let result = run_virtual(
        "negative-capacity",
        "pub fn main() -> Int {\n  let made = Channel.bounded(-1)\n  1\n}\n",
    );
    assert_defect(&result, "verified standard function has invalid runtime arguments");
}

// ----- Watch --------------------------------------------------------------------

// A subscription's first receive is the current value; after that each
// receive is the latest value it has not seen, so publishes it missed are
// skipped. After the close it gets its last unseen value, then `Drained`.
#[test]
fn watch_delivers_the_latest_value() {
    let result = run_virtual(
        "watch",
        concat!(
            "pub fn main() -> Int {\n",
            "  let state = Watch.starting_with(5)\n",
            "  let early = Watch.subscribe(state.publisher)\n",
            "  let first = value(Watch.receive(early))\n",
            "  Watch.publish(state.publisher, 6)\n",
            "  Watch.publish(state.publisher, 7)\n",
            "  let latest = value(Watch.receive(early))\n",
            "  let late = Watch.subscribe(state.publisher)\n",
            "  let current = value(Watch.receive(late))\n",
            "  Watch.publish(state.publisher, 8)\n",
            "  Channel.close(state.completion)\n",
            "  let last = value(Watch.receive(early))\n",
            "  let done = value(Watch.receive(early))\n",
            "  let late_last = value(Watch.receive(late))\n",
            "  first * 100000 + latest * 10000 + current * 1000 + last * 100 + done * 10 + late_last\n",
            "}\n",
        ),
    );
    assert_eq!(result, success("577808"));
}

// ----- Wait sources -------------------------------------------------------------

// An added receiver, a broadcast subscription and a watch subscription are
// each a wait source, committed in list order, and a publish from another
// task wakes a parked wait on a watch.
#[test]
fn every_variant_is_a_select_source() {
    let result = run_virtual(
        "select",
        concat!(
            "fn push(publisher: Watch.Publisher<Int>, message: Int) -> Int {\n",
            "  Watch.publish(publisher, message)\n",
            "  0\n",
            "}\n",
            "pub fn main() -> Int {\n",
            "  let jobs = Channel.bounded(1)\n",
            "  let other = Channel.add_receiver(jobs.receiver)\n",
            "  let news = Broadcast.bounded(1)\n",
            "  let sub = Broadcast.subscribe(news.publisher)\n",
            "  let state = Watch.starting_with(3)\n",
            "  let follower = Watch.subscribe(state.publisher)\n",
            "  Channel.send(jobs.sender, 1)\n",
            "  Broadcast.publish(news.publisher, 2)\n",
            "  let a = Wait.first([Wait.receive(sub, value), Wait.receive(other, value)])\n",
            "  let b = Wait.first([Wait.receive(sub, value), Wait.receive(other, value)])\n",
            "  let c = Wait.first([Wait.receive(sub, value), Wait.watch(follower, value)])\n",
            "  let publisher = state.publisher\n",
            "  let pusher = Task.start(fn() { push(publisher, 9) })\n",
            "  let d = Wait.first([Wait.receive(sub, value), Wait.watch(follower, value)])\n",
            "  a * 1000 + b * 100 + c * 10 + d + Task.result(pusher)\n",
            "}\n",
        ),
    );
    assert_eq!(result, success("2139"));
}

// ----- Checker ------------------------------------------------------------------

// `Channel.add_receiver` leaves its argument with its owner, so both
// receivers stay usable.
#[test]
fn add_receiver_keeps_its_argument() {
    let codes = type_codes(concat!(
        "pub fn main() -> Int {\n",
        "  let jobs = Channel.bounded(1)\n",
        "  let mine = jobs.receiver\n",
        "  let other = Channel.add_receiver(mine)\n",
        "  value(Channel.receive(mine)) + value(Channel.receive(other))\n",
        "}\n",
    ));
    assert!(codes.is_empty(), "{codes:?}");
}

// Every subscriber gets its own copy of a published value, so a value that
// holds a Receiver or Completion is `T4244` at each publishing call.
#[test]
fn an_exclusive_value_cannot_be_published() {
    let codes = type_codes(concat!(
        "fn jobs() -> Receiver<Int> {\n",
        "  let made = Channel.bounded(1)\n",
        "  made.receiver\n",
        "}\n",
        "pub fn main() -> Int {\n",
        "  let news = Broadcast.bounded(2)\n",
        "  let reached = Broadcast.publish(news.publisher, jobs())\n",
        "  let state = Watch.starting_with(jobs())\n",
        "  Watch.publish(state.publisher, jobs())\n",
        "  reached\n",
        "}\n",
    ));
    assert_eq!(codes, ["T4244", "T4244", "T4244"]);
}

// The run-time backstop for managed handles (CHI-240): generic code can hand a
// managed process handle to a broadcast or watch, which would give every
// subscriber a copy of a handle that has one owner. The publish defects and
// names the handle. A watch that starts with `None` and is then given
// `Some(handle)` reaches the check in `Watch.publish`. A plain-data generic
// publish still goes through, and a
// managed handle sent on an ordinary channel still moves, since it has one
// receiver. Rejects a backstop that skips managed handles, and one that
// refuses them on every channel.
const IDLE_PROCESS: &str = concat!(
    "fn idle(inbox: Inbox<Int>) -> Void {\n",
    "  let _ = Process.receive(inbox)\n",
    "  Void\n",
    "}\n",
    "fn managed_idle(root: Supervisor.Running) -> Supervisor.ManagedProcess<Int> {\n",
    "  Supervisor.start_process(root, Process.spec(idle, 1, Duration.zero), Supervisor.Restart.Never)\n",
    "}\n",
    "fn root() -> Supervisor.Running {\n",
    "  Supervisor.start(Supervisor.policy([], 1, Supervisor.restarts(1, Duration.seconds(30))))\n",
    "}\n",
    "fn publish<T>(news: Broadcast.Publisher<T>, value: T) -> Int { Broadcast.publish(news, value) }\n",
    "fn start_with<T>(value: T) -> Watch.Publisher<T> { Watch.starting_with(value).publisher }\n",
    "fn give<T>(state: Watch.Publisher<T>, value: T) -> Void { Watch.publish(state, value) }\n",
);

#[test]
fn a_generic_publish_of_a_managed_handle_defects() {
    let broadcast = run_virtual(
        "managed-broadcast",
        &format!(
            "{IDLE_PROCESS}pub fn main() -> Int {{\n\
             \x20 let numbers = Broadcast.bounded(2)\n\
             \x20 let one = Broadcast.subscribe(numbers.publisher)\n\
             \x20 let two = Broadcast.subscribe(numbers.publisher)\n\
             \x20 let handles = Broadcast.bounded(2)\n\
             \x20 let first = Broadcast.subscribe(handles.publisher)\n\
             \x20 let second = Broadcast.subscribe(handles.publisher)\n\
             \x20 publish(numbers.publisher, 4) + publish(handles.publisher, managed_idle(root()))\n\
             }}\n"
        ),
    );
    assert_defect(
        &broadcast,
        "Broadcast.publish cannot hold a managed process or supervisor handle",
    );
    let watch = run_virtual(
        "managed-watch",
        &format!(
            "{IDLE_PROCESS}pub fn main() -> Int {{\n\
             \x20 let supervisor = root()\n\
             \x20 let state = start_with(managed_idle(supervisor))\n\
             \x20 1\n\
             }}\n"
        ),
    );
    assert_defect(
        &watch,
        "Watch.starting_with cannot hold a managed process or supervisor handle",
    );
    let later = run_virtual(
        "managed-watch-publish",
        &format!(
            "{IDLE_PROCESS}pub fn main() -> Int {{\n\
             \x20 let supervisor = root()\n\
             \x20 let state = start_with(None)\n\
             \x20 give(state, Some(managed_idle(supervisor)))\n\
             \x20 1\n\
             }}\n"
        ),
    );
    assert_defect(
        &later,
        "Watch.publish cannot hold a managed process or supervisor handle",
    );
    let moved = run_virtual(
        "managed-channel",
        &format!(
            "{IDLE_PROCESS}fn pass<T>(sender: Channel.Sender<T>, value: T) -> Void {{ Channel.send(sender, value) }}\n\
             pub fn main() -> Int {{\n\
             \x20 let supervisor = root()\n\
             \x20 let handles = Channel.bounded(1)\n\
             \x20 pass(handles.sender, managed_idle(supervisor))\n\
             \x20 let next = Channel.receive(handles.receiver)\n\
             \x20 case next {{\n\
             \x20   Receive.Received(managed) -> {{\n\
             \x20     Supervisor.remove_process(supervisor, managed)\n\
             \x20     Supervisor.stop(supervisor)\n\
             \x20     6\n\
             \x20   }}\n\
             \x20   Receive.Drained -> 0\n\
             \x20 }}\n\
             }}\n"
        ),
    );
    assert_eq!(moved, success("6"));
}

// A generic function can still copy a managed handle inside one task, which
// neither the checker nor the transfer backstop can see. Removing through the
// second copy then defects with a message that says so, instead of a generic
// scheduler failure (CHI-240).
#[test]
fn removing_a_copied_managed_handle_twice_names_the_copy() {
    let outcome = run_virtual(
        "managed-copy-removed-twice",
        &format!(
            "{IDLE_PROCESS}fn dup<T>(value: T) -> (T, T) {{ (value, value) }}\n\
             pub fn main() -> Int {{\n\
             \x20 let supervisor = root()\n\
             \x20 let (first, second) = dup(managed_idle(supervisor))\n\
             \x20 Supervisor.remove_process(supervisor, first)\n\
             \x20 Supervisor.remove_process(supervisor, second)\n\
             \x20 1\n\
             }}\n"
        ),
    );
    assert_defect(
        &outcome,
        "removed already, through this handle or a copy of it",
    );
}

// A `Result` is what its arguments are, though `Ok` and `Err` are written in
// the runtime's own generics: one that holds a Receiver is exclusive and
// `T4244`, and one that holds plain data is published freely. Rejects a
// `Result` walk that never bound its arguments, which hid the Receiver and left
// only the run-time backstop.
#[test]
fn a_result_is_as_shareable_as_its_arguments() {
    let codes = type_codes(concat!(
        "import std/result as Result\n",
        "import std/result.{Result}\n",
        "enum Lost {\n  Gone\n}\n",
        "pub fn main() -> Int {\n",
        "  let plain_news = Broadcast.bounded(2)\n",
        "  let held_news = Broadcast.bounded(2)\n",
        "  let inner = Channel.bounded(1)\n",
        "  let held: Result<Receiver<Int>, Lost> = Result.Ok(inner.receiver)\n",
        "  let plain: Result<Int, Lost> = Result.Ok(1)\n",
        "  Broadcast.publish(plain_news.publisher, plain) + Broadcast.publish(held_news.publisher, held)\n",
        "}\n",
    ));
    assert_eq!(codes, ["T4244"]);
}

// A receive or watch event used after its receiver left the task is `T4241`
// at the event, for both kinds of event and for a receiver read from a field
// (CHI-224). A select's answer borrows nothing, and moving the receiver into
// another binding of the same task leaves the event valid.
#[test]
fn an_event_cannot_outlive_its_receiver() {
    let codes = type_codes(concat!(
        "pub fn main() -> Int {\n",
        "  let jobs = Channel.bounded(1)\n",
        "  let receiver = jobs.receiver\n",
        "  let event = Wait.receive(receiver, value)\n",
        "  let worker = Task.start(fn() { drain(receiver) })\n",
        "  let state = Watch.starting_with(5)\n",
        "  let follower = Watch.subscribe(state.publisher)\n",
        "  let watched = Wait.watch(follower, value)\n",
        "  let reader = Task.start(fn() { value(Watch.receive(follower)) })\n",
        "  Wait.first([event]) + Wait.first([watched])\n",
        "}\n",
    ));
    assert_eq!(codes, ["T4241", "T4241"]);
    let answered = type_codes(concat!(
        "pub fn main() -> Int {\n",
        "  let jobs = Channel.bounded(1)\n",
        "  Channel.send(jobs.sender, 4)\n",
        "  let receiver = jobs.receiver\n",
        "  let chosen = Wait.first([Wait.receive(receiver, value)])\n",
        "  let worker = Task.start(fn() { drain(receiver) })\n",
        "  chosen\n",
        "}\n",
    ));
    assert!(answered.is_empty(), "{answered:?}");
    let through_a_field = type_codes(concat!(
        "pub fn main() -> Int {\n",
        "  let jobs = Channel.bounded(1)\n",
        "  let event = Wait.receive(jobs.receiver, value)\n",
        "  let worker = Task.start(fn() { drain(jobs.receiver) })\n",
        "  Wait.first([event])\n",
        "}\n",
    ));
    assert_eq!(through_a_field, ["T4241"], "a receiver read from a field");
    let rebound = type_codes(concat!(
        "pub fn main() -> Int {\n",
        "  let jobs = Channel.bounded(1)\n",
        "  Channel.send(jobs.sender, 4)\n",
        "  let receiver = jobs.receiver\n",
        "  let event = Wait.receive(receiver, value)\n",
        "  let mine = receiver\n",
        "  Wait.first([event])\n",
        "}\n",
    ));
    assert!(rebound.is_empty(), "a receiver rebound in its own task: {rebound:?}");
}

// An event keeps its link to the receiver when it comes out of a helper
// function or a closure, is read from a struct field, leaves a loop through
// `break`, or is taken out of a tuple by a pattern, so each use after the
// receiver left is `T4241` (CHI-241). Used before the receiver leaves, each
// form stays valid.
#[test]
fn an_event_stays_tied_to_its_receiver_through_calls_breaks_and_patterns() {
    let escapes = |make: &str, moved: &str, after_move: bool| {
        let (before, after) = if after_move {
            ("", "  Wait.first([event])\n")
        } else {
            ("  let answer = Wait.first([event])\n", "  answer\n")
        };
        type_codes(&format!(
            "import std/wait.{{Event}}\n\
             struct Held {{\n\
             \x20 event: Event<Int, ChannelError>,\n\
             \x20 n: Int,\n\
             }}\n\
             fn event_of(receiver: Receiver<Int>) -> Event<Int, ChannelError> {{\n\
             \x20 Wait.receive(receiver, value)\n\
             }}\n\
             pub fn main() -> Int {{\n\
             \x20 let jobs = Channel.bounded(1)\n\
             \x20 Channel.send(jobs.sender, 4)\n\
             \x20 let receiver = jobs.receiver\n\
             {make}\
             {before}\
             \x20 let worker = Task.start(fn() {{ drain({moved}) }})\n\
             {after}\
             }}\n"
        ))
    };
    let helper = "  let event = event_of(receiver)\n";
    let broken = "  let event = loop n = 0 {\n    break Wait.receive(receiver, value)\n  }\n";
    let taken_apart = "  let (event, _) = (Wait.receive(receiver, value), 1)\n";
    let through_a_field = "  let event = event_of(jobs.receiver)\n";
    let from_a_closure = "  let made = Wait.receive(receiver, value)\n  let get = fn() { made }\n  let event = get()\n";
    let from_a_struct = "  let held = Held { event: Wait.receive(receiver, value), n: 1 }\n  let event = held.event\n";
    for (form, make, moved) in [
        ("closure", from_a_closure, "receiver"),
        ("struct field", from_a_struct, "receiver"),
        ("helper", helper, "receiver"),
        ("break", broken, "receiver"),
        ("pattern", taken_apart, "receiver"),
        ("field", through_a_field, "jobs.receiver"),
    ] {
        assert_eq!(escapes(make, moved, true), ["T4241"], "{form}, used after the move");
        let used_first = escapes(make, moved, false);
        assert!(used_first.is_empty(), "{form}, used before the move: {used_first:?}");
    }
    // Only a binding that may hold the event borrows its receiver.
    let plain_part = type_codes(concat!(
        "pub fn main() -> Int {\n",
        "  let jobs = Channel.bounded(1)\n",
        "  let receiver = jobs.receiver\n",
        "  let (event, n) = (Wait.receive(receiver, value), 1)\n",
        "  let worker = Task.start(fn() { drain(receiver) })\n",
        "  n\n",
        "}\n",
    ));
    assert!(plain_part.is_empty(), "the plain part of a pattern: {plain_part:?}");
}

// ----- Interpreter --------------------------------------------------------------

#[test]
fn the_interpreter_reports_the_variants_as_native_only() {
    let scratch = scratch_directory("interpreter");
    let path = scratch.join("main.chimi");
    std::fs::write(
        &path,
        format!(
            "{IMPORTS}{HELPERS}pub fn main() -> Int {{\n  \
             let news = Broadcast.bounded(1)\n  1\n}}\n"
        ),
    )
    .unwrap();
    let output = interpreter::traced(
        Command::new(env!("CARGO_BIN_EXE_chimi"))
            .arg("run")
            .arg(&path),
    )
    .expect("run the reference interpreter");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let outcome = stdout.lines().last().unwrap_or_default();
    assert_defect(
        outcome,
        "Broadcast.bounded requires the bounded Channel runtime",
    );
    std::fs::remove_dir_all(scratch).unwrap();
}

// CHI-214: sending a channel's only Receiver through that channel queues a
// message nobody could ever receive, so the send defects. With a second
// receiver of the same queue, the message is still receivable and goes
// through. Rejects a check that ignores the queue's other receivers.
#[test]
fn sending_a_channel_its_own_only_receiver_defects() {
    let hold = "enum Hold {\n  Empty\n  Has(receiver: Receiver<Hold>)\n}\n";
    let alone = run_virtual(
        "own-receiver",
        &format!(
            "{hold}pub fn main() -> Int {{\n\
             \x20 let channel = Channel.bounded(1)\n\
             \x20 Channel.send(channel.sender, Hold.Has(channel.receiver))\n\
             \x20 Channel.close(channel.completion)\n\
             \x20 7\n\
             }}\n"
        ),
    );
    assert_defect(
        &alone,
        "through that same channel, because nobody could ever receive the message",
    );
    let shared = run_virtual(
        "own-receiver-shared",
        &format!(
            "{hold}pub fn main() -> Int {{\n\
             \x20 let channel = Channel.bounded(1)\n\
             \x20 let other = Channel.add_receiver(channel.receiver)\n\
             \x20 Channel.send(channel.sender, Hold.Has(channel.receiver))\n\
             \x20 let next = Channel.receive(other)\n\
             \x20 case next {{\n\
             \x20   Receive.Received(Hold.Has(_)) -> 7\n\
             \x20   Receive.Received(Hold.Empty) -> 1\n\
             \x20   Receive.Drained -> 0\n\
             \x20 }}\n\
             }}\n"
        ),
    );
    assert!(shared.contains("\"outcome\":\"success\",\"value\":7"), "{shared}");
}
