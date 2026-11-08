use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Component, Path, PathBuf},
};

use crate::model::Finding;

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum PathNormalizeError {
    #[error("path '{0}' is outside project directory")]
    OutsideProject(String),
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum AnchorError {
    #[error("habit-sensors: sensor '{sensor}' reported a path outside the project: '{path}'")]
    OutsideProject {
        sensor: String,
        path: String,
    },

    #[error(
        "habit-sensors: sensor '{sensor}' keys {count} files as '{key}' ({files}) — snoozing it would exempt them all"
    )]
    AliasedKey {
        sensor: String,
        count: usize,
        key: String,
        files: String,
    },
}

pub fn normalize_path(path: &Path) -> PathBuf {
    let mut components = Vec::new();
    for comp in path.components() {
        match comp {
            Component::CurDir => {}
            Component::ParentDir => {
                if let Some(Component::Normal(_)) = components.last() {
                    components.pop();
                } else {
                    components.push(comp);
                }
            }
            _ => components.push(comp),
        }
    }
    components.into_iter().collect()
}

pub fn normalize_to_relative_slash(
    project_dir: &Path,
    raw_path_str: &str,
) -> Result<String, PathNormalizeError> {
    let raw_path = Path::new(raw_path_str);
    let canonical_proj = project_dir
        .canonicalize()
        .unwrap_or_else(|_| project_dir.to_path_buf());

    let abs_project = if canonical_proj.is_absolute() {
        normalize_path(&canonical_proj)
    } else {
        match std::env::current_dir() {
            Ok(cwd) => normalize_path(&cwd.join(&canonical_proj)),
            Err(_) => normalize_path(&canonical_proj),
        }
    };

    let target_abs = if raw_path.is_absolute() {
        raw_path.canonicalize().unwrap_or_else(|_| normalize_path(raw_path))
    } else {
        normalize_path(&abs_project.join(raw_path))
    };

    // Check if target_abs is under abs_project
    let relative = match pathdiff::diff_paths(&target_abs, &abs_project) {
        Some(diff) => diff,
        None => {
            return Err(PathNormalizeError::OutsideProject(
                raw_path_str.to_string(),
            ))
        }
    };

    let diff_str = relative.to_string_lossy();
    if diff_str.starts_with("..")
        || diff_str.contains("/../")
        || diff_str.contains("\\..\\")
    {
        return Err(PathNormalizeError::OutsideProject(
            raw_path_str.to_string(),
        ));
    }

    let normalized = diff_str.replace('\\', "/");
    let trimmed = normalized.strip_prefix("./").unwrap_or(&normalized);
    Ok(trimmed.to_string())
}

pub fn is_path_like(s: &str) -> bool {
    s.contains('/')
        || s.contains('\\')
        || s.ends_with(".rs")
        || s.ends_with(".ts")
        || s.ends_with(".js")
        || s.ends_with(".py")
}

