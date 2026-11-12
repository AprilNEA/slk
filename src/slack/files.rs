use super::{Slack, map_write_error};
use crate::auth::Credentials;
use crate::error::{Error, Result};
use crate::files::{File, FileUrl, Transfer, Upload};
use crate::ids::parse_file;
use serde::Deserialize;
use slack_morphism::api::{
    SlackApiFilesComplete, SlackApiFilesCompleteUploadExternalRequest,
    SlackApiFilesGetUploadUrlExternalResponse,
};
use slack_morphism::prelude::ValueStruct;
use slack_morphism::{
    SlackApiToken, SlackChannelId, SlackClientApiCallContext, SlackClientHttpConnector, SlackFileId,
};
use url::form_urlencoded;

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
