//! The runner's `--carriers=<n>` (parallel-scheduler.md, step 3 PR 5d-4):
//! a program's processes run on that many carriers, carriers 1 and up on
//! threads of their own, with the same result as on one. The flag is
//! refused when it is not a count from 1 through 256, and above 1 with the
//! virtual clock.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

/// Four workers, each a process on its own membership (so on carriers 0
/// to 3 with four carriers), answer 50 requests each. Every answer does
/// some allocating work in the worker and in a task it starts, so heaps on
/// every carrier thread are used. The sum does not depend on the order the
/// answers arrive in.
const WORKERS: &str = concat!(
    "import std/channel as Channel\n",
    "import std/channel.{Receive, Sender}\n",
    "import std/duration as Duration\n",
    "import std/list as List\n",
    "import std/process as Process\n",
    "import std/process.{Inbox}\n",
    "import std/supervisor as Supervisor\n",
    "import std/task as Task\n",
    "pub enum Message {\n  Work(size: Int, reply: Sender<Int>)\n}\n",
    "fn deep(n: Int) -> Int {\n  case n {\n    0 -> 0\n    _ -> 1 + deep(n - 1)\n  }\n}\n",
    "fn items(n: Int) -> List<Int> {\n",
    "  loop state = (0, []) {\n",
    "    let (step, kept) = state\n",
    "    case step == n {\n",
    "      True -> break kept\n",
    "      False -> (step + 1, List.append(kept, [step]))\n",
    "    }\n",
    "  }\n",
    "}\n",
    "fn work(inbox: Inbox<Message>) -> Void {\n",
    "  loop handled = 0 {\n",
    "    case Process.receive(inbox) {\n",
    "      Message.Work(size, reply) -> {\n",
    "        let helper = Task.start(fn() { deep(size) })\n",
    "        Channel.send(reply, List.length(items(size)) + Task.result(helper))\n",
    "        handled + 1\n",
    "      }\n",
    "    }\n",
    "  }\n",
    "  Void\n",
    "}\n",
    "fn received(next: Receive<Int>) -> Int {\n",
    "  case next {\n    Receive.Received(value) -> value\n    Receive.Drained -> 0\n  }\n",
    "}\n",
    "pub fn main() -> Int {\n",
    "  let root = Supervisor.start(Supervisor.policy([], 4, Supervisor.restarts(0, Duration.seconds(30))))\n",
    "  let spec = Process.spec(work, 64, Duration.seconds(1))\n",
    "  let a = Supervisor.address(Supervisor.start_process(root, spec, Supervisor.Restart.Never))\n",
    "  let b = Supervisor.address(Supervisor.start_process(root, spec, Supervisor.Restart.Never))\n",
    "  let c = Supervisor.address(Supervisor.start_process(root, spec, Supervisor.Restart.Never))\n",
    "  let d = Supervisor.address(Supervisor.start_process(root, spec, Supervisor.Restart.Never))\n",
    "  let replies = Channel.bounded(256)\n",
    "  let sent = loop round = 0 {\n",
    "    case round == 50 {\n",
    "      True -> break round\n",
    "      False -> {\n",
    "        let _ = Process.send(a, Message.Work(round, replies.sender))\n",
    "        let _ = Process.send(b, Message.Work(round, replies.sender))\n",
    "        let _ = Process.send(c, Message.Work(round, replies.sender))\n",
    "        let _ = Process.send(d, Message.Work(round, replies.sender))\n",
    "        round + 1\n",
    "      }\n",
    "    }\n",
    "  }\n",
    "  let total = loop state = (0, 0) {\n",
    "    let (count, sum) = state\n",
    "    case count == sent * 4 {\n",
    "      True -> break sum\n",
    "      False -> (count + 1, sum + received(Channel.receive(replies.receiver)))\n",
    "    }\n",
    "  }\n",
    "  Supervisor.stop(root)\n",
    "  total\n",
    "}\n",
);

fn scratch_directory(name: &str) -> PathBuf {
    let path = std::env::temp_dir().join(format!(
        "chimi-carriers-{name}-{}-{:?}",
        std::process::id(),
        std::thread::current().id()
    ));
    if path.exists() {
        std::fs::remove_dir_all(&path).unwrap();
    }
    std::fs::create_dir_all(&path).unwrap();
    path
}

fn build(scratch: &Path, name: &str, source: &str) -> PathBuf {
    let path = scratch.join(format!("{name}.chimi"));
    let executable = scratch.join(name);
    std::fs::write(&path, source).unwrap();
    let build = Command::new(env!("CARGO_BIN_EXE_chimi"))
        .current_dir(scratch)
        .arg("build")
        .arg(&path)
        .arg("-o")
        .arg(&executable)
        .output()
        .expect("build native program");
    assert!(build.status.success(), "{}", String::from_utf8_lossy(&build.stderr));
    executable
}

