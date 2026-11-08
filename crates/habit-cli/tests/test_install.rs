use std::fs;
use std::process::Command;
use pretty_assertions::assert_eq;
use tempfile::tempdir;

#[test]
fn test_install_pre_commit_hook() {
    let dir = tempdir().unwrap();
    let path = dir.path();

    git(path, &["init", "-b", "main"]);

    // Run habit-hooks install in that dir
    let habit_hooks_bin = env!("CARGO_BIN_EXE_habit-hooks");
    let output = Command::new(habit_hooks_bin)
        .arg("install")
        .current_dir(path)
        .output()
        .expect("failed to execute habit-hooks install");

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("✅ Installed Habit Hooks pre-commit hook in .git/hooks/pre-commit"));

    let hook_file = path.join(".git").join("hooks").join("pre-commit");
    assert!(hook_file.is_file());

    let content = fs::read_to_string(&hook_file).unwrap();
    assert!(content.contains("# Habit Hooks automated pre-commit gate"));
    assert!(content.contains("exec habit-hooks --staged"));

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let perms = fs::metadata(&hook_file).unwrap().permissions();
        assert_eq!(perms.mode() & 0o111, 0o111);
    }
}

#[test]
fn test_install_fails_outside_git_repo() {
    let dir = tempdir().unwrap();
    let path = dir.path();

    let habit_hooks_bin = env!("CARGO_BIN_EXE_habit-hooks");
    let output = Command::new(habit_hooks_bin)
        .arg("install")
        .current_dir(path)
        .output()
        .expect("failed to execute habit-hooks install");

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("Not a git repository"));
}

#[test]
fn test_pre_commit_blocks_commit_on_enforced_smell() {
    let dir = tempdir().unwrap();
    let path = dir.path();

    // Initialize git repo
    let run_cmd = |cmd: &str, args: &[&str]| {
        let out = Command::new(cmd)
            .current_dir(path)
            .args(args)
            .output()
            .expect("failed to run command");
        assert!(out.status.success(), "command failed: {} {:?}", cmd, args);
    };

    run_cmd("git", &["init", "-b", "main"]);
    run_cmd("git", &["config", "user.name", "Test Runner"]);
    run_cmd("git", &["config", "user.email", "test@runner.local"]);

    let habit_hooks_bin = env!("CARGO_BIN_EXE_habit-hooks");
    let bin_dir = std::path::Path::new(habit_hooks_bin).parent().unwrap();

    // Install pre-commit hook
    let output = Command::new(habit_hooks_bin)
        .arg("install")
        .current_dir(path)
        .output()
        .unwrap();
    assert!(output.status.success());

    // Setup mock sensor that emits an enforced smell ("lock-across-await")
    let plugin_dir = path.join(".habit-hooks").join("mock");
    fs::create_dir_all(&plugin_dir).unwrap();

    // Script that outputs JSON finding for lock-across-await
    let sensor_script = plugin_dir.join("sensor.sh");
    fs::write(
        &sensor_script,
        r#"#!/bin/sh
echo '[{"smell":"lock-across-await","language":"rust","details":{},"issues":[{"key":"src/lib.rs","details":{"file":"src/lib.rs","line":1,"message":"holding lock across await"}}]}]'
"#,
    ).unwrap();

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = fs::metadata(&sensor_script).unwrap().permissions();
        perms.set_mode(0o755);
        let _ = fs::set_permissions(&sensor_script, perms);
    }

    let plugin_toml = plugin_dir.join("config.toml");
    fs::write(
        &plugin_toml,
        r#"files = ["**/*.rs"]
[[sensors]]
name = "mock-sensor"
tool = "${dir}/sensor.sh"
args = []
success_exit_codes = [0]
"#,
    ).unwrap();

    let project_config_toml = path.join(".habit-hooks").join("config.toml");
    fs::write(
        &project_config_toml,
        r#"plugins = ["mock"]
[scope]
changedOnly = true
"#,
    ).unwrap();

    // Create and stage file
    fs::create_dir_all(path.join("src")).unwrap();
    fs::write(path.join("src/lib.rs"), "async fn foo() {}\n").unwrap();
    run_cmd("git", &["add", "src/lib.rs"]);

    // Construct PATH containing habit-hooks binary
    let current_path = std::env::var("PATH").unwrap_or_default();
    let new_path = format!("{}:{}", bin_dir.display(), current_path);

    let commit_output = Command::new("git")
        .current_dir(path)
        .args(&["commit", "-m", "attempt commit"])
        .env("PATH", new_path)
        .output()
        .expect("git commit failed to execute");

    // Commit must be blocked (exit code non-zero, specifically 1 from habit-hooks)
    assert!(
        !commit_output.status.success(),
        "git commit should have been blocked by pre-commit hook"
    );
    let combined_output = format!(
        "{}\n{}",
        String::from_utf8_lossy(&commit_output.stdout),
        String::from_utf8_lossy(&commit_output.stderr)
    );
    assert!(
        combined_output.contains("lock-across-await") || commit_output.status.code() == Some(1),
        "pre-commit hook output did not reflect enforced smell failure"
    );
}

