mod model;

use std::env;
use std::io::{self, Write};

use model::{Action, State};

const BOLD: &str = "\x1b[1m";
const DIM: &str = "\x1b[2m";
const RESET: &str = "\x1b[0m";
const CLEAR: &str = "\x1b[2J\x1b[H";

struct Step {
    description: &'static str,
    action: Action,
}

struct Scenario {
    name: &'static str,
    question: &'static str,
    initial: State,
    steps: Vec<Step>,
}

fn step(description: &'static str, action: Action) -> Step {
    Step {
        description,
        action,
    }
}

fn scenarios() -> Vec<Scenario> {
    vec![
        Scenario {
            name: "Ordinary call suspension",
            question: "Does receive park the current task without creating concurrent work?",
            initial: State::new(&[("main", None)], &[("events", 1)]),
            steps: vec![
                step(
                    "main calls Channel.receive(events)",
                    Action::Receive {
                        task: "main".into(),
                        channel: "events".into(),
                    },
                ),
                step(
                    "the host delivers an external event",
                    Action::HostSend {
                        channel: "events".into(),
                        value: "Ready".into(),
                    },
                ),
                step(
                    "main completes after resuming at the call site",
                    Action::Succeed {
                        task: "main".into(),
                        value: "Ready".into(),
                    },
                ),
            ],
        },
        Scenario {
            name: "Zero-capacity rendezvous",
            question: "Does capacity zero pair sender and receiver without buffering?",
            initial: State::new(&[("sender", None), ("receiver", None)], &[("handoff", 0)]),
            steps: vec![
                step(
                    "sender attempts A and parks because no receiver is ready",
                    Action::Send {
                        task: "sender".into(),
                        channel: "handoff".into(),
                        value: "A".into(),
                    },
                ),
                step(
                    "receiver pairs with the oldest sender; A is never buffered",
                    Action::Receive {
                        task: "receiver".into(),
                        channel: "handoff".into(),
                    },
                ),
            ],
        },
        Scenario {
            name: "Bounded backpressure",
            question: "Does a full queue park only the sender and wake it when capacity returns?",
            initial: State::new(&[("main", None)], &[("jobs", 1)]),
            steps: vec![
                step(
                    "main starts producer",
                    Action::StartTask {
                        owner: "main".into(),
                        child: "producer".into(),
                    },
                ),
                step(
                    "main starts consumer",
                    Action::StartTask {
                        owner: "main".into(),
                        child: "consumer".into(),
                    },
                ),
                step(
                    "producer buffers job-1",
                    Action::Send {
                        task: "producer".into(),
                        channel: "jobs".into(),
                        value: "job-1".into(),
                    },
                ),
                step(
                    "producer attempts job-2 and parks because capacity is one",
                    Action::Send {
                        task: "producer".into(),
                        channel: "jobs".into(),
                        value: "job-2".into(),
                    },
                ),
                step(
                    "consumer receives job-1; job-2 is admitted and producer resumes",
                    Action::Receive {
                        task: "consumer".into(),
                        channel: "jobs".into(),
                    },
                ),
                step(
                    "consumer receives job-2",
                    Action::Receive {
                        task: "consumer".into(),
                        channel: "jobs".into(),
                    },
                ),
            ],
        },
        Scenario {
            name: "FIFO sender admission",
            question: "Do parked producers enter in request order without barging?",
            initial: State::new(
                &[
                    ("seed", None),
                    ("producer-1", None),
                    ("producer-2", None),
                    ("consumer", None),
                ],
                &[("jobs", 1)],
            ),
            steps: vec![
                step(
                    "seed fills the only buffer slot",
                    Action::Send {
                        task: "seed".into(),
                        channel: "jobs".into(),
                        value: "seed".into(),
                    },
                ),
                step(
                    "producer-1 parks while attempting A",
                    Action::Send {
                        task: "producer-1".into(),
                        channel: "jobs".into(),
                        value: "A".into(),
                    },
                ),
                step(
                    "producer-2 parks behind it while attempting B",
                    Action::Send {
                        task: "producer-2".into(),
                        channel: "jobs".into(),
                        value: "B".into(),
                    },
                ),
                step(
                    "consumer receives seed; the freed slot is reserved for producer-1",
                    Action::Receive {
                        task: "consumer".into(),
                        channel: "jobs".into(),
                    },
                ),
                step(
                    "consumer receives A; producer-2 is admitted next",
                    Action::Receive {
                        task: "consumer".into(),
                        channel: "jobs".into(),
                    },
                ),
                step(
                    "consumer receives B",
                    Action::Receive {
                        task: "consumer".into(),
                        channel: "jobs".into(),
                    },
                ),
            ],
        },
        Scenario {
            name: "Graceful close and drain",
            question: "Does close reject new sends while preserving already accepted values?",
            initial: State::new(&[("sender", None), ("consumer", None)], &[("items", 2)]),
            steps: vec![
                step(
                    "sender buffers A",
                    Action::Send {
                        task: "sender".into(),
                        channel: "items".into(),
                        value: "A".into(),
                    },
                ),
                step(
                    "sender buffers B",
                    Action::Send {
                        task: "sender".into(),
                        channel: "items".into(),
                        value: "B".into(),
                    },
                ),
                step(
                    "sender attempts C and parks because the buffer is full",
                    Action::Send {
                        task: "sender".into(),
                        channel: "items".into(),
                        value: "C".into(),
                    },
                ),
                step(
                    "the coordinator closes; unaccepted C returns as Channel.Closed(C)",
                    Action::Close {
                        channel: "items".into(),
                    },
                ),
                step(
                    "consumer receives accepted A",
                    Action::Receive {
                        task: "consumer".into(),
                        channel: "items".into(),
                    },
                ),
                step(
                    "consumer receives accepted B",
                    Action::Receive {
                        task: "consumer".into(),
                        channel: "items".into(),
                    },
                ),
                step(
                    "consumer observes Drained only after the queue drains",
                    Action::Receive {
                        task: "consumer".into(),
                        channel: "items".into(),
                    },
                ),
            ],
        },
        Scenario {
            name: "Destructive abort",
            question: "Does abort discard buffered work and fail operations without cancelling unrelated tasks?",
            initial: State::new(
                &[("sender", None), ("consumer", None), ("observer", None)],
                &[("items", 1)],
            ),
            steps: vec![
                step(
                    "sender buffers A",
                    Action::Send {
                        task: "sender".into(),
                        channel: "items".into(),
                        value: "A".into(),
                    },
                ),
                step(
                    "sender attempts B and parks",
                    Action::Send {
                        task: "sender".into(),
                        channel: "items".into(),
                        value: "B".into(),
                    },
                ),
                step(
                    "consumer aborts; A is discarded and B returns as Channel.Aborted(B)",
                    Action::Abort {
                        channel: "items".into(),
                    },
                ),
            ],
        },
        Scenario {
            name: "Cancelled send is not delivered",
            question: "Does cancellation remove a parked send before it can be accepted?",
            initial: State::new(&[("producer", None), ("consumer", None)], &[("items", 1)]),
            steps: vec![
                step(
                    "producer buffers A",
                    Action::Send {
                        task: "producer".into(),
                        channel: "items".into(),
                        value: "A".into(),
                    },
                ),
                step(
                    "producer attempts B and parks",
                    Action::Send {
                        task: "producer".into(),
                        channel: "items".into(),
                        value: "B".into(),
                    },
                ),
                step(
                    "producer is cancelled before B is admitted",
                    Action::Cancel {
                        task: "producer".into(),
                    },
                ),
                step(
                    "consumer receives accepted A; cancelled B does not enter the queue",
                    Action::Receive {
                        task: "consumer".into(),
                        channel: "items".into(),
                    },
                ),
                step(
                    "the coordinator closes the now-empty channel",
                    Action::Close {
                        channel: "items".into(),
                    },
                ),
                step(
                    "consumer observes Drained rather than B",
                    Action::Receive {
                        task: "consumer".into(),
                        channel: "items".into(),
                    },
                ),
            ],
        },
        Scenario {
            name: "Failure observation",
            question: "Does expected Failure remain inert until observed, then bubble through structured unwinding?",
            initial: State::new(&[("main", None)], &[]),
            steps: vec![
                step(
                    "main starts failing child",
                    Action::StartTask {
                        owner: "main".into(),
                        child: "failing".into(),
                    },
                ),
                step(
                    "main starts sibling",
                    Action::StartTask {
                        owner: "main".into(),
                        child: "sibling".into(),
                    },
                ),
                step(
                    "child returns an expected Failure",
                    Action::Fail {
                        task: "failing".into(),
                        error: "Job.Unavailable".into(),
                    },
                ),
                step(
                    "main observes and bubbles Failure; structured unwind cancels sibling",
                    Action::Observe {
                        owner: "main".into(),
                        child: "failing".into(),
                    },
                ),
            ],
        },
        Scenario {
            name: "Defect containment",
            question: "Does a child defect immediately unwind its task containment without becoming Failure?",
            initial: State::new(&[("main", None)], &[]),
            steps: vec![
                step(
                    "main starts defective child",
                    Action::StartTask {
                        owner: "main".into(),
                        child: "defective".into(),
                    },
                ),
                step(
                    "main starts sibling",
                    Action::StartTask {
                        owner: "main".into(),
                        child: "sibling".into(),
                    },
                ),
                step(
                    "child defects; main defects immediately and structured unwind cancels sibling",
                    Action::Defect {
                        task: "defective".into(),
                        cause: "Runtime.InvariantBroken".into(),
                    },
                ),
            ],
        },
        Scenario {
            name: "Cancellation removes waiters",
            question: "Does cancelling a parked task remove its channel wait and recursively cancel its children?",
            initial: State::new(&[("main", None)], &[("events", 1)]),
            steps: vec![
                step(
                    "main starts listener",
                    Action::StartTask {
                        owner: "main".into(),
                        child: "listener".into(),
                    },
                ),
                step(
                    "listener starts parser",
                    Action::StartTask {
                        owner: "listener".into(),
                        child: "parser".into(),
                    },
                ),
                step(
                    "listener parks on receive",
                    Action::Receive {
                        task: "listener".into(),
                        channel: "events".into(),
                    },
                ),
                step(
                    "main cancels listener",
                    Action::Cancel {
                        task: "listener".into(),
                    },
                ),
            ],
        },
    ]
}

