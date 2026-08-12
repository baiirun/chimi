mod model;

use std::io::{self, Read, Write};

use model::{Approach, Model};

const CLEAR: &str = "\x1b[2J\x1b[H";
const BOLD: &str = "\x1b[1m";
const DIM: &str = "\x1b[2m";
const RESET: &str = "\x1b[0m";

fn main() {
    let mut model = Model::new(Approach::SupervisorOwned);

    if std::env::args().any(|argument| argument == "--all") {
        transcript(&mut model);
        return;
    }

    loop {
        render(&model);
        let mut key = [0_u8; 1];
        if io::stdin().read_exact(&mut key).is_err() {
            break;
        }
        match key[0] {
            b'1' => model.defect(0),
            b'2' => model.defect(1),
            b's' => model.switch(),
            b'r' => model.reset(),
            b'q' => break,
            b'\n' | b'\r' => {}
            _ => model.events.push("unknown command".into()),
        }
    }
}

fn transcript(model: &mut Model) {
    println!("=== supervisor-owned restart ===");
    model.defect(0);
    model.defect(1);
    model.defect(0);
    print_state(model);

    println!("\n=== ordinary per-member Retry ===");
    model.switch();
    model.defect(0);
    model.defect(1);
    model.defect(0);
    print_state(model);
}

fn render(model: &Model) {
    print!("{CLEAR}");
    print_state(model);
    println!(
        "\n{BOLD}[1]{RESET} defect counter  {BOLD}[2]{RESET} defect cache  \
         {BOLD}[s]{RESET} switch approach  {BOLD}[r]{RESET} reset  {BOLD}[q]{RESET} quit"
    );
    print!("{DIM}> {RESET}");
    io::stdout().flush().expect("stdout should flush");
}

fn print_state(model: &Model) {
    println!("{BOLD}Restart composition prototype{RESET}");
    println!("{BOLD}approach:{RESET} {}", model.approach.label());
    println!("{BOLD}supervisor:{RESET} {}", state(model.supervisor_running));
    println!(
        "{BOLD}shared budget:{RESET} {}/{}",
        model.shared_restart_attempts, model.restart_limit
    );
    for member in &model.members {
        println!(
            "{BOLD}{}:{RESET} incarnation={} state={} unread={} private_retry={}/{}",
            member.name,
            member.incarnation,
            state(member.running),
            member.unread_messages,
            member.retry_attempts,
            model.restart_limit
        );
    }
    println!("\n{BOLD}events{RESET}");
    for event in model.events.iter().rev().take(6).rev() {
        println!("{DIM}- {event}{RESET}");
    }
}

fn state(running: bool) -> &'static str {
    if running { "running" } else { "stopped" }
}
