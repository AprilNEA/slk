use crate::auth::{Identity, Workspace};
use crate::error::{Error, Result};
use crate::ids::{parse_channel, parse_timestamp};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use slack_morphism::blocks::SlackBlock;
use slack_morphism::{SlackChannelId, SlackMessageContent, SlackReactionName, SlackTs};
use std::str::FromStr;
use url::Url;

// Slack may split longer text into multiple messages even below its truncation limit.
pub(crate) const MAX_TEXT_CHARS: usize = 4_000;

#[derive(Debug, Serialize)]
pub(crate) struct Message {
    pub channel_id: SlackChannelId,
    pub ts: SlackTs,
    pub thread_ts: Option<SlackTs>,
    pub author: Option<String>,
    pub text: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub blocks: Option<Value>,
    pub permalink: Option<String>,
}

#[derive(Clone, Debug)]
pub(crate) struct MessageLink {
    pub workspace: Workspace,
    pub channel_id: SlackChannelId,
    pub ts: SlackTs,
    pub thread_ts: Option<SlackTs>,
}

impl FromStr for MessageLink {
    type Err = Error;

    fn from_str(value: &str) -> Result<Self> {
        if value.trim() != value
            || value.chars().any(char::is_control)
            || value.contains(['@', '\\'])
        {
            return Err(Error::MessageLink);
        }
        let url = Url::parse(value).map_err(|_| Error::MessageLink)?;
        let mut origin = url.clone();
        origin.set_path("");
        origin.set_query(None);
        origin.set_fragment(None);
        let workspace = Workspace::try_from(origin.to_string()).map_err(|_| Error::MessageLink)?;
        let segments: Vec<_> = url.path_segments().ok_or(Error::MessageLink)?.collect();
        let ["archives", channel, packed] = segments.as_slice() else {
            return Err(Error::MessageLink);
        };
        let channel_id = parse_channel(channel).map_err(|_| Error::MessageLink)?;
        let digits = packed.strip_prefix('p').ok_or(Error::MessageLink)?;
        if digits.len() <= 6 || !digits.bytes().all(|byte| byte.is_ascii_digit()) {
            return Err(Error::MessageLink);
        }
        let (seconds, micros) = digits.split_at(digits.len() - 6);
        let ts = parse_timestamp(&format!("{seconds}.{micros}"))?;
        let mut thread_ts = None;
        for (key, value) in url.query_pairs() {
            match key.as_ref() {
                "thread_ts" => {
                    if thread_ts.is_some() {
                        return Err(Error::MessageLink);
                    }
                    thread_ts = Some(parse_timestamp(&value).map_err(|_| Error::MessageLink)?);
                }
                "cid" if value != *channel => return Err(Error::MessageLink),
                _ => (),
            }
        }
        Ok(Self {
            workspace,
            channel_id,
            ts,
            thread_ts,
        })
    }
}

impl MessageLink {
    pub(crate) fn check_workspace(&self, workspace: &Workspace) -> Result<()> {
        if &self.workspace != workspace {
            return Err(Error::WorkspaceMismatch);
        }
        Ok(())
    }

    pub(crate) fn root_hint<'a>(&'a self, explicit: Option<&'a SlackTs>) -> Result<&'a SlackTs> {
        if let (Some(explicit), Some(link)) = (explicit, &self.thread_ts)
            && explicit != link
        {
            return Err(Error::Arguments(
                "--thread conflicts with the message URL's thread_ts.",
            ));
        }
        Ok(explicit.or(self.thread_ts.as_ref()).unwrap_or(&self.ts))
    }
}

#[derive(Debug, Serialize)]
pub(crate) struct MessageText(String);

impl TryFrom<String> for MessageText {
    type Error = Error;

    fn try_from(text: String) -> Result<Self> {
        if text.trim().is_empty() {
            return Err(Error::Arguments("Message text must not be empty."));
        }
        if text.chars().count() > MAX_TEXT_CHARS {
            return Err(Error::MessageTooLong {
                limit: MAX_TEXT_CHARS,
            });
        }
        Ok(Self(text))
    }
}

