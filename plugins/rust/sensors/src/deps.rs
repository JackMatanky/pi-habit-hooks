use std::{
    collections::BTreeSet,
    env,
    path::{Path, PathBuf},
    process::{Command, Stdio},
};

use clap::Parser;

#[derive(Parser, Debug)]
#[command(
    name = "deps_sensor",
    about = "Cargo Dependency Sensor for Habit Hooks"
)]
struct Args {
    #[arg(num_args = 0..)]
    files: Vec<String>,
}

fn find_cargo_root(start: &Path) -> Option<PathBuf> {
    let mut current = start.to_path_buf();
    loop {
        if current.join("Cargo.toml").is_file() {
            return Some(current);
        }
        if let Some(parent) = current.parent() {
            current = parent.to_path_buf();
        } else {
            break;
        }
    }
    None
}

fn check_machete_or_native(root: &Path) -> Vec<serde_json::Value> {
    let mut violations = Vec::new();

    // Try cargo-machete if available
    let machete_cmd = Command::new("cargo")
        .current_dir(root)
        .args(["machete", "--with-metadata"])
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .output();

    if let Ok(output) = machete_cmd {
        let stdout_str = String::from_utf8_lossy(&output.stdout);
        for line in stdout_str.lines() {
            let trimmed = line.trim();
            if trimmed.contains("Cargo.toml:") || trimmed.starts_with("- ") {
                let dep_name = trimmed
                    .trim_start_matches("- ")
                    .split_whitespace()
                    .next()
                    .unwrap_or_default();
                if !dep_name.is_empty() {
                    violations.push(serde_json::json!({
                        "kind": "unused",
                        "name": dep_name,
                        "file": "Cargo.toml",
                        "message": format!("Unused dependency '{dep_name}' declared in Cargo.toml")
                    }));
                }
            }
        }
    }

    violations
}

fn check_duplicate_dependencies(root: &Path) -> Vec<serde_json::Value> {
    let mut violations = Vec::new();

    let tree_cmd = Command::new("cargo")
        .current_dir(root)
        .args(["tree", "-d"])
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .output();

    let Ok(output) = tree_cmd else {
        return violations;
    };
    if !output.status.success() {
        return violations;
    }

    let stdout_str = String::from_utf8_lossy(&output.stdout);
    let mut seen_duplicates = BTreeSet::new();

    for line in stdout_str.lines() {
        let trimmed = line.trim();
        let clean = trimmed.trim_start_matches(|c: char| !c.is_alphanumeric());
        let Some(crate_name) = clean.split_whitespace().next() else {
            continue;
        };
        if crate_name.is_empty()
            || !seen_duplicates.insert(crate_name.to_string())
        {
            continue;
        }
        violations.push(serde_json::json!({
            "kind": "duplicate",
            "name": crate_name,
            "file": "Cargo.lock",
            "message": format!("Multiple versions of dependency '{crate_name}' in Cargo.lock")
        }));
    }

    violations
}

fn main() {
    let _args = Args::parse();
    let cwd = env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let root = find_cargo_root(&cwd).unwrap_or_else(|| cwd.clone());

    let mut violations = Vec::new();
    violations.extend(check_machete_or_native(&root));
    violations.extend(check_duplicate_dependencies(&root));

    let json =
        serde_json::to_string(&violations).unwrap_or_else(|_| "[]".to_string());
    println!("{json}");
}
