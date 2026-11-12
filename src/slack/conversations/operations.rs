use super::{CONVERSATION_TYPES, Channel, Conversation};
use crate::auth::Credentials;
use crate::error::{Error, Result};
use crate::ids::parse_channel;
use crate::slack::{CursorPage, Page, Slack, pagination};
use serde::{Deserialize, Serialize};
use serde_json::json;
use slack_morphism::api::{
    SlackApiConversationsArchiveRequest, SlackApiConversationsCloseRequest,
    SlackApiConversationsCreateRequest, SlackApiConversationsJoinRequest,
    SlackApiConversationsLeaveRequest, SlackApiConversationsMembersResponse,
    SlackApiConversationsOpenRequest, SlackApiConversationsSetTopicRequest,
    SlackApiTeamInfoResponse,
};
use slack_morphism::{
    SlackChannelCurrentState, SlackChannelId, SlackChannelTopicInfo, SlackClientHttpConnector,
    SlackConversationType, SlackResponseMetadata, SlackTeamInfo, SlackTs, SlackUserId,
};

#[derive(Debug, Serialize)]
pub(crate) struct ChannelDetails {
    #[serde(flatten)]
    pub channel: Channel,
    pub is_member: Option<bool>,
    pub topic: Option<SlackChannelTopicInfo>,
    #[serde(flatten)]
    pub read_state: SlackChannelCurrentState,
}

#[derive(Deserialize)]
struct DetailedConversation {
    #[serde(flatten)]
    conversation: Conversation,
    topic: Option<SlackChannelTopicInfo>,
    #[serde(flatten)]
    read_state: SlackChannelCurrentState,
}

impl From<DetailedConversation> for ChannelDetails {
    fn from(value: DetailedConversation) -> Self {
        Self {
            is_member: value.conversation.flags.is_member,
            channel: value.conversation.into(),
            topic: value.topic,
            read_state: value.read_state,
        }
    }
}

#[derive(Deserialize)]
struct ChannelResponse {
    channel: DetailedConversation,
}

#[derive(Deserialize)]
struct JoinedResponse {
    channels: Vec<DetailedConversation>,
    response_metadata: Option<SlackResponseMetadata>,
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

#[derive(Deserialize)]
struct GroupResponse {
    channel: DetailedConversation,
    already_open: Option<bool>,
    no_op: Option<bool>,
}

#[derive(Debug, Serialize)]
pub(crate) struct OpenedGroup {
    pub channel: ChannelDetails,
    pub requested_users: Vec<SlackUserId>,
    pub already_open: Option<bool>,
    pub no_op: Option<bool>,
}

impl<C: SlackClientHttpConnector + Send + Sync> Slack<C> {
    pub(crate) async fn channel_info(
        &self,
        credentials: &Credentials,
        channel: &SlackChannelId,
    ) -> Result<ChannelDetails> {
        let response: ChannelResponse = self
            .get(
                credentials,
                "conversations.info",
                vec![("channel", Some(&channel.0))],
            )
            .await?;
        if response.channel.conversation.info.id != *channel {
            return Err(Error::Response);
        }
        Ok(response.channel.into())
    }

    pub(crate) async fn joined(
        &self,
        credentials: &Credentials,
        page: &CursorPage,
        kind: Option<SlackConversationType>,
    ) -> Result<Page<ChannelDetails>> {
        let types = kind
            .as_ref()
            .map_or_else(|| CONVERSATION_TYPES.to_owned(), ToString::to_string);
        let types = types.as_str();
        let kind = kind.as_ref();
        pagination::collect(page, |page| async move {
            let response: JoinedResponse = self
                .get(
                    credentials,
                    "users.conversations",
                    vec![
                        ("types", Some(types)),
                        ("limit", Some(&page.limit.to_string())),
                        ("cursor", page.cursor.as_deref()),
                    ],
                )
                .await?;
            let data: Vec<ChannelDetails> = response
                .channels
                .into_iter()
                .map(ChannelDetails::from)
                .collect();
            if kind.is_some()
                && data
                    .iter()
                    .any(|channel| channel.channel.conversation_type.as_ref() != kind)
            {
                return Err(Error::Response);
            }
            Ok(Page::cursor(data, response.response_metadata, None))
        })
        .await
    }

    pub(crate) async fn members(
        &self,
        credentials: &Credentials,
        channel: &SlackChannelId,
        page: &CursorPage,
    ) -> Result<Page<SlackUserId>> {
        pagination::collect(page, |page| async move {
            let response: SlackApiConversationsMembersResponse = self
                .get(
                    credentials,
                    "conversations.members",
                    vec![
                        ("channel", Some(&channel.0)),
                        ("limit", Some(&page.limit.to_string())),
                        ("cursor", page.cursor.as_deref()),
                    ],
                )
                .await?;
            Ok(Page::cursor(
                response.members,
                response.response_metadata,
                None,
            ))
        })
        .await
    }

