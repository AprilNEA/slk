use crate::error::{Error, Result};
use serde::Serialize;
use slack_morphism::{
    SlackChannelCurrentState, SlackChannelId, SlackChannelTopicInfo, SlackConversationType,
    SlackTs, SlackUserId,
};
use std::collections::HashSet;

#[derive(Debug, Serialize)]
pub(crate) struct Channel {
    pub channel_id: SlackChannelId,
    pub name: Option<String>,
    pub conversation_type: Option<SlackConversationType>,
    pub user_id: Option<String>,
    pub is_archived: bool,
}

#[derive(Debug, Serialize)]
pub(crate) struct ChannelDetails {
    #[serde(flatten)]
    pub channel: Channel,
    pub is_member: Option<bool>,
    pub topic: Option<SlackChannelTopicInfo>,
    #[serde(flatten)]
    pub read_state: SlackChannelCurrentState,
}

#[derive(Debug, Serialize)]
#[serde(tag = "action", rename_all = "snake_case")]
pub(crate) enum ConversationAction {
    Join,
    Leave,
    Archive,
    Topic { topic: String },
    Mark { ts: SlackTs },
    ResumeGroup,
    CloseGroup,
}

impl ConversationAction {
    pub(crate) fn validate(&self, details: &ChannelDetails) -> Result<()> {
        let kind = details.channel.conversation_type.as_ref();
        let allowed = match self {
            Self::Mark { .. } => true,
            Self::ResumeGroup | Self::CloseGroup => kind == Some(&SlackConversationType::Mpim),
            Self::Join => kind == Some(&SlackConversationType::Public),
            Self::Leave | Self::Archive | Self::Topic { .. } => {
                matches!(
                    kind,
                    Some(SlackConversationType::Public | SlackConversationType::Private)
                )
            }
        };
        if !allowed {
            return Err(Error::Arguments(
                "The conversation type does not support this command. No change was attempted.",
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Serialize)]
pub(crate) struct OpenedGroup {
    pub channel: ChannelDetails,
    pub requested_users: Vec<SlackUserId>,
    pub already_open: Option<bool>,
    pub no_op: Option<bool>,
}

/// Rejects a channel name that Slack would normalize or refuse.
pub(crate) fn check_channel_name(name: &str) -> Result<()> {
    if name.is_empty()
        || name.len() > 80
        || !name.bytes().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'_' | b'-')
        })
    {
        return Err(Error::Arguments(
            "Use a channel name of 1–80 lowercase ASCII letters, digits, underscores, or hyphens. Names are not silently normalized.",
        ));
    }
    Ok(())
}

pub(crate) fn check_topic(topic: &str) -> Result<()> {
    if topic.chars().count() > 250 {
        return Err(Error::Arguments(
            "Slack channel topics are limited to 250 Unicode characters.",
        ));
    }
    Ok(())
}

/// Rejects repeated members and the identity that opens the group DM.
pub(crate) fn check_group_members(users: &[SlackUserId], own_user_id: &str) -> Result<()> {
    if users.iter().collect::<HashSet<_>>().len() != users.len()
        || users.iter().any(|user| user.0 == own_user_id)
    {
        return Err(Error::Arguments(
            "Group DM members must be distinct other users. Slack includes the selected identity automatically.",
        ));
    }
    Ok(())
}
