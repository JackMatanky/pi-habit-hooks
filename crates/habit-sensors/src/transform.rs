use std::{
    io::Write,
    path::Path,
    process::{Command, Stdio},
};

use habit_core::Finding;

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum TransformError {
    #[error("failed to execute jq transform: {message}")]
    ProcessFailed {
        message: String,
    },

    #[error("transform output is not valid JSON: {message}")]
    InvalidJson {
        message: String,
    },

    #[error("transform output must be a single JSON array, but received: {received}")]
    NotAnArray {
        received: String,
    },

    #[error("transform output failed finding schema validation: {message}")]
    SchemaMismatch {
        message: String,
    },
}

pub fn evaluate_jq(
    transform_path: &Path,
    input_json: &str,
) -> Result<Vec<Finding>, TransformError> {
    // Check if input is empty
    let trimmed_input = input_json.trim();
    if trimmed_input.is_empty() {
        return Ok(Vec::new());
    }

    let mut child = Command::new("jq")
        .arg("-f")
        .arg(transform_path)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|err| TransformError::ProcessFailed {
            message: format!("cannot spawn jq: {err}"),
        })?;

    if let Some(mut stdin) = child.stdin.take() {
        stdin.write_all(trimmed_input.as_bytes()).map_err(|err| {
            TransformError::ProcessFailed {
                message: format!("failed writing to jq stdin: {err}"),
            }
        })?;
    }

    let output = child.wait_with_output().map_err(|err| {
        TransformError::ProcessFailed {
            message: format!("failed reading jq output: {err}"),
        }
    })?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(TransformError::ProcessFailed {
            message: stderr.to_string(),
        });
    }

    let stdout_str = String::from_utf8_lossy(&output.stdout);
    let trimmed_stdout = stdout_str.trim();
    if trimmed_stdout.is_empty() {
        return Ok(Vec::new());
    }

    let parsed_val: serde_json::Value = serde_json::from_str(trimmed_stdout)
        .map_err(|err| TransformError::InvalidJson {
            message: format!("{err}: output was {trimmed_stdout}"),
        })?;

    if !parsed_val.is_array() {
        return Err(TransformError::NotAnArray {
            received: trimmed_stdout.chars().take(80).collect(),
        });
    }

    let findings: Vec<Finding> =
        serde_json::from_value(parsed_val).map_err(|err| {
            TransformError::SchemaMismatch {
                message: err.to_string(),
            }
        })?;

    Ok(findings)
}

#[cfg(test)]
mod tests {
    use std::{fs, path::PathBuf};

    use tempfile::tempdir;

    use super::*;

    mod transform_evaluation {
        use pretty_assertions::assert_eq;

        use super::*;

        #[test]
        fn evaluates_valid_jq_filter_into_findings() {
            let dir = tempdir().unwrap();
            let filter_file = dir.path().join("test.jq");
            fs::write(
                &filter_file,
                r#"
                map({
                    smell: "oversized-file",
                    details: {},
                    issues: [{
                        key: .path,
                        details: {
                            file: .path,
                            line: .lines
                        }
                    }]
                })
                "#,
            )
            .unwrap();

            let input = r#"[{"path": "src/lib.rs", "lines": 500}]"#;
            let findings = evaluate_jq(&filter_file, input).unwrap();

            assert_eq!(findings.len(), 1);
            assert_eq!(findings[0].smell, "oversized-file");
            assert_eq!(findings[0].issues[0].key, "src/lib.rs");
            assert_eq!(findings[0].issues[0].details.file, "src/lib.rs");
            assert_eq!(findings[0].issues[0].details.line, Some(500));
        }

        #[test]
        fn rejects_transform_producing_non_array() {
            let dir = tempdir().unwrap();
            let filter_file = dir.path().join("not_array.jq");
            fs::write(&filter_file, r#"{"error": "not an array"}"#).unwrap();

            let input = r#"{"test": 1}"#;
            let err = evaluate_jq(&filter_file, input).unwrap_err();
            match err {
                TransformError::NotAnArray {
                    ..
                } => {}
                other => panic!("expected NotAnArray error, got {:?}", other),
            }
        }

        #[test]
        fn verifies_clippy_jq_mappings_for_rust_skills_lints() {
            let workspace = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .parent()
                .unwrap()
                .parent()
                .unwrap()
                .to_path_buf();
            let clippy_jq =
                workspace.join("plugins").join("rust").join("clippy.jq");

            let input = r#"[
                {"code": "clippy::await_holding_lock", "filename": "src/async_mod.rs", "line": 20, "column": 5, "content": "state.lock()", "message": "lock held across await"},
                {"code": "clippy::undocumented_unsafe_blocks", "filename": "src/ffi.rs", "line": 40, "column": 5, "content": "unsafe { ... }", "message": "undocumented unsafe"},
                {"code": "clippy::unwrap_used", "filename": "src/main.rs", "line": 15, "column": 10, "content": "opt.unwrap()", "message": "unwrap used"},
                {"code": "clippy::redundant_clone", "filename": "src/service.rs", "line": 35, "column": 8, "content": "data.clone()", "message": "redundant clone"},
                {"code": "clippy::ptr_arg", "filename": "src/util.rs", "line": 50, "column": 12, "content": "v: &Vec<u8>", "message": "ptr arg"}
            ]"#;

            let findings = evaluate_jq(&clippy_jq, input).unwrap();
            let reported_smells: Vec<&str> =
                findings.iter().map(|f| f.smell.as_str()).collect();

            assert_eq!(reported_smells.contains(&"lock-across-await"), true);
            assert_eq!(reported_smells.contains(&"undocumented-unsafe"), true);
            assert_eq!(reported_smells.contains(&"unhandled-unwrap"), true);
            assert_eq!(reported_smells.contains(&"redundant-clone"), true);
            assert_eq!(
                reported_smells.contains(&"suboptimal-borrow-type"),
                true
            );
        }
    }
}
