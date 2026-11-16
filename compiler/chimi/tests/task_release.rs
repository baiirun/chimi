//! A finished task's record is freed once its handle can no longer be
//! reached (supervised-process-runtime.md, "Task release"; CHI-264).
//!
//! Each program runs natively and writes a memory report, whose
//! `task_records_peak` is the most task records the scheduler held at once.

use std::process::Command;

const IMPORTS: &str = concat!(
    "import std/channel as Channel\n",
    "import std/channel.{Receive, Receiver}\n",
    "import std/clock as Clock\n",
    "import std/clock.{Instant, Timestamp}\n",
    "import std/duration as Duration\n",
    "import std/list as List\n",
    "import std/select as Select\n",
    "import std/task as Task\n",
);

struct Run {
    outcome: String,
    peak_task_records: u64,
}

fn run(name: &str, body: &str) -> Run {
    let scratch = std::env::temp_dir().join(format!(
        "chimi-task-release-{name}-{}",
        std::process::id()
    ));
    if scratch.exists() {
        std::fs::remove_dir_all(&scratch).unwrap();
    }
    std::fs::create_dir_all(&scratch).unwrap();
    let source = scratch.join(format!("{name}.chimi"));
    let executable = scratch.join(name);
    std::fs::write(&source, format!("{IMPORTS}{body}")).unwrap();
    let build = Command::new(env!("CARGO_BIN_EXE_chimi"))
        .current_dir(&scratch)
        .arg("build")
        .arg(&source)
        .arg("-o")
        .arg(&executable)
        .output()
        .expect("build native task program");
    assert!(
        build.status.success(),
        "{}",
        String::from_utf8_lossy(&build.stderr)
    );
    let outcome_path = scratch.join("outcome.jsonl");
    let report_path = scratch.join("memory.json");
    let process = Command::new(&executable)
        .arg("--chimi-runner")
        .arg("--trace=off")
        .arg("--clock=virtual")
        .arg(format!("--outcome-file={}", outcome_path.display()))
        .arg(format!("--memory-report-file={}", report_path.display()))
        .arg("--")
        .output()
        .expect("execute native task program");
    let outcome = std::fs::read_to_string(outcome_path).unwrap_or_default();
    let report = std::fs::read_to_string(report_path).unwrap_or_else(|_| {
        panic!("no memory report: {}", String::from_utf8_lossy(&process.stderr))
    });
    std::fs::remove_dir_all(scratch).unwrap();
    Run {
        outcome,
        peak_task_records: json_u64_field(&report, "task_records_peak"),
    }
}

fn json_u64_field(document: &str, name: &str) -> u64 {
    let marker = format!("\"{name}\":");
    document
        .split_once(&marker)
        .unwrap_or_else(|| panic!("{name} missing from {document}"))
        .1
        .split(|character: char| !character.is_ascii_digit())
        .next()
        .unwrap()
        .parse()
        .unwrap()
}

fn success(value: &str) -> String {
    format!("{{\"schema\":\"chimi.runtime.v1\",\"outcome\":\"success\",\"value\":{value}}}\n")
}

#[test]
fn a_loop_of_finished_tasks_holds_a_bounded_number_of_records() {
    let source = concat!(
        "pub fn main() -> Int {\n",
        "  loop count = 0 {\n",
        "    case count == 5000 {\n",
        "      True -> break count\n",
        "      False -> {\n",
        "        let child = Task.start(fn() { count })\n",
        "        count + 1 + Task.result(child) - count\n",
        "      }\n",
        "    }\n",
        "  }\n",
        "}\n",
    );
    let run = run("bounded", source);
    assert_eq!(run.outcome, success("5000"));
    // The root plus the one child of the current pass.
    assert!(run.peak_task_records <= 3, "{}", run.peak_task_records);
}

#[test]
fn a_handle_still_held_reads_the_result_again() {
    let source = concat!(
        "pub fn main() -> Int {\n",
        "  let child = Task.start(fn() { 20 })\n",
        "  let first = Task.result(child)\n",
        "  let total = loop step = 0 {\n",
        "    case step == 3 {\n",
        "      True -> break step\n",
        "      False -> {\n",
        "        let other = Task.start(fn() { 1 })\n",
        "        step + Task.result(other)\n",
        "      }\n",
        "    }\n",
        "  }\n",
        "  first + Task.result(child) + total - 1\n",
        "}\n",
    );
    let run = run("held", source);
    assert_eq!(run.outcome, success("42"));
}

