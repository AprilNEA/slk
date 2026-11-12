use super::conversations::{ChannelCommand, ConversationCommand};
use super::messages::{MessageCommand, Target};
use super::write::{BodyInput, SendInput, WriteInput, WriteOptions};
use super::{Effect, FilePath, Paging};
use crate::app::{self, messages::Order};
use crate::auth::{ProfileView, Store};
use crate::error::{Error, Result};
use crate::ids::{parse_channel, parse_timestamp};
use crate::model::messages::{MessageLink, parse_reaction};
use clap::{ArgGroup, ArgMatches, Args, Subcommand};
use slack_morphism::{SlackChannelId, SlackReactionName, SlackTs};
use std::path::PathBuf;

#[derive(Args)]
pub(super) struct Selector {
    /// slackcli selector: an exact profile name or a unique saved workspace ID.
    #[arg(long, global = true, conflicts_with = "profile")]
    pub(super) workspace: Option<String>,
}

impl Selector {
    /// Whether `matches`, the innermost subcommand's matches, carry `--workspace`.
    pub(super) fn given(matches: &ArgMatches) -> bool {
        matches.try_contains_id("workspace").unwrap_or(false)
    }

    pub(super) fn resolve(&self, store: &Store, selected: Option<&str>) -> Result<Option<String>> {
        let Some(selector) = &self.workspace else {
            return Ok(selected.map(str::to_owned));
        };
        if selected.is_some() {
            return Err(Error::Arguments("Use --profile or --workspace, not both."));
        }
        let profiles = store.profiles();
        if let Some(profile) = profiles.iter().find(|profile| profile.profile == *selector) {
            return Ok(Some(profile.profile.clone()));
        }
        let matches = profiles
            .into_iter()
            .filter(|profile| profile.identity.team_id == *selector)
            .collect::<Vec<_>>();
        match matches.as_slice() {
            [profile] => Ok(Some(profile.profile.clone())),
            [] => Err(Error::ProfileNotFound),
            _ => Err(Error::AmbiguousProfile),
        }
    }
}

#[derive(Subcommand)]
pub(super) enum Command {
    /// Supported slackcli conversation syntax; uses slk JSON and bounded pagination.
    Conversations {
        #[command(flatten)]
        selector: Selector,
        #[command(subcommand)]
        command: Conversation,
    },
    /// Supported slackcli message syntax; retains slk identity and approval rules.
    Messages {
        #[command(flatten)]
        selector: Selector,
        #[command(subcommand)]
        command: Message,
    },
}

#[derive(Subcommand)]
pub(super) enum Conversation {
    /// List visible conversations; use native pagination defaults and --all explicitly.
    List(Paging),
    /// Read history oldest first, or read the verified thread identified by a permalink.
    Read {
        #[arg(value_parser = parse_channel, required_unless_present = "permalink", conflicts_with = "permalink")]
        channel_id: Option<SlackChannelId>,
        #[arg(long, value_parser = timestamp, requires = "channel_id", conflicts_with = "permalink")]
        thread_ts: Option<SlackTs>,
        #[arg(long, conflicts_with_all = ["channel_id", "thread_ts"])]
        permalink: Option<MessageLink>,
        #[command(flatten)]
        paging: Paging,
    },
    /// Read one exact message; a permalink's thread_ts never changes the selected message.
    Get {
        #[arg(value_parser = parse_channel, requires = "timestamp", required_unless_present = "permalink", conflicts_with = "permalink")]
        channel_id: Option<SlackChannelId>,
        #[arg(value_parser = timestamp, requires = "channel_id", conflicts_with = "permalink")]
        timestamp: Option<SlackTs>,
        #[arg(long, conflicts_with_all = ["channel_id", "timestamp"])]
        permalink: Option<MessageLink>,
    },
    /// Inspect members without opening or changing the conversation.
    Members {
        #[command(subcommand)]
        command: Members,
    },
}

#[derive(Subcommand)]
pub(super) enum Members {
    /// List member IDs; --limit is a per-page limit, not a total-result limit.
    List {
        #[arg(value_parser = parse_channel)]
        channel: SlackChannelId,
        #[command(flatten)]
        paging: Paging,
    },
}