/// Runs `executable` with the runner arguments `flags`, returning the
/// process and its outcome file.
fn run(executable: &Path, scratch: &Path, flags: &[&str]) -> (Output, String) {
    let outcome = scratch.join("outcome.jsonl");
    let _ = std::fs::remove_file(&outcome);
    let process = Command::new(executable)
        .arg("--chimi-runner")
        .args(flags)
        .arg(format!("--outcome-file={}", outcome.display()))
        .arg("--")
        .output()
        .expect("run native program");
    (process, std::fs::read_to_string(&outcome).unwrap_or_default())
}

fn success(value: &str) -> String {
    format!("{{\"schema\":\"chimi.runtime.v1\",\"outcome\":\"success\",\"value\":{value}}}\n")
}

/// The same program on one, two, four and eight carriers gives the same
/// value, again and again; with four or more, each worker runs on a
/// carrier thread of its own.
#[test]
fn a_program_runs_alike_on_one_carrier_and_on_several() {
    let scratch = scratch_directory("workers");
    let executable = build(&scratch, "workers", WORKERS);
    // Each round r sends r to four workers; each answer is r + r.
    let expected = success(&(4 * 2 * (0..50).sum::<i64>()).to_string());
    let (one, outcome) = run(&executable, &scratch, &["--clock=real"]);
    assert!(one.status.success(), "{one:?}");
    assert_eq!(outcome, expected);
    for carriers in ["1", "2", "4", "8"] {
        for _ in 0..10 {
            let flag = format!("--carriers={carriers}");
            let (process, outcome) = run(&executable, &scratch, &["--clock=real", &flag]);
            assert!(process.status.success(), "{carriers} carriers: {process:?}");
            assert_eq!(outcome, expected, "{carriers} carriers");
        }
    }
    std::fs::remove_dir_all(scratch).unwrap();
}

/// A count that is not 1 through 256 exits 64 before the program runs, as
/// does more than one carrier under the virtual clock, which moves only
/// when every carrier is idle.
#[test]
fn the_runner_refuses_a_bad_carrier_count() {
    let scratch = scratch_directory("refused");
    let executable = build(&scratch, "refused", "pub fn main() -> Int {\n  7\n}\n");
    for value in ["0", "-1", "", "two", "4x", "257", "18446744073709551617"] {
        let flag = format!("--carriers={value}");
        let (process, outcome) = run(&executable, &scratch, &[&flag]);
        assert_eq!(process.status.code(), Some(64), "--carriers={value}: {process:?}");
        assert!(
            String::from_utf8_lossy(&process.stderr).contains("--carriers requires an integer from 1 through 256"),
            "{process:?}"
        );
        assert_eq!(outcome, "", "nothing ran");
    }
    for clock in [&["--clock=virtual", "--carriers=2"][..], &["--carriers=2", "--clock=virtual"]] {
        let (process, outcome) = run(&executable, &scratch, clock);
        assert_eq!(process.status.code(), Some(64), "{process:?}");
        assert!(
            String::from_utf8_lossy(&process.stderr).contains("--carriers above 1 requires --clock=real"),
            "{process:?}"
        );
        assert_eq!(outcome, "");
    }
    let (process, outcome) = run(&executable, &scratch, &["--clock=virtual", "--carriers=1"]);
    assert!(process.status.success(), "{process:?}");
    assert_eq!(outcome, success("7"));
    let (process, outcome) = run(&executable, &scratch, &["--carriers=256"]);
    assert!(process.status.success(), "{process:?}");
    assert_eq!(outcome, success("7"));
    std::fs::remove_dir_all(scratch).unwrap();
}

/// With `--carriers=4` the program runs on four threads while it runs:
/// the main thread, which is carrier 0, and one for each other carrier.
/// The program sleeps half a second so the threads can be counted.
#[cfg(target_os = "linux")]
#[test]
fn four_carriers_run_on_four_threads() {
    let scratch = scratch_directory("threads");
    let executable = build(
        &scratch,
        "threads",
        concat!(
            "import std/clock as Clock\n",
            "import std/clock.{Clock}\n",
            "import std/duration as Duration\n",
            "pub fn main() -> Int needs { clock: Clock } {\n",
            "  Clock.sleep(Duration.milliseconds(500))\n",
            "  7\n",
            "}\n",
        ),
    );
    let count_threads = |carriers: &str| {
        let mut child = Command::new(&executable)
            .arg("--chimi-runner")
            .arg("--clock=real")
            .arg(format!("--carriers={carriers}"))
            .arg("--")
            .spawn()
            .expect("run native program");
        let tasks = format!("/proc/{}/task", child.id());
        let mut most = 0;
        let started = std::time::Instant::now();
        while started.elapsed() < std::time::Duration::from_millis(400) {
            if let Ok(entries) = std::fs::read_dir(&tasks) {
                most = most.max(entries.count());
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        assert!(child.wait().expect("wait").success());
        most
    };
    assert_eq!(count_threads("1"), 1);
    assert_eq!(count_threads("4"), 4);
    std::fs::remove_dir_all(scratch).unwrap();
}
