use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
};

use habit_core::Finding;
use minijinja::{value::Serde, Environment, Value};

const DEFAULT_LINE_LEVEL_TEMPLATE: &str = r#"{% for issue in issues %}{{ issue.details.file }}{% if issue.details.line %}:{{ issue.details.line }}{% endif %}{% if issue.details.content %}  {{ issue.details.content }}{% elif issue.details.message %}  {{ issue.details.message }}{% endif %}
{% endfor %}"#;

const DEFAULT_FILE_LEVEL_TEMPLATE: &str = r#"{% for issue in issues %}{{ issue.details.file }}
{% endfor %}"#;

const DEFAULT_UNCOACHED_TEMPLATE: &str = r#"### {{ smell }}
{% if issues %}
{% for issue in issues %}- {{ issue.details.file }}{% if issue.details.line %}:{{ issue.details.line }}{% endif %}{% if issue.details.message %}: {{ issue.details.message }}{% endif %}
{% endfor %}
{% endif %}
"#;

const DEFAULT_INCOMPLETE_RUN_TEMPLATE: &str = r#"⚠️ Habit Hooks: Incomplete Run

Automated checks could not run to completion.
{% for issue in issues %}
- {{ issue.key }}{% if issue.details.message %}: {{ issue.details.message }}{% endif %}
{% endfor %}
"#;

pub struct GuideResolver {
    project_dir: PathBuf,
    plugin_dirs: Vec<PathBuf>,
    custom_guides: BTreeMap<String, PathBuf>,
}

impl GuideResolver {
    pub fn new(
        project_dir: &Path,
        plugin_dirs: &[PathBuf],
        custom_guides: &BTreeMap<String, String>,
    ) -> Self {
        let mut custom = BTreeMap::new();
        for (smell, rel_path) in custom_guides {
            let p = PathBuf::from(rel_path);
            let abs_path = if p.is_absolute() {
                p
            } else {
                project_dir.join(p)
            };
            custom.insert(smell.clone(), abs_path);
        }

        Self {
            project_dir: project_dir.to_path_buf(),
            plugin_dirs: plugin_dirs.to_vec(),
            custom_guides: custom,
        }
    }

    pub fn find_guide_content(&self, smell: &str) -> (String, Option<PathBuf>) {
        if smell == "incomplete-run" {
            return (DEFAULT_INCOMPLETE_RUN_TEMPLATE.to_string(), None);
        }

        // 1. Project config override for this smell
        if let Some(path) = self.custom_guides.get(smell) {
            if path.is_file() {
                if let Ok(content) = fs::read_to_string(path) {
                    return (content, Some(path.clone()));
                }
            }
        }

        // 2. Project directory overrides:
        //    .habit-hooks/<plugin>/guides/<smell>.md
        for plugin_dir in &self.plugin_dirs {
            let plugin_name =
                plugin_dir.file_name().unwrap_or_default().to_string_lossy();
            let project_plugin_guide = self
                .project_dir
                .join(".habit-hooks")
                .join(plugin_name.as_ref())
                .join("guides")
                .join(format!("{smell}.md"));

            if project_plugin_guide.is_file() {
                if let Ok(content) = fs::read_to_string(&project_plugin_guide) {
                    return (content, Some(project_plugin_guide));
                }
            }
        }

        // 3. Plugin directory: guides/<smell>.md
        for plugin_dir in &self.plugin_dirs {
            let guide_path =
                plugin_dir.join("guides").join(format!("{smell}.md"));
            if guide_path.is_file() {
                if let Ok(content) = fs::read_to_string(&guide_path) {
                    return (content, Some(guide_path));
                }
            }
        }

        // 4. Default uncoached fallback
        (DEFAULT_UNCOACHED_TEMPLATE.to_string(), None)
    }

