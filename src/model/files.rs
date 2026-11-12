use crate::auth::{FileSettings, Realm};
use crate::error::{Error, Result};
use bytes::Bytes;
use serde::{Deserialize, Serialize};
use slack_morphism::SlackFileId;
use std::fs;
use std::io::Read;
use std::path::Path;
use url::Url;

#[derive(Debug, Deserialize, Serialize)]
pub(crate) struct File {
    #[serde(rename(deserialize = "id"))]
    pub file_id: SlackFileId,
    pub name: Option<String>,
    pub title: Option<String>,
    pub size: Option<u64>,
    pub mimetype: Option<String>,
    pub mode: Option<String>,
    pub is_external: Option<bool>,
    pub filetype: Option<String>,
    #[serde(skip_serializing)]
    url_private: Option<Url>,
    #[serde(skip_serializing)]
    url_private_download: Option<Url>,
}

impl File {
    pub(crate) fn download_url(&self) -> Result<FileUrl> {
        if self.is_external == Some(true)
            || self.mode.as_deref() == Some("external")
            || self.filetype.as_deref() == Some("remote")
        {
            return Err(Error::ExternalFile);
        }
        self.url_private_download
            .as_ref()
            .or(self.url_private.as_ref())
            .ok_or(Error::FileUnavailable)?
            .clone()
            .try_into()
    }
}

pub(crate) struct FileUrl(Url);

impl TryFrom<Url> for FileUrl {
    type Error = Error;

    fn try_from(url: Url) -> Result<Self> {
        if url.scheme() != "https"
            || ![Realm::Commercial, Realm::Gov]
                .into_iter()
                .any(|realm| url.host_str() == Some(realm.file_host().as_str()))
            || !url.username().is_empty()
            || url.password().is_some()
            || url.port().is_some()
            || url.fragment().is_some()
        {
            return Err(Error::FileUrl);
        }
        Ok(Self(url))
    }
}

impl FileUrl {
    pub(crate) fn check_realm(&self, realm: Realm) -> Result<()> {
        if self.0.host_str() != Some(realm.file_host().as_str()) {
            return Err(Error::FileUrl);
        }
        Ok(())
    }
}

impl From<FileUrl> for Url {
    fn from(url: FileUrl) -> Self {
        url.0
    }
}

pub(crate) struct Upload {
    pub name: String,
    pub content: Bytes,
}

impl Upload {
    pub(crate) fn read(path: &Path, settings: &FileSettings) -> Result<Self> {
        let metadata =
            fs::metadata(path).map_err(|error| Error::io("inspect the upload file", error))?;
        if !metadata.is_file() {
            return Err(Error::Arguments(
                "Uploads require a regular file, not a directory or device.",
            ));
        }
        let limit = settings.max_bytes.get();
        if metadata.len() > limit {
            return Err(Error::FileTooLarge { limit });
        }
        let name = path
            .file_name()
            .and_then(|name| name.to_str())
            .filter(|name| !name.is_empty() && !name.chars().any(char::is_control))
            .ok_or(Error::Arguments(
                "The upload filename must be UTF-8 without controls.",
            ))?
            .to_owned();
        let mut content = Vec::new();
        fs::File::open(path)
            .and_then(|file| file.take(limit.saturating_add(1)).read_to_end(&mut content))
            .map_err(|error| Error::io("read the upload file", error))?;
        if content.len() as u64 > limit {
            return Err(Error::FileTooLarge { limit });
        }
        if content.is_empty() {
            return Err(Error::Arguments("Slack uploads require a nonempty file."));
        }
        Ok(Self {
            name,
            content: content.into(),
        })
    }
}

#[cfg(test)]
mod tests;