#[test]
#[cfg(unix)]
fn test_install_with_hk_flag_creates_hk_pkl() {
    let dir = tempdir().unwrap();
    let path = dir.path();
    git(path, &["init", "-b", "main"]);

    let fake_bin = path.join("bin");
    fs::create_dir(&fake_bin).unwrap();
    fake_hk(&fake_bin, false);
    let output = Command::new(env!("CARGO_BIN_EXE_habit-hooks"))
        .args(["install", "--hk"])
        .current_dir(path)
        .env("PATH", path_with_bin(&fake_bin))
        .output()
        .expect("failed to execute habit-hooks install --hk");

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("hk.pkl"));

    let hk_pkl = path.join("hk.pkl");
    assert!(hk_pkl.is_file());
    let content = fs::read_to_string(&hk_pkl).unwrap();
    assert!(content.contains("[\"habit-hooks\"]"));
    assert!(content.contains("glob = \"**/*.rs\""));
    assert!(content.contains("check = \"habit-hooks --staged\""));

    assert!(path.join(".git/hooks/pre-commit").is_file());
    assert!(fs::read_to_string(path.join(".git/hooks/pre-commit"))
        .unwrap()
        .contains("exec habit-hooks --staged"));
}

#[test]
#[cfg(unix)]
fn test_install_auto_detects_existing_hk_pkl() {
    let dir = tempdir().unwrap();
    let path = dir.path();
    git(path, &["init", "-b", "main"]);

    // Pre-create an existing hk.pkl with dummy steps
    let existing_hk_pkl = r#"amends "package://github.com/jdx/hk/releases/download/v2.0.0/hk@2.0.0#/Config.pkl"

steps {
    ["prettier"] {
        glob = "**/*.json"
        check = "prettier --check {{files}}"
    }
}
"#;
    fs::write(path.join("hk.pkl"), existing_hk_pkl).unwrap();

    let fake_bin = path.join("bin");
    fs::create_dir(&fake_bin).unwrap();
    fake_hk(&fake_bin, false);
    let output = Command::new(env!("CARGO_BIN_EXE_habit-hooks"))
        .arg("install")
        .current_dir(path)
        .env("PATH", path_with_bin(&fake_bin))
        .output()
        .expect("failed to execute habit-hooks install");

    assert!(output.status.success());
    let hk_pkl = path.join("hk.pkl");
    let content = fs::read_to_string(&hk_pkl).unwrap();
    assert!(content.contains("[\"habit-hooks\"]"));
    assert!(content.contains("[\"prettier\"]"));
}

#[test]
#[cfg(unix)]
fn upgrades_legacy_hk_step_to_staged_command() {
    let dir = tempdir().unwrap();
    let path = dir.path();
    git(path, &["init", "-b", "main"]);
    fs::write(
        path.join("hk.pkl"),
        "amends \"package://github.com/jdx/hk/releases/download/v2.0.0/hk@2.0.0#/Config.pkl\"\n\nsteps {\n    [\"habit-hooks\"] {\n        glob = \"**/*.rs\"\n        check = \"habit-hooks --changed\"\n    }\n}\n",
    )
    .unwrap();

    let bin = path.join("bin");
    fs::create_dir(&bin).unwrap();
    fake_hk(&bin, false);
    let output = Command::new(env!("CARGO_BIN_EXE_habit-hooks"))
        .arg("install")
        .current_dir(path)
        .env("PATH", path_with_bin(&bin))
        .output()
        .unwrap();

    assert!(output.status.success());
    let content = fs::read_to_string(path.join("hk.pkl")).unwrap();
    assert!(content.contains("check = \"habit-hooks --staged\""));
    assert!(!content.contains("--changed"));
}

