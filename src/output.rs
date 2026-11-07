use crate::auth::{Kind, ProfileView};
use crate::error::{Error, Result};
use crate::files::File;
use crate::messages::{Change, Changed, Draft, Message, Sent};
use crate::scheduled::{Receipt, Scheduled};
use crate::slack::{Channel, Page, Pagination, Resolution, ResolutionStatus};
use crate::users::{OpenedDm, User};
use serde::Serialize;
use slack_morphism::prelude::ValueStruct;
use slack_morphism::{SlackChannelId, SlackFileId};
use std::io::{self, Write};
use std::path::PathBuf;

pub(crate) mod canvas;
pub(crate) mod conversations;
pub(crate) mod messages;
mod scheduled;
use messages::{batch_preview_text, change_preview_text, change_text, message_text, preview_text};
pub(crate) use scheduled::{cancel_preview_text, schedule_preview_text};

#[derive(Serialize)]
#[serde(untagged)]
pub(crate) enum Output {
    Update(crate::update::Report),
    Profile(ProfileView),
    Profiles {
        data: Vec<ProfileView>,
    },
    LoggedOut {
        profile: String,
        local_credentials_removed: bool,
    },
    Channels(Page<Channel>),
    Messages(Page<Message>),
    Message(Message),
    Pins {
        channel_id: SlackChannelId,
        data: Vec<serde_json::Value>,
    },
    Users(Page<User>),
    User(User),
    ResolvedUsers(Resolution<User>),
    ResolvedChannels(Resolution<Channel>),
    DmPreview {
        dry_run: bool,
        #[serde(flatten)]
        profile: ProfileView,
        target: User,
    },
    DmOpened(OpenedDm),
    Preview {
        dry_run: bool,
        #[serde(flatten)]
        profile: ProfileView,
        #[serde(flatten)]
        draft: Draft,
    },
    Sent(Sent),
    BatchPreview {
        dry_run: bool,
        #[serde(flatten)]
        profile: ProfileView,
        drafts: Vec<Draft>,
    },
    SentBatch {
        data: Vec<Sent>,
    },
    ChangePreview {
        dry_run: bool,
        #[serde(flatten)]
        profile: ProfileView,
        message: Message,
        #[serde(flatten)]
        change: Change,
    },
    Changed(Changed),
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
    Scheduled(Receipt),
    Schedules(Page<Scheduled>),
    SchedulePreview {
        dry_run: bool,
        #[serde(flatten)]
        profile: ProfileView,
        #[serde(flatten)]
        draft: Draft,
        post_at: i64,
    },
    ScheduleCancellationPreview {
        dry_run: bool,
        #[serde(flatten)]
        profile: ProfileView,
        scheduled: Scheduled,
    },
    ScheduleCancelled {
        cancelled: Receipt,
    },
    Conversations(conversations::Output),
    Canvas(canvas::Output),
}

