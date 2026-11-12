use super::{CursorPage, Page, Slack, pagination};
use crate::auth::{Credentials, Kind};
use crate::error::{Error, Result};
use crate::model::conversations::Channel;
use crate::model::messages::{Message, MessageLink, MessageTarget};
use serde::Deserialize;
use serde_json::Value;
use slack_morphism::prelude::ValueStruct;
use slack_morphism::{
    SlackBasicChannelInfo, SlackChannelFlags, SlackChannelId, SlackClientHttpConnector,
    SlackConversationType, SlackResponseMetadata, SlackTs,
};

mod operations;

pub(super) const CONVERSATION_TYPES: &str = "public_channel,private_channel,mpim,im";

#[derive(Deserialize)]
struct Conversation {
    #[serde(flatten)]
    info: SlackBasicChannelInfo,
    #[serde(flatten)]
    flags: SlackChannelFlags,
    user: Option<String>,
}

impl From<Conversation> for Channel {
    fn from(channel: Conversation) -> Self {
        let flags = channel.flags;
        let conversation_type = if flags.is_im == Some(true) {
            Some(SlackConversationType::Im)
        } else if flags.is_mpim == Some(true) {
            Some(SlackConversationType::Mpim)
        } else if flags.is_private == Some(true) || flags.is_group == Some(true) {
            Some(SlackConversationType::Private)
        } else if flags.is_channel == Some(true) {
            Some(SlackConversationType::Public)
        } else {
            None
        };
        Self {
            channel_id: channel.info.id,
            name: channel.info.name,
            conversation_type,
            user_id: channel.user,
            is_archived: flags.is_archived.unwrap_or(false),
        }
    }
}

#[derive(Deserialize)]
struct ChannelsResponse {
    channels: Vec<Conversation>,
    response_metadata: Option<SlackResponseMetadata>,
    has_more: Option<bool>,
}

// Project only the fields slk consumes; new Slack subtypes and blocks remain opaque.
#[derive(Deserialize)]
pub(super) struct WireMessage {
    pub ts: SlackTs,
    pub thread_ts: Option<SlackTs>,
    subtype: Option<String>,
    user: Option<String>,
    bot_id: Option<String>,
    text: Option<String>,
    permalink: Option<String>,
    blocks: Option<Value>,
    attachments: Option<Value>,
    files: Option<Value>,
}

impl WireMessage {
    pub(super) fn in_channel(self, channel_id: SlackChannelId) -> Message {
        Message {
            channel_id,
            ts: self.ts,
            thread_ts: self.thread_ts,
            author: self
                .user
                .filter(|value| !value.is_empty())
                .or(self.bot_id.filter(|value| !value.is_empty())),
            text: self.text.unwrap_or_default(),
            blocks: self.blocks,
            permalink: self.permalink,
        }
    }
}

#[derive(Deserialize)]
struct MessagesResponse {
    messages: Vec<WireMessage>,
    response_metadata: Option<SlackResponseMetadata>,
    has_more: Option<bool>,
}

impl MessagesResponse {
    fn in_channel(self, channel_id: &SlackChannelId) -> Page<Message> {
        Page::cursor(
            self.messages
                .into_iter()
                .map(|message| message.in_channel(channel_id.clone()))
                .collect(),
            self.response_metadata,
            self.has_more,
        )
    }
}

pub(crate) struct ThreadRequest<'a> {
    pub channel_id: &'a SlackChannelId,
    pub thread_ts: &'a SlackTs,
    pub page: &'a CursorPage,
}

impl<C: SlackClientHttpConnector + Send + Sync> Slack<C> {
    pub(crate) async fn channels(
        &self,
        credentials: &Credentials,
        page: &CursorPage,
    ) -> Result<Page<Channel>> {
        pagination::collect(page, |page| async move {
            let response: ChannelsResponse = self
                .get(
                    credentials,
                    "conversations.list",
                    vec![
                        ("types", Some(CONVERSATION_TYPES)),
                        ("limit", Some(&page.limit.to_string())),
                        ("cursor", page.cursor.as_deref()),
                    ],
                )
                .await?;
            let data = response.channels.into_iter().map(Channel::from).collect();
            Ok(Page::cursor(
                data,
                response.response_metadata,
                response.has_more,
            ))
        })
        .await
    }

    pub(crate) async fn history(
        &self,
        credentials: &Credentials,
        channel_id: &SlackChannelId,
        page: &CursorPage,
    ) -> Result<Page<Message>> {
        pagination::collect(page, |page| async move {
            let response: MessagesResponse = self
                .get(
                    credentials,
                    "conversations.history",
                    vec![
                        ("channel", Some(channel_id.value())),
                        ("limit", Some(&page.limit.to_string())),
                        ("cursor", page.cursor.as_deref()),
                    ],
                )
                .await?;
            Ok(response.in_channel(channel_id))
        })
        .await
    }

