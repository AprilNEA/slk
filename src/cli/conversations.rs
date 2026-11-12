use super::{Effect, FilePath, Paging, write::WriteOptions};
use crate::app::{self, conversations::Request};
use crate::auth::{ProfileView, Store};
use crate::error::Result;
use crate::ids::{parse_channel, parse_timestamp, parse_user};
use crate::model::conversations::{
    ConversationAction, check_channel_name, check_group_members, check_topic,
};
use clap::{Args, Subcommand, ValueEnum};
use slack_morphism::{SlackChannelId, SlackConversationType, SlackTs, SlackUserId};

#[derive(Subcommand)]
pub(super) enum ConversationCommand {
    /// List visible conversations without joining them; one page by default.
    Channels(Paging),
    /// Inspect conversations or explicitly change a channel or read cursor.
    Channel {
        #[command(subcommand)]
        command: ChannelCommand,
    },
    /// List, open, resume, or close group DMs without sending messages.
    GroupDm {
        #[command(subcommand)]
        command: GroupCommand,
    },
    /// List joined conversations' server-provided read state, not All unreads.
    Unread(Paging),
    /// Read the selected workspace's information; no Enterprise administration.
    Workspace,
}

#[derive(Subcommand)]
pub(super) enum ChannelCommand {
    /// Inspect one conversation, preserving unknown read-state fields.
    Info {
        #[arg(value_parser = parse_channel)]
        conversation: SlackChannelId,
    },
    /// List member IDs in a channel or group DM; one page by default.
    Members {
        #[arg(value_parser = parse_channel)]
        conversation: SlackChannelId,
        #[command(flatten)]
        paging: Paging,
    },
    /// Create a channel with explicitly selected visibility.
    Create {
        name: String,
        #[arg(long, value_enum)]
        visibility: Visibility,
        #[command(flatten)]
        options: WriteOptions,
    },
    /// Join a public channel; never enter a private channel without an invitation.
    Join(ChangeTarget),
    /// Leave a channel; private channels require a new invitation to rejoin.
    Leave(ChangeTarget),
    /// Archive a channel for its members, without deleting history.
    Archive(ChangeTarget),
    /// Set plain-text channel topic; an empty string clears the topic.
    Topic {
        #[command(flatten)]
        target: ChangeTarget,
        topic: String,
    },
    /// Explicitly set your read cursor to this timestamp; not client-level Mark unread.
    Mark {
        #[command(flatten)]
        target: ChangeTarget,
        #[arg(value_parser = parse_timestamp)]
        ts: SlackTs,
    },
}

#[derive(Clone, Copy, ValueEnum)]
pub(super) enum Visibility {
    Public,
    Private,
}

#[derive(Args)]
pub(super) struct ChangeTarget {
    #[arg(value_parser = parse_channel)]
    conversation: SlackChannelId,
    #[command(flatten)]
    options: WriteOptions,
}

#[derive(Subcommand)]
pub(super) enum GroupCommand {
    /// List your existing group DMs without opening them; one page by default.
    List(Paging),
    /// Open or create a DM with 2–8 distinct other human users; omit yourself.
    Open {
        #[arg(value_parser = parse_user, num_args = 2..=8, required = true)]
        users: Vec<SlackUserId>,
        #[command(flatten)]
        options: WriteOptions,
    },
    /// Resume an explicitly identified existing group DM.
    Resume(ChangeTarget),
    /// Close your view of a group DM; do not delete history or remove other members.
    Close(ChangeTarget),
}