impl AsRef<str> for MessageText {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(try_from = "Vec<Value>")]
pub(crate) struct Blocks(Vec<Value>);

impl TryFrom<Vec<Value>> for Blocks {
    type Error = Error;

    fn try_from(blocks: Vec<Value>) -> Result<Self> {
        if blocks.len() > 50
            || blocks.iter().any(|block| {
                block
                    .get("type")
                    .and_then(Value::as_str)
                    .is_none_or(str::is_empty)
            })
        {
            return Err(Error::Arguments(
                "Provide at most 50 Block Kit objects, each with a nonempty string type.",
            ));
        }
        Ok(Self(blocks))
    }
}

#[derive(Debug, Serialize)]
pub(crate) struct Content {
    pub text: MessageText,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub blocks: Option<Blocks>,
}

impl From<MessageText> for Content {
    fn from(text: MessageText) -> Self {
        Self { text, blocks: None }
    }
}

impl From<&Content> for SlackMessageContent {
    fn from(content: &Content) -> Self {
        Self::new()
            .with_text(content.text.as_ref().to_owned())
            .opt_blocks(
                content
                    .blocks
                    .as_ref()
                    .map(|blocks| blocks.0.iter().cloned().map(SlackBlock::Unknown).collect()),
            )
    }
}

#[derive(Debug, Serialize)]
pub(crate) struct Draft {
    pub channel_id: SlackChannelId,
    pub thread_ts: Option<SlackTs>,
    #[serde(flatten)]
    pub content: Content,
}

#[derive(Debug, Serialize)]
pub(crate) struct Sent {
    pub channel_id: SlackChannelId,
    pub ts: SlackTs,
    pub thread_ts: Option<SlackTs>,
}

pub(crate) struct MessageTarget {
    pub message: Message,
    pub editable: bool,
}

#[derive(Debug, Serialize)]
#[serde(tag = "action", rename_all = "snake_case")]
pub(crate) enum Change {
    Edit {
        #[serde(flatten)]
        content: Content,
    },
    Delete,
    React {
        name: SlackReactionName,
    },
    Unreact {
        name: SlackReactionName,
    },
    Pin,
    Unpin,
}

impl Change {
    pub(crate) fn validate(&self, target: &MessageTarget, identity: &Identity) -> Result<()> {
        if matches!(self, Self::Edit { .. } | Self::Delete) {
            if target.message.author.as_deref() != Some(&identity.user_id)
                && !identity
                    .bot_id
                    .as_ref()
                    .is_some_and(|id| target.message.author.as_ref() == Some(id))
            {
                return Err(Error::MessageNotOwned);
            }
            let explicit_blocks = matches!(
                self,
                Self::Edit {
                    content: Content {
                        blocks: Some(_),
                        ..
                    }
                }
            );
            let has_blocks =
                target.message.blocks.as_ref().is_some_and(|blocks| {
                    blocks.as_array().is_none_or(|blocks| !blocks.is_empty())
                });
            if !target.editable || (has_blocks && !explicit_blocks) {
                return Err(Error::MessageNotText);
            }
        }
        Ok(())
    }
}

#[derive(Debug, Serialize)]
pub(crate) struct Changed {
    pub channel_id: SlackChannelId,
    pub ts: SlackTs,
    #[serde(flatten)]
    pub change: Change,
}

pub(crate) fn parse_reaction(value: &str) -> Result<SlackReactionName> {
    if value.is_empty()
        || value.starts_with(':')
        || value.ends_with(':')
        || value
            .chars()
            .any(|character| character.is_control() || character.is_whitespace())
    {
        return Err(Error::Arguments(
            "Use a Slack emoji name without surrounding colons, whitespace, or controls.",
        ));
    }
    Ok(SlackReactionName(value.to_owned()))
}

#[cfg(test)]
mod tests;
