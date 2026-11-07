use crate::APP_ID;
use crate::auth::Settings;
use crate::error::{Error, Result};
use self_update::{AsyncReleaseSource, Checksum, Release, ReleaseAsset};
use semver::Version;
use serde::Serialize;
use std::path::Path;
use std::time::Duration;

pub(crate) struct Updater {
    client: reqwest::Client,
    timeout: Duration,
}

pub(crate) struct Plan {
    release: Release,
    current: Version,
    target: String,
    asset: Option<ReleaseAsset>,
    member: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum State {
    Current,
    Available,
    Updated,
}

#[derive(Debug, Serialize)]
pub(crate) struct Report {
    pub state: State,
    pub current_version: String,
    pub latest_version: String,
    pub target: String,
    pub asset: Option<String>,
    pub digest: Option<String>,
}

struct Snapshot(Release);

impl AsyncReleaseSource for Snapshot {
    async fn get_releases(&self) -> self_update::Result<Vec<Release>> {
        Ok(vec![self.0.clone()])
    }
}

impl Updater {
    pub(crate) fn new(settings: &Settings) -> Result<Self> {
        crate::initialize_tls();
        let timeout = Duration::from_secs(settings.request_timeout_secs.get());
        let client = reqwest::Client::builder()
            .https_only(true)
            .timeout(timeout)
            .connect_timeout(timeout)
            .retry(reqwest::retry::never())
            .build()
            .map_err(|source| {
                Error::io(
                    "configure the public release client",
                    std::io::Error::other(source),
                )
            })?;
        Ok(Self { client, timeout })
    }

    pub(crate) async fn discover(&self) -> Result<Plan> {
        let repository = url::Url::parse(env!("CARGO_PKG_REPOSITORY"))
            .expect("Cargo repository metadata is an HTTPS GitHub URL");
        let (owner, name) = repository
            .path()
            .trim_matches('/')
            .split_once('/')
            .expect("Cargo repository metadata contains owner and repository");
        let releases = self_update::backends::github::Update::configure()
            .repo_owner(owner)
            .repo_name(name)
            .bin_name(APP_ID)
            .current_version(env!("CARGO_PKG_VERSION"))
            .reqwest_async_client(self.client.clone())
            .timeout(self.timeout)
            .retries(0)
            .unattended()
            .build_async()?
            .get_latest_release_async()
            .await?;
        let release = releases.latest().ok_or(Error::UpdatePolicy(
            "No stable GitHub release is available.",
        ))?;
        Plan::new(
            release.clone(),
            env!("CARGO_PKG_VERSION"),
            self_update::get_target(),
        )
    }

    pub(crate) async fn install(&self, plan: Plan, path: &Path) -> Result<Report> {
        let Some(asset) = &plan.asset else {
            return Ok(plan.report(State::Current));
        };
        let name = asset.name().to_owned();
        let report = plan.report(State::Updated);
        let status = self_update::backends::custom::AsyncUpdate::configure()
            .source(Snapshot(plan.release))
            .current_version(plan.current.to_string())
            .target(plan.target)
            .bin_name(APP_ID)
            .bin_path_in_archive(plan.member)
            .bin_install_path(path)
            .asset_matcher(move |assets| assets.iter().find(|asset| asset.name() == name).cloned())
            .verify_release_digest(true)
            .reqwest_async_client(self.client.clone())
            .timeout(self.timeout)
            .retries(0)
            .unattended()
            .build_async()?
            .update_async()
            .await?;
        if !status.is_updated() || status.version() != report.latest_version {
            return Err(Error::UpdatePolicy(
                "The updater did not install the approved release.",
            ));
        }
        Ok(report)
    }
}

impl Plan {
    fn new(release: Release, current: &str, target: &str) -> Result<Self> {
        let current = Version::parse(current)
            .map_err(|_| Error::UpdatePolicy("Current version is not valid SemVer."))?;
        let latest = Version::parse(release.version())
            .map_err(|_| Error::UpdatePolicy("Release version is not valid SemVer."))?;
        if !latest.pre.is_empty() {
            return Err(Error::UpdatePolicy(
                "The latest release has a prerelease version; refusing to install it.",
            ));
        }
        let (name, member) = if target.contains("-windows-") {
            (format!("{APP_ID}-{target}.zip"), format!("{APP_ID}.exe"))
        } else {
            (
                format!("{APP_ID}-{target}.tar.xz"),
                format!("{APP_ID}-{target}/{APP_ID}"),
            )
        };
        let asset = if latest.cmp_precedence(&current).is_gt() {
            let assets = release
                .assets()
                .iter()
                .filter(|asset| asset.name() == name)
                .collect::<Vec<_>>();
            let [asset] = assets.as_slice() else {
                return Err(Error::UpdatePolicy(
                    "The release must contain exactly one archive for this target; no architecture or ABI fallback is allowed.",
                ));
            };
            let digest = asset.digest().ok_or(Error::UpdatePolicy(
                "The release archive has no SHA256 digest; refusing an unverified update.",
            ))?;
            match Checksum::parse_digest(digest)? {
                Checksum::Sha256(hex)
                    if hex.len() == 64 && hex.bytes().all(|byte| byte.is_ascii_hexdigit()) => {}
                _ => {
                    return Err(Error::UpdatePolicy(
                        "The release archive must have a valid SHA256 digest.",
                    ));
                }
            }
            Some((*asset).clone())
        } else {
            None
        };
        Ok(Self {
            release,
            current,
            target: target.to_owned(),
            asset,
            member,
        })
    }

    pub(crate) fn available(&self) -> bool {
        self.asset.is_some()
    }

    pub(crate) fn report(&self, state: State) -> Report {
        Report {
            state,
            current_version: self.current.to_string(),
            latest_version: self.release.version().to_owned(),
            target: self.target.clone(),
            asset: self.asset.as_ref().map(|asset| asset.name().to_owned()),
            digest: self
                .asset
                .as_ref()
                .and_then(|asset| asset.digest().map(str::to_owned)),
        }
    }
}

#[cfg(test)]
mod tests;
