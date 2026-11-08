use std::{
    path::{Path, PathBuf},
    sync::LazyLock,
};

use habit_core::{Finding, Severity};
use regex::Regex;

use crate::model::TriageResult;

static UNWRAP_GUARD_REGEX: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(!\s*[\w\.\(\)\->]+\s*\.is_empty\(\)|\.is_some\(\)|matches!\s*\()",
    )
    .unwrap()
});

fn get_file_content(file: &str, project_dir: Option<&Path>) -> Option<String> {
    let path = if let Some(dir) = project_dir {
        dir.join(file)
    } else {
        PathBuf::from(file)
    };
    std::fs::read_to_string(&path).ok()
}

fn get_surrounding_context(
    file: &str,
    line: usize,
    project_dir: Option<&Path>,
) -> Option<String> {
    let content = get_file_content(file, project_dir)?;
    let lines: Vec<&str> = content.lines().collect();
    if lines.is_empty() {
        return None;
    }
    let target_idx = line.saturating_sub(1);
    let start_idx = target_idx.saturating_sub(10);
    let end_idx = (target_idx + 1).min(lines.len());
    Some(lines[start_idx..end_idx].join("\n"))
}

pub fn triage_finding(
    finding: &Finding,
    project_dir: Option<&Path>,
) -> TriageResult {
    let mut result = TriageResult::default();

    // Rule 1: Proven Invariant Unwrap
    if finding.smell == "unhandled-unwrap" {
        let mut all_issues_guarded = !finding.issues.is_empty();
        for issue in &finding.issues {
            let mut issue_guarded = false;

            // Check issue.details.content
            if let Some(ref content) = issue.details.content {
                if UNWRAP_GUARD_REGEX.is_match(content)
                    || content.contains("!is_empty")
                    || content.contains("is_some")
                    || content.contains("matches!")
                {
                    issue_guarded = true;
                }
            }

            // Check surrounding lines from file
            if !issue_guarded {
                let context = issue
                    .details
                    .line
                    .and_then(|line| {
                        get_surrounding_context(
                            &issue.details.file,
                            line,
                            project_dir,
                        )
                    })
                    .unwrap_or_default();
                issue_guarded = UNWRAP_GUARD_REGEX.is_match(&context)
                    || context.contains("!is_empty")
                    || context.contains("is_some")
                    || context.contains("matches!");
            }

            if !issue_guarded {
                all_issues_guarded = false;
                break;
            }
        }

        if all_issues_guarded {
            result.is_benign = true;
            result.rationale =
                Some("Guarded by preceding non-empty condition".to_string());
            return result;
        }
    }

    // Rule 2: Contextual Severity Adjustment
    if finding.smell == "too-many-parameters"
        || finding.smell == "high-complexity"
    {
        let is_test_file =
            finding.issues.iter().any(|i| i.details.file.contains("test"));

        let primary_content = finding
            .issues
            .first()
            .and_then(|i| i.details.content.clone())
            .or_else(|| {
                let first_issue = finding.issues.first()?;
                let line = first_issue.details.line?;
                let content =
                    get_file_content(&first_issue.details.file, project_dir)?;
                content
                    .lines()
                    .nth(line.saturating_sub(1))
                    .map(|s| s.to_string())
            });

        let trimmed_content =
            primary_content.as_deref().unwrap_or("").trim_start();

        if is_test_file
            || trimmed_content.starts_with("fn ")
            || trimmed_content.starts_with("async fn ")
        {
            result.adjusted_severity = Some(Severity::Suggested);
        } else if trimmed_content.starts_with("pub ") {
            result.adjusted_severity = Some(Severity::Enforced);
        }
    }

    // Rule 3: Coaching Archetype Selection
    if finding.smell == "high-complexity" {
        let combined_content = finding
            .issues
            .iter()
            .filter_map(|i| i.details.content.as_deref())
            .collect::<Vec<_>>()
            .join("\n");

        let content_to_check = if !combined_content.is_empty() {
            combined_content
        } else if let Some(first_issue) = finding.issues.first() {
            get_file_content(&first_issue.details.file, project_dir)
                .unwrap_or_default()
        } else {
            String::new()
        };

        let archetype = if content_to_check.contains("match ") {
            "flatten_match"
        } else if content_to_check.contains("for ")
            && content_to_check.contains("if ")
        {
            "monadic_combinator"
        } else {
            "extract_helper"
        };

        result.coaching_archetype = Some(archetype.to_string());
    }

    result
}

pub fn validate_snooze_reason(reason: &str) -> Result<(), &'static str> {
    let trimmed = reason.trim();
    if trimmed.len() < 10 {
        return Err("snooze reason must be at least 10 characters");
    }

    let lower = trimmed.to_ascii_lowercase();
    let trivial_patterns = ["todo", "fix later", "ignore", "temp", "bypass"];
    for pat in &trivial_patterns {
        if lower.contains(pat) {
            return Err("snooze reason contains trivial or non-actionable justification");
        }
    }

    let technical_contexts =
        ["ffi", "lifetime", "legacy", "hardware", "external", "upstream"];
    let has_technical_context =
        technical_contexts.iter().any(|kw| lower.contains(kw));
    if !has_technical_context {
        return Err("snooze reason must include technical context (e.g. ffi, lifetime, legacy, hardware, external, upstream)");
    }

    Ok(())
}

