# Distribution and installation

slk is experimental and retains the known SDK 429 defect. Its regression test is temporarily ignored for approved experimental releases; other release checks remain required. Neither publication nor passing CI establishes live Slack compatibility. Local archives are build artifacts, not published GitHub Releases. [Live workflow checks](development.md#verification-limits) remain incomplete. Publishing, release tags, signing, and notarization require explicit authorization.

## Install a local source build

Use the stable toolchain specified in `rust-toolchain.toml` on macOS, Linux, or Windows with the MSVC C++ build tools:

```sh
cargo install --path . --locked
slk --version
slk --help
```

Cargo installs into its configured bin directory, normally `~/.cargo/bin`; add that directory to your shell's PATH if necessary. This command builds the local checkout, not a crates.io release. Do not run slk as root to access another user's keyring.

Persistent authentication needs an unlocked macOS Keychain, Linux Secret Service, or Windows Credential Manager. Linux builds compile the bundled libdbus but still need an actual Secret Service provider at runtime. Explicit [headless sessions](authentication.md#headless-sessions) do not access a keyring. Browser login additionally requires an installed Chrome/Chromium browser and a graphical session. No browser or keyring daemon is bundled into an archive, and there is no plaintext fallback.

## Build local archives

Install [cargo-dist](https://github.com/axodotdev/cargo-dist) at the `cargo-dist-version` recorded in `dist-workspace.toml`. This file defines targets, archive formats, included documentation, and checksums. cargo-dist packages artifacts; the repository maintains its own GitHub Actions workflows. Do not enable generated CI or overwrite `release.yml` with `dist generate`. Local builds do not publish releases or execute installer scripts. Self-update requires an eligible published release.

```sh
cargo run --locked --package xtask -- plan
dist build --artifacts=local --target x86_64-unknown-linux-gnu --output-format=json > target/native-dist-manifest.json
cargo run --locked --package xtask -- smoke --manifest target/native-dist-manifest.json
```

Run this example on Linux x64. The smoke check verifies SHA256, extracts the archive with the updater's library, checks packaged assets, and runs the extracted executable's version, root help, and MCP help. It requires exactly one archive for the native host; it does not install or update slk. On macOS, install the second CPU target to build both configured Mac archives:

```sh
rustup target add aarch64-apple-darwin x86_64-apple-darwin
dist build --artifacts=local --target aarch64-apple-darwin --target x86_64-apple-darwin
```

On Windows with the MSVC toolchain, build the configured ZIP:

```sh
dist build --artifacts=local --target x86_64-pc-windows-msvc
```

Archives appear in `target/distrib` with a corresponding `.sha256` file. Linux and macOS use `slk-TARGET.tar.xz`, containing a target-named directory with the `slk` executable, README, both license texts, and command documentation. Windows uses `slk-x86_64-pc-windows-msvc.zip`, with `slk.exe` at the archive root. macOS builds require the Apple linker and SDK; a Linux orb alone cannot provide them. Cross-compilation and archive inspection do not establish native acceptance on the target platform.

Linux archives use the GNU/glibc target, not a universal static Linux binary. The build host determines the required glibc version; inspect the executable's symbol requirements and dynamic dependencies before selecting a distribution baseline. No minimum macOS or Linux release is claimed without testing that release.

## Verify before installing an archive

From the directory containing the Linux archive and its checksum file:

```sh
sha256sum --check slk-x86_64-unknown-linux-gnu.tar.xz.sha256
tar -xf slk-x86_64-unknown-linux-gnu.tar.xz
./slk-x86_64-unknown-linux-gnu/slk --version
./slk-x86_64-unknown-linux-gnu/slk --help
```

On macOS, use `shasum -a 256 --check` with the checksum file for the chosen architecture. Compare checksums only from a trusted build or release source; a checksum distributed beside a binary detects corruption but does not independently authenticate its publisher.

After verifying the executable, place it in a user-owned directory on PATH. Review the destination before replacing an existing binary. Current local Mac artifacts are not release-signed or notarized; do not disable platform security controls as an installation step.

## Release workflows

`.github/workflows/ci.yml` runs on branch pushes and can be called by the release workflow. `.github/workflows/release.yml` runs on pull requests and `v*` tag pushes. Pull requests check the proposed merge through the same CI, then build native archives only after every check succeeds. PR runs upload temporary Actions artifacts, never crates or GitHub Releases.

The release sequence is:

1. Validate the exact `v<Cargo package version>` tag. Tag runs also require a license declaration and enabled crates.io publishing; preview plans do not.
2. Run formatting, strict Clippy, release builds, and the full ordinary test suite on Linux, macOS, and Windows. Dependency auditing, crate packaging and verification, and workflow linting must also pass. The SDK 429 regression is an explicitly documented ignored test, not a successful acceptance check; see [verification limits](development.md#verification-limits) for the command and condition for restoring this test.
3. Build Linux x64, macOS arm64, macOS x64, and Windows x64 archives on native runners. Each archive must pass its smoke check before upload. Targets come from `dist-workspace.toml`; runners come from `workspace.metadata.release.runners` in `Cargo.toml`.
4. After approval through the `release` environment, verify the Cargo package and publish to crates.io. No build cache is restored by the publishing or archive jobs.
5. Only after crates.io publication succeeds, create the GitHub Release with archives and per-archive SHA256 files. SemVer prereleases become GitHub prereleases and are excluded by self-update.

Failed, cancelled, or skipped prerequisites cannot authorize automated publication. Only the crates.io job has `id-token: write`; only the GitHub Release job has `contents: write`. Checkouts do not persist credentials, reusable CI inherits no secrets, and tag and target values enter shell commands through quoted environment variables. `CI_TIMEOUT_MINUTES` is an optional repository variable overriding the default 60-minute timeout per job.

### Configure the first publication

crates.io requires an existing crate before Trusted Publishing can be configured. The initial publication used an explicitly approved manual bootstrap from a developer machine or orb. The SDK regression failed in that initial CI; the bootstrap was not a passing acceptance result.

1. Confirm that the account may publish the `slk` crate name; an absent index entry is not a reservation. The root package is configured for crates.io publication under the [selected dual license](../README.md#license); `xtask` remains unpublished.
2. Review the CI results and known limitations before approving publication. Use a clean, committed checkout with the intended package version.
3. Supply an expiring API token through the secret environment variable `CARGO_REGISTRY_TOKEN`. Grant only the token permissions needed for the initial publication. Never put a token in repository files, ordinary variables, command arguments, or chat.
4. Run `cargo publish --locked --package slk --dry-run`, then run `cargo publish --locked --package slk` only with explicit publication approval. Cargo compiles the packaged source but does not run the test suite. Verify the published version before repeating a command whose outcome is uncertain.
5. After the first publication succeeds, register a crates.io trusted publisher with the values below. Later releases obtain a short-lived OIDC token; an OIDC error never falls back to an API token.

| crates.io trusted-publisher field | Value |
| --- | --- |
| Repository owner | `AprilNEA` |
| Repository name | `slk` |
| Workflow filename | `release.yml` |
| Environment | `release` |

Use the workflow filename alone, not `.github/workflows/release.yml`. crates.io matches the originating workflow's `workflow_ref`; a reusable workflow's filename is not a substitute. See the [trusted-publishing documentation](https://crates.io/docs/trusted-publishing).

Protect `master` and `v*` tags. Create a GitHub environment named `release` with required reviewers and tag restrictions before enabling automated publication. An `environment: release` label in YAML does not create reviewer protections. Leave `CRATES_IO_BOOTSTRAP` unset when using OIDC; the workflow's alternative token-based bootstrap path is not needed for a local first publication. Revoke the initial API token after Trusted Publishing is configured and verified.

Do not push the ordinary release tag to republish a version already uploaded locally. The current workflow attempts crates.io publication before creating the GitHub Release, so an existing crate version fails that job. Use an unpublished package version for the normal tag workflow. Completing a GitHub Release for an already published crate requires a separately approved recovery with verified archives.

### Publish an approved version

For an unpublished version, commit the intended version and updated lockfile, push the reviewed source, and wait for CI. Only after release approval, validate and push the matching tag. Replace `vX.Y.Z` below with the tag matching the package version:

```sh
tag=vX.Y.Z
cargo run --locked --package xtask -- plan --tag "$tag"
cargo publish --locked --dry-run
git tag "$tag"
git push origin "$tag"
```

The tag triggers publication after all gates and environment approvals. Ordinary branch pushes cannot publish. Do not use `--force-tag`, force-push release tags, or ignore a failed check. The workflow does not sign or notarize binaries; add an approved signing process before claiming signed distribution.

crates.io and GitHub publication are separate, irreversible operations, not a transaction. If crates.io succeeds but GitHub fails before creating a release, use GitHub's **Re-run failed jobs** instead of republishing the crate. If either command may have completed before reporting failure, inspect the registry or release and its assets before retrying. The workflow does not overwrite an existing GitHub Release or automatically roll back a published crate. If a release was partially created, finish recovery explicitly after inspecting the existing assets.

## Explicit self-update

```sh
slk update --check --json
slk update --yes --json
```

The first command only reads release metadata. The second approves replacing the running executable when a newer eligible release exists; it is not a dry-run. Interactive `slk update` previews the version, target, executable path, and replacement warning, then defaults to no. JSON and non-interactive installation require `--yes`; `--check` conflicts with `--yes`. No Slack profile or credential is needed. No ordinary command checks for updates, and MCP rejects update commands.

The updater uses GitHub's `/releases/latest` for the fixed repository in Cargo metadata, `aprilnea/slk`. GitHub excludes drafts and marked prereleases from that endpoint; slk also rejects SemVer prerelease tags. A candidate must have strictly higher SemVer precedence than the running version. Build metadata alone is not an upgrade. An older latest release produces `current`, never a downgrade. The CLI does not enumerate every release to find the greatest version, install a supplied tag, or change update repositories.

An eligible release must contain exactly one archive named for the executable's build target. No CPU, OS, ABI, filename-substring, or checksum-file fallback is allowed. The selected archive must have a valid `sha256:` digest in GitHub release metadata. Missing or unsupported digests fail closed; a `.sha256` sidecar does not bypass this requirement. GitHub/TLS and repository release permissions are trusted. The digest detects mismatched bytes; it is not an independent publisher signature.

After approval, `self_update` downloads that captured release asset, verifies its digest, extracts only the expected executable member, and replaces the current executable path. The release is not rediscovered between approval and installation. The isolated client sends no Slack tokens or cookies, follows bounded HTTPS redirects, rejects HTTP, applies `settings.request_timeout_secs` per request, and does not retry. It does not download or execute an installer script. A missing release, missing target, failed download, corrupt archive, or digest mismatch remains an error rather than an unverified install.

**Replacement is not an all-platform rollback transaction.** Download, digest, and extraction failures occur before replacement. Once replacement begins, filesystem or OS failures can leave the installation changed. In particular, `self-replace` on Windows can rename the old executable before failing to install the new one; the original path may be missing and cleanup can remove the renamed image. Keep a verified installer/archive available and reinstall manually after such a failure. Native Windows replacement and failure-injection acceptance remain pending. Do not promise that every failed update leaves the old executable usable.

The updater does not elevate privileges. Use the owning package manager for package-managed installations, and do not run as administrator merely to override destination permissions. Local build artifacts from an earlier source revision are not proof of an update path.

JSON success contains `state` (`current`, `available`, or `updated`), `current_version`, `latest_version`, `target`, `asset`, and `digest`. `current_version` is the version of the running process; after an update, start the replacement executable to observe its version. `--check` returns metadata, not proof that the archive has been downloaded or its bytes verified. Errors use `invalid_release` or `update_failed` and leave stdout empty.

## Release checks

Before approving publication, inspect the CI results, archive paths, checksums, executable type, dynamic dependencies, and native help output on each target. Complete keyring, browser, live Slack, and self-replacement acceptance separately; CI does not contact Slack or unlock a desktop keyring. Verify GitHub's SHA256 asset digests before recommending self-update. These workflows distribute the CLI; they do not deploy a remote MCP service.