fn render(scenario: &Scenario, state: &State, next_step: usize, interactive: bool) {
    if interactive {
        print!("{CLEAR}");
    }
    println!("{BOLD}Chimi task/channel semantics{RESET}");
    println!("{BOLD}scenario:{RESET} {}", scenario.name);
    println!("{DIM}{}{RESET}", scenario.question);
    println!(
        "{BOLD}progress:{RESET} {next_step}/{}",
        scenario.steps.len()
    );

    println!("\n{BOLD}Tasks{RESET}");
    for (name, task) in &state.tasks {
        let owner = task.owner.as_deref().unwrap_or("runtime root");
        let result = task.last_result.as_deref().unwrap_or("—");
        println!(
            "  {name:<12} owner={owner:<12} status={:<34} observed={} result={result}",
            task.status.label(),
            task.observed
        );
    }

    println!("\n{BOLD}Channels{RESET}");
    if state.channels.is_empty() {
        println!("  {DIM}none{RESET}");
    }
    for (name, channel) in &state.channels {
        let queue: Vec<&str> = channel.queue.iter().map(String::as_str).collect();
        let waiting: Vec<String> = channel
            .waiting_senders
            .iter()
            .map(|send| format!("{}:{}", send.task, send.value))
            .collect();
        println!(
            "  {name:<12} capacity={} state={} queue={queue:?}",
            channel.capacity,
            channel.lifecycle.label(channel.queue.is_empty())
        );
        println!(
            "  {:<12} waiting_senders={waiting:?} waiting_receiver={}",
            "",
            channel.waiting_receiver.as_deref().unwrap_or("—")
        );
    }

    println!("\n{BOLD}Recent events{RESET}");
    for event in state.events.iter().rev().take(6).rev() {
        println!("  {event}");
    }

    if let Some(step) = scenario.steps.get(next_step) {
        println!("\n{BOLD}Next{RESET}: {}", step.description);
    } else {
        println!("\n{BOLD}Scenario complete{RESET}");
    }
}

