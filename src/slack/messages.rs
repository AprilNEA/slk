use super::{Slack, pagination};
use crate::auth::{Credentials, Kind};
use crate::error::{Error, Result};
use crate::ids::parse_timestamp;
use crate::model::messages::{Change, Changed, Draft, Message, MessageLink, MessageTarget, Sent};
use crate::model::pagination::{CursorPage, Page};
use serde::Deserialize;
use serde_json::Value;
use slack_morphism::api::{
    SlackApiChatDeleteRequest, SlackApiChatPostMessageRequest, SlackApiChatUpdateRequest,
    SlackApiPinsAddRequest, SlackApiPinsAddResponse, SlackApiPinsRemoveRequest,
    SlackApiPinsRemoveResponse, SlackApiReactionsAddRequest, SlackApiReactionsAddResponse,
    SlackApiReactionsRemoveRequest, SlackApiReactionsRemoveResponse,
};
use slack_morphism::prelude::ValueStruct;
use slack_morphism::{
    SlackChannelId, SlackClientHttpConnector, SlackMessageContent, SlackResponseMetadata, SlackTs,
};

mod search;

#[derive(Deserialize)]
struct PostResponse {
    channel: SlackChannelId,
    ts: SlackTs,
    message: PostedMessage,
}

#[derive(Deserialize)]
struct PostedMessage {
    thread_ts: Option<SlackTs>,
}

// Project only the fields slk consumes; new Slack subtypes and blocks remain opaque.
#[derive(Deserialize)]
struct WireMessage {
    ts: SlackTs,
    thread_ts: Option<SlackTs>,
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
    fn in_channel(self, channel_id: SlackChannelId) -> Message {
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

#[derive(Deserialize)]
struct PinsResponse {
    items: Vec<serde_json::Value>,
}

#[derive(Deserialize)]
struct MessageAcknowledgement {
    channel: SlackChannelId,
    ts: SlackTs,
}

impl<C: SlackClientHttpConnector + Send + Sync> Slack<C> {
    pub(crate) async fn send(&self, credentials: &Credentials, draft: &Draft) -> Result<Sent> {
        let response: PostResponse = self
            .write(credentials, "chat.postMessage", &message_request(draft))
            .await?;
        if response.channel != draft.channel_id
            || response.message.thread_ts != draft.thread_ts
            || parse_timestamp(response.ts.value()).is_err()
        {
            return Err(Error::WriteOutcomeUnknown { slack_error: None });
        }
        Ok(Sent {
            channel_id: response.channel,
            ts: response.ts,
            thread_ts: response.message.thread_ts,
        })
    }

    pub(crate) async fn pins(
        &self,
        credentials: &Credentials,
        channel: &SlackChannelId,
    ) -> Result<Vec<serde_json::Value>> {
        let response: PinsResponse = self
            .get(
                credentials,
                "pins.list",
                vec![("channel", Some(&channel.0))],
            )
            .await?;
        Ok(response.items)
    }

    pub(crate) async fn change(
        &self,
        credentials: &Credentials,
        message: Message,
        change: Change,
    ) -> Result<Changed> {
        let acknowledgement: Option<MessageAcknowledgement> = match &change {
            Change::Edit { content } => {
                let request = SlackApiChatUpdateRequest::new(
                    message.channel_id.clone(),
                    SlackMessageContent::from(content),
                    message.ts.clone(),
                )
                .with_link_names(false)
                .with_parse("none".to_owned());
                Some(self.write(credentials, "chat.update", &request).await?)
            }
            Change::Delete => {
                let request =
                    SlackApiChatDeleteRequest::new(message.channel_id.clone(), message.ts.clone());
                Some(self.write(credentials, "chat.delete", &request).await?)
            }
            Change::React { name } => {
                let request = SlackApiReactionsAddRequest::new(
                    message.channel_id.clone(),
                    name.clone(),
                    message.ts.clone(),
                );
                let _: SlackApiReactionsAddResponse =
                    self.write(credentials, "reactions.add", &request).await?;
                None
            }
            Change::Unreact { name } => {
                let request = SlackApiReactionsRemoveRequest::new(name.clone())
                    .with_channel(message.channel_id.clone())
                    .with_timestamp(message.ts.clone());
                let _: SlackApiReactionsRemoveResponse = self
                    .write(credentials, "reactions.remove", &request)
                    .await?;
                None
            }
            Change::Pin => {
                let request =
                    SlackApiPinsAddRequest::new(message.channel_id.clone(), message.ts.clone());
                let _: SlackApiPinsAddResponse =
                    self.write(credentials, "pins.add", &request).await?;
                None
            }
            Change::Unpin => {
                let request =
                    SlackApiPinsRemoveRequest::new(message.channel_id.clone(), message.ts.clone());
                let _: SlackApiPinsRemoveResponse =
                    self.write(credentials, "pins.remove", &request).await?;
                None
            }
        };
        if acknowledgement.is_some_and(|response| {
            response.channel != message.channel_id || response.ts != message.ts
        }) {
            return Err(Error::WriteOutcomeUnknown { slack_error: None });
        }
        Ok(Changed {
            channel_id: message.channel_id,
            ts: message.ts,
            change,
        })
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

pub(super) fn message_request(draft: &Draft) -> SlackApiChatPostMessageRequest {
    SlackApiChatPostMessageRequest::new(
        draft.channel_id.clone(),
        SlackMessageContent::from(&draft.content),
    )
    .opt_thread_ts(draft.thread_ts.clone())
    .with_reply_broadcast(false)
    .with_unfurl_links(false)
    .with_unfurl_media(false)
    .with_link_names(false)
    .with_parse("none".to_owned())
}
