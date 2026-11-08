use std::path::PathBuf;

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
fn test_parallel_sensor_execution_determinism() {
    let workspace = get_workspace_root();
    let sample_file =
        "plugins/rust/scenarios/clippy/sample/src/lib.rs".to_string();

    let plugin_config_path =
        workspace.join("plugins").join("rust").join("config.toml");
    let plugin_cfg = PluginConfig::load_from_file(&plugin_config_path).unwrap();

    let scope_args = ScopeArgs {
        files: vec![sample_file],
        ..Default::default()
    };

    let project_cfg = ProjectConfig::default();
    let plugins = [plugin_cfg];

    // Run in a thread pool with 4 threads
    let pool_multi =
        rayon::ThreadPoolBuilder::new().num_threads(4).build().unwrap();

    let findings_multi: Vec<Finding> = pool_multi.install(|| {
        let runner =
            SensorRunner::new(&workspace, &project_cfg, &plugins, &scope_args);
        runner.run_all()
    });

    // Run in a single-threaded pool
    let pool_single =
        rayon::ThreadPoolBuilder::new().num_threads(1).build().unwrap();

    let findings_single: Vec<Finding> = pool_single.install(|| {
        let runner =
            SensorRunner::new(&workspace, &project_cfg, &plugins, &scope_args);
        runner.run_all()
    });

    // Both multi-threaded and single-threaded runs must produce identical
    // findings
    assert_eq!(findings_multi, findings_single);
    assert!(!findings_multi.is_empty(), "Expected findings from sample file");
}
