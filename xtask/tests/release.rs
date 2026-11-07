//! Release planning contracts exercised through the repository task CLI.

use serde_json::json;
use std::fs;
use std::path::Path;
use std::process::{Command, Output};

fn manifest(root: &Path, version: &str, policy: &str) {
    fs::write(
        root.join("Cargo.toml"),
        format!(
            "[package]\nname = 'release-fixture'\nversion = '{version}'\nedition = '2024'\n{policy}\n[workspace.metadata.release.runners]\naarch64-apple-darwin = 'macos-15'\nx86_64-unknown-linux-gnu = 'ubuntu-22.04'\n"
        ),
    )
    .unwrap();
}

fn plan(root: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_xtask"))
        .current_dir(root)
        .arg("plan")
        .args(args)
        .output()
        .unwrap()
}

#[test]
fn plans_use_configured_targets_and_gate_publication_on_exact_tags_and_package_policy() {
    let root = tempfile::tempdir().unwrap();
    fs::create_dir(root.path().join("src")).unwrap();
    fs::write(root.path().join("src/main.rs"), "fn main() {}\n").unwrap();
    fs::write(
        root.path().join("dist-workspace.toml"),
        "[dist]\ncargo-dist-version = '0.32.2'\ntargets = ['aarch64-apple-darwin', 'x86_64-unknown-linux-gnu']\n",
    )
    .unwrap();
    manifest(root.path(), "2.4.1", "publish = false");

    let output = plan(root.path(), &[]);
    assert!(output.status.success(), "{:?}", output);
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.contains("tag=v2.4.1\nprerelease=false\n"));
    assert!(stdout.contains("dist-version=0.32.2\n"));
    let matrix = stdout
        .lines()
        .find_map(|line| line.strip_prefix("matrix="))
        .unwrap();
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(matrix).unwrap(),
        json!({"include": [
            {"target": "aarch64-apple-darwin", "runner": "macos-15"},
            {"target": "x86_64-unknown-linux-gnu", "runner": "ubuntu-22.04"}
        ]})
    );

    for policy in ["", "license = 'MIT'\npublish = false"] {
        manifest(root.path(), "2.4.1", policy);
        let output = plan(root.path(), &["--tag", "v2.4.1"]);
        assert!(!output.status.success(), "{policy}");
        assert!(output.stdout.is_empty());
    }
    manifest(root.path(), "2.4.1", "license = 'MIT'");
    for tag in ["v2.4.0", "2.4.1", "other/v2.4.1", "v2.4.1;echo injected"] {
        let output = plan(root.path(), &["--tag", tag]);
        assert!(!output.status.success(), "{tag}");
        assert!(output.stdout.is_empty());
    }
    assert!(plan(root.path(), &["--tag", "v2.4.1"]).status.success());

    manifest(root.path(), "3.0.0-rc.2", "license = 'MIT'");
    let output = plan(root.path(), &["--tag", "v3.0.0-rc.2"]);
    assert!(output.status.success(), "{:?}", output);
    assert!(
        String::from_utf8(output.stdout)
            .unwrap()
            .contains("prerelease=true\n")
    );

    for targets in [
        "[]",
        "['x86_64-pc-windows-msvc']",
        "['aarch64-apple-darwin', 'aarch64-apple-darwin']",
    ] {
        fs::write(
            root.path().join("dist-workspace.toml"),
            format!("[dist]\ncargo-dist-version = '0.32.2'\ntargets = {targets}\n"),
        )
        .unwrap();
        let output = plan(root.path(), &[]);
        assert!(!output.status.success(), "{targets}");
        assert!(output.stdout.is_empty());
    }
}
