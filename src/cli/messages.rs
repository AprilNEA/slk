use super::write::{SendInput, WriteInput, WriteOptions};
use super::{Effect, FilePath, Paging, Traversal};
use crate::app::{
    self,
    messages::{Destination, Order, Request},
};
use crate::auth::{ProfileView, Store};
use crate::error::{Error, Result};
use crate::ids::{parse_channel, parse_timestamp};
use crate::model::messages::{
    Change, MAX_SEARCH_PAGE, MAX_SEARCH_SIZE, MessageLink, MessageReference, SearchPage,
    parse_reaction,
};
use clap::{Args, Subcommand};
use slack_morphism::{SlackChannelId, SlackReactionName, SlackTs};

#[derive(Subcommand)]
pub(super) enum MessageCommand {
    /// Search with RTS (user) or experimental legacy search (browser).
    Search {
        /// A native Slack query; quote queries containing spaces.
        query: String,
        /// With `search messages QUERY`, use QUERY as the Slack query.
        #[arg(value_name = "SLACKCLI_QUERY")]
        compatibility_query: Option<String>,
        #[command(flatten)]
        selector: super::compat::Selector,
        /// Maximum results per page; capped at the RTS maximum.
        #[arg(long, value_parser = clap::value_parser!(u16).range(1..=i64::from(MAX_SEARCH_SIZE)))]
        limit: Option<u16>,
        /// Continue official user search with this cursor.
        #[arg(long, conflicts_with = "page")]
        cursor: Option<String>,
        /// Fetch a browser search page, starting at 1.
        #[arg(long, value_parser = clap::value_parser!(u32).range(1..=i64::from(MAX_SEARCH_PAGE)))]
        page: Option<u32>,
        #[command(flatten)]
        traversal: Traversal,
    },
    /// Read conversation history, newest first; one page by default.
    Read {
        #[arg(value_parser = parse_channel)]
        conversation: SlackChannelId,
        #[command(flatten)]
        paging: Paging,
    },
    /// Read the exact message in a permalink, not its thread root.
    Get {
        #[command(flatten)]
        target: Target,
    },
    /// Read a verified thread, oldest first; subsequent pages may omit the root.
    Thread {
        #[command(flatten)]
        target: Target,
        #[command(flatten)]
        paging: Paging,
    },
    /// Send to an existing conversation; --split creates ordered top-level messages.
    Send {
        #[arg(value_parser = parse_channel)]
        conversation: SlackChannelId,
        #[command(flatten)]
        input: SendInput,
    },
    /// Reply to a verified thread root, never broadcast to the channel.
    Reply {
        #[command(flatten)]
        target: Target,
        #[command(flatten)]
        input: SendInput,
    },
    /// Edit your exact message; replacing blocks requires --blocks-file.
    Edit {
        #[command(flatten)]
        target: Target,
        #[command(flatten)]
        input: WriteInput,
    },
    /// Delete your own text-only message at the URL's exact timestamp.
    Delete {
        #[command(flatten)]
        target: Target,
        #[command(flatten)]
        options: WriteOptions,
    },
    /// Add an emoji reaction to the exact linked message, not its thread root.
    React {
        #[command(flatten)]
        target: Target,
        /// Emoji name without surrounding colons, such as thumbsup::skin-tone-3.
        #[arg(value_parser = parse_reaction)]
        name: SlackReactionName,
        #[command(flatten)]
        options: WriteOptions,
    },
    /// Remove your emoji reaction from the exact linked message.
    Unreact {
        #[command(flatten)]
        target: Target,
        #[arg(value_parser = parse_reaction)]
        name: SlackReactionName,
        #[command(flatten)]
        options: WriteOptions,
    },
    /// List channel pins, including opaque file and future item kinds; not personal Later items.
    Pins {
        #[arg(value_parser = parse_channel)]
        conversation: SlackChannelId,
    },
    /// Pin the exact linked message for everyone in the conversation.
    Pin {
        #[command(flatten)]
        target: Target,
        #[command(flatten)]
        options: WriteOptions,
    },
    /// Unpin the exact linked message without deleting it.
    Unpin {
        #[command(flatten)]
        target: Target,
        #[command(flatten)]
        options: WriteOptions,
    },
}

#[derive(Args)]
pub(super) struct Target {
    /// HTTPS message permalink in the selected workspace.
    pub(super) message_url: MessageLink,
    /// Explicit root timestamp when the URL does not identify the root.
    #[arg(long, value_parser = parse_timestamp)]
    pub(super) thread: Option<SlackTs>,
}

impl Target {
    pub(super) fn reference(self, profile: &ProfileView) -> Result<MessageReference> {
        let reference = MessageReference {
            link: self.message_url,
            thread: self.thread,
        };
        reference.validate(&profile.identity.workspace_url)?;
        Ok(reference)
    }
}

