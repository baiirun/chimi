//! Looped stress programs at 2 to 8 carriers (parallel-scheduler.md, step 3
//! PR 5e). Each program's value does not depend on how its processes
//! interleave or on how long anything takes, so every run on any number of
//! carriers must give the value one carrier gives: a process ring passing
//! tokens, selects racing plain receives on one queue, a restart storm with
//! shutdown deadlines, stops that force processes parked in their cleanup,
//! and socket handles opened and closed on every carrier.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

const IMPORTS: &str = r#"import std/bytes as Bytes
import std/channel as Channel
import std/channel.{Receive, Receiver, Sender}
import std/clock as Clock
import std/clock.{Clock}
import std/duration as Duration
import std/net as Net
import std/net.{Network}
import std/process as Process
import std/process.{Inbox}
import std/resource as Resource
import std/result as Result
import std/select as Select
import std/stream as Stream
import std/supervisor as Supervisor
import std/task as Task
"#;

/// Six processes in a ring pass four tokens around it at once, 3000 hops
/// each; every token reports when its hops run out.
const RING: &str = r#"
pub enum Message {
  Next(to: Process.Address<Message>)
  Token(hops: Int, done: Sender<Int>)
}

fn relay(inbox: Inbox<Message>) -> Void {
  let first = case Process.receive(inbox) {
    Message.Next(to) -> to
    Message.Token(_, _) -> Debug.crash("a token before the ring was closed")
  }
  loop next = first {
    case Process.receive(inbox) {
      Message.Next(to) -> to
      Message.Token(hops, done) -> {
        case hops == 0 {
          True -> Channel.send(done, 1)
          False -> {
            let _ = Process.send(next, Message.Token(hops - 1, done))
            Void
          }
        }
        next
      }
    }
  }
  Void
}

pub fn main() -> Int {
  let root = Supervisor.start(Supervisor.policy([], 6, Supervisor.restarts(0, Duration.seconds(30))))
  let spec = Process.spec(relay, 64, Duration.seconds(1))
  let a = Supervisor.address(Supervisor.start_process(root, spec, Supervisor.Restart.Never))
  let b = Supervisor.address(Supervisor.start_process(root, spec, Supervisor.Restart.Never))
  let c = Supervisor.address(Supervisor.start_process(root, spec, Supervisor.Restart.Never))
  let d = Supervisor.address(Supervisor.start_process(root, spec, Supervisor.Restart.Never))
  let e = Supervisor.address(Supervisor.start_process(root, spec, Supervisor.Restart.Never))
  let f = Supervisor.address(Supervisor.start_process(root, spec, Supervisor.Restart.Never))
  let _ = Process.send(a, Message.Next(b))
  let _ = Process.send(b, Message.Next(c))
  let _ = Process.send(c, Message.Next(d))
  let _ = Process.send(d, Message.Next(e))
  let _ = Process.send(e, Message.Next(f))
  let _ = Process.send(f, Message.Next(a))
  let done = Channel.bounded(4)
  let _ = Process.send(a, Message.Token(3000, done.sender))
  let _ = Process.send(c, Message.Token(3000, done.sender))
  let _ = Process.send(e, Message.Token(3000, done.sender))
  let _ = Process.send(b, Message.Token(3000, done.sender))
  let finished = loop count = 0 {
    case count == 4 {
      True -> break count
      False -> {
        let next = Channel.receive(done.receiver)
        case next {
          Receive.Received(one) -> count + one
          Receive.Drained -> break -1
        }
      }
    }
  }
  Supervisor.stop(root)
  finished
}
"#;

/// Six processes share one queue through receivers of their own, half
/// taking values with `Select.first` and half with `Channel.receive`, while
/// main sends 1 to 4000 and then closes it. Each value must reach exactly
/// one of them: the counts add up to 4000 and the values to their sum.
const SELECT_RACE: &str = r#"
pub enum Message {
  Drain(jobs: Receiver<Int>, selects: Bool, reply: Sender<Int>)
}

fn same(next: Receive<Int>) -> Receive<Int> {
  next
}

fn drain(inbox: Inbox<Message>) -> Void {
  case Process.receive(inbox) {
    Message.Drain(jobs, selects, reply) -> {
      let (count, sum) = loop state = (0, 0) {
        let (count, sum) = state
        let next = case selects {
          True -> Select.first([Select.receive(jobs, same)])
          False -> Channel.receive(jobs)
        }
        case next {
          Receive.Received(value) -> (count + 1, sum + value)
          Receive.Drained -> break state
        }
      }
      Channel.send(reply, count * 1000000000 + sum)
    }
  }
  loop idle = 0 {
    let _ = Process.receive(inbox)
    idle + 1
  }
  Void
}

