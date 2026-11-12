use super::users::user_text;
use super::{channel_text, identity_text, page_text, terminal_text};
use crate::auth::ProfileView;
use crate::model::users::User;
use crate::slack::{ChannelDetails, ConversationAction, OpenedGroup, Page};
use serde::Serialize;
use slack_morphism::{SlackChannelId, SlackTeamInfo, SlackUserId};

#[derive(Serialize)]
#[serde(untagged)]
pub(crate) enum Output {
    Info(ChannelDetails),
    Members {
        channel_id: SlackChannelId,
        #[serde(flatten)]
        page: Page<SlackUserId>,
    },
    Joined(Page<ChannelDetails>),
    Workspace(SlackTeamInfo),
    CreatePreview {
        dry_run: bool,
        #[serde(flatten)]
        profile: ProfileView,
        name: String,
        is_private: bool,
    },
    Created {
        channel: ChannelDetails,
    },
    ChangePreview {
        dry_run: bool,
        #[serde(flatten)]
        profile: ProfileView,
        before: ChannelDetails,
        #[serde(flatten)]
        action: ConversationAction,
    },
    Changed {
        channel_id: SlackChannelId,
        #[serde(flatten)]
        action: ConversationAction,
    },
    GroupPreview {
        dry_run: bool,
        #[serde(flatten)]
        profile: ProfileView,
        targets: Vec<User>,
    },
    GroupOpened(OpenedGroup),
}

impl Output {
    pub(super) fn text(&self) -> String {
        match self {
            Self::Info(channel) => details_text(channel),
            Self::Members { channel_id, page } => format!(
                "Conversation: {}\n{}",
                terminal_text(&channel_id.0),
                page_text(
                    page.data
                        .iter()
                        .map(|user| terminal_text(&user.0))
                        .collect(),
                    &page.pagination
                )
            ),
            Self::Joined(page) => format!(
                "Server-provided read state only; missing counts are unknown, not zero. Not Slack All unreads.\n{}",
                page_text(
                    page.data.iter().map(details_text).collect(),
                    &page.pagination
                )
            ),
            Self::Workspace(team) => format!(
                "{}\t{}\t{}\nRead-only workspace information.",
                terminal_text(&team.id.0),
                terminal_text(team.name.as_deref().unwrap_or("-")),
                terminal_text(team.domain.as_deref().unwrap_or("-"))
            ),
            Self::CreatePreview {
                profile,
                name,
                is_private,
                ..
            } => format!(
                "Dry run; no channel was created.\n{}",
                create_preview(profile, name, *is_private)
            ),
            Self::Created { channel } => format!("Created channel\n{}", details_text(channel)),
            Self::ChangePreview {
                profile,
                before,
                action,
                ..
            } => format!(
                "Dry run; no conversation change was made.\n{}",
                change_preview(profile, before, action)
            ),
            Self::Changed { channel_id, action } => format!(
                "Acknowledged\t{}\n{}",
                terminal_text(&channel_id.0),
                action_text(action)
            ),
            Self::GroupPreview {
                profile, targets, ..
            } => format!(
                "Dry run; no group DM was opened or created.\n{}",
                group_preview(profile, targets)
            ),
            Self::GroupOpened(group) => format!(
                "Opened or created group DM\n{}\nalready_open:{}\tno_op:{}\nNo message was sent.",
                details_text(&group.channel),
                group
                    .already_open
                    .map_or_else(|| "unknown".to_owned(), |value| value.to_string()),
                group
                    .no_op
                    .map_or_else(|| "unknown".to_owned(), |value| value.to_string())
            ),
        }
    }
}

fn details_text(details: &ChannelDetails) -> String {
    format!(
        "{}\nTopic: {}\nLast read: {}\nUnread count: {}\nUnread display count: {}",
        channel_text(&details.channel),
        terminal_text(
            details
                .topic
                .as_ref()
                .map_or("unknown", |topic| &topic.value)
        ),
        terminal_text(
            details
                .read_state
                .last_read
                .as_ref()
                .map_or("unknown", |ts| &ts.0)
        ),
        details
            .read_state
            .unread_count
            .map_or_else(|| "unknown".to_owned(), |count| count.to_string()),
        details
            .read_state
            .unread_count_display
            .map_or_else(|| "unknown".to_owned(), |count| count.to_string()),
    )
}

pub(crate) fn create_preview(profile: &ProfileView, name: &str, is_private: bool) -> String {
    format!(
        "{}\nAction: create channel\nName: {}\nVisibility: {}\nCreation has no automatic rollback.",
        identity_text(profile),
        terminal_text(name),
        if is_private { "private" } else { "public" }
    )
}

pub(crate) fn change_preview(
    profile: &ProfileView,
    before: &ChannelDetails,
    action: &ConversationAction,
) -> String {
    format!(
        "{}\nCurrent conversation:\n{}\n{}",
        identity_text(profile),
        details_text(before),
        action_text(action)
    )
}

pub(crate) fn group_preview(profile: &ProfileView, targets: &[User]) -> String {
    format!(
        "{}\nAction: open or create a group DM with these other users\n{}\nSlack includes you automatically. No message will be sent.",
        identity_text(profile),
        targets.iter().map(user_text).collect::<Vec<_>>().join("\n")
    )
}

fn action_text(action: &ConversationAction) -> String {
    match action {
        ConversationAction::Join => "Action: join this public channel".to_owned(),
        ConversationAction::Leave => "Action: leave this channel. A private channel requires a new invitation to rejoin.".to_owned(),
        ConversationAction::Archive => "Action: archive this channel for its members; do not delete history".to_owned(),
        ConversationAction::Topic { topic } => format!("Action: replace channel topic\nNew topic: {}", terminal_text(topic)),
        ConversationAction::Mark { ts } => format!("Action: set your Slack read cursor to {} across clients. This is not client-level Mark unread.", terminal_text(&ts.0)),
        ConversationAction::ResumeGroup => "Action: resume this existing group DM; do not send a message".to_owned(),
        ConversationAction::CloseGroup => "Action: close your view of this group DM; do not delete history or remove other members".to_owned(),
    }
}