    pub(crate) async fn replies(
        &self,
        credentials: &Credentials,
        request: &ThreadRequest<'_>,
    ) -> Result<Page<Message>> {
        pagination::collect(request.page, |page| async move {
            let response: MessagesResponse = self
                .get(
                    credentials,
                    "conversations.replies",
                    vec![
                        ("channel", Some(request.channel_id.value())),
                        ("ts", Some(request.thread_ts.value())),
                        ("limit", Some(&page.limit.to_string())),
                        ("cursor", page.cursor.as_deref()),
                    ],
                )
                .await?;
            Ok(response.in_channel(request.channel_id))
        })
        .await
    }

    pub(crate) async fn resolve_thread(
        &self,
        credentials: &Credentials,
        link: &MessageLink,
        explicit: Option<&SlackTs>,
    ) -> Result<SlackTs> {
        let candidate = link.root_hint(explicit)?;
        let message = self
            .message_at(credentials, &link.channel_id, candidate)
            .await?;
        let root = message.thread_ts.as_ref().unwrap_or(&message.ts);
        if (explicit.is_some() || link.thread_ts.is_some()) && root != candidate {
            return Err(Error::ThreadUnresolved);
        }
        if root != &message.ts {
            let parent = self.message_at(credentials, &link.channel_id, root).await?;
            if parent.thread_ts.as_ref().is_some_and(|ts| ts != root) {
                return Err(Error::ThreadUnresolved);
            }
        }
        if link.ts != *root && message.ts != link.ts {
            let reply = self.reply_message(credentials, link, root).await?;
            if reply.is_none_or(|reply| reply.thread_ts.as_ref() != Some(root)) {
                return Err(Error::ThreadUnresolved);
            }
        }
        Ok(root.clone())
    }

    pub(crate) async fn message(
        &self,
        credentials: &Credentials,
        link: &MessageLink,
        explicit: Option<&SlackTs>,
    ) -> Result<MessageTarget> {
        let root = link.root_hint(explicit)?;
        let message = if root != &link.ts {
            self.reply_message(credentials, link, root).await?
        } else {
            self.history_message(credentials, &link.channel_id, &link.ts)
                .await?
        }
        .ok_or(Error::MessageUnresolved)?;
        if (explicit.is_some() || link.thread_ts.is_some())
            && message.thread_ts.as_ref().unwrap_or(&message.ts) != root
        {
            return Err(Error::MessageUnresolved);
        }
        let authored = match credentials.kind() {
            Kind::Bot => {
                message.bot_id.is_some()
                    && matches!(
                        message.subtype.as_deref(),
                        None | Some("bot_message" | "thread_broadcast")
                    )
            }
            Kind::User | Kind::Browser => {
                message.user.is_some()
                    && message.bot_id.is_none()
                    && matches!(message.subtype.as_deref(), None | Some("thread_broadcast"))
            }
        };
        let editable = authored
            && message.blocks.as_ref().is_none_or(Value::is_array)
            && (message.text.is_some()
                || message
                    .blocks
                    .as_ref()
                    .and_then(Value::as_array)
                    .is_some_and(|blocks| !blocks.is_empty()))
            && [&message.attachments, &message.files]
                .into_iter()
                .all(|content| {
                    content
                        .as_ref()
                        .is_none_or(|value| value.as_array().is_some_and(Vec::is_empty))
                });
        Ok(MessageTarget {
            editable,
            message: message.in_channel(link.channel_id.clone()),
        })
    }

    async fn reply_message(
        &self,
        credentials: &Credentials,
        link: &MessageLink,
        root: &SlackTs,
    ) -> Result<Option<WireMessage>> {
        let response: MessagesResponse = self
            .get(
                credentials,
                "conversations.replies",
                vec![
                    ("channel", Some(link.channel_id.value())),
                    ("ts", Some(root.value())),
                    ("oldest", Some(link.ts.value())),
                    ("latest", Some(link.ts.value())),
                    ("inclusive", Some("true")),
                    ("limit", Some("2")),
                ],
            )
            .await?;
        Ok(response
            .messages
            .into_iter()
            .find(|message| message.ts == link.ts))
    }

    async fn message_at(
        &self,
        credentials: &Credentials,
        channel_id: &SlackChannelId,
        ts: &SlackTs,
    ) -> Result<WireMessage> {
        self.history_message(credentials, channel_id, ts)
            .await?
            .filter(|message| {
                !matches!(
                    message.subtype.as_deref(),
                    Some("channel_join" | "channel_leave")
                )
            })
            .ok_or(Error::ThreadUnresolved)
    }

    async fn history_message(
        &self,
        credentials: &Credentials,
        channel_id: &SlackChannelId,
        ts: &SlackTs,
    ) -> Result<Option<WireMessage>> {
        let response: MessagesResponse = self
            .get(
                credentials,
                "conversations.history",
                vec![
                    ("channel", Some(channel_id.value())),
                    ("oldest", Some(ts.value())),
                    ("latest", Some(ts.value())),
                    ("inclusive", Some("true")),
                    ("limit", Some("1")),
                ],
            )
            .await?;
        Ok(response
            .messages
            .into_iter()
            .find(|message| &message.ts == ts))
    }
}
