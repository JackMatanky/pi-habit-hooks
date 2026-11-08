use std::{fs, path::PathBuf};

use habit_core::{Finding, PluginConfig, ProjectConfig};
use habit_sensors::{runner::SensorRunner, scope::ScopeArgs};
use pretty_assertions::assert_eq;

fn get_workspace_root() -> PathBuf {
    let mut current = std::env::current_dir().unwrap();
    loop {
        if current.join("Cargo.toml").is_file()
            && current.join("plugins").is_dir()
        {
            return current;
        }
        if let Some(parent) = current.parent() {
            current = parent.to_path_buf();
        } else {
            break;
        }
    }
    std::env::current_dir().unwrap()
}

#[test]
fn verifies_clippy_scenario_against_approved_json() {
    let workspace = get_workspace_root();
    let scenario_dir =
        workspace.join("plugins").join("rust").join("scenarios").join("clippy");

    let approved_path = scenario_dir.join("approved.json");
    let approved_str = fs::read_to_string(&approved_path).unwrap();
    let expected: Vec<Finding> = serde_json::from_str(&approved_str).unwrap();

    let plugin_config_path =
        workspace.join("plugins").join("rust").join("config.toml");
    let mut plugin_cfg =
        PluginConfig::load_from_file(&plugin_config_path).unwrap();

    // Limit sensors to just clippy
    plugin_cfg.sensors.retain(|s| s.name == "clippy");

    let sample_file =
        "plugins/rust/scenarios/clippy/sample/src/lib.rs".to_string();
    let scope_args = ScopeArgs {
        files: vec![sample_file],
        ..Default::default()
    };

    let project_cfg = ProjectConfig::default();
    let plugins = [plugin_cfg];
    let runner =
        SensorRunner::new(&workspace, &project_cfg, &plugins, &scope_args);
    let actual = runner.run_all();

    assert_eq!(actual, expected);
}

#[test]
fn verifies_messrust_scenario_against_approved_json() {
    let workspace = get_workspace_root();
    let scenario_dir = workspace
        .join("plugins")
        .join("rust")
        .join("scenarios")
        .join("messrust");

    let approved_path = scenario_dir.join("approved.json");
    let approved_str = fs::read_to_string(&approved_path).unwrap();
    let expected: Vec<Finding> = serde_json::from_str(&approved_str).unwrap();

    let plugin_config_path =
        workspace.join("plugins").join("rust").join("config.toml");
    let mut plugin_cfg =
        PluginConfig::load_from_file(&plugin_config_path).unwrap();

    // Limit sensors to just messrust
    plugin_cfg.sensors.retain(|s| s.name == "messrust");

    let sample_file =
        "plugins/rust/scenarios/messrust/sample/complex.rs".to_string();
    let scope_args = ScopeArgs {
        files: vec![sample_file],
        ..Default::default()
    };

    let project_cfg = ProjectConfig::default();
    let plugins = [plugin_cfg];
    let runner =
        SensorRunner::new(&workspace, &project_cfg, &plugins, &scope_args);
    let actual = runner.run_all();

    assert_eq!(actual, expected);
}

#[test]
fn verifies_line_count_scenario_against_approved_json() {
    let workspace = get_workspace_root();
    let scenario_dir = workspace
        .join("plugins")
        .join("generic")
        .join("scenarios")
        .join("line-count");

    let approved_path = scenario_dir.join("approved.json");
    let approved_str = fs::read_to_string(&approved_path).unwrap();
    let expected: Vec<Finding> = serde_json::from_str(&approved_str).unwrap();

    let plugin_config_path =
        workspace.join("plugins").join("generic").join("config.toml");
    let mut plugin_cfg =
        PluginConfig::load_from_file(&plugin_config_path).unwrap();

    // Limit sensors to just line-count
    plugin_cfg.sensors.retain(|s| s.name == "line-count");

    let sample_file =
        "plugins/generic/scenarios/line-count/sample/big.txt".to_string();
    let scope_args = ScopeArgs {
        files: vec![sample_file],
        ..Default::default()
    };

    let project_cfg = ProjectConfig::default();
    let plugins = [plugin_cfg];
    let runner =
        SensorRunner::new(&workspace, &project_cfg, &plugins, &scope_args);
    let actual = runner.run_all();

    assert_eq!(actual, expected);
}

#[test]
fn verifies_jscpd_scenario_against_approved_json() {
    let workspace = get_workspace_root();
    let scenario_dir = workspace
        .join("plugins")
        .join("generic")
        .join("scenarios")
        .join("jscpd");

    let approved_path = scenario_dir.join("approved.json");
    let approved_str = fs::read_to_string(&approved_path).unwrap();
    let expected: Vec<Finding> = serde_json::from_str(&approved_str).unwrap();

    let plugin_config_path =
        workspace.join("plugins").join("generic").join("config.toml");
    let mut plugin_cfg =
        PluginConfig::load_from_file(&plugin_config_path).unwrap();

    // Limit sensors to just jscpd
    plugin_cfg.sensors.retain(|s| s.name == "jscpd");

    let sample_file1 =
        "plugins/generic/scenarios/jscpd/sample/first.js".to_string();
    let sample_file2 =
        "plugins/generic/scenarios/jscpd/sample/second.js".to_string();
    let scope_args = ScopeArgs {
        files: vec![sample_file1, sample_file2],
        ..Default::default()
    };

    let project_cfg = ProjectConfig::default();
    let plugins = [plugin_cfg];
    let runner =
        SensorRunner::new(&workspace, &project_cfg, &plugins, &scope_args);
    let actual = runner.run_all();

    assert_eq!(actual, expected);
}
