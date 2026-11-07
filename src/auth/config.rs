use super::{Identity, Kind, valid_profile_name};
use crate::error::{Error, Result};
use crate::platform::{Native, Platform};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fs::{self, File, OpenOptions, TryLockError};
use std::io::{ErrorKind, Write};
use std::num::{NonZeroU16, NonZeroU64, NonZeroUsize};
use std::path::{Path, PathBuf};

const CONFIG_FILE: &str = "config.json";

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub(crate) struct Settings {
    pub request_timeout_secs: NonZeroU64,
    pub login_timeout_secs: NonZeroU64,
    pub page_size: NonZeroU16,
    pub max_pages: NonZeroU16,
    pub browser: BrowserSettings,
    pub files: FileSettings,
    pub messages: MessageSettings,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            request_timeout_secs: NonZeroU64::new(30).expect("the default timeout is nonzero"),
            login_timeout_secs: NonZeroU64::new(300).expect("the default login timeout is nonzero"),
            page_size: NonZeroU16::new(20).expect("the default page size is nonzero"),
            max_pages: NonZeroU16::new(100).expect("the default page budget is nonzero"),
            browser: BrowserSettings::default(),
            files: FileSettings::default(),
            messages: MessageSettings::default(),
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub(crate) struct MessageSettings {
    pub chunk_chars: NonZeroUsize,
    pub max_chunks: NonZeroU16,
}

impl Default for MessageSettings {
    fn default() -> Self {
        Self {
            chunk_chars: NonZeroUsize::new(crate::messages::MAX_TEXT_CHARS)
                .expect("the message limit is nonzero"),
            max_chunks: NonZeroU16::new(10).expect("the default chunk budget is nonzero"),
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub(crate) struct FileSettings {
    pub max_bytes: NonZeroU64,
    pub timeout_secs: NonZeroU64,
}

impl Default for FileSettings {
    fn default() -> Self {
        Self {
            max_bytes: NonZeroU64::new(20 * 1024 * 1024)
                .expect("the default file limit is nonzero"),
            timeout_secs: NonZeroU64::new(120).expect("the default transfer timeout is nonzero"),
        }
    }
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub(crate) struct BrowserSettings {
    pub executable: Option<PathBuf>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Profile {
    pub kind: Kind,
    pub identity: Identity,
    pub keyring_ref: String,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub(super) struct Config {
    pub settings: Settings,
    pub profiles: BTreeMap<String, Profile>,
}

impl Config {
    pub fn load(directory: &Path) -> Result<Self> {
        if !Self::secure_existing(directory, true)? {
            return Ok(Self::default());
        }
        let path = directory.join(CONFIG_FILE);
        if !Self::secure_existing(&path, false)? {
            return Ok(Self::default());
        }
        let contents = fs::read(path).map_err(|error| Error::io("read config.json", error))?;
        let config: Self = serde_json::from_slice(&contents).map_err(|_| Error::Config)?;
        if config.profiles.iter().any(|(name, profile)| {
            !valid_profile_name(name)
                || profile.identity.validate().is_err()
                || uuid::Uuid::parse_str(&profile.keyring_ref).is_err()
        }) {
            return Err(Error::Config);
        }
        Ok(config)
    }

    pub fn lock(directory: &Path) -> Result<File> {
        Self::create_directory(directory)?;
        let path = directory.join("auth.lock");
        Self::secure_existing(&path, false)?;
        let file = OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(&path)
            .map_err(|error| Error::io("open the authentication lock", error))?;
        Native::restrict(&path, false)
            .map_err(|error| Error::io("secure the authentication lock", error))?;
        file.try_lock().map_err(|error| match error {
            TryLockError::WouldBlock => Error::AuthBusy,
            TryLockError::Error(error) => Error::io("lock authentication changes", error),
        })?;
        Ok(file)
    }

    pub fn save(&self, directory: &Path) -> Result<()> {
        Self::create_directory(directory)?;
        let path = directory.join(CONFIG_FILE);
        Self::secure_existing(&path, false)?;
        let mut temporary = tempfile::NamedTempFile::new_in(directory)
            .map_err(|error| Error::io("create a temporary configuration file", error))?;
        Native::restrict(temporary.path(), false)
            .map_err(|error| Error::io("secure the configuration file", error))?;
        let json = serde_json::to_vec_pretty(self).map_err(|_| Error::Config)?;
        temporary
            .write_all(&json)
            .and_then(|()| temporary.as_file().sync_all())
            .map_err(|error| Error::io("write config.json", error))?;
        temporary
            .persist(path)
            .map_err(|error| Error::io("replace config.json", error.error))?;
        Ok(())
    }

    fn create_directory(directory: &Path) -> Result<()> {
        if !Self::secure_existing(directory, true)? {
            fs::create_dir_all(directory)
                .map_err(|error| Error::io("create the configuration directory", error))?;
            Self::secure_existing(directory, true)?;
        }
        Ok(())
    }

    fn secure_existing(path: &Path, directory: bool) -> Result<bool> {
        let metadata = match fs::symlink_metadata(path) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == ErrorKind::NotFound => return Ok(false),
            Err(error) => return Err(Error::io("inspect the configuration path", error)),
        };
        if (directory && !metadata.is_dir()) || (!directory && !metadata.is_file()) {
            return Err(Error::ConfigPath);
        }
        Native::restrict(path, directory)
            .map_err(|error| Error::io("secure the configuration path", error))?;
        Ok(true)
    }
}
