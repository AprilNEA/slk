use super::{Approval, Connection, Prompt};
use crate::error::Result;
use crate::model::conversations::ConversationAction;
use crate::model::pagination::CursorPage;
use crate::output::conversations::{self as output, Output};
use slack_morphism::{
    SlackChannelId, SlackClientHttpConnector, SlackConversationType, SlackUserId,
};

pub(crate) enum Request {
    Channels(CursorPage),
    Resolve {
        name: String,
        page: CursorPage,
    },
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
        Ok(match self {
            Self::Channels(page) => Output::Channels(slack.channels(credentials, &page).await?),
            Self::Resolve { name, page } => {
                let mut candidates = slack.channels(credentials, &page).await?;
                candidates.data.retain(|channel| {
                    channel
                        .name
                        .as_ref()
                        .is_some_and(|candidate| candidate.eq_ignore_ascii_case(&name))
                });
                Output::ResolvedChannels(candidates.resolve(&page))
            }
            Self::Info(channel) => Output::Info(slack.channel_info(credentials, &channel).await?),
            Self::Members { channel, page } => Output::Members {
                page: slack.members(credentials, &channel, &page).await?,
                channel_id: channel,
            },
            Self::Joined { kind, page } => {
                Output::Joined(slack.joined(credentials, &page, kind).await?)
            }
            Self::Workspace => Output::Workspace(
                slack
                    .workspace_info(credentials, &profile.identity.team_id)
                    .await?,
            ),
            Self::Create {
                name,
                is_private,
                approval,
            } => {
                if !approval.confirm(prompt, "Create this channel?", || {
                    output::create_preview(&profile, &name, is_private)
                })? {
                    Output::CreatePreview {
                        dry_run: true,
                        profile,
                        name,
                        is_private,
                    }
                } else {
                    Output::Created {
                        channel: slack.create_channel(credentials, &name, is_private).await?,
                    }
                }
            }
            Self::Change {
                channel,
                action,
                approval,
            } => {
                let before = slack.channel_info(credentials, &channel).await?;
                action.validate(&before)?;
                if !approval.confirm(prompt, "Apply this conversation change?", || {
                    output::change_preview(&profile, &before, &action)
                })? {
                    Output::ChangePreview {
                        dry_run: true,
                        profile,
                        before,
                        action,
                    }
                } else {
                    slack
                        .change_conversation(credentials, &channel, &action)
                        .await?;
                    Output::Changed {
                        channel_id: channel,
                        action,
                    }
                }
            }
            Self::OpenGroup { users, approval } => {
                let mut targets = Vec::with_capacity(users.len());
                for id in &users {
                    let target = slack.user(credentials, id).await?;
                    target.check_dm_target()?;
                    targets.push(target);
                }
                if !approval.confirm(prompt, "Open or create this group DM?", || {
                    output::group_preview(&profile, &targets)
                })? {
                    Output::GroupPreview {
                        dry_run: true,
                        profile,
                        targets,
                    }
                } else {
                    Output::GroupOpened(slack.open_group_dm(credentials, users).await?)
                }
            }
        })
    }
}
