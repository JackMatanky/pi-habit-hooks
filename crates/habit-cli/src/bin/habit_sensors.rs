use std::process;

use clap::Parser;
use habit_cli::{find_project_dir, load_enabled_plugins, load_project_config};
use habit_sensors::{runner::SensorRunner, scope::ScopeArgs};
use habit_snooze::{filter_snoozed_findings, load_snooze_file};

#[derive(Parser, Debug)]
#[command(name = "habit-sensors", about = "Habit Hooks Sensor ETL Runner")]
struct Args {
    #[arg(long, help = "Scan all project files")]
    all: bool,

    #[arg(long = "file", help = "Scan specific files")]
    files: Vec<String>,

    #[arg(long, help = "Scan files changed relative to branch base")]
    branch: Option<Option<String>>,

    #[arg(long, help = "Scan files changed since git ref")]
    since: Option<String>,

    #[arg(long, help = "Scan files changed in last N commits")]
    last: Option<usize>,

    #[arg(long, help = "Scan working tree modified files")]
    changed: bool,

    #[arg(long = "no-snooze", help = "Do not filter out snoozed issues")]
    no_snooze: bool,
}

fn main() {
    let args = Args::parse();
    let project_dir = find_project_dir();
    let project_config = load_project_config(&project_dir);
    let plugins = load_enabled_plugins(&project_dir, &project_config);

    let scope_args = ScopeArgs {
        all: args.all,
        files: args.files,
        branch: args.branch,
        since: args.since,
        last: args.last,
        changed: args.changed,
        no_snooze: args.no_snooze,
    };

    let runner =
        SensorRunner::new(&project_dir, &project_config, &plugins, &scope_args);
    let findings = runner.run_all();

    let final_findings = if args.no_snooze {
        findings
    } else {
        match load_snooze_file(&project_dir) {
            Ok(snooze) => {
                filter_snoozed_findings(findings, &snooze, &project_dir)
            }
            Err(_) => findings,
        }
    };

    let json =
        serde_json::to_string_pretty(&final_findings).unwrap_or_else(|err| {
            eprintln!("habit-sensors: failed to serialize findings: {err}");
            process::exit(1);
        });

    println!("{json}");
}