fn run_all() -> Result<(), String> {
    for scenario in scenarios() {
        let mut state = scenario.initial.clone();
        println!("## {}", scenario.name);
        println!("{}", scenario.question);
        for (index, step) in scenario.steps.iter().enumerate() {
            state.apply(&step.action)?;
            println!("{}. {}", index + 1, step.description);
            println!(
                "   {}",
                state.events.last().expect("action emitted an event")
            );
        }
        let tasks = state
            .tasks
            .iter()
            .map(|(name, task)| format!("{name}={}", task.status.label()))
            .collect::<Vec<_>>()
            .join(", ");
        println!("   final tasks: {tasks}");
        for (name, channel) in &state.channels {
            println!(
                "   final channel {name}: queue={:?}, state={}",
                channel.queue,
                channel.lifecycle.label(channel.queue.is_empty())
            );
        }
        println!();
    }
    Ok(())
}

fn run_interactive() -> Result<(), String> {
    let scenarios = scenarios();
    let mut scenario_index = 0;
    let mut next_step = 0;
    let mut state = scenarios[scenario_index].initial.clone();
    loop {
        let scenario = &scenarios[scenario_index];
        render(scenario, &state, next_step, true);
        println!("\n[n] next  [r] reset  [s] scenario  [q] quit");
        print!("> ");
        io::stdout().flush().map_err(|error| error.to_string())?;
        let mut command = String::new();
        if io::stdin()
            .read_line(&mut command)
            .map_err(|error| error.to_string())?
            == 0
        {
            return Ok(());
        }
        match command.trim() {
            "n" if next_step < scenario.steps.len() => {
                state.apply(&scenario.steps[next_step].action)?;
                next_step += 1;
            }
            "r" => {
                state = scenario.initial.clone();
                next_step = 0;
            }
            "s" => {
                scenario_index = (scenario_index + 1) % scenarios.len();
                state = scenarios[scenario_index].initial.clone();
                next_step = 0;
            }
            "q" => return Ok(()),
            _ => {}
        }
    }
}

fn main() {
    let result = if env::args().any(|argument| argument == "--all") {
        run_all()
    } else {
        run_interactive()
    };
    if let Err(error) = result {
        eprintln!("prototype transition failed: {error}");
        std::process::exit(1);
    }
}