pub fn anchor_findings(
    findings: &mut [Finding],
    project_dir: &Path,
    sensor_name: &str,
) -> Result<(), AnchorError> {
    // Map from key -> set of distinct anchored file paths
    let mut key_to_files: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();

    for finding in findings.iter_mut() {
        for issue in finding.issues.iter_mut() {
            let original_file = &issue.details.file;
            let anchored_file =
                match normalize_to_relative_slash(project_dir, original_file) {
                    Ok(f) => f,
                    Err(_) => {
                        return Err(AnchorError::OutsideProject {
                            sensor: sensor_name.to_string(),
                            path: original_file.to_string(),
                        });
                    }
                };

            let is_key_matching_file = issue.key == issue.details.file;

            // Update file in details
            issue.details.file = anchored_file.clone();

            // Anchor key if it matches the file or is path-like pointing to
            // this file
            if is_key_matching_file {
                issue.key = anchored_file.clone();
            } else if is_path_like(&issue.key) {
                if let Ok(anchored_key) =
                    normalize_to_relative_slash(project_dir, &issue.key)
                {
                    issue.key = anchored_key;
                }
            }

            key_to_files
                .entry(issue.key.clone())
                .or_default()
                .insert(issue.details.file.clone());
        }
    }

    // Check for aliased keys
    for (key, files) in key_to_files {
        if files.len() > 1 && is_path_like(&key) {
            let files_joined = files.into_iter().collect::<Vec<_>>().join(", ");
            return Err(AnchorError::AliasedKey {
                sensor: sensor_name.to_string(),
                count: files_joined.split(',').count(),
                key,
                files: files_joined,
            });
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;
    use crate::model::{Issue, IssueDetails};

    mod anchoring {
        use pretty_assertions::assert_eq;

        use super::*;

        #[test]
        fn anchors_relative_and_absolute_paths_within_project() {
            let project = PathBuf::from("/workspace/my-project");
            let mut findings = vec![Finding {
                smell: "unused-variable".to_string(),
                language: None,
                details: serde_json::json!({}),
                issues: vec![
                    Issue {
                        key: "src/main.rs".to_string(),
                        details: IssueDetails {
                            file: "src/./main.rs".to_string(),
                            line: Some(10),
                            column: None,
                            content: None,
                            message: None,
                            source: None,
                            extra: BTreeMap::new(),
                        },
                    },
                    Issue {
                        key: "/workspace/my-project/src/lib.rs".to_string(),
                        details: IssueDetails {
                            file: "/workspace/my-project/src/lib.rs"
                                .to_string(),
                            line: Some(20),
                            column: None,
                            content: None,
                            message: None,
                            source: None,
                            extra: BTreeMap::new(),
                        },
                    },
                ],
            }];

            anchor_findings(&mut findings, &project, "test-sensor").unwrap();

            assert_eq!(findings[0].issues[0].details.file, "src/main.rs");
            assert_eq!(findings[0].issues[0].key, "src/main.rs");
            assert_eq!(findings[0].issues[1].details.file, "src/lib.rs");
            assert_eq!(findings[0].issues[1].key, "src/lib.rs");
        }

        #[test]
        fn rejects_paths_escaping_project_root() {
            let project = PathBuf::from("/workspace/my-project");
            let mut findings = vec![Finding {
                smell: "oversized-file".to_string(),
                language: None,
                details: serde_json::json!({}),
                issues: vec![Issue {
                    key: "../outside.rs".to_string(),
                    details: IssueDetails {
                        file: "../outside.rs".to_string(),
                        line: Some(1),
                        column: None,
                        content: None,
                        message: None,
                        source: None,
                        extra: BTreeMap::new(),
                    },
                }],
            }];

            let err =
                anchor_findings(&mut findings, &project, "clippy").unwrap_err();
            assert_eq!(err, AnchorError::OutsideProject {
                sensor: "clippy".to_string(),
                path: "../outside.rs".to_string()
            });
        }

        #[test]
        fn preserves_non_path_keys() {
            let project = PathBuf::from("/workspace/my-project");
            let mut findings = vec![Finding {
                smell: "unused-dependency".to_string(),
                language: None,
                details: serde_json::json!({}),
                issues: vec![Issue {
                    key: "serde_json".to_string(),
                    details: IssueDetails {
                        file: "Cargo.toml".to_string(),
                        line: Some(5),
                        column: None,
                        content: None,
                        message: None,
                        source: None,
                        extra: BTreeMap::new(),
                    },
                }],
            }];

            anchor_findings(&mut findings, &project, "cargo-machete").unwrap();

            assert_eq!(findings[0].issues[0].key, "serde_json");
            assert_eq!(findings[0].issues[0].details.file, "Cargo.toml");
        }

        #[test]
        fn rejects_aliased_path_keys_mapping_to_multiple_files() {
            let project = PathBuf::from("/workspace/my-project");
            let mut findings = vec![Finding {
                smell: "duplicated-code".to_string(),
                language: None,
                details: serde_json::json!({}),
                issues: vec![
                    Issue {
                        key: "lib.rs".to_string(),
                        details: IssueDetails {
                            file: "src/lib.rs".to_string(),
                            line: Some(1),
                            column: None,
                            content: None,
                            message: None,
                            source: None,
                            extra: BTreeMap::new(),
                        },
                    },
                    Issue {
                        key: "lib.rs".to_string(),
                        details: IssueDetails {
                            file: "tests/lib.rs".to_string(),
                            line: Some(1),
                            column: None,
                            content: None,
                            message: None,
                            source: None,
                            extra: BTreeMap::new(),
                        },
                    },
                ],
            }];

            let err =
                anchor_findings(&mut findings, &project, "jscpd").unwrap_err();
            match err {
                AnchorError::AliasedKey {
                    sensor,
                    count,
                    key,
                    ..
                } => {
                    assert_eq!(sensor, "jscpd");
                    assert_eq!(count, 2);
                    assert_eq!(key, "lib.rs");
                }
                _ => panic!("expected AliasedKey error"),
            }
        }
    }
}
