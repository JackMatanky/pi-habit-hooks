use std::{
    io::{self, Read},
    process,
};

use clap::Parser;
use habit_cli::find_project_dir;
use habit_core::Finding;
use habit_snooze::{
    add_snooze_findings, compute_blob_sha, filter_snoozed_findings,
    load_snooze_file, prune_snooze_entries, save_snooze_file,
};

#[derive(Parser, Debug)]
#[command(name = "habit-snooze", about = "Habit Hooks Snooze Manager")]
struct Args {
    #[arg(short, long, help = "Snooze all findings received on stdin")]
    snooze: bool,

    #[arg(long, help = "Optional reason for snoozing")]
    reason: Option<String>,

    #[arg(short, long, help = "Prune expired or inactive snoozes")]
    prune: bool,

    #[arg(short, long, help = "List active snoozes")]
    list: bool,
}

fn main() {
    let args = Args::parse();
    let project_dir = find_project_dir();
    let mut snooze_file = load_snooze_file(&project_dir).unwrap_or_default();

    if args.list {
        if snooze_file.entries.is_empty() {
            println!("No active snoozes recorded.");
            return;
        }

        println!(
            "{:<24} {:<32} {:<12} {:<8}",
            "SMELL", "KEY", "DATE", "STATUS"
        );
        println!("{}", "-".repeat(80));

        for entry in &snooze_file.entries {
            let status = if let (Some(expected), Some(ref f)) =
                (&entry.blob_sha, &entry.file)
            {
                let current = compute_blob_sha(&project_dir, f);
                if current.as_deref() == Some(expected.as_str()) {
                    "active"
                } else {
                    "lapsed (modified)"
                }
            } else {
                "active"
            };

            println!(
                "{:<24} {:<32} {:<12} {:<8}",
                entry.smell, entry.key, entry.date, status
            );
        }
        return;
    }

    let mut stdin_content = String::new();
    let _ = io::stdin().read_to_string(&mut stdin_content);

    let findings: Vec<Finding> = if !stdin_content.trim().is_empty() {
        serde_json::from_str(&stdin_content).unwrap_or_else(|err| {
            eprintln!("habit-snooze: invalid JSON on stdin: {err}");
            process::exit(2);
        })
    } else {
        Vec::new()
    };

    if args.snooze {
        add_snooze_findings(
            &mut snooze_file,
            &findings,
            &project_dir,
            args.reason.as_deref(),
        );
        if let Err(err) = save_snooze_file(&project_dir, &snooze_file) {
            eprintln!("habit-snooze: failed saving snooze file: {err}");
            process::exit(1);
        }
        println!("Snoozed {} finding(s).", findings.len());
        return;
    }

    if args.prune {
        prune_snooze_entries(&mut snooze_file, &findings, &project_dir);
        if let Err(err) = save_snooze_file(&project_dir, &snooze_file) {
            eprintln!("habit-snooze: failed saving snooze file: {err}");
            process::exit(1);
        }
        println!("Active snoozes: {}", snooze_file.entries.len());
        return;
    }

    // Default filter mode: stream filtered findings to stdout
    let filtered =
        filter_snoozed_findings(findings, &snooze_file, &project_dir);
    let output_json =
        serde_json::to_string_pretty(&filtered).unwrap_or_else(|err| {
            eprintln!("habit-snooze: failed to serialize findings: {err}");
            process::exit(1);
        });

    println!("{output_json}");
}
