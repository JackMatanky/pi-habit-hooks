use std::collections::BTreeMap;

use serde::{Deserialize, Deserializer, Serialize, Serializer};

fn default_details() -> serde_json::Value {
    serde_json::Value::Object(serde_json::Map::new())
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Finding {
    pub smell: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub language: Option<String>,
    #[serde(default = "default_details")]
    pub details: serde_json::Value,
    pub issues: Vec<Issue>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Issue {
    pub key: String,
    pub details: IssueDetails,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct IssueDetails {
    pub file: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub line: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub column: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    #[serde(flatten)]
    pub extra: BTreeMap<String, serde_json::Value>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Severity {
    Enforced,
    Suggested,
}

impl Severity {
    pub fn is_enforced(&self) -> bool {
        matches!(self, Severity::Enforced)
    }

    pub fn is_suggested(&self) -> bool {
        matches!(self, Severity::Suggested)
    }
}

impl Serialize for Severity {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        match self {
            Severity::Enforced => serializer.serialize_str("enforced"),
            Severity::Suggested => serializer.serialize_str("suggested"),
        }
    }
}

impl<'de> Deserialize<'de> for Severity {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let s = String::deserialize(deserializer)?;
        match s.to_ascii_lowercase().as_str() {
            "enforced" => Ok(Severity::Enforced),
            "suggested" => Ok(Severity::Suggested),
            other => Err(serde::de::Error::custom(format!(
                "invalid severity '{}', expected 'enforced' or 'suggested'",
                other
            ))),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum UncoachedHandling {
    #[default]
    Suggest,
    Ignore,
    Enforce,
}

impl UncoachedHandling {
    pub fn is_ignored(&self) -> bool {
        matches!(self, UncoachedHandling::Ignore)
    }

    pub fn is_enforced(&self) -> bool {
        matches!(self, UncoachedHandling::Enforce)
    }

    pub fn is_suggested(&self) -> bool {
        matches!(self, UncoachedHandling::Suggest)
    }
}

impl Serialize for UncoachedHandling {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        match self {
            UncoachedHandling::Suggest => serializer.serialize_str("suggest"),
            UncoachedHandling::Ignore => serializer.serialize_str("ignore"),
            UncoachedHandling::Enforce => serializer.serialize_str("enforce"),
        }
    }
}

impl<'de> Deserialize<'de> for UncoachedHandling {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let s = String::deserialize(deserializer)?;
        match s.to_ascii_lowercase().as_str() {
            "suggest" => Ok(UncoachedHandling::Suggest),
            "ignore" => Ok(UncoachedHandling::Ignore),
            "enforce" => Ok(UncoachedHandling::Enforce),
            other => Err(serde::de::Error::custom(format!(
                "invalid uncoached handling '{}', expected 'suggest', 'ignore', or 'enforce'",
                other
            ))),
        }
    }
}

pub struct SmellCatalogue;

impl SmellCatalogue {
    pub const CHANGE_RISK_HOTSPOT: &'static str = "change-risk-hotspot";
    pub const CYCLIC_DEPENDENCY: &'static str = "cyclic-dependency";
    pub const DUPLICATED_CODE: &'static str = "duplicated-code";
    pub const DUPLICATE_DEPENDENCY: &'static str = "duplicate-dependency";
    pub const HIGH_COMPLEXITY: &'static str = "high-complexity";
    pub const INCOMPLETE_RUN: &'static str = "incomplete-run";
    // New additions from rust-skills
    pub const LOCK_ACROSS_AWAIT: &'static str = "lock-across-await";
    pub const NON_CONST_BINDING: &'static str = "non-const-binding";
    pub const NON_ESSENTIAL_COMMENT: &'static str = "non-essential-comment";
    pub const OVERSIZED_FILE: &'static str = "oversized-file";
    pub const OVERSIZED_FUNCTION: &'static str = "oversized-function";
    pub const PARSE_ERROR: &'static str = "parse-error";
    pub const REDUNDANT_CLONE: &'static str = "redundant-clone";
    pub const SUBOPTIMAL_BORROW_TYPE: &'static str = "suboptimal-borrow-type";
    pub const SWALLOWED_EXCEPTION: &'static str = "swallowed-exception";
    pub const TEST_ONLY_DEAD_CODE: &'static str = "test-only-dead-code";
    pub const TOO_MANY_PARAMETERS: &'static str = "too-many-parameters";
    pub const UNDOCUMENTED_UNSAFE: &'static str = "undocumented-unsafe";
    pub const UNHANDLED_UNWRAP: &'static str = "unhandled-unwrap";
    pub const UNUSED_DEPENDENCY: &'static str = "unused-dependency";
    pub const UNUSED_EXPORT: &'static str = "unused-export";
    pub const UNUSED_FILE: &'static str = "unused-file";
    pub const UNUSED_IMPORT: &'static str = "unused-import";
    pub const UNUSED_VARIABLE: &'static str = "unused-variable";
    pub const WARNING_COMMENT: &'static str = "warning-comment";

    pub fn default_severity(smell: &str) -> Severity {
        match smell {
            Self::TOO_MANY_PARAMETERS
            | Self::HIGH_COMPLEXITY
            | Self::OVERSIZED_FUNCTION
            | Self::OVERSIZED_FILE
            | Self::CYCLIC_DEPENDENCY
            | Self::CHANGE_RISK_HOTSPOT
            | Self::SWALLOWED_EXCEPTION
            | Self::DUPLICATE_DEPENDENCY
            | Self::INCOMPLETE_RUN
            | Self::PARSE_ERROR
            | Self::LOCK_ACROSS_AWAIT
            | Self::UNDOCUMENTED_UNSAFE => Severity::Enforced,

            Self::UNUSED_VARIABLE
            | Self::NON_CONST_BINDING
            | Self::UNUSED_IMPORT
            | Self::UNUSED_FILE
            | Self::UNUSED_DEPENDENCY
            | Self::UNUSED_EXPORT
            | Self::TEST_ONLY_DEAD_CODE
            | Self::WARNING_COMMENT
            | Self::NON_ESSENTIAL_COMMENT
            | Self::DUPLICATED_CODE
            | Self::UNHANDLED_UNWRAP
            | Self::REDUNDANT_CLONE
            | Self::SUBOPTIMAL_BORROW_TYPE => Severity::Suggested,

            // Unknown / open-vocabulary smells default to Suggested unless
            // configured otherwise
            _ => Severity::Suggested,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    mod serialization {
        use pretty_assertions::assert_eq;

        use super::*;

        #[test]
        fn deserializes_finding_with_empty_details_when_omitted() {
            let json = r#"{
                "smell": "too-many-parameters",
                "issues": [
                    {
                        "key": "src/lib.rs",
                        "details": {
                            "file": "src/lib.rs",
                            "line": 42,
                            "custom_field": "hello"
                        }
                    }
                ]
            }"#;

            let finding: Finding = serde_json::from_str(json).unwrap();
            assert_eq!(finding.smell, "too-many-parameters");
            assert_eq!(finding.language, None);
            assert_eq!(finding.details, serde_json::json!({}));
            assert_eq!(finding.issues.len(), 1);
            assert_eq!(finding.issues[0].details.file, "src/lib.rs");
            assert_eq!(finding.issues[0].details.line, Some(42));
            assert_eq!(
                finding.issues[0].details.extra.get("custom_field").unwrap(),
                &serde_json::json!("hello")
            );
        }

        #[test]
        fn parses_severity_case_insensitively() {
            let s1: Severity = serde_json::from_str(r#""enforced""#).unwrap();
            let s2: Severity = serde_json::from_str(r#""ENFORCED""#).unwrap();
            let s3: Severity = serde_json::from_str(r#""suggested""#).unwrap();

            assert_eq!(s1, Severity::Enforced);
            assert_eq!(s2, Severity::Enforced);
            assert_eq!(s3, Severity::Suggested);
        }

        #[test]
        fn rejects_invalid_severity_value() {
            let result: Result<Severity, _> =
                serde_json::from_str(r#""unknown""#);
            assert!(result.is_err());
        }

        #[test]
        fn verifies_default_severity_for_new_smells() {
            assert_eq!(
                SmellCatalogue::default_severity(
                    SmellCatalogue::LOCK_ACROSS_AWAIT
                ),
                Severity::Enforced
            );
            assert_eq!(
                SmellCatalogue::default_severity(
                    SmellCatalogue::UNDOCUMENTED_UNSAFE
                ),
                Severity::Enforced
            );
            assert_eq!(
                SmellCatalogue::default_severity(
                    SmellCatalogue::UNHANDLED_UNWRAP
                ),
                Severity::Suggested
            );
            assert_eq!(
                SmellCatalogue::default_severity(
                    SmellCatalogue::REDUNDANT_CLONE
                ),
                Severity::Suggested
            );
            assert_eq!(
                SmellCatalogue::default_severity(
                    SmellCatalogue::SUBOPTIMAL_BORROW_TYPE
                ),
                Severity::Suggested
            );
        }
    }
}
