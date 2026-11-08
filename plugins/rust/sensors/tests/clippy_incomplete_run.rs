use std::fs;
use std::path::PathBuf;
use std::process::Command;

/// A throwaway crate whose `src/lib.rs` pulls in one valid and one broken module.
fn fixture(name: &str, other_rs: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("clippy-sensor-{name}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(dir.join("src")).unwrap();
    fs::write(
        dir.join("Cargo.toml"),
        "[package]\nname = \"fixture\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[workspace]\n",
    )
    .unwrap();
    fs::write(dir.join("src/lib.rs"), "pub mod other;\npub mod target;\n").unwrap();
    fs::write(dir.join("src/target.rs"), "pub fn ok() -> u32 {\n    1\n}\n").unwrap();
    fs::write(dir.join("src/other.rs"), other_rs).unwrap();
    dir
}

fn run_sensor(dir: &PathBuf) -> serde_json::Value {
    let output = Command::new(env!("CARGO_BIN_EXE_clippy_sensor"))
        .current_dir(dir)
        .args(["--target-dir", "target/sensor", "--files", "src/target.rs"])
        .output()
        .unwrap();
    serde_json::from_slice(&output.stdout).unwrap()
}

#[test]
fn broken_foreign_file_yields_incomplete_run_instead_of_clean() {
    let dir = fixture("broken", "pub fn broken( {\n");
    let findings = run_sensor(&dir);
    let _ = fs::remove_dir_all(&dir);

    let findings = findings.as_array().unwrap();
    assert_eq!(findings.len(), 1, "{findings:?}");
    assert_eq!(findings[0]["code"], "incomplete-run");
    assert_eq!(findings[0]["filename"], "Cargo.toml");
}

#[test]
fn compiling_crate_stays_clean() {
    let dir = fixture("clean", "pub fn fine() -> u32 {\n    2\n}\n");
    let findings = run_sensor(&dir);
    let _ = fs::remove_dir_all(&dir);

    assert_eq!(findings, serde_json::json!([]));
}
