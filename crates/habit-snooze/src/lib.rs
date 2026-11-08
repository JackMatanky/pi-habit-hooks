use std::{
    fmt::Write,
    fs,
    path::{Path, PathBuf},
    process::Command,
};

use chrono::Utc;
use habit_core::Finding;
use serde::{Deserialize, Serialize};
use sha1::{Digest, Sha1};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SnoozeEntry {
    pub smell: String,
    pub key: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub blob_sha: Option<String>,
    pub date: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
pub struct SnoozeFile {
    pub entries: Vec<SnoozeEntry>,
}

#[derive(Debug, thiserror::Error)]
pub enum SnoozeError {
    #[error("failed to read snooze file '{path}': {message}")]
    ReadError {
        path: String,
        message: String,
    },

    #[error("failed to write snooze file '{path}': {message}")]
    WriteError {
        path: String,
        message: String,
    },

    #[error("failed to parse snooze file '{path}': {message}")]
    ParseError {
        path: String,
        message: String,
    },
}

pub fn snooze_file_path(project_dir: &Path) -> PathBuf {
    project_dir.join(".habit-hooks").join("snooze.json")
}

pub fn load_snooze_file(project_dir: &Path) -> Result<SnoozeFile, SnoozeError> {
    let path = snooze_file_path(project_dir);
    if !path.is_file() {
        return Ok(SnoozeFile::default());
    }

    let content =
        fs::read_to_string(&path).map_err(|err| SnoozeError::ReadError {
            path: path.display().to_string(),
            message: err.to_string(),
        })?;

    if content.trim().is_empty() {
        return Ok(SnoozeFile::default());
    }

    serde_json::from_str(&content).map_err(|err| SnoozeError::ParseError {
        path: path.display().to_string(),
        message: err.to_string(),
    })
}

pub fn save_snooze_file(
    project_dir: &Path,
    snooze: &SnoozeFile,
) -> Result<(), SnoozeError> {
    let path = snooze_file_path(project_dir);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|err| SnoozeError::WriteError {
            path: parent.display().to_string(),
            message: err.to_string(),
        })?;
    }

    let json = serde_json::to_string_pretty(snooze).map_err(|err| {
        SnoozeError::WriteError {
            path: path.display().to_string(),
            message: err.to_string(),
        }
    })?;

    fs::write(&path, json).map_err(|err| SnoozeError::WriteError {
        path: path.display().to_string(),
        message: err.to_string(),
    })
}

pub fn compute_blob_sha(
    project_dir: &Path,
    relative_file: &str,
) -> Option<String> {
    // 1. Try git hash-object
    if let Ok(output) = Command::new("git")
        .current_dir(project_dir)
        .args(["hash-object", relative_file])
        .output()
    {
        if output.status.success() {
            let sha =
                String::from_utf8_lossy(&output.stdout).trim().to_string();
            if sha.len() == 40 {
                return Some(sha);
            }
        }
    }

    // 2. Pure Rust SHA-1 fallback for git blob format
    let target = project_dir.join(relative_file);
    let bytes = fs::read(&target).ok()?;
    let header = format!("blob {}\0", bytes.len());
    let mut hasher = Sha1::new();
    hasher.update(header.as_bytes());
    hasher.update(&bytes);
    let hash_bytes = hasher.finalize();
    let mut hex_str = String::with_capacity(40);
    for b in hash_bytes {
        let _ = write!(&mut hex_str, "{:02x}", b);
    }
    Some(hex_str)
}

pub fn filter_snoozed_findings(
    findings: Vec<Finding>,
    snooze: &SnoozeFile,
    project_dir: &Path,
) -> Vec<Finding> {
    let mut kept_findings = Vec::new();

    for mut finding in findings {
        let smell = &finding.smell;
        let mut kept_issues = Vec::new();

        for issue in finding.issues {
            // Find active snooze entry
            let matched_entry = snooze
                .entries
                .iter()
                .find(|e| e.smell == *smell && e.key == issue.key);

            if let Some(entry) = matched_entry {
                let has_drift = match (&entry.blob_sha, &entry.file) {
                    (Some(expected_sha), Some(file_path)) => {
                        let current_sha =
                            compute_blob_sha(project_dir, file_path);
                        current_sha.as_deref() != Some(expected_sha.as_str())
                    }
                    _ => false,
                };
                if has_drift {
                    kept_issues.push(issue);
                }
            } else {
                kept_issues.push(issue);
            }
        }

        if !kept_issues.is_empty() {
            finding.issues = kept_issues;
            kept_findings.push(finding);
        }
    }

    kept_findings
}

