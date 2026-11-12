use crate::APP_ID;
use crate::auth::{Store, valid_profile_name};
use crate::error::{Error, Result};
use clap::{ArgMatches, Args};
use directories::ProjectDirs;
use std::io::{self, Read};
use std::path::PathBuf;
use zeroize::Zeroizing;

#[derive(Args)]
pub(super) struct SessionSource {
    /// Use this configuration directory instead of the platform default.
    #[arg(long, global = true, conflicts_with_all = ["session_stdin", "session_env"])]
    config_dir: Option<PathBuf>,
    /// Read an identity-bound session JSON from stdin; never read or write the keyring.
    #[arg(
        long,
        global = true,
        requires = "profile",
        conflicts_with = "session_env"
    )]
    pub session_stdin: bool,
    /// Read session JSON from this environment variable, not from an argument value.
    #[arg(long, global = true, value_name = "NAME", requires = "profile")]
    session_env: Option<String>,
}

impl SessionSource {
    pub(super) fn configured(&self) -> bool {
        self.injected() || self.config_dir.is_some()
    }

    pub(super) fn injected(&self) -> bool {
        self.session_stdin || self.session_env.is_some()
    }

    pub(super) fn load(&self, selected: Option<&str>) -> Result<Store> {
        if selected.is_some_and(|name| !valid_profile_name(name)) {
            return Err(Error::ProfileName);
        }
        if self.injected() {
            let name = selected.ok_or(Error::Arguments("Session injection requires --profile."))?;
            let input = if let Some(variable) = &self.session_env {
                Zeroizing::new(std::env::var(variable).map_err(|_| Error::Credentials)?)
            } else {
                let mut input = Zeroizing::new(String::new());
                io::stdin()
                    .read_to_string(&mut input)
                    .map_err(|error| Error::io("read session JSON from stdin", error))?;
                input
            };
            Store::memory(name, &input)
        } else {
            let directory = match &self.config_dir {
                Some(directory) => directory.clone(),
                None => ProjectDirs::from("", "", APP_ID)
                    .ok_or(Error::ConfigDirectory)?
                    .config_dir()
                    .to_path_buf(),
            };
            Store::load(directory)
        }
    }
}

#[derive(Args)]
pub(super) struct Selector {
    /// slackcli selector: an exact profile name or a unique saved workspace ID.
    #[arg(long, global = true, conflicts_with = "profile")]
    workspace: Option<String>,
}

impl Selector {
    /// Whether `matches`, the innermost subcommand's matches, carry `--workspace`.
    pub(super) fn given(matches: &ArgMatches) -> bool {
        matches.try_contains_id("workspace").unwrap_or(false)
    }

    pub(super) fn resolve(&self, store: &Store, selected: Option<&str>) -> Result<Option<String>> {
        let Some(selector) = &self.workspace else {
            return Ok(selected.map(str::to_owned));
        };
        if selected.is_some() {
            return Err(Error::Arguments("Use --profile or --workspace, not both."));
        }
        let profiles = store.profiles();
        if let Some(profile) = profiles.iter().find(|profile| profile.profile == *selector) {
            return Ok(Some(profile.profile.clone()));
        }
        let matches = profiles
            .into_iter()
            .filter(|profile| profile.identity.team_id == *selector)
            .collect::<Vec<_>>();
        match matches.as_slice() {
            [profile] => Ok(Some(profile.profile.clone())),
            [] => Err(Error::ProfileNotFound),
            _ => Err(Error::AmbiguousProfile),
        }
    }
}
