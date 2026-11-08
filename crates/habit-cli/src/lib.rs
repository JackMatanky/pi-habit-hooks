use std::{
    env,
    path::{Path, PathBuf},
};

use habit_core::{PluginConfig, ProjectConfig};

pub fn find_project_dir() -> PathBuf {
    if let Ok(dir) = env::var("HABIT_HOOKS_PROJECT_DIR") {
        return PathBuf::from(dir);
    }

    let mut current = env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    loop {
        if current.join(".habit-hooks").is_dir()
            || current.join(".git").exists()
            || current.join("Cargo.toml").is_file()
            || current.join("package.json").is_file()
        {
            return current;
        }
        if let Some(parent) = current.parent() {
            current = parent.to_path_buf();
        } else {
            break;
        }
    }

    env::current_dir().unwrap_or_else(|_| PathBuf::from("."))
}

pub fn load_project_config(project_dir: &Path) -> ProjectConfig {
    let config_path = project_dir.join(".habit-hooks").join("config.toml");
    if config_path.is_file() {
        if let Ok(cfg) = ProjectConfig::load_from_file(&config_path) {
            return cfg;
        }
    }

    // Default configuration: detect language
    let mut plugins = Vec::new();
    if project_dir.join("Cargo.toml").is_file() {
        plugins.push("rust".to_string());
    }
    plugins.push("generic".to_string());

    ProjectConfig {
        plugins,
        ..Default::default()
    }
}

pub fn find_plugin_dir(
    project_dir: &Path,
    plugin_name: &str,
) -> Option<PathBuf> {
    // 1. Project local plugin override: .habit-hooks/<plugin>
    let local = project_dir.join(".habit-hooks").join(plugin_name);
    if local.join("config.toml").is_file() {
        return Some(local);
    }

    // 2. HABIT_HOOKS_PLUGINS_DIR env var
    if let Ok(env_dir) = env::var("HABIT_HOOKS_PLUGINS_DIR") {
        let p = PathBuf::from(env_dir).join(plugin_name);
        if p.join("config.toml").is_file() {
            return Some(p);
        }
    }

    // 3. Relative to current working dir
    let cwd_plugin = PathBuf::from("plugins").join(plugin_name);
    if cwd_plugin.join("config.toml").is_file() {
        return Some(cwd_plugin);
    }

    // 4. Relative to executable path
    if let Ok(exe_path) = env::current_exe() {
        if let Some(exe_dir) = exe_path.parent() {
            for ancestor in &[
                exe_dir,
                exe_dir.parent().unwrap_or(exe_dir),
                exe_dir.parent().and_then(|p| p.parent()).unwrap_or(exe_dir),
            ] {
                let candidate = ancestor.join("plugins").join(plugin_name);
                if candidate.join("config.toml").is_file() {
                    return Some(candidate);
                }
            }
        }
    }

    None
}

pub fn load_enabled_plugins(
    project_dir: &Path,
    config: &ProjectConfig,
) -> Vec<PluginConfig> {
    let mut plugins = Vec::new();

    let is_rust_project = project_dir.join("Cargo.toml").is_file();

    for plugin_name in &config.plugins {
        // Auto-disable rust plugin if project is not a Rust project (no
        // Cargo.toml)
        if plugin_name == "rust" && !is_rust_project {
            continue;
        }

        if let Some(p_dir) = find_plugin_dir(project_dir, plugin_name) {
            let config_file = p_dir.join("config.toml");
            if let Ok(p_cfg) = PluginConfig::load_from_file(&config_file) {
                plugins.push(p_cfg);
            }
        }
    }

    plugins
}

#[cfg(test)]
mod tests {
    use tempfile::tempdir;

    use super::*;

    #[test]
    fn auto_disables_rust_plugin_when_cargo_toml_is_absent() {
        let dir = tempdir().unwrap();
        let config = ProjectConfig {
            plugins: vec!["rust".to_string()],
            ..ProjectConfig::default()
        };

        let plugins = load_enabled_plugins(dir.path(), &config);
        assert!(
            plugins.is_empty(),
            "rust plugin should be auto-disabled when Cargo.toml is absent"
        );
    }

    #[test]
    fn keeps_rust_plugin_when_cargo_toml_is_present() {
        let dir = tempdir().unwrap();
        std::fs::write(
            dir.path().join("Cargo.toml"),
            "[package]\nname = \"test\"\n",
        )
        .unwrap();
        // Create local .habit-hooks/rust/config.toml so find_plugin_dir
        // succeeds in isolated tempdir
        let rust_plugin_dir = dir.path().join(".habit-hooks").join("rust");
        std::fs::create_dir_all(&rust_plugin_dir).unwrap();
        std::fs::write(
            rust_plugin_dir.join("config.toml"),
            "language = \"rust\"\nfiles = [\"**/*.rs\"]\nsensors = []\n",
        )
        .unwrap();

        let config = ProjectConfig {
            plugins: vec!["rust".to_string()],
            ..ProjectConfig::default()
        };

        let plugins = load_enabled_plugins(dir.path(), &config);
        assert_eq!(
            plugins.len(),
            1,
            "rust plugin should be enabled when Cargo.toml is present"
        );
    }
}
