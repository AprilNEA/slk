use super::write::{Approval, WriteOptions};
use crate::auth::{Credentials, FileSettings, ProfileView, Store};
use crate::error::{Error, Result};
use crate::files::{Transfer, Upload};
use crate::ids::{parse_channel, parse_file};
use crate::output::{self, Output};
use crate::slack::Slack;
use clap::Subcommand;
use slack_morphism::{SlackChannelId, SlackClientHttpConnector, SlackFileId};
use std::path::PathBuf;

#[derive(Subcommand)]
pub(super) enum FileCommand {
    /// Inspect one file without downloading it or exposing private URLs.
    Info {
        #[arg(value_parser = parse_file)]
        file: SlackFileId,
    },
    /// Upload one regular file; omit --channel to keep it private in Slack.
    Upload {
        path: PathBuf,
        #[arg(long, value_parser = parse_channel)]
        channel: Option<SlackChannelId>,
        #[command(flatten)]
        options: WriteOptions,
    },
    /// Download one Slack-hosted file to an explicit new path; never overwrite.
    Download {
        #[arg(value_parser = parse_file)]
        file: SlackFileId,
        #[arg(long, value_name = "PATH")]
        output: PathBuf,
    },
}

enum Request {
    Info(SlackFileId),
    Upload {
        upload: Upload,
        channel: Option<SlackChannelId>,
        approval: Approval,
    },
    Download {
        file: SlackFileId,
        output: PathBuf,
    },
}

pub(super) struct Prepared {
    pub profile: ProfileView,
    request: Request,
    settings: FileSettings,
}

impl FileCommand {
    pub(super) fn prepare(
        self,
        store: &Store,
        selected: Option<&str>,
        json: bool,
    ) -> Result<Prepared> {
        let profile = store.select(selected)?;
        let settings = store.settings().files.clone();
        let request = match self {
            Self::Info { file } => Request::Info(file),
            Self::Upload {
                path,
                channel,
                options,
            } => {
                let approval = options.prepare(selected.is_some(), json)?;
                Request::Upload {
                    upload: Upload::read(&path, &settings)?,
                    channel,
                    approval,
                }
            }
            Self::Download { file, output } => {
                if output.as_os_str().is_empty()
                    || output.as_os_str() == "-"
                    || output.file_name().is_none()
                {
                    return Err(Error::Arguments(
                        "Download requires an explicit destination file path, not stdout.",
                    ));
                }
                Request::Download { file, output }
            }
        };
        Ok(Prepared {
            profile,
            request,
            settings,
        })
    }
}

impl Prepared {
    pub(super) async fn execute<C: SlackClientHttpConnector + Send + Sync>(
        self,
        slack: &Slack<C>,
        credentials: &Credentials,
    ) -> Result<Output> {
        if slack.identify(credentials).await? != self.profile.identity {
            return Err(Error::IdentityMismatch);
        }
        match self.request {
            Request::Info(id) => slack.file(credentials, &id).await.map(Output::File),
            Request::Upload {
                upload,
                channel,
                approval,
            } => {
                let size = upload.content.len() as u64;
                if !approval.confirm("Upload and share this file as shown?", || {
                    output::upload_preview_text(&self.profile, &upload.name, size, channel.as_ref())
                })? {
                    return Ok(Output::UploadPreview {
                        dry_run: true,
                        profile: self.profile,
                        filename: upload.name,
                        size,
                        channel_id: channel,
                    });
                }
                let transfer = Transfer::new(&self.settings)?;
                let cancelled = super::cancellation()?;
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
            Request::Download { file, output } => {
                let file = slack.file(credentials, &file).await?;
                let url = file.download_url()?;
                let expected_size = file.size.ok_or(Error::FileUnavailable)?;
                let transfer = Transfer::new(&self.settings)?;
                let cancelled = super::cancellation()?;
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

#[cfg(test)]
mod tests;
