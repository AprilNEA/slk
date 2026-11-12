use super::{Paging, write::WriteOptions};
use crate::app::{Approval, Connection, Prompt};
use crate::auth::{ProfileView, Store};
use crate::error::{Error, Result};
use crate::ids::{parse_channel, parse_timestamp, parse_user};
use crate::output::{Output, conversations as output};
use crate::slack::{ConversationAction, CursorPage};
use clap::{Args, Subcommand, ValueEnum};
use slack_morphism::{
    SlackChannelId, SlackClientHttpConnector, SlackConversationType, SlackTs, SlackUserId,
};
use std::collections::HashSet;

#[derive(Subcommand)]
pub(super) enum ConversationCommand {
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

enum Request {
    Info(SlackChannelId),
    Members {
        channel: SlackChannelId,
        page: CursorPage,
    },
    Joined {
        kind: Option<SlackConversationType>,
        page: CursorPage,
    },
    Workspace,
    Create {
        name: String,
        is_private: bool,
        approval: Approval,
    },
    Change {
        channel: SlackChannelId,
        action: ConversationAction,
        approval: Approval,
    },
    OpenGroup {
        users: Vec<SlackUserId>,
        approval: Approval,
    },
}

pub(super) struct Prepared {
    request: Request,
}

impl ConversationCommand {
    pub(super) fn prepare(
        self,
        store: &Store,
        selected: Option<&str>,
        json: bool,
    ) -> Result<(ProfileView, Prepared)> {
        let profile = store.select(selected)?;
        let change = |target: ChangeTarget, action| -> Result<Request> {
            Ok(Request::Change {
                channel: target.conversation,
                action,
                approval: target.options.prepare(selected.is_some(), json)?,
            })
        };
        let request = match self {
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
                    if name.is_empty()
                        || name.len() > 80
                        || !name.bytes().all(|byte| {
                            byte.is_ascii_lowercase()
                                || byte.is_ascii_digit()
                                || matches!(byte, b'_' | b'-')
                        })
                    {
                        return Err(Error::Arguments(
                            "Use a channel name of 1–80 lowercase ASCII letters, digits, underscores, or hyphens. Names are not silently normalized.",
                        ));
                    }
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
                    if topic.chars().count() > 250 {
                        return Err(Error::Arguments(
                            "Slack channel topics are limited to 250 Unicode characters.",
                        ));
                    }
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
                    if users.iter().collect::<HashSet<_>>().len() != users.len()
                        || users.iter().any(|user| user.0 == profile.identity.user_id)
                    {
                        return Err(Error::Arguments(
                            "Group DM members must be distinct other users. Slack includes the selected identity automatically.",
                        ));
                    }
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
        Ok((profile, Prepared { request }))
    }
}

impl Prepared {
    pub(super) async fn execute<C: SlackClientHttpConnector + Send + Sync>(
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
        let result = match self.request {
            Request::Info(channel) => {
                output::Output::Info(slack.channel_info(credentials, &channel).await?)
            }
            Request::Members { channel, page } => output::Output::Members {
                page: slack.members(credentials, &channel, &page).await?,
                channel_id: channel,
            },
            Request::Joined { kind, page } => {
                output::Output::Joined(slack.joined(credentials, &page, kind).await?)
            }
            Request::Workspace => output::Output::Workspace(
                slack
                    .workspace_info(credentials, &profile.identity.team_id)
                    .await?,
            ),
            Request::Create {
                name,
                is_private,
                approval,
            } => {
                if !approval.confirm(prompt, "Create this channel?", || {
                    output::create_preview(&profile, &name, is_private)
                })? {
                    output::Output::CreatePreview {
                        dry_run: true,
                        profile,
                        name,
                        is_private,
                    }
                } else {
                    output::Output::Created {
                        channel: slack.create_channel(credentials, &name, is_private).await?,
                    }
                }
            }
            Request::Change {
                channel,
                action,
                approval,
            } => {
                let before = slack.channel_info(credentials, &channel).await?;
                action.validate(&before)?;
                if !approval.confirm(prompt, "Apply this conversation change?", || {
                    output::change_preview(&profile, &before, &action)
                })? {
                    output::Output::ChangePreview {
                        dry_run: true,
                        profile,
                        before,
                        action,
                    }
                } else {
                    slack
                        .change_conversation(credentials, &channel, &action)
                        .await?;
                    output::Output::Changed {
                        channel_id: channel,
                        action,
                    }
                }
            }
            Request::OpenGroup { users, approval } => {
                let mut targets = Vec::with_capacity(users.len());
                for id in &users {
                    let target = slack.user(credentials, id).await?;
                    target.check_dm_target()?;
                    targets.push(target);
                }
                if !approval.confirm(prompt, "Open or create this group DM?", || {
                    output::group_preview(&profile, &targets)
                })? {
                    output::Output::GroupPreview {
                        dry_run: true,
                        profile,
                        targets,
                    }
                } else {
                    output::Output::GroupOpened(slack.open_group_dm(credentials, users).await?)
                }
            }
        };
        Ok(Output::Conversations(result))
    }
}

#[cfg(test)]
mod tests;