#[test]
fn handles_kept_in_loop_state_survive_each_pass() {
    let source = concat!(
        "pub fn main() -> Int {\n",
        "  let children = loop state = (0, []) {\n",
        "    let (step, kept) = state\n",
        "    case step == 4 {\n",
        "      True -> break kept\n",
        "      False -> (step + 1, List.append(kept, [Task.start(fn() { step + 10 })]))\n",
        "    }\n",
        "  }\n",
        "  List.reduce(children, 0, fn(sum, child) { sum + Task.result(child) })\n",
        "}\n",
    );
    let run = run("kept", source);
    assert_eq!(run.outcome, success("46"));
    assert_eq!(run.peak_task_records, 5);
}

// CHI-225: a user-clock timer runs in a hidden child task. When another
// event wins, the select cancels that child and drops its handle, so the
// record goes once the child has drained, like any other released task.
#[test]
fn losing_user_clock_timers_leave_no_records_behind() {
    let source = concat!(
        "fn received(next: Receive<Int>) -> Int {\n",
        "  case next {\n",
        "    Receive.Received(value) -> value\n",
        "    Receive.Drained -> 0\n",
        "  }\n",
        "}\n",
        "fn wait(receiver: Receiver<Int>) -> Int {\n",
        "  Select.first([Select.receive(receiver, received), Select.after(Duration.seconds(10), 0)])\n",
        "}\n",
        "pub fn main() -> Int needs { clock: Clock.Clock } {\n",
        "  let base = clock\n",
        "  let passing = {\n",
        "    now: fn() -> Instant { base.now() },\n",
        "    timestamp: fn() -> Timestamp { base.timestamp() },\n",
        "    sleep_until: fn(at: Instant) -> Void { base.sleep_until(at) },\n",
        "  }\n",
        "  let waited = wait using { clock: passing }\n",
        "  let channel = Channel.bounded(1)\n",
        "  loop count = 0 {\n",
        "    case count == 2000 {\n",
        "      True -> break count\n",
        "      False -> {\n",
        "        Channel.send(channel.sender, 1)\n",
        "        count + waited(channel.receiver)\n",
        "      }\n",
        "    }\n",
        "  }\n",
        "}\n",
    );
    let run = run("select-timers", source);
    assert_eq!(run.outcome, success("2000"));
    assert!(run.peak_task_records <= 4, "{}", run.peak_task_records);
}

// A handle copied to another heap is one more reference. Generic code can
// send a handle on a channel, which copies it into the message and then into
// the receiver's heap; the original and the message go away while the copy
// is still live.
#[test]
fn a_handle_sent_through_a_channel_still_reads_its_result() {
    let source = concat!(
        "fn pass<T>(sender: Channel.Sender<T>, value: T) -> Void { Channel.send(sender, value) }\n",
        "pub fn main() -> Int {\n",
        "  let handles = Channel.bounded(1)\n",
        "  let child = loop step = 0 {\n",
        "    case step {\n",
        "      0 -> {\n",
        "        pass(handles.sender, Task.start(fn() { 42 }))\n",
        "        1\n",
        "      }\n",
        "      3 -> break Channel.receive(handles.receiver)\n",
        "      _ -> step + Task.result(Task.start(fn() { 1 }))\n",
        "    }\n",
        "  }\n",
        "  let total = loop step = 0 {\n",
        "    case step == 3 {\n",
        "      True -> break step\n",
        "      False -> step + Task.result(Task.start(fn() { 1 }))\n",
        "    }\n",
        "  }\n",
        "  case child {\n",
        "    Receive.Received(task) -> Task.result(task) + total - 3\n",
        "    Receive.Drained -> 0\n",
        "  }\n",
        "}\n",
    );
    let run = run("sent", source);
    assert_eq!(run.outcome, success("42"));
}