    pub fn render(
        &self,
        finding: &Finding,
    ) -> Result<String, minijinja::Error> {
        let (content, guide_path) = self.find_guide_content(&finding.smell);

        let mut env = Environment::new();

        // Load partials if available from guide path parent or plugin includes
        let mut line_template = DEFAULT_LINE_LEVEL_TEMPLATE.to_string();
        let mut file_template = DEFAULT_FILE_LEVEL_TEMPLATE.to_string();

        if let Some(guide_dir) =
            guide_path.as_ref().and_then(|path| path.parent())
        {
            let inc_line =
                guide_dir.join("includes").join("line_level_issues.md");
            let inc_file =
                guide_dir.join("includes").join("file_level_issues.md");

            if let Ok(content) = fs::read_to_string(&inc_line) {
                line_template = content;
            }
            if let Ok(content) = fs::read_to_string(&inc_file) {
                file_template = content;
            }
        }

        // Also check plugin includes
        for plugin_dir in &self.plugin_dirs {
            let inc_line = plugin_dir
                .join("guides")
                .join("includes")
                .join("line_level_issues.md");
            let inc_file = plugin_dir
                .join("guides")
                .join("includes")
                .join("file_level_issues.md");

            if let Ok(content) = fs::read_to_string(&inc_line) {
                line_template = content;
            }
            if let Ok(content) = fs::read_to_string(&inc_file) {
                file_template = content;
            }
        }

        let _ =
            env.add_template("includes/line_level_issues.md", &line_template);
        let _ =
            env.add_template("includes/file_level_issues.md", &file_template);

        let tmpl = env.template_from_str(&content)?;
        let ctx = minijinja::context! {
            smell => &finding.smell,
            language => &finding.language,
            details => Value::from(Serde(&finding.details)),
            issues => Value::from(Serde(&finding.issues)),
        };

        tmpl.render(ctx)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    mod rendering {
        use habit_core::{Issue, IssueDetails};
        use pretty_assertions::assert_eq;

        use super::*;

        #[test]
        fn renders_uncoached_template_with_issues() {
            let finding = Finding {
                smell: "custom-smell".to_string(),
                language: Some("rust".to_string()),
                details: serde_json::json!({}),
                issues: vec![Issue {
                    key: "src/main.rs".to_string(),
                    details: IssueDetails {
                        file: "src/main.rs".to_string(),
                        line: Some(15),
                        column: None,
                        content: None,
                        message: Some("variable never used".to_string()),
                        source: None,
                        extra: BTreeMap::new(),
                    },
                }],
            };

            let resolver =
                GuideResolver::new(Path::new("."), &[], &BTreeMap::new());
            let output = resolver.render(&finding).unwrap();

            assert_eq!(output.contains("### custom-smell"), true);
            assert_eq!(
                output.contains("- src/main.rs:15: variable never used"),
                true
            );
        }

        #[test]
        fn renders_incomplete_run_template() {
            let finding = Finding {
                smell: "incomplete-run".to_string(),
                language: None,
                details: serde_json::json!({}),
                issues: vec![Issue {
                    key: "sensor:clippy".to_string(),
                    details: IssueDetails {
                        file: ".habit-hooks/config.toml".to_string(),
                        line: None,
                        column: None,
                        content: None,
                        message: Some("compiler failed".to_string()),
                        source: None,
                        extra: BTreeMap::new(),
                    },
                }],
            };

            let resolver =
                GuideResolver::new(Path::new("."), &[], &BTreeMap::new());
            let output = resolver.render(&finding).unwrap();

            assert_eq!(output.contains("⚠️ Habit Hooks: Incomplete Run"), true);
            assert_eq!(output.contains("sensor:clippy: compiler failed"), true);
        }

        #[test]
        fn renders_shipped_rust_guides_when_configured() {
            let workspace = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .parent()
                .unwrap()
                .parent()
                .unwrap()
                .to_path_buf();
            let rust_plugin_dir = workspace.join("plugins").join("rust");
            let resolver = GuideResolver::new(
                &workspace,
                &[rust_plugin_dir],
                &BTreeMap::new(),
            );

            let test_smells = [
                ("lock-across-await", "lock across an `.await`"),
                (
                    "undocumented-unsafe",
                    "Unsafe code blocks without explicit safety",
                ),
                ("unhandled-unwrap", "Bare `.unwrap()` or panic-inducing"),
                ("redundant-clone", "Excessive or unnecessary `.clone()`"),
                ("suboptimal-borrow-type", "concrete container references"),
            ];

            for (smell, expected_snippet) in test_smells {
                let finding = Finding {
                    smell: smell.to_string(),
                    language: Some("rust".to_string()),
                    details: serde_json::json!({}),
                    issues: vec![Issue {
                        key: "src/lib.rs".to_string(),
                        details: IssueDetails {
                            file: "src/lib.rs".to_string(),
                            line: Some(10),
                            column: Some(4),
                            content: Some("code snippet".to_string()),
                            message: Some("violation message".to_string()),
                            source: Some("clippy".to_string()),
                            extra: BTreeMap::new(),
                        },
                    }],
                };

                let rendered = resolver.render(&finding).unwrap();
                assert_eq!(rendered.contains(expected_snippet), true);
            }
        }
    }
}
