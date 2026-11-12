use super::write::WriteOptions;
use super::{Effect, FilePath};
use crate::app::{self, files::Request};
use crate::auth::{ProfileView, Store};
use crate::error::{Error, Result};
use crate::ids::{parse_channel, parse_file};
use crate::model::files::Upload;
use clap::Subcommand;
use slack_morphism::{SlackChannelId, SlackFileId};
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

impl FileCommand {
    pub(super) fn effect(&self) -> Effect {
        match self {
            Self::Info { .. } => Effect::Read,
            Self::Upload { .. } => Effect::Write,
            Self::Download { .. } => Effect::Download,
        }
    }

    pub(super) fn file_paths(&mut self) -> Vec<FilePath<'_>> {
        match self {
            Self::Info { .. } => Vec::new(),
            Self::Upload { path, .. } => vec![FilePath::Upload(path)],
            Self::Download { output, .. } => vec![FilePath::Download(output)],
        }
    }

    pub(super) fn prepare(
        self,
        store: &Store,
        selected: Option<&str>,
        json: bool,
    ) -> Result<(ProfileView, app::Request)> {
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
                    settings,
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
                Request::Download {
                    file,
                    output,
                    settings,
                }
            }
        };
        Ok((profile, app::Request::Files(request)))
    }
}

#[cfg(test)]
mod tests;
