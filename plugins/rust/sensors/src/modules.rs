use std::{
    collections::BTreeSet,
    env, fs,
    path::{Path, PathBuf},
    process::{Command, Stdio},
};

use clap::Parser;
use walkdir::WalkDir;

#[derive(Parser, Debug)]
#[command(
    name = "modules_sensor",
    about = "Cargo Modules Sensor for Habit Hooks"
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

fn check_orphans_natively(
    root: &Path,
    filter_files: &BTreeSet<String>,
) -> Vec<serde_json::Value> {
    let mut violations = Vec::new();
    let src_dir = root.join("src");
    if !src_dir.is_dir() {
        return violations;
    }

    // Collect all declared modules in the crate
    let mut declared_mods = BTreeSet::new();
    let mut all_rs_files = Vec::new();

    for entry in WalkDir::new(&src_dir).into_iter().filter_map(|e| e.ok()) {
        if !entry.file_type().is_file()
            || !entry.path().extension().is_some_and(|ext| ext == "rs")
        {
            continue;
        }
        all_rs_files.push(entry.path().to_path_buf());
        let Ok(content) = fs::read_to_string(entry.path()) else {
            continue;
        };
        for line in content.lines() {
            let trimmed = line.trim();
            let Some(rest) = trimmed
                .strip_prefix("pub mod ")
                .or_else(|| trimmed.strip_prefix("mod "))
            else {
                continue;
            };
            if let Some(mod_token) = rest.split_whitespace().next() {
                let mod_name = mod_token.trim_end_matches(';').trim();
                if !mod_name.is_empty() {
                    declared_mods.insert(mod_name.to_string());
                }
            }
        }
    }

    for file_path in all_rs_files {
        let file_name =
            file_path.file_stem().unwrap_or_default().to_string_lossy();
        if file_name == "main" || file_name == "lib" || file_name == "mod" {
            continue;
        }

        let rel_path = file_path
            .strip_prefix(root)
            .unwrap_or(&file_path)
            .to_string_lossy()
            .replace('\\', "/");

        if !filter_files.is_empty() && !filter_files.contains(&rel_path) {
            continue;
        }

        if !declared_mods.contains(file_name.as_ref()) {
            violations.push(serde_json::json!({
                "kind": "orphan",
                "file": rel_path,
                "message": format!("Source file '{}' is not declared with 'mod {};' in any parent module", file_name, file_name)
            }));
        }
    }

    violations
}

fn main() {
    let args = Args::parse();
    let cwd = env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let root = find_cargo_root(&cwd).unwrap_or_else(|| cwd.clone());

    let files_set: BTreeSet<String> = args
        .files
        .iter()
        .map(|f| f.replace('\\', "/").trim_start_matches("./").to_string())
        .collect();

    let mut violations = Vec::new();

    // Check cargo-modules orphans if installed
    let modules_cmd = Command::new("cargo")
        .current_dir(&root)
        .args(["modules", "orphans", "--lib"])
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .output();

    let Ok(output) = modules_cmd else {
        violations.extend(check_orphans_natively(&root, &files_set));
        let json = serde_json::to_string(&violations)
            .unwrap_or_else(|_| "[]".to_string());
        println!("{json}");
        return;
    };

    if !output.status.success() {
        violations.extend(check_orphans_natively(&root, &files_set));
        let json = serde_json::to_string(&violations)
            .unwrap_or_else(|_| "[]".to_string());
        println!("{json}");
        return;
    }

    let stdout_str = String::from_utf8_lossy(&output.stdout);
    for line in stdout_str.lines() {
        let trimmed = line.trim();
        if !trimmed.ends_with(".rs") {
            continue;
        }
        let rel = trimmed.replace('\\', "/");
        if files_set.is_empty() || files_set.contains(&rel) {
            violations.push(serde_json::json!({
                "kind": "orphan",
                "file": rel,
                "message": format!("Orphan module file '{rel}' is not connected to crate root")
            }));
        }
    }

    let json =
        serde_json::to_string(&violations).unwrap_or_else(|_| "[]".to_string());
    println!("{json}");
}
