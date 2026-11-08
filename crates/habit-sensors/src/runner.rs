use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
    process::{Command, Stdio},
};

use habit_core::{
    anchor_findings, Finding, Issue, IssueDetails, PluginConfig, ProjectConfig,
    SensorConfig,
};
use rayon::prelude::*;
use tempfile::tempdir;

use crate::{
    detector::{DetectorError, DetectorResolver},
    scope::{filter_files_by_globs, ScopeArgs},
    transform::evaluate_jq,
};

const MAX_ARGV_CHARS: usize = 64 * 1024; // 64 KiB safe argv budget

pub fn make_incomplete_run(sensor_name: &str, message: &str) -> Finding {
    Finding {
        smell: "incomplete-run".to_string(),
        language: None,
        details: serde_json::json!({}),
        issues: vec![Issue {
            key: format!("sensor:{sensor_name}"),
            details: IssueDetails {
                file: ".habit-hooks/config.toml".to_string(),
                line: None,
                column: None,
                content: None,
                message: Some(message.to_string()),
                source: Some(format!("habit-sensors:{sensor_name}")),
                extra: BTreeMap::new(),
            },
        }],
    }
}
struct SensorJob<'a> {
    plugin: &'a PluginConfig,
    sensor: &'a SensorConfig,
    tool_path: PathBuf,
    files: Vec<String>,
}
type ExpandedArgsResult =
    Result<(Vec<String>, Option<tempfile::TempDir>), String>;

pub struct SensorRunner<'a> {
    pub project_dir: &'a Path,
    pub project_config: &'a ProjectConfig,
    pub plugins: &'a [PluginConfig],
    pub scope_args: &'a ScopeArgs,
}

impl<'a> SensorRunner<'a> {
    pub fn new(
        project_dir: &'a Path,
        project_config: &'a ProjectConfig,
        plugins: &'a [PluginConfig],
        scope_args: &'a ScopeArgs,
    ) -> Self {
        Self {
            project_dir,
            project_config,
            plugins,
            scope_args,
        }
    }