impl MessageCommand {
    pub(super) fn effect(&self) -> Effect {
        match self {
            Self::Search { .. }
            | Self::Read { .. }
            | Self::Get { .. }
            | Self::Thread { .. }
            | Self::Pins { .. } => Effect::Read,
            Self::Send { .. }
            | Self::Reply { .. }
            | Self::Edit { .. }
            | Self::Delete { .. }
            | Self::React { .. }
            | Self::Unreact { .. }
            | Self::Pin { .. }
            | Self::Unpin { .. } => Effect::Write,
        }
    }

    pub(super) fn file_paths(&mut self) -> Vec<FilePath<'_>> {
        match self {
            Self::Send { input, .. } | Self::Reply { input, .. } => input.input.file_paths(),
            Self::Edit { input, .. } => input.file_paths(),
            Self::Search { .. }
            | Self::Read { .. }
            | Self::Get { .. }
            | Self::Thread { .. }
            | Self::Delete { .. }
            | Self::React { .. }
            | Self::Unreact { .. }
            | Self::Pins { .. }
            | Self::Pin { .. }
            | Self::Unpin { .. } => Vec::new(),
        }
    }

    pub(super) fn prepare(
        self,
        store: &Store,
        selected: Option<&str>,
        json: bool,
    ) -> Result<(ProfileView, app::Request)> {
        let selection = match &self {
            Self::Search { selector, .. } => selector.resolve(store, selected)?,
            _ => selected.map(str::to_owned),
        };
        let selected = selection.as_deref();
        let profile = store.select(selected)?;
        let settings = store.settings();
        let request = match self {
            Self::Search {
                query,
                compatibility_query,
                selector: _,
                limit,
                cursor,
                page,
                traversal,
            } => {
                let query = match compatibility_query {
                    Some(value) if query == "messages" => value,
                    Some(_) => {
                        return Err(Error::Arguments(
                            "Quote the entire search query, or use search messages QUERY.",
                        ));
                    }
                    None => query,
                };
                if query.trim().is_empty() {
                    return Err(Error::Arguments("The search query must not be empty."));
                }
                let page = SearchPage {
                    limit: limit.unwrap_or(settings.page_size.get().min(MAX_SEARCH_SIZE)),
                    cursor,
                    page,
                    max_pages: traversal.budget(settings),
                };
                page.validate(profile.kind, profile.identity.workspace_url.realm())?;
                Request::Search { query, page }
            }
            Self::Read {
                conversation,
                paging,
            } => Request::Read {
                channel: conversation,
                page: paging.resolve(settings),
                order: Order::NewestFirst,
            },
            Self::Get { target } => Request::Get(target.reference(&profile)?),
            Self::Thread { target, paging } => Request::Thread {
                target: target.reference(&profile)?,
                page: paging.resolve(settings),
            },
            Self::Send {
                conversation,
                input,
            } => {
                let (content, approval) = input.prepare(settings, selected.is_some(), json)?;
                Request::Write {
                    destination: Destination::Conversation(conversation),
                    content,
                    approval,
                }
            }
            Self::Reply { target, input } => {
                let target = target.reference(&profile)?;
                let (content, approval) = input.prepare(settings, selected.is_some(), json)?;
                Request::Write {
                    destination: Destination::Thread(target),
                    content,
                    approval,
                }
            }
            Self::Edit { target, input } => {
                let target = target.reference(&profile)?;
                let (content, approval) = input.prepare(selected.is_some(), json)?;
                Request::Modify {
                    target,
                    change: Change::Edit { content },
                    approval,
                }
            }
            Self::Delete { target, options } => Request::Modify {
                target: target.reference(&profile)?,
                change: Change::Delete,
                approval: options.prepare(selected.is_some(), json)?,
            },
            Self::React {
                target,
                name,
                options,
            } => Request::Modify {
                target: target.reference(&profile)?,
                change: Change::React { name },
                approval: options.prepare(selected.is_some(), json)?,
            },
            Self::Unreact {
                target,
                name,
                options,
            } => Request::Modify {
                target: target.reference(&profile)?,
                change: Change::Unreact { name },
                approval: options.prepare(selected.is_some(), json)?,
            },
            Self::Pins { conversation } => Request::Pins(conversation),
            Self::Pin { target, options } => Request::Modify {
                target: target.reference(&profile)?,
                change: Change::Pin,
                approval: options.prepare(selected.is_some(), json)?,
            },
            Self::Unpin { target, options } => Request::Modify {
                target: target.reference(&profile)?,
                change: Change::Unpin,
                approval: options.prepare(selected.is_some(), json)?,
            },
        };
        Ok((profile, app::Request::Messages(request)))
    }
}

#[cfg(test)]
mod tests;
