use std::{
    io::{self, IsTerminal, Read},
    path::PathBuf,
    process,
};

use clap::Parser;
use habit_core::Finding;
use habit_judge::{triage_findings, validate_snooze_reason};

#[derive(Parser, Debug)]
#[command(name = "habit-judge", about = "Habit Hooks System-1 Judge Engine")]
struct Args {
    #[arg(long, help = "Validate a snooze reason")]
    reason: Option<String>,

    #[arg(long, help = "Project root directory")]
    project_dir: Option<PathBuf>,
}

fn main() {
    let args = Args::parse();

    if let Some(ref reason) = args.reason {
        if let Err(err) = validate_snooze_reason(reason) {
            eprintln!("habit-judge: invalid snooze reason: {err}");
            process::exit(2);
        }
        // If reason was checked and stdin is a terminal, we are done
        if io::stdin().is_terminal() {
            process::exit(0);
        }
    }

    let mut input = String::new();
    if let Err(err) = io::stdin().read_to_string(&mut input) {
        eprintln!("habit-judge: failed reading stdin: {err}");
        process::exit(2);
    }

    let trimmed = input.trim();
    if trimmed.is_empty() {
        println!("[]");
        return;
    }

    let findings: Vec<Finding> = match serde_json::from_str(trimmed) {
        Ok(f) => f,
        Err(err) => {
            eprintln!("habit-judge: invalid JSON on stdin: {err}");
            process::exit(2);
        }
    };

    let triaged = triage_findings(findings, args.project_dir.as_deref());

    let output = match serde_json::to_string_pretty(&triaged) {
        Ok(s) => s,
        Err(err) => {
            eprintln!("habit-judge: failed to serialize findings: {err}");
            process::exit(2);
        }
    };

    println!("{output}");
}