pub fn add_snooze_findings(
    snooze: &mut SnoozeFile,
    findings: &[Finding],
    project_dir: &Path,
    reason: Option<&str>,
) {
    let today = Utc::now().format("%Y-%m-%d").to_string();

    for finding in findings {
        for issue in &finding.issues {
            let file_opt = if !issue.details.file.is_empty() {
                Some(issue.details.file.clone())
            } else {
                None
            };

            let blob_sha = file_opt
                .as_ref()
                .and_then(|f| compute_blob_sha(project_dir, f));

            let entry = SnoozeEntry {
                smell: finding.smell.clone(),
                key: issue.key.clone(),
                file: file_opt,
                blob_sha,
                date: today.clone(),
                reason: reason.map(|s| s.to_string()),
            };

            // Replace existing or add new
            if let Some(pos) = snooze
                .entries
                .iter()
                .position(|e| e.smell == entry.smell && e.key == entry.key)
            {
                snooze.entries[pos] = entry;
            } else {
                snooze.entries.push(entry);
            }
        }
    }
}

pub fn prune_snooze_entries(
    snooze: &mut SnoozeFile,
    active_findings: &[Finding],
    project_dir: &Path,
) {
    snooze.entries.retain(|entry| {
        // If file is specified and deleted on disk, prune
        if let Some(ref f) = entry.file {
            if !project_dir.join(f).exists() {
                return false;
            }
        }

        // Check if still matches any active finding
        active_findings.iter().any(|f| {
            f.smell == entry.smell
                && f.issues.iter().any(|i| i.key == entry.key)
        })
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    mod snooze_filtering {
        use std::collections::BTreeMap;

        use habit_core::{Issue, IssueDetails};
        use pretty_assertions::assert_eq;
        use tempfile::tempdir;

        use super::*;

        #[test]
        fn suppresses_active_snooze_with_matching_blob_sha() {
            let dir = tempdir().unwrap();
            let file_rel = "src/main.rs";
            let full_path = dir.path().join(file_rel);
            fs::create_dir_all(full_path.parent().unwrap()).unwrap();
            fs::write(&full_path, "fn main() {}\n").unwrap();

            let sha = compute_blob_sha(dir.path(), file_rel).unwrap();

            let snooze = SnoozeFile {
                entries: vec![SnoozeEntry {
                    smell: "unused-variable".to_string(),
                    key: file_rel.to_string(),
                    file: Some(file_rel.to_string()),
                    blob_sha: Some(sha),
                    date: "2026-10-25".to_string(),
                    reason: Some("legacy code".to_string()),
                }],
            };

            let findings = vec![Finding {
                smell: "unused-variable".to_string(),
                language: Some("rust".to_string()),
                details: serde_json::json!({}),
                issues: vec![Issue {
                    key: file_rel.to_string(),
                    details: IssueDetails {
                        file: file_rel.to_string(),
                        line: Some(1),
                        column: None,
                        content: None,
                        message: None,
                        source: None,
                        extra: BTreeMap::new(),
                    },
                }],
            }];

            let filtered =
                filter_snoozed_findings(findings, &snooze, dir.path());
            assert_eq!(filtered.is_empty(), true);
        }

        #[test]
        fn resurrects_snooze_when_file_blob_sha_changes() {
            let dir = tempdir().unwrap();
            let file_rel = "src/main.rs";
            let full_path = dir.path().join(file_rel);
            fs::create_dir_all(full_path.parent().unwrap()).unwrap();
            fs::write(&full_path, "fn main() {}\n").unwrap();

            let snooze = SnoozeFile {
                entries: vec![SnoozeEntry {
                    smell: "unused-variable".to_string(),
                    key: file_rel.to_string(),
                    file: Some(file_rel.to_string()),
                    blob_sha: Some(
                        "0000000000000000000000000000000000000000".to_string(),
                    ),
                    date: "2026-10-25".to_string(),
                    reason: None,
                }],
            };

            let findings = vec![Finding {
                smell: "unused-variable".to_string(),
                language: Some("rust".to_string()),
                details: serde_json::json!({}),
                issues: vec![Issue {
                    key: file_rel.to_string(),
                    details: IssueDetails {
                        file: file_rel.to_string(),
                        line: Some(1),
                        column: None,
                        content: None,
                        message: None,
                        source: None,
                        extra: BTreeMap::new(),
                    },
                }],
            }];

            let filtered =
                filter_snoozed_findings(findings, &snooze, dir.path());
            assert_eq!(filtered.len(), 1);
            assert_eq!(filtered[0].issues.len(), 1);
        }
    }
}
