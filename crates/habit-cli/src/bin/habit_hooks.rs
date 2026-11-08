use clap::{Parser, Subcommand};
use habit_cli::{find_project_dir, load_enabled_plugins, load_project_config};
use habit_mapper::evaluate_and_render;
use habit_sensors::{detector::DetectorResolver, runner::SensorRunner, scope::ScopeArgs};
use habit_snooze::{filter_snoozed_findings, load_snooze_file};
use std::fs;
use std::process::{self, Command, Stdio};

#[derive(Parser, Debug)]
#[command(name = "habit-hooks", about = "Habit Hooks Code Quality Coaching Engine")]
struct Cli {
    #[command(subcommand)]
    command: Option<Commands>,

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

    #[arg(long, help = "Scan staged Rust files (for pre-commit hooks)")]
    staged: bool,
    #[arg(long = "no-snooze", help = "Do not filter out snoozed issues")]
    no_snooze: bool,

    #[arg(long = "no-judge", help = "Disable System-1 judge triage stage")]
    no_judge: bool,

    #[arg(long, value_name = "PATH", help = "Also write a machine-readable JSON summary (enforced files, incomplete run) to PATH")]
    report: Option<std::path::PathBuf>,
}

#[derive(Subcommand, Debug)]
enum Commands {
    #[command(about = "Initialize Habit Hooks configuration and verify tools")]
    Init,
    #[command(about = "Install Git pre-commit hook into .git/hooks/pre-commit and/or configure with hk")]
    Install {
        #[arg(long, help = "Configure pre-commit hook using hk (https://hk.jdx.dev)")]
        hk: bool,
    },
}
fn main() {
    let cli = Cli::parse();
    let project_dir = find_project_dir();

    if let Some(Commands::Init) = cli.command {
        run_init(&project_dir);
        return;
    }
    if let Some(Commands::Install { hk }) = cli.command {
        if let Err(err) = run_install(&project_dir, hk) {
            eprintln!("habit-hooks: {err}");
            process::exit(1);
        }
        return;
    }

    let project_config = load_project_config(&project_dir);
    let plugins = load_enabled_plugins(&project_dir, &project_config);

    let files = if cli.staged {
        staged_rust_files(&project_dir)
    } else {
        cli.files
    };

    let scope_args = ScopeArgs {
        all: cli.all,
        files,
        branch: cli.branch,
        since: cli.since,
        last: cli.last,
        changed: cli.changed,
        no_snooze: cli.no_snooze,
    };

    let runner = SensorRunner::new(&project_dir, &project_config, &plugins, &scope_args);
    let findings = runner.run_all();

    let triaged_findings = if cli.no_judge {
        findings
    } else {
        habit_judge::triage_findings(findings, Some(&project_dir))
    };

    let final_findings = if cli.no_snooze {
        triaged_findings
    } else {
        match load_snooze_file(&project_dir) {
            Ok(snooze) => filter_snoozed_findings(triaged_findings, &snooze, &project_dir),
            Err(_) => triaged_findings,
        }
    };

    let findings_json = serde_json::to_string(&final_findings).unwrap_or_else(|err| {
        eprintln!("habit-hooks: failed serializing findings: {err}");
        process::exit(1);
    });

    let plugin_dirs: Vec<_> = plugins.iter().map(|p| p.plugin_dir.clone()).collect();
    let result = evaluate_and_render(
        &findings_json,
        &project_dir,
        &plugin_dirs,
        &project_config,
    );

    if let Some(path) = &cli.report {
        // structured twin of the text below, so callers need not parse it; a failed write is not a failed check
        match serde_json::to_string(&result.report) {
            Ok(json) => {
                if let Err(err) = fs::write(path, json) {
                    eprintln!("habit-hooks: could not write report to {}: {err}", path.display());
                }
            }
            Err(err) => eprintln!("habit-hooks: failed serializing report: {err}"),
        }
    }

    print!("{}", result.output);
    process::exit(result.exit_code);
}