    pub fn run_all(&self) -> Vec<Finding> {
        let mut all_findings = Vec::new();
        let mut jobs = Vec::new();

        for plugin in self.plugins {
            let resolver = DetectorResolver::new(
                self.project_dir,
                &plugin.plugin_dir,
                &plugin.detectors,
            );

            // Discover files in scope for this plugin
            let plugin_files = match crate::scope::discover_files(
                self.project_dir,
                self.scope_args,
                &self.project_config.scope,
                &plugin.files,
            ) {
                Ok(f) => f,
                Err(err) => {
                    all_findings.push(make_incomplete_run(
                        "scope",
                        &format!("file discovery failed: {err}"),
                    ));
                    continue;
                }
            };

            // If no files match the plugin's file globs in the discovered
            // scope, skip all sensors for this plugin
            if plugin_files.is_empty() {
                continue;
            }

            for sensor in &plugin.sensors {
                // Check if sensor is disabled in project config
                let sensor_override =
                    self.project_config.sensors.get(&sensor.name);
                if sensor_override.is_some_and(|s| s.disabled.unwrap_or(false))
                {
                    continue;
                }

                // Check args placeholder requirement: if args override set,
                // sensor template must have ${args}
                if let Err(err) = self
                    .project_config
                    .validate_sensor_args(&sensor.name, &sensor.args)
                {
                    all_findings.push(make_incomplete_run(
                        &sensor.name,
                        &err.to_string(),
                    ));
                    continue;
                }

                // Compute in-scope files for this sensor
                let sensor_files = match sensor_override
                    .and_then(|s| s.files.as_ref())
                {
                    Some(globs) => filter_files_by_globs(&plugin_files, globs),
                    None => plugin_files.clone(),
                };

                // Check if sensor expects files and has none
                let template_has_files =
                    sensor.args.iter().any(|arg| arg.contains("${files"));
                if template_has_files && sensor_files.is_empty() {
                    // Nothing to scan for this sensor
                    continue;
                }

                // Expand ${dir} in tool name if present
                let expanded_tool = sensor.tool.replace(
                    "${dir}",
                    &plugin.plugin_dir.display().to_string(),
                );

                // Resolve tool
                let resolved_tool = match resolver.resolve_tool(&expanded_tool)
                {
                    Ok(p) => p,
                    Err(DetectorError::MissingTool {
                        name,
                        install,
                    }) => {
                        let msg = format!(
                            "habit-sensors: required tool '{name}' is not installed.\nTo install it, run: {install}"
                        );
                        all_findings
                            .push(make_incomplete_run(&sensor.name, &msg));
                        continue;
                    }
                    Err(err) => {
                        all_findings.push(make_incomplete_run(
                            &sensor.name,
                            &err.to_string(),
                        ));
                        continue;
                    }
                };

                jobs.push(SensorJob {
                    plugin,
                    sensor,
                    tool_path: resolved_tool,
                    files: sensor_files,
                });
            }
        }

        let job_findings: Vec<Vec<Finding>> = jobs
            .into_par_iter()
            .map(|job| {
                let mut sensor_findings = self.run_sensor_chunks(
                    job.plugin,
                    job.sensor,
                    &job.tool_path,
                    &job.files,
                );
                for finding in &mut sensor_findings {
                    if finding.language.is_none() {
                        finding.language = job.plugin.language.clone();
                    }
                }
                sensor_findings
            })
            .collect();

        for batch in job_findings {
            all_findings.extend(batch);
        }

        all_findings.sort_by(|a, b| {
            let a_file =
                a.issues.first().map(|i| i.details.file.as_str()).unwrap_or("");
            let b_file =
                b.issues.first().map(|i| i.details.file.as_str()).unwrap_or("");
            let a_line =
                a.issues.first().and_then(|i| i.details.line).unwrap_or(0);
            let b_line =
                b.issues.first().and_then(|i| i.details.line).unwrap_or(0);
            let a_col =
                a.issues.first().and_then(|i| i.details.column).unwrap_or(0);
            let b_col =
                b.issues.first().and_then(|i| i.details.column).unwrap_or(0);
            let a_key = a.issues.first().map(|i| i.key.as_str()).unwrap_or("");
            let b_key = b.issues.first().map(|i| i.key.as_str()).unwrap_or("");
            (a.smell.as_str(), a_file, a_line, a_col, a_key)
                .cmp(&(b.smell.as_str(), b_file, b_line, b_col, b_key))
                .then_with(|| {
                    serde_json::to_string(a)
                        .unwrap_or_default()
                        .cmp(&serde_json::to_string(b).unwrap_or_default())
                })
        });

        all_findings
    }

    fn run_sensor_chunks(
        &self,
        plugin: &PluginConfig,
        sensor: &SensorConfig,
        tool_path: &Path,
        files: &[String],
    ) -> Vec<Finding> {
        let template_has_files =
            sensor.args.iter().any(|arg| arg.contains("${files"));
        if !template_has_files || files.is_empty() {
            return self.run_sensor_batch(plugin, sensor, tool_path, &[]);
        }

        // Chunk files so command line doesn't exceed MAX_ARGV_CHARS
        let mut chunks: Vec<Vec<String>> = Vec::new();
        let mut current_chunk: Vec<String> = Vec::new();
        let mut current_len: usize = 0;

        for file in files {
            let file_len = file.len() + 1;
            if !current_chunk.is_empty()
                && current_len + file_len > MAX_ARGV_CHARS
            {
                chunks.push(std::mem::take(&mut current_chunk));
                current_len = 0;
            }
            current_len += file_len;
            current_chunk.push(file.clone());
        }
        if !current_chunk.is_empty() {
            chunks.push(current_chunk);
        }

        let mut combined_findings = Vec::new();
        for chunk in &chunks {
            let batch_findings =
                self.run_sensor_batch(plugin, sensor, tool_path, chunk);
            combined_findings.extend(batch_findings);
        }

        combined_findings
    }

