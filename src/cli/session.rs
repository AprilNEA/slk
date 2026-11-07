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

pub(super) fn claims_stdin(matches: &ArgMatches) -> bool {
    matches.try_get_one::<bool>("stdin").ok().flatten() == Some(&true)
        || ["file", "blocks_file", "message_file"].iter().any(|id| {
            matches
                .try_get_raw(id)
                .ok()
                .flatten()
                .is_some_and(|mut values| values.any(|value| value == "-"))
        })
        || matches
            .subcommand()
            .is_some_and(|(_, child)| claims_stdin(child))
}
