pub(crate) mod browser;
mod config;
mod credentials;
pub(crate) mod oauth;

pub(crate) use config::{FileSettings, Settings};
pub(crate) use credentials::{Credentials, Kind, Realm, Rotation, Workspace};

use crate::APP_ID;
use crate::error::{Error, Result};
use crate::platform::NativeStore;
use config::{Config, Profile};
use credentials::CredentialInput;
use keyring_core::{Entry, api::CredentialStoreApi};
use secrecy::SecretString;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use uuid::Uuid;
use zeroize::Zeroizing;

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Identity {
    pub workspace_url: Workspace,
    pub team_id: String,
    pub user_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bot_id: Option<String>,
}

impl Identity {
    pub(super) fn validate(&self) -> Result<()> {
        if !self.team_id.starts_with('T')
            || self.team_id.len() < 2
            || !self
                .team_id
                .bytes()
                .all(|byte| byte.is_ascii_uppercase() || byte.is_ascii_digit())
            || crate::users::parse_user(&self.user_id).is_err()
            || self.bot_id.as_ref().is_some_and(|id| {
                !id.starts_with('B')
                    || id.len() < 2
                    || !id
                        .bytes()
                        .all(|byte| byte.is_ascii_uppercase() || byte.is_ascii_digit())
            })
        {
            return Err(Error::Response);
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct ProfileView {
    pub profile: String,
    pub kind: Kind,
    #[serde(flatten)]
    pub identity: Identity,
}

#[derive(Clone)]
pub(crate) struct Store {
    backend: Backend,
}

#[derive(Clone)]
enum Backend {
    Persistent {
        directory: PathBuf,
        config: Config,
    },
    Memory {
        profile: Box<ProfileView>,
        credentials: Credentials,
        settings: Settings,
    },
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SessionInput {
    kind: Kind,
    identity: Identity,
    credentials: CredentialInput<SecretString>,
    #[serde(default)]
    settings: Settings,
}

pub(crate) fn valid_profile_name(name: &str) -> bool {
    !name.is_empty()
        && name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"-_.".contains(&byte))
}

impl Store {
    pub(crate) fn load(directory: PathBuf) -> Result<Self> {
        let config = Config::load(&directory)?;
        Ok(Self {
            backend: Backend::Persistent { directory, config },
        })
    }

    pub(crate) fn memory(name: &str, input: &str) -> Result<Self> {
        if !valid_profile_name(name) {
            return Err(Error::ProfileName);
        }
        let session: SessionInput = serde_json::from_str(input).map_err(|_| Error::Credentials)?;
        let credentials = Credentials::from_input(session.kind, session.credentials)?;
        session.identity.validate()?;
        validate_identity(&credentials, &session.identity)?;
        Ok(Self {
            backend: Backend::Memory {
                profile: Box::new(ProfileView {
                    profile: name.to_owned(),
                    kind: session.kind,
                    identity: session.identity,
                }),
                credentials,
                settings: session.settings,
            },
        })
    }

    pub(crate) fn reload(&self) -> Result<Self> {
        match &self.backend {
            Backend::Persistent { directory, .. } => Self::load(directory.clone()),
            Backend::Memory { .. } => Ok(self.clone()),
        }
    }

    pub(crate) fn lock(&mut self) -> Result<std::fs::File> {
        let Backend::Persistent { directory, config } = &mut self.backend else {
            return Err(Error::Arguments(
                "In-memory sessions cannot change persisted authentication. Reinject refreshed credentials explicitly.",
            ));
        };
        let lock = Config::lock(directory)?;
        *config = Config::load(directory)?;
        Ok(lock)
    }

    pub(crate) fn settings(&self) -> &Settings {
        match &self.backend {
            Backend::Persistent { config, .. } => &config.settings,
            Backend::Memory { settings, .. } => settings,
        }
    }

    pub(crate) fn profiles(&self) -> Vec<ProfileView> {
        let config = match &self.backend {
            Backend::Persistent { config, .. } => config,
            Backend::Memory { profile, .. } => return vec![profile.as_ref().clone()],
        };
        config
            .profiles
            .iter()
            .map(|(name, profile)| ProfileView {
                profile: name.clone(),
                kind: profile.kind,
                identity: profile.identity.clone(),
            })
            .collect()
    }

    pub(crate) fn select(&self, name: Option<&str>) -> Result<ProfileView> {
        let config = match &self.backend {
            Backend::Persistent { config, .. } => config,
            Backend::Memory { profile, .. } => {
                return if name.is_none_or(|name| name == profile.profile) {
                    Ok(profile.as_ref().clone())
                } else {
                    Err(Error::ProfileNotFound)
                };
            }
        };
        let (name, profile) = match name {
            Some(name) => config
                .profiles
                .get_key_value(name)
                .ok_or(Error::ProfileNotFound)?,
            None => match config.profiles.len() {
                0 => return Err(Error::NoProfiles),
                1 => config.profiles.first_key_value().ok_or(Error::NoProfiles)?,
                _ => return Err(Error::AmbiguousProfile),
            },
        };
        Ok(ProfileView {
            profile: name.clone(),
            kind: profile.kind,
            identity: profile.identity.clone(),
        })
    }

    pub(crate) fn credentials(&self, name: &str) -> Result<Credentials> {
        let config = match &self.backend {
            Backend::Persistent { config, .. } => config,
            Backend::Memory {
                profile,
                credentials,
                ..
            } => {
                return if profile.profile == name {
                    Ok(credentials.clone())
                } else {
                    Err(Error::ProfileNotFound)
                };
            }
        };
        let profile = config.profiles.get(name).ok_or(Error::ProfileNotFound)?;
        let encoded = Zeroizing::new(profile.entry()?.get_password().map_err(
            |error| match error {
                keyring_core::Error::NoEntry => Error::CredentialMissing,
                _ => Error::Keyring,
            },
        )?);
        let credentials =
            Credentials::parse(profile.kind, &encoded).map_err(|_| Error::StoredCredential)?;
        validate_identity(&credentials, &profile.identity)?;
        Ok(credentials)
    }

    pub(crate) fn login(
        &mut self,
        name: &str,
        credentials: &Credentials,
        identity: Identity,
    ) -> Result<ProfileView> {
        if !valid_profile_name(name) {
            return Err(Error::ProfileName);
        }
        let Backend::Persistent { directory, config } = &mut self.backend else {
            return Err(Error::Arguments(
                "In-memory sessions do not persist credentials.",
            ));
        };
        identity.validate()?;
        validate_identity(credentials, &identity)?;
        let encoded = credentials.encode_for_keyring()?;
        if let Some(existing) = config.profiles.get(name) {
            if existing.kind != credentials.kind() || existing.identity != identity {
                return Err(Error::ProfileConflict);
            }
            existing
                .entry()?
                .set_password(&encoded)
                .map_err(|_| Error::Keyring)?;
        } else {
            let profile = Profile {
                kind: credentials.kind(),
                identity,
                keyring_ref: Uuid::new_v4().to_string(),
            };
            let entry = profile.entry()?;
            entry.set_password(&encoded).map_err(|_| Error::Keyring)?;
            config.profiles.insert(name.to_owned(), profile);
            if let Err(error) = config.save(directory) {
                config.profiles.remove(name);
                entry
                    .delete_credential()
                    .map_err(|_| Error::CredentialCleanup)?;
                return Err(error);
            }
        }
        self.select(Some(name))
    }

    pub(crate) fn logout(&mut self, name: &str) -> Result<()> {
        let Backend::Persistent { directory, config } = &mut self.backend else {
            return Err(Error::Arguments(
                "In-memory sessions end when the process exits; no stored credential exists to delete.",
            ));
        };
        let profile = config.profiles.get(name).ok_or(Error::ProfileNotFound)?;
        match profile.entry()?.delete_credential() {
            Ok(()) | Err(keyring_core::Error::NoEntry) => (),
            Err(_) => return Err(Error::Keyring),
        }
        config.profiles.remove(name);
        config.save(directory)
    }
}

fn validate_identity(credentials: &Credentials, identity: &Identity) -> Result<()> {
    if credentials.realm() != identity.workspace_url.realm()
        || (credentials.kind() == Kind::Bot) != identity.bot_id.is_some()
        || credentials
            .workspace()
            .is_some_and(|workspace| workspace != &identity.workspace_url)
    {
        return Err(Error::IdentityMismatch);
    }
    Ok(())
}

impl Profile {
    fn entry(&self) -> Result<Entry> {
        NativeStore::new()
            .and_then(|store| store.build(APP_ID, &self.keyring_ref, None))
            .map_err(|_| Error::Keyring)
    }
}

#[cfg(test)]
mod tests;
