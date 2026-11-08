use std::{fs, path::Path};

use clap::Parser;

#[derive(Parser, Debug)]
#[command(
    name = "comment_sensor",
    about = "Rust Comment Sensor for Habit Hooks"
)]
struct Args {
    #[arg(num_args = 0..)]
    files: Vec<String>,
}

fn scan_file_comments(file_path: &str) -> Vec<serde_json::Value> {
    let mut violations = Vec::new();
    let path = Path::new(file_path);
    let content = match fs::read_to_string(path) {
        Ok(c) => c,
        Err(_) => return violations,
    };

    let mut in_test_module = false;

    for (line_idx, line) in content.lines().enumerate() {
        let line_num = line_idx + 1;
        let trimmed = line.trim();

        if trimmed.starts_with("#[cfg(test)]")
            || trimmed.contains("mod tests {")
        {
            in_test_module = true;
        }

        // Check for line comments
        if let Some(comment_start) = trimmed.find("//") {
            let comment_text = trimmed[comment_start + 2..].trim();
            let upper = comment_text.to_uppercase();

            if !in_test_module
                && (upper.starts_with("TODO")
                    || upper.starts_with("FIXME")
                    || upper.starts_with("HACK")
                    || upper.starts_with("XXX"))
            {
                violations.push(serde_json::json!({
                    "kind": "warning-comment",
                    "file": file_path,
                    "line": line_num,
                    "content": trimmed,
                    "message": format!("Unresolved warning marker in comment: '{comment_text}'")
                }));
            }
        }

        // Check for todo!() and unimplemented!() macros outside tests
        if !in_test_module
            && (trimmed.contains("todo!()")
                || trimmed.contains("unimplemented!()"))
        {
            violations.push(serde_json::json!({
                "kind": "warning-comment",
                "file": file_path,
                "line": line_num,
                "content": trimmed,
                "message": "Production code contains panic stub macro (todo!() or unimplemented!())"
            }));
        }
    }

    violations
}

fn main() {
    let args = Args::parse();
    let mut all_violations = Vec::new();

    for file in &args.files {
        all_violations.extend(scan_file_comments(file));
    }

    let json = serde_json::to_string(&all_violations)
        .unwrap_or_else(|_| "[]".to_string());
    println!("{json}");
}