fn git(path: &std::path::Path, args: &[&str]) {
    let output = Command::new("git").current_dir(path).args(args).output().unwrap();
    assert!(
        output.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

fn path_with_bin(bin: &std::path::Path) -> std::ffi::OsString {
    std::env::join_paths(
        std::iter::once(bin.to_path_buf()).chain(std::env::split_paths(
            &std::env::var_os("PATH").unwrap_or_default(),
        )),
    )
    .unwrap()
}

#[cfg(unix)]
fn executable(path: &std::path::Path, script: &str) {
    use std::os::unix::fs::PermissionsExt;
    fs::write(path, script).unwrap();
    let mut permissions = fs::metadata(path).unwrap().permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(path, permissions).unwrap();
}

#[cfg(unix)]
fn fake_hk(bin: &std::path::Path, active: bool) {
    let script = if active {
        r#"#!/bin/sh
case "$1" in
    --version) exit 0 ;;
    install)
        mkdir -p .hk-hooks
        printf '#!/bin/sh\nexec hk run pre-commit --from-hook\n' > .hk-hooks/pre-commit
        chmod +x .hk-hooks/pre-commit
        git config core.hooksPath .hk-hooks
        ;;
    run)
        if git diff --cached --quiet --diff-filter=ACMR -- '*.rs'; then exit 0; fi
        exec habit-hooks --staged
        ;;
esac
"#
    } else {
        "#!/bin/sh\nexit 1\n"
    };
    executable(&bin.join("hk"), script);
}

#[cfg(unix)]
#[test]
fn skips_native_hook_when_commit_has_no_staged_rust() {
    for has_rust in [false, true] {
        let dir = tempdir().unwrap();
        let path = dir.path();
        git(path, &["init", "-b", "main"]);
        git(path, &["config", "user.name", "Test Runner"]);
        git(path, &["config", "user.email", "test@runner.local"]);
        fs::write(path.join("Cargo.toml"), "[package]\nname = \"sample\"\n").unwrap();
        if has_rust {
            fs::create_dir(path.join("src")).unwrap();
            fs::write(path.join("src/lib.rs"), "pub fn sample() {}\n").unwrap();
        }
        git(path, &["add", "."]);
        git(path, &["commit", "-m", "baseline"]);

        let bin = path.join("bin");
        fs::create_dir(&bin).unwrap();
        executable(
            &bin.join("habit-hooks"),
            "#!/bin/sh\necho \"$*\" >> hook-invocations\nexit 1\n",
        );
        let installed = Command::new(env!("CARGO_BIN_EXE_habit-hooks"))
            .arg("install")
            .current_dir(path)
            .output()
            .unwrap();
        assert!(installed.status.success());

        fs::write(path.join("Cargo.toml"), "[package]\nname = \"changed\"\n").unwrap();
        fs::write(path.join("README.txt"), "plain text\n").unwrap();
        if has_rust {
            fs::write(path.join("src/lib.rs"), "pub fn unstaged() {}\n").unwrap();
        }
        git(path, &["add", "Cargo.toml", "README.txt"]);
        let commit = Command::new("git")
            .current_dir(path)
            .args(["commit", "-m", "non-rust changes"])
            .env("PATH", path_with_bin(&bin))
            .output()
            .unwrap();
        assert!(
            commit.status.success(),
            "{}",
            String::from_utf8_lossy(&commit.stderr)
        );
        assert!(!path.join("hook-invocations").exists());
    }
}

#[cfg(unix)]
#[test]
fn invokes_native_hook_once_for_staged_rust() {
    let dir = tempdir().unwrap();
    let path = dir.path();
    git(path, &["init", "-b", "main"]);
    git(path, &["config", "user.name", "Test Runner"]);
    git(path, &["config", "user.email", "test@runner.local"]);
    let bin = path.join("bin");
    fs::create_dir(&bin).unwrap();
    executable(
        &bin.join("habit-hooks"),
        "#!/bin/sh\necho \"$*\" >> hook-invocations\n",
    );
    let installed = Command::new(env!("CARGO_BIN_EXE_habit-hooks"))
        .arg("install")
        .current_dir(path)
        .output()
        .unwrap();
    assert!(installed.status.success());

    fs::write(path.join("root.rs"), "fn main() {}\n").unwrap();
    fs::write(path.join("README.txt"), "plain text\n").unwrap();
    git(path, &["add", "root.rs", "README.txt"]);
    let commit = Command::new("git")
        .current_dir(path)
        .args(["commit", "-m", "rust changes"])
        .env("PATH", path_with_bin(&bin))
        .output()
        .unwrap();
    assert!(commit.status.success(), "{}", String::from_utf8_lossy(&commit.stderr));
    assert_eq!(fs::read_to_string(path.join("hook-invocations")).unwrap(), "--staged\n");
}