impl Output {
    pub(crate) fn write(&self, json: bool, mut writer: impl Write) -> Result<()> {
        let rendered = if json {
            serde_json::to_string(self)
                .map_err(|error| Error::io("encode command output", error.into()))?
        } else {
            match self {
                Self::Update(report) => format!(
                    "Update status: {:?}\nRunning version: {}\nGitHub latest: {}\nTarget: {}\nArchive: {}",
                    report.state,
                    terminal_text(&report.current_version),
                    terminal_text(&report.latest_version),
                    terminal_text(&report.target),
                    terminal_text(report.asset.as_deref().unwrap_or("-")),
                ),
                Self::Profile(profile) => profile_text(profile),
                Self::Profiles { data } if data.is_empty() => "No profiles configured.".to_owned(),
                Self::Profiles { data } => {
                    data.iter().map(profile_text).collect::<Vec<_>>().join("\n")
                }
                Self::LoggedOut { profile, .. } => format!(
                    "Removed local credentials for {}. The Slack session was not revoked.",
                    terminal_text(profile)
                ),
                Self::Channels(page) => {
                    let rows = page.data.iter().map(channel_text).collect();
                    page_text(rows, &page.pagination)
                }
                Self::Users(page) => {
                    page_text(page.data.iter().map(user_text).collect(), &page.pagination)
                }
                Self::User(user) => user_text(user),
                Self::ResolvedUsers(result) => resolution_text(result, user_text),
                Self::ResolvedChannels(result) => resolution_text(result, channel_text),
                Self::DmPreview {
                    profile, target, ..
                } => format!(
                    "Dry run; no conversation was opened or created.\n{}",
                    dm_preview_text(profile, target)
                ),
                Self::DmOpened(opened) => format!(
                    "Opened DM\t{}\t{}\talready_open:{}\tno_op:{}\nNo message was sent.",
                    terminal_text(opened.channel_id.value()),
                    terminal_text(opened.user_id.value()),
                    opened
                        .already_open
                        .map_or_else(|| "unknown".to_owned(), |value| value.to_string()),
                    opened
                        .no_op
                        .map_or_else(|| "unknown".to_owned(), |value| value.to_string()),
                ),
                Self::Messages(page) => {
                    let rows = page.data.iter().map(message_text).collect();
                    page_text(rows, &page.pagination)
                }
                Self::Message(message) => message_text(message),
                Self::Pins { channel_id, data } => format!(
                    "Channel pins: {}\n{}",
                    terminal_text(channel_id.value()),
                    if data.is_empty() {
                        "No pins.".to_owned()
                    } else {
                        data.iter()
                            .map(|item| terminal_text(&item.to_string()))
                            .collect::<Vec<_>>()
                            .join("\n")
                    },
                ),
                Self::Preview { profile, draft, .. } => {
                    format!(
                        "Dry run; no message was sent.\n{}",
                        preview_text(profile, draft)
                    )
                }
                Self::Sent(sent) => format!(
                    "Sent\t{}\t{}\tthread:{}",
                    terminal_text(sent.channel_id.value()),
                    terminal_text(sent.ts.value()),
                    terminal_text(sent.thread_ts.as_ref().map_or("-", |ts| ts.value()))
                ),
                Self::BatchPreview {
                    profile, drafts, ..
                } => format!(
                    "Dry run; no messages were sent.\n{}",
                    batch_preview_text(profile, drafts),
                ),
                Self::SentBatch { data } => format!(
                    "Sent {} messages in order.\n{}",
                    data.len(),
                    terminal_text(
                        &serde_json::to_string(data)
                            .expect("receipts contain serializable strings")
                    )
                ),
                Self::ChangePreview {
                    profile,
                    message,
                    change,
                    ..
                } => format!(
                    "Dry run; no change was made.\n{}",
                    change_preview_text(profile, message, change)
                ),
                Self::Changed(result) => format!(
                    "Applied\t{}\t{}\n{}",
                    terminal_text(result.channel_id.value()),
                    terminal_text(result.ts.value()),
                    change_text(&result.change),
                ),
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
                Self::Scheduled(receipt) => {
                    format!("Scheduled on Slack\t{}", scheduled::receipt_text(receipt))
                }
                Self::Schedules(page) => page_text(
                    page.data.iter().map(scheduled::scheduled_text).collect(),
                    &page.pagination,
                ),
                Self::SchedulePreview {
                    profile,
                    draft,
                    post_at,
                    ..
                } => format!(
                    "Dry run; no message was scheduled.\n{}",
                    schedule_preview_text(profile, draft, *post_at)
                ),
                Self::ScheduleCancellationPreview {
                    profile, scheduled, ..
                } => format!(
                    "Dry run; no schedule was cancelled.\n{}",
                    cancel_preview_text(profile, scheduled)
                ),
                Self::ScheduleCancelled { cancelled } => {
                    format!("Cancelled schedule\t{}", scheduled::receipt_text(cancelled))
                }
                Self::Conversations(result) => result.text(),
                Self::Canvas(result) => result.text(),
            }
        };
        writeln!(writer, "{rendered}").map_err(|error| Error::io("write command output", error))
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

fn channel_text(channel: &Channel) -> String {
    format!(
        "{}\t{}\t{}{}",
        terminal_text(channel.channel_id.value()),
        channel
            .conversation_type
            .as_ref()
            .map_or_else(|| "unknown".to_owned(), ToString::to_string),
        terminal_text(
            channel
                .name
                .as_deref()
                .or(channel.user_id.as_deref())
                .unwrap_or("-")
        ),
        if channel.is_archived {
            " (archived)"
        } else {
            ""
        },
    )
}

fn user_text(user: &User) -> String {
    let profile = user.profile.as_ref();
    format!(
        "{}\t{}\t{}\t{}\t{}{}{}",
        terminal_text(user.user_id.value()),
        terminal_text(user.name.as_deref().unwrap_or("-")),
        terminal_text(
            profile
                .and_then(|value| value.display_name.as_deref())
                .unwrap_or("-")
        ),
        terminal_text(
            user.real_name
                .as_deref()
                .or(profile.and_then(|value| value.real_name.as_deref()))
                .unwrap_or("-")
        ),
        terminal_text(
            profile
                .and_then(|value| value.email.as_deref())
                .unwrap_or("-")
        ),
        if user.deleted { " (deactivated)" } else { "" },
        if user.is_bot || user.is_app_user {
            " (app)"
        } else {
            ""
        },
    )
}

fn resolution_text<T>(resolution: &Resolution<T>, row: impl Fn(&T) -> String) -> String {
    let status = match resolution.status {
        ResolutionStatus::Unique => "One exact match. Use its explicit ID in the next command.",
        ResolutionStatus::Ambiguous => {
            "Ambiguous name. Choose an explicit ID; no target was selected."
        }
        ResolutionStatus::NotFound => "No exact match.",
        ResolutionStatus::Incomplete => {
            "Incomplete candidate set. This page cannot establish a unique match."
        }
    };
    format!(
        "{status}\n{}",
        page_text(
            resolution.candidates.data.iter().map(row).collect(),
            &resolution.candidates.pagination
        )
    )
}

fn page_text(rows: Vec<String>, pagination: &Pagination) -> String {
    let mut text = if rows.is_empty() {
        "No results on this page.".to_owned()
    } else {
        rows.join("\n\n")
    };
    match pagination {
        Pagination::Cursor {
            next_cursor: Some(cursor),
            ..
        } => text.push_str(&format!("\n\nNext cursor: {}", terminal_text(cursor))),
        Pagination::Cursor {
            has_more: Some(true),
            ..
        } => text.push_str("\n\nSlack reports more results but provided no cursor."),
        Pagination::Number {
            next_page: Some(page),
            ..
        } => text.push_str(&format!("\n\nNext page: {page}")),
        Pagination::Number { has_more: true, .. } => {
            text.push_str("\n\nMore results exceed the legacy search page limit. Narrow the query.")
        }
        _ => (),
    }
    text
}

pub(crate) fn dm_preview_text(profile: &ProfileView, user: &User) -> String {
    format!(
        "{}\nAction: open or create a single-person DM\nTarget: {}\nNo message will be sent.",
        identity_text(profile),
        user_text(user),
    )
}

fn identity_text(profile: &ProfileView) -> String {
    format!(
        "Profile: {}\nWorkspace: {} ({})\nUser: {}",
        terminal_text(&profile.profile),
        profile.identity.workspace_url,
        terminal_text(&profile.identity.team_id),
        terminal_text(&profile.identity.user_id),
    )
}

fn profile_text(profile: &ProfileView) -> String {
    let kind = match profile.kind {
        Kind::User => "user",
        Kind::Browser => "browser (experimental)",
        Kind::Bot => "bot",
    };
    format!(
        "{}\t{}\t{}\t{}\t{}",
        terminal_text(&profile.profile),
        kind,
        profile.identity.workspace_url,
        terminal_text(&profile.identity.team_id),
        terminal_text(&profile.identity.user_id)
    )
}

pub(crate) fn write_error(error: &Error, json: bool) -> io::Result<()> {
    let report = error.report();
    let rendered = if json {
        serde_json::to_string(&report)?
    } else {
        let mut message = format!("{}: {}", report.code, terminal_text(&report.message));
        if let Some(code) = report.slack_error {
            message.push_str(&format!(" Slack error: {}.", terminal_text(code)));
        }
        if let Some(seconds) = report.retry_after {
            message.push_str(&format!(" Retry after {seconds} seconds."));
        }
        if let Some(batch) = report.batch {
            message.push_str(&format!(
                "\nConfirmed receipts: {}\nFailed chunk: {}: {}",
                terminal_text(&serde_json::to_string(batch.sent)?),
                batch.failure.code,
                terminal_text(&batch.failure.message),
            ));
            if let Some(code) = batch.failure.slack_error {
                message.push_str(&format!(" Slack error: {}.", terminal_text(code)));
            }
            if let Some(seconds) = batch.failure.retry_after {
                message.push_str(&format!(" Retry after {seconds} seconds."));
            }
        }
        message
    };
    writeln!(io::stderr().lock(), "{rendered}")
}

pub(crate) fn terminal_text(text: &str) -> String {
    #[derive(Default)]
    struct Text(String);

    impl vte::Perform for Text {
        fn print(&mut self, character: char) {
            if !character.is_control() {
                self.0.push(character);
            }
        }

        fn execute(&mut self, byte: u8) {
            if matches!(byte, b'\n' | b'\t') {
                self.0.push(char::from(byte));
            }
        }
    }

    let mut rendered = Text::default();
    vte::Parser::new().advance(&mut rendered, text.as_bytes());
    rendered.0
}

#[cfg(test)]
mod tests;
