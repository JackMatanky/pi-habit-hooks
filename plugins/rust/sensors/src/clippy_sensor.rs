use clap::Parser;
use serde::Deserialize;
use std::collections::BTreeSet;
use std::env;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

#[derive(Parser, Debug)]
#[command(name = "clippy_sensor", about = "Cargo Clippy Sensor for Habit Hooks")]
struct Args {
    #[arg(long = "target-dir", default_value = ".habit-hooks/target")]
    target_dir: String,

    #[arg(long = "files", num_args = 0..)]
    files: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct DiagnosticSpan {
    file_name: String,
    line_start: usize,
    column_start: usize,
    is_primary: bool,
    text: Vec<SpanText>,
}

#[derive(Debug, Deserialize)]
struct SpanText {
    text: String,
}

#[derive(Debug, Deserialize)]
struct DiagnosticCode {
    code: String,
}

#[derive(Debug, Deserialize)]
struct DiagnosticMessage {
    code: Option<DiagnosticCode>,
    message: String,
    spans: Vec<DiagnosticSpan>,
    level: String,
}

#[derive(Debug, Deserialize)]
struct CompilerArtifact {
    reason: String,
    message: Option<DiagnosticMessage>,
}

fn find_cargo_root(start: &Path) -> Option<PathBuf> {
    let mut current = if start.is_file() {
        start.parent().unwrap_or(start).to_path_buf()
    } else {
        start.to_path_buf()
    };

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

fn incomplete_run_finding() -> serde_json::Value {
    serde_json::json!({
        "code": "incomplete-run",
        "filename": "Cargo.toml",
        "line": 1,
        "column": 1,
        "content": null,
        "message": "Crate compilation failed in files outside sensor scope; lint results for target files cannot be confirmed."
    })
}

fn resolve_root(files: &[String], cwd: &Path) -> PathBuf {
    let start = match files.first() {
        Some(first) if Path::new(first).is_absolute() => PathBuf::from(first),
        Some(first) => cwd.join(first),
        None => cwd.to_path_buf(),
    };
    find_cargo_root(&start).unwrap_or_else(|| cwd.to_path_buf())
}

/// Span file as the requested `--files` spell it: relative to `cwd`, forward slashes.
fn clean_path(span_file: &str, root: &Path, cwd: &Path) -> String {
    let span_path = Path::new(span_file);
    let abs = if span_path.is_absolute() {
        span_path.to_path_buf()
    } else {
        root.join(span_path)
    };
    pathdiff::diff_paths(&abs, cwd)
        .unwrap_or_else(|| span_path.to_path_buf())
        .to_string_lossy()
        .replace('\\', "/")
        .trim_start_matches("./")
        .to_string()
}

fn in_scope(files_set: &BTreeSet<String>, file: &str) -> bool {
    files_set.is_empty()
        || files_set.contains(file)
        || files_set.iter().any(|f| file.ends_with(f.as_str()) || f.ends_with(file))
}

struct Scope<'a> {
    root: &'a Path,
    cwd: &'a Path,
    files: &'a BTreeSet<String>,
}

/// Normalized lint findings of one compiler message that fall inside `scope` and were not reported yet.
fn message_violations(
    diag: &DiagnosticMessage,
    scope: &Scope,
    seen: &mut BTreeSet<String>,
) -> Vec<serde_json::Value> {
    let Some(code) = diag.code.as_ref().map(|c| c.code.as_str()).filter(|c| !c.is_empty()) else {
        return Vec::new();
    };

    let mut found = Vec::new();
    for span in diag.spans.iter().filter(|s| s.is_primary) {
        let file = clean_path(&span.file_name, scope.root, scope.cwd);
        if !in_scope(scope.files, &file) {
            continue;
        }
        if !seen.insert(format!("{code}\0{file}\0{}\0{}", span.line_start, span.column_start)) {
            continue;
        }
        found.push(serde_json::json!({
            "code": code,
            "filename": file,
            "line": span.line_start,
            "column": span.column_start,
            "content": span.text.first().map(|t| t.text.trim()),
            "message": diag.message
        }));
    }
    found
}

/// Parses cargo's JSON stream; the flag tells whether the compiler reported an error.
fn collect_violations(stdout: &str, scope: &Scope) -> (Vec<serde_json::Value>, bool) {
    let mut violations = Vec::new();
    let mut saw_error = false;
    let mut seen = BTreeSet::new();

    let diagnostics = stdout
        .lines()
        .filter_map(|line| serde_json::from_str::<CompilerArtifact>(line.trim()).ok())
        .filter(|artifact| artifact.reason == "compiler-message")
        .filter_map(|artifact| artifact.message);

    for diag in diagnostics {
        saw_error |= diag.level == "error";
        if diag.level == "note" || diag.level == "help" {
            continue;
        }
        violations.extend(message_violations(&diag, scope, &mut seen));
    }
    (violations, saw_error)
}

fn main() {
    let args = Args::parse();
    let cwd = env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let root = resolve_root(&args.files, &cwd);
    let files: BTreeSet<String> = args
        .files
        .iter()
        .map(|f| f.replace('\\', "/").trim_start_matches("./").to_string())
        .collect();

    let output = Command::new("cargo")
        .current_dir(&root)
        .args(["clippy", "--message-format=json", "--all-targets"])
        .env("CARGO_TARGET_DIR", &args.target_dir)
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .output();
    let output = match output {
        Ok(o) => o,
        Err(err) => {
            eprintln!("clippy_sensor: failed to spawn cargo clippy: {err}");
            println!("[]");
            return;
        }
    };

    let scope = Scope { root: &root, cwd: &cwd, files: &files };
    let (mut violations, saw_error) = collect_violations(&String::from_utf8_lossy(&output.stdout), &scope);

    // A crate that fails to compile (typically in files outside the requested scope) yields no lints for the
    // target files; say so instead of reporting a misleading clean run.
    if !output.status.success() && saw_error && violations.is_empty() {
        violations.push(incomplete_run_finding());
    }

    println!("{}", serde_json::to_string(&violations).unwrap_or_else(|_| "[]".to_string()));
}