    fn run_sensor_batch(
        &self,
        plugin: &PluginConfig,
        sensor: &SensorConfig,
        tool_path: &Path,
        files: &[String],
    ) -> Vec<Finding> {
        let (expanded_args, temp_dir_holder) =
            match self.expand_args(plugin, sensor, files) {
                Ok(res) => res,
                Err(err) => {
                    return vec![make_incomplete_run(&sensor.name, &err)];
                }
            };

        let mut cmd = Command::new(tool_path);
        cmd.current_dir(self.project_dir);
        cmd.args(&expanded_args);
        cmd.stdout(Stdio::piped());
        cmd.stderr(Stdio::piped());

        let output = match cmd.output() {
            Ok(o) => o,
            Err(err) => {
                return vec![make_incomplete_run(
                    &sensor.name,
                    &format!(
                        "failed to spawn '{}': {err}",
                        tool_path.display()
                    ),
                )];
            }
        };

        let exit_code = output.status.code().unwrap_or(-1);
        if !sensor.success_exit_codes.contains(&exit_code) {
            let stderr_snippet = String::from_utf8_lossy(&output.stderr);
            let stdout_snippet = String::from_utf8_lossy(&output.stdout);
            let msg = format!(
                "sensor exited with code {exit_code} (expected one of {:?}).\nstdout: {}\nstderr: {}",
                sensor.success_exit_codes,
                stdout_snippet.chars().take(200).collect::<String>(),
                stderr_snippet.chars().take(200).collect::<String>(),
            );
            return vec![make_incomplete_run(&sensor.name, &msg)];
        }

        let raw_output = if let Some(ref report_holder) = temp_dir_holder {
            let report_dir = report_holder.path();
            let jscpd_json = report_dir.join("jscpd-report.json");
            let read_target = if jscpd_json.is_file() {
                jscpd_json
            } else {
                report_dir.to_path_buf()
            };

            match fs::read_to_string(&read_target) {
                Ok(content) => content,
                Err(err) => {
                    return vec![make_incomplete_run(
                        &sensor.name,
                        &format!(
                            "failed reading report file '{}': {err}",
                            read_target.display()
                        ),
                    )];
                }
            }
        } else {
            String::from_utf8_lossy(&output.stdout).to_string()
        };

        // Evaluate transform or parse JSON directly
        let mut findings = if let Some(ref transform_file) = sensor.transform {
            let transform_path = plugin.plugin_dir.join(transform_file);
            match evaluate_jq(&transform_path, &raw_output) {
                Ok(f) => f,
                Err(err) => {
                    return vec![make_incomplete_run(
                        &sensor.name,
                        &format!("transform failed: {err}"),
                    )];
                }
            }
        } else {
            let trimmed = raw_output.trim();
            if trimmed.is_empty() {
                Vec::new()
            } else {
                match serde_json::from_str::<Vec<Finding>>(trimmed) {
                    Ok(f) => f,
                    Err(err) => {
                        return vec![make_incomplete_run(
                            &sensor.name,
                            &format!("invalid finding JSON: {err}"),
                        )];
                    }
                }
            }
        };

        // Anchor paths
        if let Err(anchor_err) =
            anchor_findings(&mut findings, self.project_dir, &sensor.name)
        {
            return vec![make_incomplete_run(
                &sensor.name,
                &anchor_err.to_string(),
            )];
        }

        findings
    }

    fn expand_args(
        &self,
        plugin: &PluginConfig,
        sensor: &SensorConfig,
        files: &[String],
    ) -> ExpandedArgsResult {
        let mut result_args = Vec::new();
        let mut temp_dir_holder: Option<tempfile::TempDir> = None;
        let sensor_override = self.project_config.sensors.get(&sensor.name);
        let override_args = sensor_override.and_then(|s| s.args.as_ref());

        let comma_files = files.join(",");

        for template_arg in &sensor.args {
            if template_arg == "${args}" {
                if let Some(o_args) = override_args {
                    result_args.extend(o_args.iter().cloned());
                }
            } else if template_arg == "${files}" {
                result_args.extend(files.iter().cloned());
            } else {
                let mut expanded = template_arg.clone();

                if expanded.contains("${files:comma}") {
                    expanded = expanded.replace("${files:comma}", &comma_files);
                }

                if expanded.contains("${dir}") {
                    expanded = expanded.replace(
                        "${dir}",
                        &plugin.plugin_dir.display().to_string(),
                    );
                }

                if expanded.contains("${config}") {
                    let config_path = self
                        .project_dir
                        .join(".habit-hooks")
                        .join("config.toml");
                    expanded = expanded.replace(
                        "${config}",
                        &config_path.display().to_string(),
                    );
                }

                if expanded.contains("${report}") {
                    let temp_dir = tempdir().map_err(|e| e.to_string())?;
                    let path = temp_dir.path().to_path_buf();
                    expanded = expanded
                        .replace("${report}", &path.display().to_string());
                    temp_dir_holder = Some(temp_dir);
                }

                expanded =
                    self.expand_detector_placeholders(expanded, plugin)?;

                result_args.push(expanded);
            }
        }

        Ok((result_args, temp_dir_holder))
    }