#[cfg(unix)]
#[test]
fn routes_hk_commit_once_without_native_duplicate() {
    let dir = tempdir().unwrap();
    let path = dir.path();
    git(path, &["init", "-b", "main"]);
    git(path, &["config", "user.name", "Test Runner"]);
    git(path, &["config", "user.email", "test@runner.local"]);
    let bin = path.join("bin");
    fs::create_dir(&bin).unwrap();
    fake_hk(&bin, true);
    executable(
        &bin.join("habit-hooks"),
        "#!/bin/sh\necho \"$*\" >> hook-invocations\n",
    );
    let installed = Command::new(env!("CARGO_BIN_EXE_habit-hooks"))
        .args(["install", "--hk"])
        .current_dir(path)
        .env("PATH", path_with_bin(&bin))
        .output()
        .unwrap();
    assert!(installed.status.success(), "{}", String::from_utf8_lossy(&installed.stderr));
    assert!(!path.join(".git/hooks/pre-commit").exists());

    fs::write(path.join("Cargo.toml"), "[package]\nname = \"sample\"\n").unwrap();
    git(path, &["add", "Cargo.toml"]);
    let commit = Command::new("git")
        .current_dir(path)
        .args(["commit", "-m", "cargo only"])
        .env("PATH", path_with_bin(&bin))
        .output()
        .unwrap();
    assert!(commit.status.success(), "{}", String::from_utf8_lossy(&commit.stderr));
    assert!(!path.join("hook-invocations").exists());

    fs::write(path.join("root.rs"), "fn main() {}\n").unwrap();
    git(path, &["add", "root.rs"]);
    let commit = Command::new("git")
        .current_dir(path)
        .args(["commit", "-m", "root-level rust"])
        .env("PATH", path_with_bin(&bin))
        .output()
        .unwrap();
    assert!(commit.status.success(), "{}", String::from_utf8_lossy(&commit.stderr));
    assert_eq!(fs::read_to_string(path.join("hook-invocations")).unwrap(), "--staged\n");
}

#[test]
fn rejects_install_when_foreign_hook_exists() {
    let dir = tempdir().unwrap();
    let path = dir.path();
    git(path, &["init", "-b", "main"]);
    let hook = path.join(".git/hooks/pre-commit");
    fs::write(&hook, "#!/bin/sh\necho custom\n").unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_habit-hooks"))
        .args(["install", "--hk"])
        .current_dir(path)
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert_eq!(fs::read_to_string(&hook).unwrap(), "#!/bin/sh\necho custom\n");
    assert!(!path.join("hk.pkl").exists());
}

#[cfg(unix)]
#[test]
fn skips_staged_run_when_mode_file_says_off() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempdir().unwrap();
    let path = dir.path();
    git(path, &["init", "-b", "main"]);

    // mock plugin whose sensor always reports an enforced smell, so an active run exits 1
    let plugin_dir = path.join(".habit-hooks/mock");
    fs::create_dir_all(&plugin_dir).unwrap();
    let sensor = plugin_dir.join("sensor.sh");
    fs::write(
        &sensor,
        "#!/bin/sh\necho '[{\"smell\":\"lock-across-await\",\"language\":\"rust\",\"details\":{},\"issues\":[{\"key\":\"lib.rs\",\"details\":{\"file\":\"lib.rs\",\"line\":1,\"message\":\"held\"}}]}]'\n",
    )
    .unwrap();
    fs::set_permissions(&sensor, fs::Permissions::from_mode(0o755)).unwrap();
    fs::write(
        plugin_dir.join("config.toml"),
        "files = [\"**/*.rs\"]\n[[sensors]]\nname = \"mock-sensor\"\ntool = \"${dir}/sensor.sh\"\nargs = []\nsuccess_exit_codes = [0]\n",
    )
    .unwrap();
    fs::write(path.join(".habit-hooks/config.toml"), "plugins = [\"mock\"]\n").unwrap();
    fs::write(path.join("lib.rs"), "async fn foo() {}\n").unwrap();
    git(path, &["add", "lib.rs"]);
    let mode_file = path.join("mode");

    let run = |mode: &str| {
        fs::write(&mode_file, format!("{mode}\n")).unwrap();
        Command::new(env!("CARGO_BIN_EXE_habit-hooks"))
            .arg("--staged")
            .current_dir(path)
            .env("HABIT_HOOKS_MODE_FILE", &mode_file)
            .env_remove("HABIT_HOOKS_MODE")
            .output()
            .unwrap()
    };

    // same staged input both times, so the skip can only come from the mode
    assert_eq!(run("turn").status.code(), Some(1));
    let off = run("off");
    assert_eq!(off.status.code(), Some(0));
    assert!(off.stdout.is_empty());
}