/// Staged `.rs` files for a pre-commit run. Exits 0 right away when there is nothing to check
/// (switched off, or no Rust staged), so hooks stay silent and fast.
fn staged_rust_files(project_dir: &std::path::Path) -> Vec<String> {
    if is_switched_off() {
        process::exit(0);
    }
    let output = Command::new("git")
        .current_dir(project_dir)
        .args(["diff", "--cached", "--name-only", "-z", "--diff-filter=ACMR", "--", "*.rs"])
        .output()
        .unwrap_or_else(|err| {
            eprintln!("habit-hooks: failed to list staged Rust files: {err}");
            process::exit(1);
        });
    if !output.status.success() {
        eprintln!(
            "habit-hooks: failed to list staged Rust files: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        process::exit(1);
    }
    let files: Vec<String> = output
        .stdout
        .split(|byte| *byte == 0)
        .filter(|name| !name.is_empty())
        .map(|name| String::from_utf8_lossy(name).into_owned())
        .collect();
    if files.is_empty() {
        process::exit(0);
    }
    files
}

/// True when automatic firing is turned off (`/habit-hooks off`), so git hooks stay silent too.
/// Same precedence as the extension: HABIT_HOOKS_MODE, then the persisted mode file.
fn is_switched_off() -> bool {
    let mode = std::env::var("HABIT_HOOKS_MODE").ok().or_else(|| {
        let path = std::env::var_os("HABIT_HOOKS_MODE_FILE")
            .map(std::path::PathBuf::from)
            .or_else(|| {
                std::env::var_os("HOME").map(|h| std::path::Path::new(&h).join(".config/habit-hooks/mode"))
            })?;
        fs::read_to_string(path).ok()
    });
    mode.is_some_and(|m| m.trim() == "off")
}

fn run_init(project_dir: &std::path::Path) {
    println!("Initializing Habit Hooks in {}...", project_dir.display());

    let habit_dir = project_dir.join(".habit-hooks");
    if let Err(err) = fs::create_dir_all(&habit_dir) {
        eprintln!("Failed to create .habit-hooks directory: {err}");
        process::exit(1);
    }

    let config_file = habit_dir.join("config.toml");
    let has_rust = project_dir.join("Cargo.toml").is_file()
        || walkdir::WalkDir::new(project_dir)
            .into_iter()
            .filter_map(|e| e.ok())
            .any(|e| e.path().extension().is_some_and(|ext| ext == "rs"));

    let plugins_list = if has_rust {
        r#"plugins = ["rust", "generic"]"#
    } else {
        r#"plugins = ["generic"]"#
    };

    if !config_file.is_file() {
        let default_config = format!(
            r#"# Habit Hooks Configuration
{plugins_list}
uncoached = "suggest"

[scope]
changedOnly = false
autoBranchOffMain = false
branchBase = "main"
mainBranch = "main"
"#
        );
        if let Err(err) = fs::write(&config_file, default_config) {
            eprintln!("Failed to write config file: {err}");
            process::exit(1);
        }
        println!("Created {}", config_file.display());
    } else {
        println!("Existing configuration found at {}", config_file.display());
    }

    // Verify detectors across enabled plugins
    let project_config = load_project_config(project_dir);
    let plugins = load_enabled_plugins(project_dir, &project_config);

    println!("\nVerifying tool detectors for enabled plugins:");
    let mut missing_detectors = Vec::new();

    for plugin in &plugins {
        let resolver = DetectorResolver::new(project_dir, &plugin.plugin_dir, &plugin.detectors);
        for detector in &plugin.detectors {
            match resolver.resolve_tool(&detector.name) {
                Ok(path) => {
                    println!("  [OK] {:<16} ({})", detector.name, path.display());
                }
                Err(_) => {
                    println!("  [MISSING] {:<16}", detector.name);
                    if let Some(ref install) = detector.install {
                        println!("            Install: {install}");
                        missing_detectors.push((detector.name.clone(), install.clone()));
                    }
                }
            }
        }
    }

    if missing_detectors.is_empty() {
        println!("\n✅ All declared detectors are installed and ready.");
    } else {
        println!("\n⚠️ Some detectors are not installed yet. You can run the install commands above.");
    }
}

fn configure_hk_step(project_dir: &std::path::Path) -> Result<bool, String> {
    let hk_pkl_path = project_dir.join("hk.pkl");
    let step_snippet = r#"    ["habit-hooks"] {
        glob = "**/*.rs"
        check = "habit-hooks --staged"
    }"#;

    if hk_pkl_path.is_file() {
        let content = fs::read_to_string(&hk_pkl_path)
            .map_err(|e| format!("Failed to read existing hk.pkl: {e}"))?;
        if content.contains(step_snippet) {
            println!("ℹ️  Step 'habit-hooks' already configured in hk.pkl");
        } else if content.contains(
            "    [\"habit-hooks\"] {\n        glob = \"**/*.rs\"\n        check = \"habit-hooks --changed\"\n    }",
        ) {
            fs::write(
                &hk_pkl_path,
                content.replacen(
                    "    [\"habit-hooks\"] {\n        glob = \"**/*.rs\"\n        check = \"habit-hooks --changed\"\n    }",
                    step_snippet,
                    1,
                ),
            )
            .map_err(|e| format!("Failed to update hk.pkl with staged scope: {e}"))?;
            println!("✅ Updated 'habit-hooks' step to scan staged Rust files");
        } else if content.contains("[\"habit-hooks\"]") || content.contains("[habit-hooks]") {
            return Err("Existing 'habit-hooks' step in hk.pkl is customized; update its check command to 'habit-hooks --staged' manually".to_string());
        } else if let Some(steps_idx) = content.find("steps {") {
            let insert_pos = steps_idx + "steps {".len();
            let mut updated = content[..insert_pos].to_string();
            updated.push('\n');
            updated.push_str(step_snippet);
            updated.push_str(&content[insert_pos..]);
            fs::write(&hk_pkl_path, updated)
                .map_err(|e| format!("Failed to update hk.pkl with habit-hooks step: {e}"))?;
            println!("✅ Added 'habit-hooks' step to existing hk.pkl");
        } else {
            // Append steps block
            let mut updated = content;
            if !updated.ends_with('\n') {
                updated.push('\n');
            }
            updated.push_str("\nsteps {\n");
            updated.push_str(step_snippet);
            updated.push_str("\n}\n");
            fs::write(&hk_pkl_path, updated)
                .map_err(|e| format!("Failed to update hk.pkl with habit-hooks step: {e}"))?;
            println!("✅ Appended 'habit-hooks' step to existing hk.pkl");
        }
    } else {
        let starter = format!(
            r#"amends "package://github.com/jdx/hk/releases/download/v2.0.0/hk@2.0.0#/Config.pkl"

steps {{
{step_snippet}
}}
"#
        );
        fs::write(&hk_pkl_path, starter)
            .map_err(|e| format!("Failed to create hk.pkl: {e}"))?;
        println!("✅ Created hk.pkl with 'habit-hooks' step");
    }

    // If hk is available on PATH, run `hk install` to register hook via git config
    let hk_check = Command::new("hk")
        .arg("--version")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();

    if let Ok(status) = hk_check {
        if status.success() {
            let install_status = Command::new("hk")
                .arg("install")
                .current_dir(project_dir)
                .status()
                .map_err(|e| format!("Failed to run 'hk install': {e}"))?;
            if install_status.success() {
                println!("✅ Ran 'hk install' to register Git hooks with hk");
                return Ok(true);
            } else {
                eprintln!("⚠️  'hk install' exited with non-zero status");
            }
        }
    } else {
        println!("ℹ️  'hk' CLI not found on PATH; created/updated hk.pkl. Install hk (https://hk.jdx.dev) to enable.");
    }

    Ok(false)
}