    pub(crate) async fn create_channel(
        &self,
        credentials: &Credentials,
        name: &str,
        is_private: bool,
    ) -> Result<ChannelDetails> {
        let request =
            SlackApiConversationsCreateRequest::new(name.to_owned()).with_is_private(is_private);
        let response: ChannelResponse = self
            .write(credentials, "conversations.create", &request)
            .await?;
        let channel = ChannelDetails::from(response.channel);
        let expected = if is_private {
            SlackConversationType::Private
        } else {
            SlackConversationType::Public
        };
        if parse_channel(&channel.channel.channel_id.0).is_err()
            || channel.channel.conversation_type != Some(expected)
            || channel.channel.name.as_deref() != Some(name)
        {
            return Err(Error::WriteOutcomeUnknown { slack_error: None });
        }
        Ok(channel)
    }

    pub(crate) async fn change_conversation(
        &self,
        credentials: &Credentials,
        channel: &SlackChannelId,
        action: &ConversationAction,
    ) -> Result<()> {
        let acknowledged: Option<ChannelDetails> = match action {
            ConversationAction::Join => {
                let response: ChannelResponse = self
                    .write(
                        credentials,
                        "conversations.join",
                        &SlackApiConversationsJoinRequest::new(channel.clone()),
                    )
                    .await?;
                Some(response.channel.into())
            }
            ConversationAction::Topic { topic } => {
                let response: ChannelResponse = self
                    .write(
                        credentials,
                        "conversations.setTopic",
                        &SlackApiConversationsSetTopicRequest::new(channel.clone(), topic.clone()),
                    )
                    .await?;
                let details = ChannelDetails::from(response.channel);
                if details.topic.as_ref().map(|value| &value.value) != Some(topic) {
                    return Err(Error::WriteOutcomeUnknown { slack_error: None });
                }
                Some(details)
            }
            ConversationAction::ResumeGroup => {
                let request = SlackApiConversationsOpenRequest::new()
                    .with_channel(channel.clone())
                    .with_return_im(true);
                let response: ChannelResponse = self
                    .write(credentials, "conversations.open", &request)
                    .await?;
                Some(response.channel.into())
            }
            ConversationAction::Leave => {
                let _: serde_json::Value = self
                    .write(
                        credentials,
                        "conversations.leave",
                        &SlackApiConversationsLeaveRequest::new(channel.clone()),
                    )
                    .await?;
                None
            }
            ConversationAction::Archive => {
                let _: serde_json::Value = self
                    .write(
                        credentials,
                        "conversations.archive",
                        &SlackApiConversationsArchiveRequest::new(channel.clone()),
                    )
                    .await?;
                None
            }
            ConversationAction::CloseGroup => {
                let _: serde_json::Value = self
                    .write(
                        credentials,
                        "conversations.close",
                        &SlackApiConversationsCloseRequest::new(channel.clone()),
                    )
                    .await?;
                None
            }
            ConversationAction::Mark { ts } => {
                let _: serde_json::Value = self
                    .write(
                        credentials,
                        "conversations.mark",
                        &json!({"channel": channel, "ts": ts}),
                    )
                    .await?;
                None
            }
        };
        if acknowledged.is_some_and(|details| {
            details.channel.channel_id != *channel || action.validate(&details).is_err()
        }) {
            return Err(Error::WriteOutcomeUnknown { slack_error: None });
        }
        Ok(())
    }

    pub(crate) async fn open_group_dm(
        &self,
        credentials: &Credentials,
        users: Vec<SlackUserId>,
    ) -> Result<OpenedGroup> {
        let request = SlackApiConversationsOpenRequest::new()
            .with_users(users.clone())
            .with_return_im(true);
        let response: GroupResponse = self
            .write(credentials, "conversations.open", &request)
            .await?;
        let channel = ChannelDetails::from(response.channel);
        if parse_channel(&channel.channel.channel_id.0).is_err()
            || channel.channel.conversation_type != Some(SlackConversationType::Mpim)
        {
            return Err(Error::WriteOutcomeUnknown { slack_error: None });
        }
        Ok(OpenedGroup {
            channel,
            requested_users: users,
            already_open: response.already_open,
            no_op: response.no_op,
        })
    }

    pub(crate) async fn workspace_info(
        &self,
        credentials: &Credentials,
        team: &str,
    ) -> Result<SlackTeamInfo> {
        let response: SlackApiTeamInfoResponse = self
            .get(credentials, "team.info", vec![("team", Some(team))])
            .await?;
        if response.team.id.0 != team {
            return Err(Error::Response);
        }
        Ok(response.team)
    }
}