pub fn main() -> Int {
  let root = Supervisor.start(Supervisor.policy([], 6, Supervisor.restarts(0, Duration.seconds(30))))
  let spec = Process.spec(drain, 4, Duration.seconds(1))
  let jobs = Channel.bounded(16)
  let replies = Channel.bounded(6)
  let started = loop worker = 0 {
    case worker == 6 {
      True -> break worker
      False -> {
        let to = Supervisor.address(Supervisor.start_process(root, spec, Supervisor.Restart.Never))
        let _ = Process.send(to, Message.Drain(Channel.add_receiver(jobs.receiver), worker % 2 == 0, replies.sender))
        worker + 1
      }
    }
  }
  let sent = loop value = 1 {
    case value > 4000 {
      True -> break value - 1
      False -> {
        Channel.send(jobs.sender, value)
        value + 1
      }
    }
  }
  Channel.close(jobs.completion)
  let total = loop state = (0, 0) {
    let (answered, total) = state
    case answered == started {
      True -> break total
      False -> {
        let next = Channel.receive(replies.receiver)
        case next {
          Receive.Received(report) -> (answered + 1, total + report)
          Receive.Drained -> break -1
        }
      }
    }
  }
  Supervisor.stop(root)
  case total == sent * 1000000000 + sent * (sent + 1) / 2 {
    True -> sent
    False -> -total
  }
}
"#;

/// Four restartable workers, each under a registered name, answer 50 rounds
/// and defect after every second answer, so their supervisor restarts them
/// about a hundred times. A round whose request reached a dying incarnation,
/// or found none, is asked again after 20 ms. Every body lingers after its
/// answer, past its 10 ms shutdown deadline, so the stop at the end forces
/// whichever are still busy.
const RESTART_STORM: &str = r#"
pub enum Message {
  Work(round: Int, reply: Sender<Int>)
}

pub process name first: Message
pub process name second: Message
pub process name third: Message
pub process name fourth: Message

fn spin(n: Int) -> Int {
  loop i = 0 {
    case i == n {
      True -> break i
      False -> i + 1
    }
  }
}

fn work(inbox: Inbox<Message>) -> Void {
  loop handled = 0 {
    case Process.receive(inbox) {
      Message.Work(round, reply) -> {
        Channel.send(reply, round)
        let _ = spin(20000)
        handled + 1 / (round % 2)
      }
    }
  }
  Void
}

fn answer(next: Receive<Int>) -> Int {
  case next {
    Receive.Received(value) -> value
    Receive.Drained -> -2
  }
}

/// Asks with `send` until the answer to `round` comes back; returns the
/// number of attempts.
fn ask(send: fn(Int) -> Void, receiver: Receiver<Int>, round: Int) -> Int needs { clock: Clock } {
  loop attempts = 1 {
    send(round)
    let got = loop stale = 0 {
      let next = Select.first([Select.receive(receiver, answer), Select.after(Duration.milliseconds(20), -1)])
      case next == round or next == -1 {
        True -> break next
        False -> stale + 1
      }
    }
    case got == round {
      True -> break attempts
      False -> attempts + 1
    }
  }
}

pub fn main() -> Int needs { clock: Clock } {
  let spec = Process.spec(work, 8, Duration.milliseconds(10))
  let root = Supervisor.start(Supervisor.policy([
    spec |> Process.register(first) |> Supervisor.process(Supervisor.Restart.OnDefect),
    spec |> Process.register(second) |> Supervisor.process(Supervisor.Restart.OnDefect),
    spec |> Process.register(third) |> Supervisor.process(Supervisor.Restart.OnDefect),
    spec |> Process.register(fourth) |> Supervisor.process(Supervisor.Restart.OnDefect),
  ], 4, Supervisor.restarts(1000, Duration.seconds(30))))
  let replies = Channel.bounded(64)
  let sender = replies.sender
  let answered = loop round = 1 {
    case round > 50 {
      True -> break round - 1
      False -> {
        let _ = ask(fn(at: Int) {
          let _ = Result.capture(fn() { Process.send(first, Message.Work(at, sender)) })
          Void
        }, replies.receiver, round)
        let _ = ask(fn(at: Int) {
          let _ = Result.capture(fn() { Process.send(second, Message.Work(at, sender)) })
          Void
        }, replies.receiver, round)
        let _ = ask(fn(at: Int) {
          let _ = Result.capture(fn() { Process.send(third, Message.Work(at, sender)) })
          Void
        }, replies.receiver, round)
        let _ = ask(fn(at: Int) {
          let _ = Result.capture(fn() { Process.send(fourth, Message.Work(at, sender)) })
          Void
        }, replies.receiver, round)
        round + 1
      }
    }
  }
  Supervisor.stop(root)
  answered * 4
}
"#;

