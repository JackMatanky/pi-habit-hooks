use std::path::{Path, PathBuf};

use habit_core::DetectorConfig;

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum DetectorError {
    #[error("habit-sensors: tool '{name}' is not declared in any plugin's detectors")]
    UndeclaredTool {
        name: String,
    },

    #[error(
        "habit-sensors: required tool '{name}' is not installed.\nTo install it, run: {install}"
    )]
    MissingTool {
        name: String,
        install: String,
    },

    #[error("habit-sensors: required tool '{name}' is not installed and no install instruction was provided")]
    MissingToolNoInstall {
        name: String,
    },
}

pub struct DetectorResolver<'a> {
    pub project_dir: &'a Path,
    pub plugin_dir: &'a Path,
    pub detectors: &'a [DetectorConfig],
}

impl<'a> DetectorResolver<'a> {
    pub fn new(
        project_dir: &'a Path,
        plugin_dir: &'a Path,
        detectors: &'a [DetectorConfig],
    ) -> Self {
        Self {
            project_dir,
            plugin_dir,
            detectors,
        }
    }

    pub fn is_executable(path: &Path) -> bool {
        if !path.is_file() {
            return false;
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            if let Ok(meta) = path.metadata() {
                return (meta.permissions().mode() & 0o111) != 0;
            }
            false
        }
        #[cfg(not(unix))]
        {
            true
        }
    }

    fn check_candidate(&self, candidate: PathBuf) -> Option<PathBuf> {
        if candidate.is_file() {
            return Some(candidate);
        }
        None
    }

    pub fn resolve_tool(&self, tool: &str) -> Result<PathBuf, DetectorError> {
        // If tool is an explicit path (contains / or \)
        if tool.contains('/') || tool.contains('\\') {
            let path = PathBuf::from(tool);
            let candidate = if path.is_absolute() {
                path
            } else {
                self.plugin_dir.join(&path)
            };

            if let Some(found) = self.check_candidate(candidate) {
                return Ok(found);
            }

            // Also check relative to project_dir
            let project_candidate = self.project_dir.join(tool);
            if let Some(found) = self.check_candidate(project_candidate) {
                return Ok(found);
            }

            // If it's a binary name in target/debug or target/release
            let file_name = Path::new(tool).file_name().unwrap_or_default();
            for base in &[
                self.project_dir.join("target").join("release"),
                self.project_dir.join("target").join("debug"),
            ] {
                let target_candidate = base.join(file_name);
                if let Some(found) = self.check_candidate(target_candidate) {
                    return Ok(found);
                }
            }

            // Not found
            return Err(DetectorError::MissingToolNoInstall {
                name: tool.to_string(),
            });
        }

        // Find declared detector config
        let detector =
            self.detectors.iter().find(|d| d.name == tool).ok_or_else(
                || DetectorError::UndeclaredTool {
                    name: tool.to_string(),
                },
            )?;

        // 1. Check custom search_paths from detector config
        for sp in &detector.search_paths {
            let expanded = sp
                .replace("${dir}", &self.plugin_dir.display().to_string())
                .replace(
                    "${package_root}",
                    &self.project_dir.display().to_string(),
                );
            let base = PathBuf::from(expanded);
            let candidate = if base.is_absolute() {
                base.join(tool)
            } else {
                self.project_dir.join(base).join(tool)
            };
            if let Some(found) = self.check_candidate(candidate) {
                return Ok(found);
            }
        }

        // 2. Check local package locations
        let local_search_dirs = [
            self.project_dir.join("node_modules").join(".bin"),
            self.plugin_dir.join("node_modules").join(".bin"),
            self.project_dir.join(".venv").join("bin"),
            self.plugin_dir.join("bin"),
            self.project_dir.join("target").join("release"),
            self.project_dir.join("target").join("debug"),
        ];

        for dir in &local_search_dirs {
            let candidate = dir.join(tool);
            if let Some(found) = self.check_candidate(candidate) {
                return Ok(found);
            }
        }

        // 3. Check system PATH
        if let Some(found) = self.lookup_in_path(tool) {
            return Ok(found);
        }

        // Tool is declared but missing
        if let Some(install) = &detector.install {
            Err(DetectorError::MissingTool {
                name: tool.to_string(),
                install: install.clone(),
            })
        } else {
            Err(DetectorError::MissingToolNoInstall {
                name: tool.to_string(),
            })
        }
    }

    fn lookup_in_path(&self, tool: &str) -> Option<PathBuf> {
        let path_var = std::env::var_os("PATH")?;
        for dir in std::env::split_paths(&path_var) {
            let candidate = dir.join(tool);
            if candidate.is_file() {
                return Some(candidate);
            }
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use tempfile::tempdir;

    use super::*;

    mod resolution {
        use pretty_assertions::assert_eq;

        use super::*;

        #[test]
        fn rejects_undeclared_tool() {
            let dir = tempdir().unwrap();
            let detectors = vec![];
            let resolver =
                DetectorResolver::new(dir.path(), dir.path(), &detectors);

            let err = resolver.resolve_tool("unknown-tool").unwrap_err();
            assert_eq!(err, DetectorError::UndeclaredTool {
                name: "unknown-tool".to_string()
            });
        }

        #[test]
        fn reports_missing_tool_with_install_instruction() {
            let dir = tempdir().unwrap();
            let detectors = vec![DetectorConfig {
                name: "nonexistent".to_string(),
                kind: "command".to_string(),
                install: Some("npm install -g nonexistent".to_string()),
                search_paths: vec![],
            }];
            let resolver =
                DetectorResolver::new(dir.path(), dir.path(), &detectors);

            let err = resolver.resolve_tool("nonexistent").unwrap_err();
            assert_eq!(err, DetectorError::MissingTool {
                name: "nonexistent".to_string(),
                install: "npm install -g nonexistent".to_string()
            });
        }

        #[test]
        fn resolves_tool_in_local_node_modules() {
            let dir = tempdir().unwrap();
            let bin_dir = dir.path().join("node_modules").join(".bin");
            fs::create_dir_all(&bin_dir).unwrap();
            let tool_path = bin_dir.join("my-linter");
            fs::write(&tool_path, "#!/bin/sh\nexit 0").unwrap();

            let detectors = vec![DetectorConfig {
                name: "my-linter".to_string(),
                kind: "command".to_string(),
                install: None,
                search_paths: vec![],
            }];
            let resolver =
                DetectorResolver::new(dir.path(), dir.path(), &detectors);

            let resolved = resolver.resolve_tool("my-linter").unwrap();
            assert_eq!(resolved, tool_path);
        }
    }
}
