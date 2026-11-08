use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};

use crate::model::{Severity, UncoachedHandling};

fn default_main() -> String {
    "main".to_string()
}

fn default_false() -> bool {
    false
}

fn default_command_kind() -> String {
    "command".to_string()
}

fn default_success_exit_codes() -> Vec<i32> {
    vec![0]
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ScopeConfig {
    #[serde(default, rename = "changedOnly")]
    pub changed_only: bool,
    #[serde(default = "default_false", rename = "autoBranchOffMain")]
    pub auto_branch_off_main: bool,
    #[serde(default = "default_main", rename = "branchBase")]
    pub branch_base: String,
    #[serde(default = "default_main", rename = "mainBranch")]
    pub main_branch: String,
}

impl Default for ScopeConfig {
    fn default() -> Self {
        Self {
            changed_only: false,
            auto_branch_off_main: false,
            branch_base: default_main(),
            main_branch: default_main(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
pub struct SmellConfig {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub severity: Option<Severity>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub disabled: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub guide: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
pub struct SensorOverrideConfig {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub disabled: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub files: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub args: Option<Vec<String>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
pub struct ProjectConfig {
    #[serde(default)]
    pub plugins: Vec<String>,
    #[serde(default)]
    pub uncoached: UncoachedHandling,
    #[serde(default)]
    pub scope: ScopeConfig,
    #[serde(default)]
    pub smells: BTreeMap<String, SmellConfig>,
    #[serde(default)]
    pub sensors: BTreeMap<String, SensorOverrideConfig>,
    #[serde(default)]
    pub runners: BTreeMap<String, String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
pub struct PluginConfig {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub language: Option<String>,
    #[serde(default)]
    pub files: Vec<String>,
    #[serde(default)]
    pub detectors: Vec<DetectorConfig>,
    #[serde(default)]
    pub sensors: Vec<SensorConfig>,
    #[serde(default, skip_serializing)]
    pub plugin_dir: PathBuf,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct DetectorConfig {
    pub name: String,
    #[serde(default = "default_command_kind")]
    pub kind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub install: Option<String>,
    #[serde(default)]
    pub search_paths: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SensorConfig {
    pub name: String,
    pub tool: String,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default = "default_success_exit_codes")]
    pub success_exit_codes: Vec<i32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub transform: Option<String>,
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum ConfigError {
    #[error("failed to read config file '{path}': {message}")]
    IoError {
        path: String,
        message: String,
    },

    #[error("invalid config file '{path}': {message}")]
    ParseError {
        path: String,
        message: String,
    },

    #[error(
        "habit-sensors: sensor '{sensor}' config specifies 'args', but sensor command template does not include '${{args}}'"
    )]
    MissingArgsPlaceholder {
        sensor: String,
    },
}

impl ProjectConfig {
    pub fn load_from_str(
        content: &str,
        path_for_err: &str,
    ) -> Result<Self, ConfigError> {
        toml::from_str(content).map_err(|err| ConfigError::ParseError {
            path: path_for_err.to_string(),
            message: err.to_string(),
        })
    }

    pub fn load_from_file(path: &Path) -> Result<Self, ConfigError> {
        let content = std::fs::read_to_string(path).map_err(|err| {
            ConfigError::IoError {
                path: path.display().to_string(),
                message: err.to_string(),
            }
        })?;
        Self::load_from_str(&content, &path.display().to_string())
    }

    pub fn validate_sensor_args(
        &self,
        sensor_name: &str,
        sensor_template_args: &[String],
    ) -> Result<(), ConfigError> {
        if let Some(sensor_override) = self.sensors.get(sensor_name) {
            if sensor_override.args.is_some() {
                let has_placeholder = sensor_template_args
                    .iter()
                    .any(|arg| arg.contains("${args}"));
                if !has_placeholder {
                    return Err(ConfigError::MissingArgsPlaceholder {
                        sensor: sensor_name.to_string(),
                    });
                }
            }
        }
        Ok(())
    }
}

impl PluginConfig {
    pub fn load_from_str(
        content: &str,
        dir: &Path,
        path_for_err: &str,
    ) -> Result<Self, ConfigError> {
        let mut cfg: PluginConfig =
            toml::from_str(content).map_err(|err| ConfigError::ParseError {
                path: path_for_err.to_string(),
                message: err.to_string(),
            })?;
        cfg.plugin_dir = dir.to_path_buf();
        Ok(cfg)
    }

    pub fn load_from_file(path: &Path) -> Result<Self, ConfigError> {
        let content = std::fs::read_to_string(path).map_err(|err| {
            ConfigError::IoError {
                path: path.display().to_string(),
                message: err.to_string(),
            }
        })?;
        let dir = path.parent().unwrap_or_else(|| Path::new("."));
        Self::load_from_str(&content, dir, &path.display().to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    mod config_loading {
        use pretty_assertions::assert_eq;

        use super::*;

        #[test]
        fn parses_full_project_config() {
            let toml = r#"
            plugins = ["rust", "generic"]
            uncoached = "enforce"

            [scope]
            changedOnly = true
            autoBranchOffMain = true
            branchBase = "develop"
            mainBranch = "production"

            [smells.too-many-parameters]
            severity = "suggested"
            disabled = false
            guide = "custom/guide.md"

            [sensors.clippy]
            disabled = false
            files = ["src/**/*.rs"]
            args = ["--all-targets"]

            [runners]
            py = "python3"
            "#;

            let config =
                ProjectConfig::load_from_str(toml, "config.toml").unwrap();
            assert_eq!(config.plugins, vec!["rust", "generic"]);
            assert_eq!(config.uncoached, UncoachedHandling::Enforce);
            assert_eq!(config.scope.changed_only, true);
            assert_eq!(config.scope.auto_branch_off_main, true);
            assert_eq!(config.scope.branch_base, "develop");
            assert_eq!(config.scope.main_branch, "production");

            let smell = config.smells.get("too-many-parameters").unwrap();
            assert_eq!(smell.severity, Some(Severity::Suggested));
            assert_eq!(smell.disabled, Some(false));
            assert_eq!(smell.guide.as_deref(), Some("custom/guide.md"));

            let sensor = config.sensors.get("clippy").unwrap();
            assert_eq!(sensor.args, Some(vec!["--all-targets".to_string()]));
            assert_eq!(config.runners.get("py").unwrap(), "python3");
        }

        #[test]
        fn rejects_args_override_when_sensor_template_lacks_args_placeholder() {
            let toml = r#"
            [sensors.clippy]
            args = ["--flag"]
            "#;
            let config =
                ProjectConfig::load_from_str(toml, "config.toml").unwrap();
            let template_args = vec!["${files}".to_string()];

            let err = config
                .validate_sensor_args("clippy", &template_args)
                .unwrap_err();
            assert_eq!(err, ConfigError::MissingArgsPlaceholder {
                sensor: "clippy".to_string()
            });
        }

        #[test]
        fn accepts_args_override_when_sensor_template_has_args_placeholder() {
            let toml = r#"
            [sensors.clippy]
            args = ["--flag"]
            "#;
            let config =
                ProjectConfig::load_from_str(toml, "config.toml").unwrap();
            let template_args =
                vec!["${files}".to_string(), "${args}".to_string()];

            let result = config.validate_sensor_args("clippy", &template_args);
            assert!(result.is_ok());
        }
    }
}