/// Twenty times, six processes start inside `Resource.scoped` whose release
/// sleeps 200 ms, answer once, and are stopped with a 5 ms shutdown
/// deadline: each body's cancellation unwinds into the release, which parks
/// on the body's own carrier, and the deadline then forces it from wherever
/// it fires, abandoning the parked unwind.
const FORCED_CLEANUP: &str = r#"
pub enum Message {
  Ping(reply: Sender<Int>)
}

fn linger(token: Int) -> Void needs { clock: Clock } {
  Clock.sleep(Duration.milliseconds(200))
}

fn hold(inbox: Inbox<Message>) -> Void needs { clock: Clock } {
  Resource.scoped(fn() { 1 }, linger, fn(token: Int) {
    loop handled = 0 {
      case Process.receive(inbox) {
        Message.Ping(reply) -> {
          Channel.send(reply, token)
          handled + 1
        }
      }
    }
    Void
  })
}

pub fn main() -> Int needs { clock: Clock } {
  let spec = Process.spec(hold, 4, Duration.milliseconds(5))
  let replies = Channel.bounded(6)
  loop state = (0, 0) {
    let (round, answered) = state
    case round == 20 {
      True -> break answered
      False -> {
        let root = Supervisor.start(Supervisor.policy([], 6, Supervisor.restarts(0, Duration.seconds(30))))
        let started = loop worker = 0 {
          case worker == 6 {
            True -> break worker
            False -> {
              let to = Supervisor.address(Supervisor.start_process(root, spec, Supervisor.Restart.Never))
              let _ = Process.send(to, Message.Ping(replies.sender))
              worker + 1
            }
          }
        }
        let heard = loop count = 0 {
          case count == started {
            True -> break count
            False -> {
              let next = Channel.receive(replies.receiver)
              case next {
                Receive.Received(one) -> count + one
                Receive.Drained -> break -1
              }
            }
          }
        }
        Supervisor.stop(root)
        (round + 1, answered + heard)
      }
    }
  }
}
"#;

/// Four processes on carriers of their own each open a loopback listener,
/// connect to it, echo a line and close both ends, 25 times.
const HANDLE_CHURN: &str = r#"
pub enum Message {
  Churn(rounds: Int, reply: Sender<Int>)
}

fn echo_once(request: Text) -> Text needs { network: Network } {
  Resource.scoped(fn() { Net.listen("127.0.0.1", 0) }, Net.close_listener, fn(listener) {
    let port = Net.port(Net.listener_address(listener))
    let server = Task.start(fn() {
      let connection = Net.accept(listener)
      let got = Stream.read_all(Net.reader(connection), 65536)
      Stream.write(Net.writer(connection), got)
      Stream.close(Net.writer(connection))
      Net.close(connection)
    })
    let answer = Resource.scoped(fn() { Net.connect("127.0.0.1", port) }, Net.close, fn(connection) {
      Stream.write(Net.writer(connection), Bytes.from_text(request))
      Stream.close(Net.writer(connection))
      Stream.read_all(Net.reader(connection), 65536)
    })
    Task.result(server)
    Bytes.to_text(answer)
  })
}

fn churn(inbox: Inbox<Message>) -> Void needs { network: Network } {
  loop handled = 0 {
    case Process.receive(inbox) {
      Message.Churn(rounds, reply) -> {
        let echoed = loop state = (0, 0) {
          let (round, echoed) = state
          case round == rounds {
            True -> break echoed
            False -> {
              let echoed_back = echo_once("ping")
              case echoed_back == "ping" {
                True -> (round + 1, echoed + 1)
                False -> (round + 1, echoed)
              }
            }
          }
        }
        Channel.send(reply, echoed)
        handled + 1
      }
    }
  }
  Void
}