impl ConversationCommand {
    pub(super) fn effect(&self) -> Effect {
        match self {
            Self::Channels(_) | Self::Unread(_) | Self::Workspace => Effect::Read,
            Self::Channel { command } => match command {
                ChannelCommand::Info { .. } | ChannelCommand::Members { .. } => Effect::Read,
                ChannelCommand::Create { options, .. } => options.effect(),
                ChannelCommand::Join(target)
                | ChannelCommand::Leave(target)
                | ChannelCommand::Archive(target)
                | ChannelCommand::Topic { target, .. }
                | ChannelCommand::Mark { target, .. } => target.options.effect(),
            },
            Self::GroupDm { command } => match command {
                GroupCommand::List(_) => Effect::Read,
                GroupCommand::Open { options, .. } => options.effect(),
                GroupCommand::Resume(target) | GroupCommand::Close(target) => {
                    target.options.effect()
                }
            },
        }
    }

    pub(super) fn file_paths(&mut self) -> Vec<FilePath<'_>> {
        match self {
            Self::Channels(_) | Self::Unread(_) | Self::Workspace => Vec::new(),
            Self::Channel { command } => match command {
                ChannelCommand::Info { .. }
                | ChannelCommand::Members { .. }
                | ChannelCommand::Create { .. }
                | ChannelCommand::Join(_)
                | ChannelCommand::Leave(_)
                | ChannelCommand::Archive(_)
                | ChannelCommand::Topic { .. }
                | ChannelCommand::Mark { .. } => Vec::new(),
            },
            Self::GroupDm { command } => match command {
                GroupCommand::List(_)
                | GroupCommand::Open { .. }
                | GroupCommand::Resume(_)
                | GroupCommand::Close(_) => Vec::new(),
            },
        }
    }

    pub(super) fn prepare(
        self,
        store: &Store,
        selected: Option<&str>,
        json: bool,
    ) -> Result<(ProfileView, app::Request)> {
        let profile = store.select(selected)?;
        let change = |target: ChangeTarget, action| -> Result<Request> {
            Ok(Request::Change {
                channel: target.conversation,
                action,
                approval: target.options.prepare(selected.is_some(), json)?,
            })
        };
        let request = match self {
            Self::Channels(paging) => Request::Channels(paging.resolve(store.settings())),
            Self::Channel { command } => match command {
                ChannelCommand::Info { conversation } => Request::Info(conversation),
                ChannelCommand::Members {
                    conversation,
                    paging,
                } => Request::Members {
                    channel: conversation,
                    page: paging.resolve(store.settings()),
                },
                ChannelCommand::Create {
                    name,
                    visibility,
                    options,
                } => {
                    check_channel_name(&name)?;
                    Request::Create {
                        name,
                        is_private: matches!(visibility, Visibility::Private),
                        approval: options.prepare(selected.is_some(), json)?,
                    }
                }
                ChannelCommand::Join(target) => change(target, ConversationAction::Join)?,
                ChannelCommand::Leave(target) => change(target, ConversationAction::Leave)?,
                ChannelCommand::Archive(target) => change(target, ConversationAction::Archive)?,
                ChannelCommand::Topic { target, topic } => {
                    check_topic(&topic)?;
                    change(target, ConversationAction::Topic { topic })?
                }
                ChannelCommand::Mark { target, ts } => {
                    change(target, ConversationAction::Mark { ts })?
                }
            },
            Self::GroupDm { command } => match command {
                GroupCommand::List(paging) => Request::Joined {
                    kind: Some(SlackConversationType::Mpim),
                    page: paging.resolve(store.settings()),
                },
                GroupCommand::Resume(target) => change(target, ConversationAction::ResumeGroup)?,
                GroupCommand::Close(target) => change(target, ConversationAction::CloseGroup)?,
                GroupCommand::Open { users, options } => {
                    check_group_members(&users, &profile.identity.user_id)?;
                    Request::OpenGroup {
                        users,
                        approval: options.prepare(selected.is_some(), json)?,
                    }
                }
            },
            Self::Unread(paging) => Request::Joined {
                kind: None,
                page: paging.resolve(store.settings()),
            },
            Self::Workspace => Request::Workspace,
        };
        Ok((profile, app::Request::Conversations(request)))
    }
}

#[cfg(test)]
mod tests;
