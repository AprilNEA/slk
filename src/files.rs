use crate::auth::{Credentials, FileSettings, Realm};
use crate::error::{Error, Result};
use crate::platform::{Native, Platform};
use bytes::Bytes;
use reqwest::{Client, Response, header, redirect, retry};
use serde::{Deserialize, Serialize};
use slack_morphism::{SlackApiToken, SlackFileId};
use std::fs;
use std::io::Read;
use std::path::Path;
use std::time::Duration;
use tokio::io::AsyncWriteExt;
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

pub(crate) struct Transfer {
    client: Client,
    limit: u64,
}

impl Transfer {
    pub(crate) fn new(settings: &FileSettings) -> Result<Self> {
        Ok(Self {
            client: Self::client(settings)
                .build()
                .map_err(|_| Error::Transport)?,
            limit: settings.max_bytes.get(),
        })
    }

    fn client(settings: &FileSettings) -> reqwest::ClientBuilder {
        crate::initialize_tls();
        Client::builder()
            .redirect(redirect::Policy::none())
            .retry(retry::never())
            .timeout(Duration::from_secs(settings.timeout_secs.get()))
    }

    pub(crate) async fn upload(&self, url: FileUrl, content: Bytes) -> Result<()> {
        if content.len() as u64 > self.limit {
            return Err(Error::FileTooLarge { limit: self.limit });
        }
        let response = self
            .client
            .post(url.0)
            .header(header::CONTENT_TYPE, "application/octet-stream")
            .body(content)
            .send()
            .await
            .map_err(transfer_error)?;
        check_status(&response)
    }

    pub(crate) async fn download(
        &self,
        url: FileUrl,
        credentials: &Credentials,
        expected_size: u64,
        destination: &Path,
    ) -> Result<u64> {
        url.check_realm(credentials.realm())?;
        if expected_size > self.limit {
            return Err(Error::FileTooLarge { limit: self.limit });
        }
        let token = SlackApiToken::from(credentials);
        let mut request = self.client.get(url.0).bearer_auth(&token.token_value.0);
        if let Some(cookie) = token.cookie {
            let mut value =
                header::HeaderValue::from_str(&cookie.0).map_err(|_| Error::Credentials)?;
            value.set_sensitive(true);
            request = request.header(header::COOKIE, value);
        }
        let mut response = request.send().await.map_err(transfer_error)?;
        check_status(&response)?;
        if response
            .content_length()
            .is_some_and(|size| size != expected_size)
        {
            return Err(Error::Response);
        }
        let directory = destination
            .parent()
            .filter(|path| !path.as_os_str().is_empty())
            .unwrap_or_else(|| Path::new("."));
        let temporary = tempfile::NamedTempFile::new_in(directory)
            .map_err(|error| Error::io("create the private download file", error))?;
        Native::restrict(temporary.path(), false)
            .map_err(|error| Error::io("secure the private download file", error))?;
        let mut file = tokio::fs::File::from_std(
            temporary
                .reopen()
                .map_err(|error| Error::io("open the private download file", error))?,
        );
        let mut received = 0_u64;
        while let Some(chunk) = response.chunk().await.map_err(transfer_error)? {
            received = received
                .checked_add(chunk.len() as u64)
                .ok_or(Error::FileTooLarge { limit: self.limit })?;
            if received > self.limit {
                return Err(Error::FileTooLarge { limit: self.limit });
            }
            if received > expected_size {
                return Err(Error::Response);
            }
            file.write_all(&chunk)
                .await
                .map_err(|error| Error::io("write downloaded bytes", error))?;
        }
        if received != expected_size {
            return Err(Error::Response);
        }
        file.flush()
            .await
            .map_err(|error| Error::io("flush downloaded bytes", error))?;
        file.sync_all()
            .await
            .map_err(|error| Error::io("persist downloaded bytes", error))?;
        drop(file);
        temporary.persist_noclobber(destination).map_err(|error| {
            Error::io(
                "save the download without replacing an existing path",
                error.error,
            )
        })?;
        Ok(received)
    }
}

fn transfer_error(error: reqwest::Error) -> Error {
    if error.is_timeout() {
        Error::Timeout
    } else {
        Error::Transport
    }
}

fn check_status(response: &Response) -> Result<()> {
    match response.status().as_u16() {
        200 => Ok(()),
        429 => Err(Error::RateLimited {
            retry_after: response
                .headers()
                .get(header::RETRY_AFTER)
                .and_then(|value| value.to_str().ok()?.parse().ok()),
            slack_error: None,
        }),
        status => Err(Error::Http { status }),
    }
}

pub(crate) fn parse_file(value: &str) -> Result<SlackFileId> {
    if value.len() < 2
        || !value.starts_with('F')
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_uppercase() || byte.is_ascii_digit())
    {
        return Err(Error::Arguments(
            "Use an explicit Slack file ID starting with F, not a URL.",
        ));
    }
    Ok(SlackFileId(value.to_owned()))
}

#[cfg(test)]
pub(crate) mod tests;