pub fn main() -> Int needs { network: Network } {
  let root = Supervisor.start(Supervisor.policy([], 4, Supervisor.restarts(0, Duration.seconds(30))))
  let spec = Process.spec(churn, 64, Duration.seconds(1))
  let replies = Channel.bounded(4)
  let started = loop worker = 0 {
    case worker == 4 {
      True -> break worker
      False -> {
        let to = Supervisor.address(Supervisor.start_process(root, spec, Supervisor.Restart.Never))
        let _ = Process.send(to, Message.Churn(25, replies.sender))
        worker + 1
      }
    }
  }
  let echoed = loop state = (0, 0) {
    let (answered, echoed) = state
    case answered == started {
      True -> break echoed
      False -> {
        let next = Channel.receive(replies.receiver)
        case next {
          Receive.Received(count) -> (answered + 1, echoed + count)
          Receive.Drained -> break -1
        }
      }
    }
  }
  Supervisor.stop(root)
  echoed
}
"#;

fn scratch_directory(name: &str) -> PathBuf {
    let path = std::env::temp_dir().join(format!(
        "chimi-carrier-stress-{name}-{}-{:?}",
        std::process::id(),
        std::thread::current().id()
    ));
    if path.exists() {
        std::fs::remove_dir_all(&path).unwrap();
    }
    std::fs::create_dir_all(&path).unwrap();
    path
}

fn build(scratch: &Path, name: &str, program: &str) -> PathBuf {
    let path = scratch.join(format!("{name}.chimi"));
    let executable = scratch.join(name);
    std::fs::write(&path, format!("{IMPORTS}import std/debug as Debug\n{program}")).unwrap();
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

/// Runs `executable` on `carriers` under the real clock, returning the
/// process and its outcome file.
fn run(executable: &Path, scratch: &Path, carriers: u32) -> (Output, String) {
    let outcome = scratch.join("outcome.jsonl");
    let _ = std::fs::remove_file(&outcome);
    let process = Command::new(executable)
        .arg("--chimi-runner")
        .arg("--clock=real")
        .arg(format!("--carriers={carriers}"))
        .arg(format!("--outcome-file={}", outcome.display()))
        .arg("--")
        .output()
        .expect("run native program");
    (process, std::fs::read_to_string(&outcome).unwrap_or_default())
}

/// True when the runtime is built as the carrier simulation, which refuses
/// more than one carrier.
fn simulation() -> bool {
    std::env::var("CHIMI_TEST_RUNTIME_DEFINES").is_ok_and(|defines| defines.contains("CHIMI_CARRIER_SIMULATION"))
}

fn success(value: i64) -> String {
    format!("{{\"schema\":\"chimi.runtime.v1\",\"outcome\":\"success\",\"value\":{value}}}\n")
}

/// Builds `program` and runs it `runs` times on each of 1, 2, 4 and 8
/// carriers, each run giving `expected`.
fn stress(name: &str, program: &str, expected: i64, runs: usize) {
    let scratch = scratch_directory(name);
    let executable = build(&scratch, name, program);
    let carriers: &[u32] = if simulation() { &[1] } else { &[1, 2, 4, 8] };
    for &count in carriers {
        for run_index in 0..runs {
            let (process, outcome) = run(&executable, &scratch, count);
            assert!(
                process.status.success(),
                "{name}, {count} carriers, run {run_index}: {process:?}"
            );
            assert_eq!(outcome, success(expected), "{name}, {count} carriers, run {run_index}");
        }
    }
    std::fs::remove_dir_all(scratch).unwrap();
}

#[test]
fn tokens_pass_around_a_ring_of_processes_on_every_carrier() {
    stress("ring", RING, 4, 20);
}

#[test]
fn a_select_racing_plain_receives_on_one_queue_loses_and_doubles_nothing() {
    stress("select", SELECT_RACE, 4000, 20);
}

#[test]
fn a_restart_storm_with_shutdown_deadlines_answers_every_round() {
    stress("restarts", RESTART_STORM, 200, 10);
}

#[test]
fn stops_that_force_processes_parked_in_their_cleanup_end_every_round() {
    stress("cleanup", FORCED_CLEANUP, 120, 10);
}

#[test]
fn socket_handles_open_and_close_on_every_carrier() {
    stress("handles", HANDLE_CHURN, 100, 10);
}
