use super::{Slack, map_write_error};
use crate::auth::{Credentials, FileSettings};
use crate::error::{Error, Result};
use crate::ids::parse_file;
use crate::model::files::{File, FileUrl, Upload};
use crate::platform::{Native, Platform};
use bytes::Bytes;
use reqwest::{Client, Response, header, redirect, retry};
use serde::Deserialize;
use slack_morphism::api::{
    SlackApiFilesComplete, SlackApiFilesCompleteUploadExternalRequest,
    SlackApiFilesGetUploadUrlExternalResponse,
};
use slack_morphism::prelude::ValueStruct;
use slack_morphism::{
    SlackApiToken, SlackChannelId, SlackClientApiCallContext, SlackClientHttpConnector, SlackFileId,
};
use std::path::Path;
use std::time::Duration;
use tokio::io::AsyncWriteExt;
use url::{Url, form_urlencoded};

#[derive(Deserialize)]
struct FileResponse {
    file: File,
}

#[derive(Deserialize)]
struct Completed {
    files: Vec<File>,
}

impl<C: SlackClientHttpConnector + Send + Sync> Slack<C> {
    pub(crate) async fn file(&self, credentials: &Credentials, id: &SlackFileId) -> Result<File> {
        let response: FileResponse = self
            .get(credentials, "files.info", vec![("file", Some(id.value()))])
            .await?;
        if response.file.file_id != *id {
            return Err(Error::Response);
        }
        Ok(response.file)
    }

    pub(crate) async fn upload_file(
        &self,
        credentials: &Credentials,
        upload: Upload,
        channel: Option<&SlackChannelId>,
        transfer: &Transfer,
    ) -> Result<File> {
        let token = SlackApiToken::from(credentials);
        let span = tracing::Span::none();
        let context = SlackClientApiCallContext {
            rate_control_params: None,
            token: Some(&token),
            tracing_span: &span,
            is_sensitive_url: false,
        };
        let body = form_urlencoded::Serializer::new(String::new())
            .append_pair("filename", &upload.name)
            .append_pair("length", &upload.content.len().to_string())
            .finish();
        let connector = &self.client.http_api.connector;
        let endpoint = connector
            .create_method_uri_path("files.getUploadURLExternal")
            .map_err(super::map_slack_error)?;
        let ticket: SlackApiFilesGetUploadUrlExternalResponse = self
            .response(connector.http_post_uri_binary(
                endpoint,
                "application/x-www-form-urlencoded".to_owned(),
                body.as_bytes(),
                context,
            ))
            .await
            .map_err(map_write_error)?;
        parse_file(ticket.file_id.value()).map_err(|_| Error::Response)?;
        let destination = FileUrl::try_from(ticket.upload_url.0)?;
        destination.check_realm(credentials.realm())?;
        transfer
            .upload(destination, upload.content)
            .await
            .map_err(map_write_error)?;
        let request = SlackApiFilesCompleteUploadExternalRequest::new(vec![
            SlackApiFilesComplete::new(ticket.file_id.clone()).with_title(upload.name),
        ])
        .opt_channel_id(channel.cloned());
        let mut completed: Completed = self
            .write(credentials, "files.completeUploadExternal", &request)
            .await?;
        if completed.files.len() != 1 || completed.files[0].file_id != ticket.file_id {
            return Err(Error::WriteOutcomeUnknown { slack_error: None });
        }
        Ok(completed.files.remove(0))
    }
}

/// Transfers file bytes to and from Slack file hosts, outside the SDK.
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
            .post(Url::from(url))
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
        let mut request = self
            .client
            .get(Url::from(url))
            .bearer_auth(&token.token_value.0);
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

#[cfg(test)]
mod tests;