    fn expand_detector_placeholders(
        &self,
        mut text: String,
        plugin: &PluginConfig,
    ) -> Result<String, String> {
        while let Some(start) = text.find("${detector:") {
            let Some(end) = text[start..].find('}') else {
                break;
            };
            let detector_name = &text[start + 11..start + end];
            let resolver = DetectorResolver::new(
                self.project_dir,
                &plugin.plugin_dir,
                &plugin.detectors,
            );
            let resolved = resolver
                .resolve_tool(detector_name)
                .map_err(|e| e.to_string())?;
            text.replace_range(
                start..=start + end,
                &resolved.display().to_string(),
            );
        }
        Ok(text)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    mod runner_execution {
        use pretty_assertions::assert_eq;
        use tempfile::tempdir;

        use super::*;

        #[test]
        fn creates_incomplete_run_on_sensor_failure() {
            let incomplete = make_incomplete_run("clippy", "compilation error");
            assert_eq!(incomplete.smell, "incomplete-run");
            assert_eq!(incomplete.issues[0].key, "sensor:clippy");
            assert_eq!(
                incomplete.issues[0].details.message,
                Some("compilation error".to_string())
            );
        }

        #[test]
        fn expands_placeholders_accurately() {
            let dir = tempdir().unwrap();
            let project_cfg = ProjectConfig::default();
            let plugin_cfg = PluginConfig {
                language: Some("rust".to_string()),
                plugin_dir: dir.path().to_path_buf(),
                files: vec!["**/*.rs".to_string()],
                detectors: vec![],
                sensors: vec![SensorConfig {
                    name: "test-sensor".to_string(),
                    tool: "echo".to_string(),
                    args: vec![
                        "${dir}/run".to_string(),
                        "${files:comma}".to_string(),
                        "${files}".to_string(),
                    ],
                    success_exit_codes: vec![0],
                    transform: None,
                }],
            };

            let scope_args = ScopeArgs::default();
            let plugins = [plugin_cfg.clone()];
            let runner = SensorRunner::new(
                dir.path(),
                &project_cfg,
                &plugins,
                &scope_args,
            );

            let files = vec!["src/a.rs".to_string(), "src/b.rs".to_string()];
            let (args, _) = runner
                .expand_args(&plugin_cfg, &plugin_cfg.sensors[0], &files)
                .unwrap();

            assert_eq!(args, vec![
                format!("{}/run", dir.path().display()),
                "src/a.rs,src/b.rs".to_string(),
                "src/a.rs".to_string(),
                "src/b.rs".to_string(),
            ]);
        }

        #[test]
        fn short_circuits_when_no_matching_files_in_scope() {
            let dir = tempdir().unwrap();
            // Create non-rust file
            fs::write(dir.path().join("README.md"), "hello").unwrap();

            let project_cfg = ProjectConfig::default();
            let plugin_cfg = PluginConfig {
                language: Some("rust".to_string()),
                plugin_dir: dir.path().to_path_buf(),
                files: vec!["**/*.rs".to_string()],
                detectors: vec![],
                sensors: vec![SensorConfig {
                    name: "test-sensor".to_string(),
                    // tool that doesn't exist; if run, it would fail with
                    // missing tool
                    tool: "nonexistent_tool_12345".to_string(),
                    args: vec!["${files}".to_string()],
                    success_exit_codes: vec![0],
                    transform: None,
                }],
            };

            let scope_args = ScopeArgs {
                all: true,
                ..Default::default()
            };
            let plugins = [plugin_cfg];
            let runner = SensorRunner::new(
                dir.path(),
                &project_cfg,
                &plugins,
                &scope_args,
            );

            let findings = runner.run_all();
            // Because there are no .rs files, the plugin was short-circuited
            // and nonexistent_tool wasn't even resolved
            assert_eq!(findings, vec![]);
        }
    }
}
