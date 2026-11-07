//! Repository release planning and native archive verification.

use anyhow::{Context, Result, bail, ensure};
use cargo_dist_schema::{ArtifactKind, AssetKind, DistManifest};
use cargo_metadata::{Metadata, MetadataCommand};
use clap::{Parser, Subcommand};
use self_update::{Checksum, Extract};
use serde::Deserialize;
use serde_json::json;
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

#[derive(Parser)]
#[command(about = "Repository release checks; run from the repository root")]
struct Args {
    #[command(subcommand)]
    command: Task,
}

#[derive(Subcommand)]
enum Task {
    /// Print GitHub Actions outputs derived from Cargo and cargo-dist configuration.
    Plan {
        /// Require this exact release tag and crates.io eligibility; empty means preview.
        #[arg(long, default_value = "")]
        tag: String,
    },
    /// Verify and run the native archive described by a local cargo-dist build manifest.
    Smoke {
        #[arg(long)]
        manifest: PathBuf,
    },
}

#[derive(Deserialize)]
struct Distribution {
    dist: Dist,
}

#[derive(Deserialize)]
#[serde(rename_all = "kebab-case")]
struct Dist {
    cargo_dist_version: String,
    targets: Vec<String>,
}

fn main() -> Result<()> {
    match Args::parse().command {
        Task::Plan { tag } => plan(&tag),
        Task::Smoke { manifest } => smoke(&manifest),
    }
}

fn workspace() -> Result<Metadata> {
    MetadataCommand::new()
        .manifest_path("Cargo.toml")
        .no_deps()
        .other_options(vec!["--locked".to_owned()])
        .exec()
        .context("Read Cargo workspace metadata")
}

fn plan(requested_tag: &str) -> Result<()> {
    let workspace = workspace()?;
    let package = workspace
        .root_package()
        .context("No root application package")?;
    let tag = format!("v{}", package.version);
    if !requested_tag.is_empty() {
        ensure!(requested_tag == tag, "Release tag must be exactly {tag}");
        ensure!(
            package.license.is_some() || package.license_file.is_some(),
            "Choose a license before publishing a release"
        );
        ensure!(
            package
                .publish
                .as_ref()
                .is_none_or(|registries| registries.iter().any(|name| name == "crates-io")),
            "Cargo.toml disables crates.io publishing"
        );
    }

    let config: Distribution = toml::from_str(
        &fs::read_to_string("dist-workspace.toml").context("Read distribution configuration")?,
    )
    .context("Parse distribution configuration")?;
    let dist = config.dist;
    ensure!(
        !dist.targets.is_empty(),
        "No distribution targets configured"
    );
    ensure!(
        dist.targets.iter().collect::<BTreeSet<_>>().len() == dist.targets.len(),
        "Distribution targets must be unique"
    );
    let runners: BTreeMap<String, String> =
        serde_json::from_value(workspace.workspace_metadata["release"]["runners"].clone())
            .context("Read workspace.metadata.release.runners from Cargo.toml")?;
    let matrix = dist
        .targets
        .iter()
        .map(|target| {
            let runner = runners
                .get(target)
                .with_context(|| format!("No native GitHub runner configured for {target}"))?;
            Ok(json!({"target": target, "runner": runner}))
        })
        .collect::<Result<Vec<_>>>()?;

    println!("tag={tag}");
    println!("prerelease={}", !package.version.pre.is_empty());
    println!("dist-version={}", dist.cargo_dist_version);
    println!("matrix={}", json!({"include": matrix}));
    Ok(())
}

fn smoke(manifest_path: &Path) -> Result<()> {
    let workspace = workspace()?;
    let package = workspace
        .root_package()
        .context("No root application package")?;
    let manifest: DistManifest = serde_json::from_slice(
        &fs::read(manifest_path).context("Read the cargo-dist build manifest")?,
    )
    .context("Parse the cargo-dist build manifest")?;
    let archives = manifest
        .artifacts
        .iter()
        .filter(|(_, artifact)| matches!(artifact.kind, ArtifactKind::ExecutableZip))
        .collect::<Vec<_>>();
    let [(name, archive)] = archives.as_slice() else {
        bail!("Smoke checks require exactly one native executable archive");
    };
    ensure!(
        archive
            .target_triples
            .iter()
            .map(|target| target.as_str())
            .eq([self_update::get_target()]),
        "The archive must target the smoke check's native platform"
    );
    let archive_path = archive.path.as_ref().context("Archive has no local path")?;
    let checksum_id = archive
        .checksum
        .as_ref()
        .context("Archive has no checksum")?;
    let checksum_path = manifest
        .artifacts
        .get(checksum_id)
        .and_then(|artifact| artifact.path.as_ref())
        .context("Checksum has no local path")?;
    let checksum = Checksum::from_sums_file(
        &fs::read_to_string(checksum_path.as_str()).context("Read archive checksum")?,
        name.as_str(),
    )?;
    let Checksum::Sha256(expected) = checksum else {
        bail!("Release archives require SHA256 checksums");
    };
    let actual = hex::encode(Sha256::digest(
        fs::read(archive_path.as_str()).context("Read archive bytes")?,
    ));
    ensure!(
        actual.eq_ignore_ascii_case(&expected),
        "Archive SHA256 does not match its checksum"
    );

    let extracted = tempfile::tempdir().context("Create archive smoke directory")?;
    Extract::from_source(archive_path.as_str())
        .extract_into(extracted.path())
        .context("Extract the release archive with the updater's archive library")?;
    let root = if let Some(directory) = name.as_str().strip_suffix(".tar.xz") {
        extracted.path().join(directory)
    } else {
        ensure!(
            name.as_str().ends_with(".zip"),
            "Unsupported archive format"
        );
        extracted.path().to_owned()
    };
    for asset in &archive.assets {
        let path = asset.path.as_ref().context("Archive asset has no path")?;
        ensure!(
            root.join(path.as_str()).exists(),
            "Archive is missing asset {path}"
        );
    }
    let binaries = archive
        .assets
        .iter()
        .filter(|asset| matches!(asset.kind, AssetKind::Executable(_)))
        .collect::<Vec<_>>();
    let [binary] = binaries.as_slice() else {
        bail!("The archive must contain exactly one executable");
    };
    let executable = root.join(binary.path.as_ref().context("Binary has no path")?.as_str());
    for args in [vec!["--version"], vec!["--help"], vec!["mcp", "--help"]] {
        let output = Command::new(&executable)
            .args(&args)
            .stdin(Stdio::null())
            .output()
            .with_context(|| format!("Run extracted executable with {args:?}"))?;
        ensure!(
            output.status.success(),
            "Archive executable failed {args:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        let stdout = String::from_utf8(output.stdout).context("Decode executable output")?;
        if args == ["--version"] {
            ensure!(
                stdout.trim() == format!("{} {}", package.name, package.version),
                "Archive executable version does not match Cargo.toml"
            );
        } else {
            ensure!(stdout.contains("Usage:"), "Archive help is missing usage");
        }
    }
    eprintln!("Verified {name}: SHA256, packaged assets, version, CLI help, MCP help");
    println!("files<<SLK_DIST_FILES\n{archive_path}\n{checksum_path}\nSLK_DIST_FILES");
    Ok(())
}