pub fn triage_findings(
    findings: Vec<Finding>,
    project_dir: Option<&Path>,
) -> Vec<Finding> {
    let mut result = Vec::new();

    for mut finding in findings {
        let triage = triage_finding(&finding, project_dir);
        if triage.is_benign {
            continue;
        }

        if !finding.details.is_object() {
            finding.details = serde_json::Value::Object(serde_json::Map::new());
        }

        if let Some(map) = finding.details.as_object_mut() {
            if let Some(severity) = triage.adjusted_severity {
                map.insert(
                    "severity".to_string(),
                    serde_json::to_value(severity).unwrap(),
                );
            }
            if let Some(archetype) = triage.coaching_archetype {
                map.insert(
                    "archetype".to_string(),
                    serde_json::Value::String(archetype),
                );
            }
        }

        result.push(finding);
    }

    result
}

#[cfg(test)]
mod tests {
    use habit_core::{Finding, Issue, IssueDetails};
    use pretty_assertions::assert_eq;

    use super::*;

    #[test]
    fn test_unwrap_guarded_by_is_empty_is_benign() {
        let finding = Finding {
            smell: "unhandled-unwrap".to_string(),
            language: Some("rust".to_string()),
            details: serde_json::json!({}),
            issues: vec![Issue {
                key: "src/lib.rs".to_string(),
                details: IssueDetails {
                    file: "src/lib.rs".to_string(),
                    line: Some(10),
                    column: Some(5),
                    content: Some("if !vec.is_empty() { let val = vec.first().unwrap(); }".to_string()),
                    message: Some("unhandled unwrap".to_string()),
                    source: None,
                    extra: Default::default(),
                },
            }],
        };

        let result = triage_finding(&finding, None);
        assert_eq!(result.is_benign, true);
        assert_eq!(
            result.rationale,
            Some("Guarded by preceding non-empty condition".to_string())
        );

        let triaged = triage_findings(vec![finding], None);
        assert_eq!(triaged.len(), 0);
    }

    #[test]
    fn test_unwrap_unguarded_is_not_benign() {
        let finding = Finding {
            smell: "unhandled-unwrap".to_string(),
            language: Some("rust".to_string()),
            details: serde_json::json!({}),
            issues: vec![Issue {
                key: "src/lib.rs".to_string(),
                details: IssueDetails {
                    file: "src/lib.rs".to_string(),
                    line: Some(10),
                    column: Some(5),
                    content: Some(
                        "let val = vec.first().unwrap();".to_string(),
                    ),
                    message: Some("unhandled unwrap".to_string()),
                    source: None,
                    extra: Default::default(),
                },
            }],
        };

        let result = triage_finding(&finding, None);
        assert_eq!(result.is_benign, false);
    }

    #[test]
    fn test_severity_adjustment_for_test_and_pub() {
        let test_finding = Finding {
            smell: "high-complexity".to_string(),
            language: Some("rust".to_string()),
            details: serde_json::json!({}),
            issues: vec![Issue {
                key: "tests/integration_test.rs".to_string(),
                details: IssueDetails {
                    file: "tests/integration_test.rs".to_string(),
                    line: Some(1),
                    column: None,
                    content: Some("pub fn complex_helper() {}".to_string()),
                    message: None,
                    source: None,
                    extra: Default::default(),
                },
            }],
        };

        let triage_test = triage_finding(&test_finding, None);
        assert_eq!(triage_test.adjusted_severity, Some(Severity::Suggested));

        let pub_finding = Finding {
            smell: "high-complexity".to_string(),
            language: Some("rust".to_string()),
            details: serde_json::json!({}),
            issues: vec![Issue {
                key: "src/api.rs".to_string(),
                details: IssueDetails {
                    file: "src/api.rs".to_string(),
                    line: Some(1),
                    column: None,
                    content: Some("pub fn public_complex_api() {}".to_string()),
                    message: None,
                    source: None,
                    extra: Default::default(),
                },
            }],
        };

        let triage_pub = triage_finding(&pub_finding, None);
        assert_eq!(triage_pub.adjusted_severity, Some(Severity::Enforced));
    }

    #[test]
    fn test_coaching_archetypes() {
        let match_finding = Finding {
            smell: "high-complexity".to_string(),
            language: Some("rust".to_string()),
            details: serde_json::json!({}),
            issues: vec![Issue {
                key: "src/lib.rs".to_string(),
                details: IssueDetails {
                    file: "src/lib.rs".to_string(),
                    line: Some(1),
                    column: None,
                    content: Some("match val { A => 1, B => 2 }".to_string()),
                    message: None,
                    source: None,
                    extra: Default::default(),
                },
            }],
        };
        let res_match = triage_finding(&match_finding, None);
        assert_eq!(
            res_match.coaching_archetype,
            Some("flatten_match".to_string())
        );

        let loop_finding = Finding {
            smell: "high-complexity".to_string(),
            language: Some("rust".to_string()),
            details: serde_json::json!({}),
            issues: vec![Issue {
                key: "src/lib.rs".to_string(),
                details: IssueDetails {
                    file: "src/lib.rs".to_string(),
                    line: Some(1),
                    column: None,
                    content: Some(
                        "for item in items { if item.valid() { count += 1; } }"
                            .to_string(),
                    ),
                    message: None,
                    source: None,
                    extra: Default::default(),
                },
            }],
        };
        let res_loop = triage_finding(&loop_finding, None);
        assert_eq!(
            res_loop.coaching_archetype,
            Some("monadic_combinator".to_string())
        );
    }

    #[test]
    fn test_snooze_reason_validation() {
        assert!(validate_snooze_reason("fix later").is_err());
        assert!(validate_snooze_reason("todo: cleanup later").is_err());
        assert!(validate_snooze_reason("too short").is_err());
        assert!(validate_snooze_reason(
            "legacy C FFI boundary requires raw unwrap"
        )
        .is_ok());
        assert!(validate_snooze_reason(
            "lifetime constraint prevents safe borrowing"
        )
        .is_ok());
    }
}
