//! Module layer check contract exercised through the repository task CLI.

use std::fs;
use std::path::Path;
use std::process::{Command, Output};

fn layers(root: &Path) -> Output {
    Command::new(env!("CARGO_BIN_EXE_xtask"))
        .current_dir(root)
        .arg("layers")
        .output()
        .unwrap()
}

#[test]
fn layers_fail_on_a_denied_module_reference_and_name_its_location() {
    let root = tempfile::tempdir().unwrap();
    let write = |path: &str, text: &str| fs::write(root.path().join(path), text).unwrap();
    fs::create_dir(root.path().join("src")).unwrap();
    write(
        "Cargo.toml",
        "[package]\nname = 'layers-fixture'\nversion = '0.0.0'\nedition = '2024'\n\n[package.metadata.layers]\norder = ['cli', 'model']\n",
    );
    write("src/lib.rs", "mod cli;\nmod model;\n");
    write("src/cli.rs", "pub struct Flag;\n");
    write("src/model.rs", "pub struct Record;\n");

    let output = layers(root.path());
    assert!(output.status.success(), "{output:?}");

    write(
        "src/model.rs",
        "pub struct Record;\n\nuse crate::{cli::Flag as _, model::Record as _};\n",
    );
    let output = layers(root.path());
    assert!(!output.status.success(), "{output:?}");
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(
        stderr.contains("model.rs:3: `model` references `cli`"),
        "{stderr}"
    );
}
