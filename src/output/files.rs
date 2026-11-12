use super::{identity_text, terminal_text};
use crate::auth::ProfileView;
use crate::model::files::File;
use serde::Serialize;
use slack_morphism::prelude::ValueStruct;
use slack_morphism::{SlackChannelId, SlackFileId};
use std::path::PathBuf;

#[derive(Serialize)]
#[serde(untagged)]
pub(crate) enum Output {
    File(File),
    UploadPreview {
        dry_run: bool,
        #[serde(flatten)]
        profile: ProfileView,
        filename: String,
        size: u64,
        channel_id: Option<SlackChannelId>,
    },
    Uploaded {
        file: File,
        channel_id: Option<SlackChannelId>,
    },
    Downloaded {
        file_id: SlackFileId,
        path: PathBuf,
        size: u64,
    },
}

impl Output {
    pub(super) fn text(&self) -> String {
        match self {
            Self::File(file) => file_text(file),
            Self::UploadPreview {
                profile,
                filename,
                size,
                channel_id,
                ..
            } => format!(
                "Dry run; no file was uploaded.\n{}",
                upload_preview_text(profile, filename, *size, channel_id.as_ref()),
            ),
            Self::Uploaded { file, channel_id } => format!(
                "Uploaded\t{}\nConversation: {}",
                file_text(file),
                terminal_text(
                    channel_id
                        .as_ref()
                        .map_or("private (not shared)", |id| id.value())
                ),
            ),
            Self::Downloaded {
                file_id,
                path,
                size,
            } => format!(
                "Downloaded\t{}\t{size} bytes\n{}",
                terminal_text(file_id.value()),
                terminal_text(&path.to_string_lossy()),
            ),
        }
    }
}

fn file_text(file: &File) -> String {
    format!(
        "{}\t{}\t{} bytes",
        terminal_text(file.file_id.value()),
        terminal_text(
            file.title
                .as_deref()
                .or(file.name.as_deref())
                .unwrap_or("-")
        ),
        file.size
            .map_or_else(|| "unknown".to_owned(), |size| size.to_string()),
    )
}

pub(crate) fn upload_preview_text(
    profile: &ProfileView,
    name: &str,
    size: u64,
    channel: Option<&SlackChannelId>,
) -> String {
    format!(
        "{}\nFile: {}\nSize: {size} bytes\nConversation: {}",
        identity_text(profile),
        terminal_text(name),
        terminal_text(channel.map_or("private (not shared)", |id| id.value())),
    )
}