fn run_install(project_dir: &std::path::Path, force_hk: bool) -> Result<(), String> {
    let git_dir = project_dir.join(".git");
    if !git_dir.exists() {
        return Err("Not a git repository".to_string());
    }

    let pre_commit_path = git_dir.join("hooks/pre-commit");
    if pre_commit_path.exists() {
        let existing = fs::read_to_string(&pre_commit_path)
            .map_err(|e| format!("Failed to read existing pre-commit hook: {e}"))?;
        if !existing.starts_with("#!/bin/sh\n# Habit Hooks automated pre-commit gate") {
            return Err(format!(
                "Existing pre-commit hook at {} is not managed by Habit Hooks; refusing to overwrite it",
                pre_commit_path.display()
            ));
        }
    }

    let hk_detected = force_hk || project_dir.join("hk.pkl").is_file();
    if hk_detected && configure_hk_step(project_dir)? {
        return Ok(());
    }

    let hooks_dir = git_dir.join("hooks");
    fs::create_dir_all(&hooks_dir)
        .map_err(|err| format!("Failed to create .git/hooks directory: {err}"))?;

    let script = r#"#!/bin/sh
# Habit Hooks automated pre-commit gate
if git diff --cached --quiet --diff-filter=ACMR -- '*.rs'; then
    exit 0
fi
if command -v habit-hooks >/dev/null 2>&1; then
    exec habit-hooks --staged
else
    echo "⚠️ habit-hooks not found in PATH; skipping pre-commit gate."
fi
"#;

    fs::write(&pre_commit_path, script)
        .map_err(|err| format!("Failed to write pre-commit hook: {err}"))?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = fs::metadata(&pre_commit_path)
            .map_err(|err| format!("Failed to read pre-commit hook permissions: {err}"))?
            .permissions();
        perms.set_mode(0o755);
        fs::set_permissions(&pre_commit_path, perms)
            .map_err(|err| format!("Failed to make pre-commit hook executable: {err}"))?;
    }

    println!("✅ Installed Habit Hooks pre-commit hook in .git/hooks/pre-commit");
    Ok(())
}