#[derive(Subcommand)]
pub(super) enum Message {
    /// Send text to an existing conversation or verified thread; never create a DM implicitly.
    Send {
        #[arg(long, value_parser = parse_channel, required_unless_present = "permalink", conflicts_with = "permalink")]
        recipient_id: Option<SlackChannelId>,
        #[arg(long, value_parser = timestamp, requires = "recipient_id", conflicts_with = "permalink")]
        thread_ts: Option<SlackTs>,
        #[arg(long, conflicts_with_all = ["recipient_id", "thread_ts"])]
        permalink: Option<MessageLink>,
        #[command(flatten)]
        body: Body,
    },
    /// Edit your exact plain-text message.
    Edit {
        #[command(flatten)]
        target: ExactTarget,
        #[command(flatten)]
        body: Body,
    },
    /// Delete your exact supported message; --yes is still required for automation.
    Delete {
        #[command(flatten)]
        target: ExactTarget,
        #[command(flatten)]
        options: WriteOptions,
    },
    /// Add an emoji to the exact message, not its thread root.
    React {
        #[command(flatten)]
        target: ExactTarget,
        #[arg(long, value_parser = parse_reaction)]
        emoji: SlackReactionName,
        #[command(flatten)]
        options: WriteOptions,
    },
    /// Remove your emoji from the exact message.
    Unreact {
        #[command(flatten)]
        target: ExactTarget,
        #[arg(long, value_parser = parse_reaction)]
        emoji: SlackReactionName,
        #[command(flatten)]
        options: WriteOptions,
    },
}

#[derive(Args)]
pub(super) struct ExactTarget {
    #[arg(long, value_parser = parse_channel, requires = "timestamp", required_unless_present = "permalink", conflicts_with = "permalink")]
    channel_id: Option<SlackChannelId>,
    #[arg(long, value_parser = timestamp, requires = "channel_id", conflicts_with = "permalink")]
    timestamp: Option<SlackTs>,
    #[arg(long, conflicts_with_all = ["channel_id", "timestamp"])]
    permalink: Option<MessageLink>,
}

impl ExactTarget {
    fn resolve(self, profile: &ProfileView) -> Result<Target> {
        let message_url = match (self.permalink, self.channel_id, self.timestamp) {
            (Some(link), None, None) => link,
            (None, Some(channel_id), Some(ts)) => MessageLink {
                workspace: profile.identity.workspace_url.clone(),
                channel_id,
                ts,
                thread_ts: None,
            },
            _ => {
                return Err(Error::Arguments(
                    "Provide a permalink or both channel and timestamp.",
                ));
            }
        };
        Ok(Target {
            message_url,
            thread: None,
        })
    }
}

#[derive(Args)]
#[command(group(ArgGroup::new("compat_body").required(true).args(["message", "message_file"])))]
pub(super) struct Body {
    #[arg(long)]
    message: Option<String>,
    /// UTF-8 body file, or - for stdin. This is not slackcli's attachment --file option.
    #[arg(long, value_name = "PATH")]
    message_file: Option<PathBuf>,
    #[command(flatten)]
    options: WriteOptions,
}

impl From<Body> for WriteInput {
    fn from(body: Body) -> Self {
        Self {
            body: BodyInput {
                text: body.message,
                file: body.message_file,
            },
            blocks_file: None,
            options: body.options,
        }
    }
}

impl Command {
    pub(super) fn effect(&self) -> Effect {
        match self {
            Self::Conversations { command, .. } => match command {
                Conversation::List(_) | Conversation::Read { .. } | Conversation::Get { .. } => {
                    Effect::Read
                }
                Conversation::Members { command } => match command {
                    Members::List { .. } => Effect::Read,
                },
            },
            Self::Messages { command, .. } => match command {
                Message::Send { .. }
                | Message::Edit { .. }
                | Message::Delete { .. }
                | Message::React { .. }
                | Message::Unreact { .. } => Effect::Write,
            },
        }
    }

