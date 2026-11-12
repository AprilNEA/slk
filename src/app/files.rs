use super::{Approval, Connection, Prompt, cancellation};
use crate::auth::FileSettings;
use crate::error::{Error, Result};
use crate::model::files::Upload;
use crate::output::files::{Output, upload_preview_text};
use crate::slack::files::Transfer;
use slack_morphism::{SlackChannelId, SlackClientHttpConnector, SlackFileId};
use std::path::PathBuf;

pub(crate) enum Request {
    Info(SlackFileId),
    Upload {
        upload: Upload,
        channel: Option<SlackChannelId>,
        approval: Approval,
        settings: FileSettings,
    },
    Download {
        file: SlackFileId,
        output: PathBuf,
        settings: FileSettings,
    },
}

impl Request {
    pub(crate) async fn execute<C: SlackClientHttpConnector + Send + Sync>(
        self,
        connection: Connection<C>,
        prompt: &dyn Prompt,
    ) -> Result<Output> {
        let Connection {
            slack,
            credentials,
            profile,
        } = connection;
        let credentials = &credentials;
        match self {
            Self::Info(id) => slack.file(credentials, &id).await.map(Output::File),
            Self::Upload {
                upload,
                channel,
                approval,
                settings,
            } => {
                let size = upload.content.len() as u64;
                if !approval.confirm(prompt, "Upload and share this file as shown?", || {
                    upload_preview_text(&profile, &upload.name, size, channel.as_ref())
                })? {
                    return Ok(Output::UploadPreview {
                        dry_run: true,
                        profile,
                        filename: upload.name,
                        size,
                        channel_id: channel,
                    });
                }
                let transfer = Transfer::new(&settings)?;
                let cancelled = cancellation()?;
                let file = tokio::select! {
                    biased;
                    () = cancelled => return Err(Error::WriteOutcomeUnknown { slack_error: None }),
                    result = slack.upload_file(credentials, upload, channel.as_ref(), &transfer) => result?,
                };
                Ok(Output::Uploaded {
                    file,
                    channel_id: channel,
                })
            }
            Self::Download {
                file,
                output,
                settings,
            } => {
                let file = slack.file(credentials, &file).await?;
                let url = file.download_url()?;
                let expected_size = file.size.ok_or(Error::FileUnavailable)?;
                let transfer = Transfer::new(&settings)?;
                let cancelled = cancellation()?;
                let size = tokio::select! {
                    biased;
                    () = cancelled => return Err(Error::Cancelled),
                    result = transfer.download(url, credentials, expected_size, &output) => result?,
                };
                Ok(Output::Downloaded {
                    file_id: file.file_id,
                    path: output,
                    size,
                })
            }
        }
    }
}