    pub(super) fn file_paths(&mut self) -> Vec<FilePath<'_>> {
        match self {
            Self::Conversations { command, .. } => match command {
                Conversation::List(_) | Conversation::Read { .. } | Conversation::Get { .. } => {
                    Vec::new()
                }
                Conversation::Members { command } => match command {
                    Members::List { .. } => Vec::new(),
                },
            },
            Self::Messages { command, .. } => match command {
                Message::Send { body, .. } | Message::Edit { body, .. } => {
                    body.message_file.iter_mut().map(FilePath::Body).collect()
                }
                Message::Delete { .. } | Message::React { .. } | Message::Unreact { .. } => {
                    Vec::new()
                }
            },
        }
    }

    pub(super) fn prepare(
        self,
        store: &Store,
        selected: Option<&str>,
        json: bool,
    ) -> Result<(ProfileView, app::Request)> {
        let selector = match &self {
            Self::Conversations { selector, .. } | Self::Messages { selector, .. } => selector,
        };
        let selected = selector.resolve(store, selected)?;
        let selected = selected.as_deref();
        let profile = store.select(selected)?;
        let native = match self {
            Self::Conversations {
                command: Conversation::List(paging),
                ..
            } => {
                return ConversationCommand::Channels(paging).prepare(store, selected, json);
            }
            Self::Conversations {
                command:
                    Conversation::Read {
                        channel_id,
                        thread_ts,
                        permalink,
                        paging,
                    },
                ..
            } => {
                if permalink.is_none() && thread_ts.is_none() {
                    let request = app::messages::Request::Read {
                        channel: channel_id.ok_or(Error::Arguments("Provide a channel ID."))?,
                        page: paging.resolve(store.settings()),
                        order: Order::OldestFirst,
                    };
                    return Ok((profile, app::Request::Messages(request)));
                }
                MessageCommand::Thread {
                    target: ExactTarget {
                        channel_id,
                        timestamp: thread_ts,
                        permalink,
                    }
                    .resolve(&profile)?,
                    paging,
                }
            }
            Self::Conversations {
                command:
                    Conversation::Get {
                        channel_id,
                        timestamp,
                        permalink,
                    },
                ..
            } => MessageCommand::Get {
                target: ExactTarget {
                    channel_id,
                    timestamp,
                    permalink,
                }
                .resolve(&profile)?,
            },
            Self::Conversations {
                command:
                    Conversation::Members {
                        command: Members::List { channel, paging },
                    },
                ..
            } => {
                return ConversationCommand::Channel {
                    command: ChannelCommand::Members {
                        conversation: channel,
                        paging,
                    },
                }
                .prepare(store, selected, json);
            }
            Self::Messages {
                command:
                    Message::Send {
                        recipient_id,
                        thread_ts,
                        permalink,
                        body,
                    },
                ..
            } => {
                let input = SendInput {
                    input: body.into(),
                    split: false,
                    chunk_chars: None,
                    max_chunks: None,
                };
                if permalink.is_none() && thread_ts.is_none() {
                    MessageCommand::Send {
                        conversation: recipient_id
                            .ok_or(Error::Arguments("Provide an existing conversation ID."))?,
                        input,
                    }
                } else {
                    MessageCommand::Reply {
                        target: ExactTarget {
                            channel_id: recipient_id,
                            timestamp: thread_ts,
                            permalink,
                        }
                        .resolve(&profile)?,
                        input,
                    }
                }
            }
            Self::Messages {
                command: Message::Edit { target, body },
                ..
            } => MessageCommand::Edit {
                target: target.resolve(&profile)?,
                input: body.into(),
            },
            Self::Messages {
                command: Message::Delete { target, options },
                ..
            } => MessageCommand::Delete {
                target: target.resolve(&profile)?,
                options,
            },
            Self::Messages {
                command:
                    Message::React {
                        target,
                        emoji,
                        options,
                    },
                ..
            } => MessageCommand::React {
                target: target.resolve(&profile)?,
                name: emoji,
                options,
            },
            Self::Messages {
                command:
                    Message::Unreact {
                        target,
                        emoji,
                        options,
                    },
                ..
            } => MessageCommand::Unreact {
                target: target.resolve(&profile)?,
                name: emoji,
                options,
            },
        };
        native.prepare(store, selected, json)
    }
}

fn timestamp(value: &str) -> Result<SlackTs> {
    let packed = value.strip_prefix('p').unwrap_or(value);
    if packed.len() == 16 && packed.bytes().all(|byte| byte.is_ascii_digit()) {
        return parse_timestamp(&format!("{}.{}", &packed[..10], &packed[10..]));
    }
    if !value.is_empty() && value.bytes().all(|byte| byte.is_ascii_digit()) {
        return parse_timestamp(&format!("{value}.000000"));
    }
    parse_timestamp(value)
}

#[cfg(test)]
mod tests;
